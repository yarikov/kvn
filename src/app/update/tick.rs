use crate::app::effect::Effect;
use crate::app::model::{ConnectionState, Model, Overlay, UpdateTrigger};
use crate::config::profile::update_window_open;
use crate::geo::{RuleSet, RuleSetState};
use chrono::Local;
use std::time::Instant;

use crate::app::update::traffic::TRAFFIC_POLL_INTERVAL;

const AUTOMATIC_ATTEMPT_SPACING: chrono::TimeDelta = chrono::TimeDelta::minutes(1);

pub(in crate::app::update) fn handle_tick(model: &mut Model) -> Vec<Effect> {
    handle_tick_at(model, Local::now())
}

fn handle_tick_at(model: &mut Model, now: chrono::DateTime<Local>) -> Vec<Effect> {
    let mut effects = Vec::new();

    let due = due_rule_sets(model, now);
    for rule_set in &due {
        model
            .rule_set_automatic_attempts
            .insert(rule_set.key(), now);
    }
    effects.extend(crate::app::update::geo::start_rule_set_download(
        model,
        due,
        UpdateTrigger::Automatic,
        false,
    ));

    effects.extend(check_due_subscriptions_at(model, now));

    effects.extend(crate::app::update::onboarding::open_when_idle(model));
    effects.extend(crate::app::update::onboarding::probe_visible_card(model));
    effects.extend(tick_running_work(model));
    effects
}

fn tick_running_work(model: &mut Model) -> Vec<Effect> {
    let mut effects = Vec::new();

    if model.connection == ConnectionState::Connecting {
        let profile = model.connecting_profile_id.and_then(|id| {
            model
                .config
                .profiles
                .iter()
                .find(|profile| profile.id == id)
                .cloned()
        });
        if let Some(profile) = profile {
            let settings = model.config.settings.clone();
            effects.push(Effect::Connect {
                profile,
                settings,
                attempt_id: model.connect_attempt_id,
            });
        } else {
            model.connection = ConnectionState::Idle;
            model.connecting_profile_id = None;
            model.overlay = Overlay::None;
            effects.push(Effect::BroadcastState);
        }
    }

    while model.testing_profiles.len() < 4 {
        let Some(id) = model.pending_tests.pop_front() else {
            break;
        };
        model.testing_profiles.insert(id);
        effects.push(Effect::TestProfile { id });
    }

    if model.connection == ConnectionState::Connected {
        let now = Instant::now();
        let due = match model.last_traffic_fetch_at {
            None => true,
            Some(prev) => now.duration_since(prev) >= TRAFFIC_POLL_INTERVAL,
        };
        if due {
            model.last_traffic_fetch_at = Some(now);
            model.traffic_request_id = model.traffic_request_id.wrapping_add(1);
            effects.push(Effect::FetchTrafficStats {
                attempt_id: model.connect_attempt_id,
                request_id: model.traffic_request_id,
            });
        }
    }

    effects
}

fn due_rule_sets(model: &Model, now: chrono::DateTime<Local>) -> Vec<RuleSet> {
    if model.config.settings.kill_switch && model.connection != ConnectionState::Connected {
        return Vec::new();
    }
    let interval = model
        .config
        .settings
        .geo_routing
        .auto_update
        .interval_minutes();
    if interval == 0 {
        return Vec::new();
    }
    let idle: Vec<RuleSet> = model
        .selectable_rule_sets()
        .into_iter()
        .filter(|rule_set| !model.rule_set_updates.contains_key(&rule_set.key()))
        .collect();
    let any_due = idle.iter().any(|rule_set| {
        let key = rule_set.key();
        !attempted_recently(model, &key, now)
            && rule_set_due(model.rule_set_states.get(&key), interval, now)
    });
    if any_due { idle } else { Vec::new() }
}

fn attempted_recently(model: &Model, key: &str, now: chrono::DateTime<Local>) -> bool {
    model
        .rule_set_automatic_attempts
        .get(key)
        .is_some_and(|attempt| now.signed_duration_since(*attempt) < AUTOMATIC_ATTEMPT_SPACING)
}

fn rule_set_due(state: Option<&RuleSetState>, interval: u64, now: chrono::DateTime<Local>) -> bool {
    let state = state.cloned().unwrap_or_default();
    if let Some(retry) = state.retry
        && retry.governs(now)
    {
        return retry.retry_due(now);
    }
    update_window_open(now)
        && state.next_update.map_or_else(
            || {
                state.checked_at.is_none_or(|checked| {
                    now.signed_duration_since(checked).num_minutes() >= interval as i64
                })
            },
            |date| now.date_naive() >= date,
        )
}

fn check_due_subscriptions_at(model: &mut Model, now: chrono::DateTime<Local>) -> Vec<Effect> {
    if model.config.settings.kill_switch && model.connection != ConnectionState::Connected {
        return Vec::new();
    }

    let mut effects = Vec::new();
    for sub in &model.config.subscriptions {
        let interval = sub.auto_update.interval_minutes();
        if interval == 0 {
            continue;
        }
        if model.subscription_updates.contains_key(&sub.id) {
            continue;
        }
        let today = now.date_naive();
        let due = match &sub.retry_state {
            Some(state) if state.governs(now) => state.retry_due(now),
            _ => {
                update_window_open(now)
                    && sub.next_auto_update.map_or_else(
                        || {
                            sub.last_updated.is_none_or(|last| {
                                now.signed_duration_since(last).num_minutes() >= interval as i64
                            })
                        },
                        |date| today >= date,
                    )
            }
        };
        if due {
            model
                .subscription_updates
                .insert(sub.id, UpdateTrigger::Automatic);
            effects.push(Effect::UpdateSubscription { id: sub.id });
        }
    }
    effects
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::app::msg::Msg;
    use crate::app::update::key::sources::handle_sources;
    use crate::app::update::update;
    use crate::config::profile::{
        GeoAutoUpdate, GeoRegion, Profile, Subscription, SubscriptionAutoUpdate,
    };
    use crate::test_helpers::*;
    use crossterm::event::{KeyCode, KeyEvent};
    use uuid::Uuid;

    fn ru_auto_updated() -> Model {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Every1d;
        model
    }

    fn is_due(model: &Model, now: chrono::DateTime<Local>) -> bool {
        !due_rule_sets(model, now).is_empty()
    }

    #[test]
    fn rule_set_auto_update_is_due_without_a_previous_check() {
        let mut model = ru_auto_updated();

        let effects = handle_tick_at(&mut model, after_update_window());

        assert!(effects.contains(&Effect::DownloadRuleSets {
            rule_sets: crate::test_helpers::region_rule_sets(GeoRegion::Ru),
            only_missing: false,
            record_schedule: true,
        }));
        assert!(
            model
                .rule_set_updates
                .values()
                .all(|trigger| *trigger == UpdateTrigger::Automatic)
        );
    }

    #[test]
    fn rule_set_auto_update_respects_the_interval_since_the_last_check() {
        let mut model = ru_auto_updated();
        let now = after_update_window();
        model.mark_selectable_rule_sets_checked(now - chrono::Duration::try_hours(23).unwrap());
        assert!(!is_due(&model, now));

        model.mark_selectable_rule_sets_checked(now - chrono::Duration::try_hours(25).unwrap());
        assert!(is_due(&model, now));
    }

    #[test]
    fn one_due_rule_set_updates_the_whole_region_in_one_download() {
        let mut model = ru_auto_updated();
        let now = after_update_window();
        let [_, geoip] = <[crate::geo::RuleSet; 2]>::try_from(
            crate::test_helpers::region_rule_sets(GeoRegion::Ru),
        )
        .unwrap();
        model.mark_selectable_rule_sets_checked(now);
        assert!(!is_due(&model, now));
        model
            .rule_set_states
            .get_mut(&geoip.key())
            .unwrap()
            .next_update = Some(now.date_naive());

        assert_eq!(
            due_rule_sets(&model, now),
            crate::test_helpers::region_rule_sets(GeoRegion::Ru)
        );
    }

    #[test]
    fn an_automatic_attempt_waits_a_minute_even_when_its_result_was_not_saved() {
        let mut model = ru_auto_updated();
        let now = after_update_window();
        handle_tick_at(&mut model, now);
        model.rule_set_updates.clear();

        assert!(!is_due(&model, now + chrono::Duration::seconds(59)));
        assert!(is_due(&model, now + chrono::Duration::minutes(1)));
    }

    #[test]
    fn rule_set_auto_update_honors_a_retry_deadline_before_the_interval() {
        let mut model = ru_auto_updated();
        let now = after_update_window();
        model.mark_selectable_rule_sets_checked(now - chrono::Duration::days(2));
        for state in model.rule_set_states.values_mut() {
            state.retry = Some(crate::config::profile::RetryState {
                consecutive_failures: 2,
                retry_at: now + chrono::Duration::minutes(5),
                attempt_date: None,
            });
        }

        assert!(!is_due(&model, now));
        assert!(!is_due(&model, now + chrono::Duration::minutes(4)));
        assert!(is_due(&model, now + chrono::Duration::minutes(5)));
    }

    #[test]
    fn rule_set_auto_update_waits_for_vpn_when_kill_switch_is_enabled() {
        let mut model = ru_auto_updated();
        model.config.settings.kill_switch = true;
        let now = after_update_window();

        model.connection = ConnectionState::Connecting;
        assert!(!is_due(&model, now));

        model.connection = ConnectionState::ConnectPending;
        assert!(!is_due(&model, now));

        model.connection = ConnectionState::Connected;
        assert!(is_due(&model, now));
    }

    #[test]
    fn rule_set_auto_update_skips_off_global_and_rule_sets_in_flight() {
        let mut model = model_with_profiles(vec![]);
        let now = after_update_window();
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        assert!(!is_due(&model, now));

        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Every1d;
        model
            .config
            .settings
            .geo_routing
            .set_region(GeoRegion::Global);
        assert!(!is_due(&model, now));

        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        for rule_set in crate::test_helpers::region_rule_sets(GeoRegion::Ru) {
            model
                .rule_set_updates
                .insert(rule_set.key(), UpdateTrigger::Manual);
        }
        assert!(!is_due(&model, now));
    }

    #[test]
    fn tick_idle_fallback_broadcasts_state() {
        let mut model = Model::test_new(crate::config::profile::Config::default());
        model.connection = ConnectionState::Connecting;
        let effects = handle_tick(&mut model);
        assert_eq!(model.connection, ConnectionState::Idle);
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn handle_tick_skips_connect_when_pending() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.connection = ConnectionState::ConnectPending;
        let effects = handle_tick(&mut model);
        assert!(effects.iter().all(|e| !matches!(e, Effect::Connect { .. })));
    }

    #[test]
    fn due_subscriptions_are_queued_for_update() {
        let sub_id = Uuid::new_v4();
        let now = after_update_window();
        let mut model = model_with_profiles(vec![]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Sub".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: Some(now - chrono::Duration::try_hours(25).unwrap()),
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });

        let effects = check_due_subscriptions_at(&mut model, now);

        assert_eq!(effects, vec![Effect::UpdateSubscription { id: sub_id }]);
        assert_eq!(
            model.subscription_updates.get(&sub_id),
            Some(&UpdateTrigger::Automatic)
        );
    }

    #[test]
    fn failed_subscription_waits_until_retry_deadline() {
        let sub_id = Uuid::new_v4();
        let now = Local::now();
        let mut model = model_with_profiles(vec![]);
        let mut sub = Subscription {
            id: sub_id,
            name: "Sub".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: Some(now - chrono::Duration::hours(2)),
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        };
        sub.record_fetch_failure(now);
        model.config.subscriptions.push(sub);

        assert!(check_due_subscriptions_at(&mut model, now).is_empty());
        assert!(
            check_due_subscriptions_at(&mut model, now + chrono::Duration::seconds(59)).is_empty()
        );
        assert_eq!(
            check_due_subscriptions_at(&mut model, now + chrono::Duration::minutes(1)),
            vec![Effect::UpdateSubscription { id: sub_id }]
        );
    }

    #[test]
    fn subscription_stops_after_five_failures_and_reopens_at_eight_next_day() {
        use chrono::TimeZone;

        let today = Local.with_ymd_and_hms(2026, 8, 22, 12, 0, 0).unwrap();
        let mut model = model_with_profiles(vec![]);
        let id = Uuid::new_v4();
        model.config.subscriptions.push(Subscription {
            id,
            name: "Sub".into(),
            url: "https://example.com/sub".into(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: None,
            next_auto_update: Some(today.date_naive()),
            retry_state: Some(crate::config::profile::RetryState {
                consecutive_failures: 5,
                retry_at: today,
                attempt_date: Some(today.date_naive()),
            }),
            send_hwid: false,
            hwid: None,
        });

        assert!(check_due_subscriptions_at(&mut model, today).is_empty());
        let before_window = today + chrono::Duration::hours(19);
        assert!(check_due_subscriptions_at(&mut model, before_window).is_empty());
        let at_window = today + chrono::Duration::hours(20);
        assert_eq!(
            check_due_subscriptions_at(&mut model, at_window),
            vec![Effect::UpdateSubscription { id }]
        );
    }

    #[test]
    fn manual_subscription_update_ignores_retry_deadline() {
        let sub_id = Uuid::new_v4();
        let mut model = model_with_profiles(vec![]);
        let mut sub = Subscription {
            id: sub_id,
            name: "Sub".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        };
        sub.record_fetch_failure(Local::now());
        model.config.subscriptions.push(sub);
        model.selected = 0;

        let effects = handle_sources(&mut model, KeyEvent::from(KeyCode::Char('u')));

        assert!(effects.contains(&Effect::UpdateSubscription { id: sub_id }));
        assert!(model.subscription_updates.contains_key(&sub_id));
    }

    #[test]
    fn subscription_auto_update_waits_for_vpn_when_kill_switch_is_enabled() {
        let sub_id = Uuid::new_v4();
        let mut model = model_with_profiles(vec![]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Sub".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        model.config.settings.kill_switch = true;
        let now = after_update_window();

        model.connection = ConnectionState::Connecting;
        assert!(check_due_subscriptions_at(&mut model, now).is_empty());
        assert!(!model.subscription_updates.contains_key(&sub_id));

        model.connection = ConnectionState::ConnectPending;
        assert!(check_due_subscriptions_at(&mut model, now).is_empty());
        assert!(!model.subscription_updates.contains_key(&sub_id));

        model.connection = ConnectionState::Connected;
        assert_eq!(
            check_due_subscriptions_at(&mut model, now),
            vec![Effect::UpdateSubscription { id: sub_id }]
        );
        assert!(model.subscription_updates.contains_key(&sub_id));
    }

    #[test]
    fn non_due_subscriptions_are_skipped() {
        let sub_id = Uuid::new_v4();
        let now = after_update_window();
        let mut model = model_with_profiles(vec![]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Sub".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: Some(now),
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });

        let effects = check_due_subscriptions_at(&mut model, now);

        assert!(effects.is_empty());
        assert!(!model.subscription_updates.contains_key(&sub_id));
    }

    #[test]
    fn disabled_subscription_does_not_retry_persisted_failure() {
        let sub_id = Uuid::new_v4();
        let now = Local::now();
        let mut model = model_with_profiles(vec![]);
        let mut sub = Subscription {
            id: sub_id,
            name: "Sub".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Off,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        };
        sub.record_fetch_failure(now - chrono::Duration::hours(1));
        model.config.subscriptions.push(sub);

        assert!(check_due_subscriptions_at(&mut model, now).is_empty());
        assert!(!model.subscription_updates.contains_key(&sub_id));
    }

    #[test]
    fn tick_dispatches_up_to_4_tests_from_pending() {
        let profiles = sample_profiles();
        let mut profiles5 = profiles;
        let extra_a = crate::config::profile::Profile::new_vless(
            "D".into(),
            "d.example.com".into(),
            443,
            "00000000-0000-0000-0000-000000000004".into(),
        );
        let extra_b = crate::config::profile::Profile::new_vless(
            "E".into(),
            "e.example.com".into(),
            443,
            "00000000-0000-0000-0000-000000000005".into(),
        );
        profiles5.push(extra_a);
        profiles5.push(extra_b);
        let mut model = model_with_profiles(profiles5);
        let key = KeyEvent::new(KeyCode::Char('T'), crossterm::event::KeyModifiers::NONE);
        let _ = update(&mut model, Msg::Key(key));
        assert_eq!(model.pending_tests.len(), 5);
        let effects = update(&mut model, Msg::Tick);
        let test_effects: Vec<_> = effects
            .iter()
            .filter(|e| matches!(e, Effect::TestProfile { .. }))
            .collect();
        assert_eq!(test_effects.len(), 4);
        assert_eq!(model.testing_profiles.len(), 4);
        assert_eq!(model.pending_tests.len(), 1, "one still waiting");
    }
}

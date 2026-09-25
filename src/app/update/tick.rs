use crate::app::effect::Effect;
use crate::app::model::{ConnectionState, Model, Overlay};
use crate::config::profile::{GeoRegion, RoutedService};
use chrono::Local;
use std::time::Instant;

use crate::app::update::status::download_allowed;
use crate::app::update::traffic::TRAFFIC_POLL_INTERVAL;

pub(in crate::app::update) fn handle_tick(model: &mut Model) -> Vec<Effect> {
    handle_tick_at(model, Local::now())
}

fn handle_tick_at(model: &mut Model, now: chrono::DateTime<Local>) -> Vec<Effect> {
    let mut effects = Vec::new();

    if model.restart_required {
        return tick_running_work(model);
    }

    if geo_update_due(model, now) {
        model.geo_updating = true;
        model.geo_automatic_update = true;
        model.geo_last_attempt_at = Some(now);
        effects.push(Effect::DownloadGeo);
    } else if !model.geo_updating && download_allowed(model) {
        let due_services: Vec<_> = model
            .config
            .settings
            .geo_routing
            .enabled_services()
            .into_iter()
            .filter(|service| service_update_due(model, *service, now))
            .collect();
        if !due_services.is_empty() {
            model.geo_updating = true;
            model.geo_automatic_update = true;
            effects.push(Effect::RetryServiceRuleSets {
                services: due_services,
            });
        }
    }

    // Auto-update subscriptions that are due.
    effects.extend(check_due_subscriptions_at(model, now));

    effects.extend(crate::app::update::onboarding::open_when_idle(model));
    effects.extend(crate::app::update::onboarding::probe_visible_card(model));
    effects.extend(tick_running_work(model));
    effects
}

fn tick_running_work(model: &mut Model) -> Vec<Effect> {
    let mut effects = Vec::new();

    // Connection handling
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

    // Dispatch pending profile tests, max 4 concurrent.
    while model.testing_profiles.len() < 4 {
        let Some(id) = model.pending_tests.pop_front() else {
            break;
        };
        model.testing_profiles.insert(id);
        effects.push(Effect::TestProfile { id });
    }

    // Throttled Clash-API poll for live traffic stats.
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

fn service_update_due(model: &Model, service: RoutedService, now: chrono::DateTime<Local>) -> bool {
    if let Some(state) = model.service_retry_states.get(&service)
        && (state.attempt_date.is_none() || state.attempt_date == Some(now.date_naive()))
    {
        return state.consecutive_failures < 5 && now >= state.retry_at;
    }
    model
        .config
        .settings
        .geo_routing
        .auto_update
        .interval_minutes()
        != 0
        && now.time() >= chrono::NaiveTime::from_hms_opt(8, 0, 0).unwrap()
        && model.service_next_updates.get(&service).map_or_else(
            || {
                model
                    .service_checked_at
                    .get(&service)
                    .is_none_or(|checked| {
                        now.signed_duration_since(*checked).num_minutes()
                            >= model
                                .config
                                .settings
                                .geo_routing
                                .auto_update
                                .interval_minutes() as i64
                    })
            },
            |date| now.date_naive() >= *date,
        )
}

fn geo_update_due(model: &Model, now: chrono::DateTime<Local>) -> bool {
    if model.geo_updating {
        return false;
    }
    // With the kill switch active, ordinary application traffic cannot leave
    // through the physical interface. Wait for sing-box to bring up the TUN
    // before starting the HTTP check; otherwise startup auto-connect and the
    // geo download race each other and the request is dropped by nftables.
    if model.config.settings.kill_switch && model.connection != ConnectionState::Connected {
        return false;
    }
    let Some(region) = model.config.settings.geo_routing.current_region else {
        return false;
    };
    if region == GeoRegion::Global {
        return false;
    }
    let interval = model
        .config
        .settings
        .geo_routing
        .auto_update
        .interval_minutes();
    if interval == 0 {
        return false;
    }
    if let Some(retry) = model.geo_retry_state
        && (retry.attempt_date.is_none() || retry.attempt_date == Some(now.date_naive()))
    {
        return retry.consecutive_failures < 5 && now >= retry.retry_at;
    }
    now.time() >= chrono::NaiveTime::from_hms_opt(8, 0, 0).unwrap()
        && model.geo_next_update.map_or_else(
            || {
                let reference = match (model.geo_last_checked_at, model.geo_last_attempt_at) {
                    (Some(checked), Some(attempt)) => Some(checked.max(attempt)),
                    (Some(checked), None) => Some(checked),
                    (None, Some(attempt)) => Some(attempt),
                    (None, None) => None,
                };
                reference.is_none_or(|last| {
                    now.signed_duration_since(last).num_minutes() >= interval as i64
                })
            },
            |date| now.date_naive() >= date,
        )
}

fn check_due_subscriptions_at(model: &mut Model, now: chrono::DateTime<Local>) -> Vec<Effect> {
    // Subscription fetches use ordinary application networking, so with the
    // kill switch active they must wait until sing-box has created the TUN.
    if model.config.settings.kill_switch && model.connection != ConnectionState::Connected {
        return Vec::new();
    }

    let mut effects = Vec::new();
    for sub in &model.config.subscriptions {
        let interval = sub.auto_update.interval_minutes();
        if interval == 0 {
            continue;
        }
        if model.subscription_updates.contains(&sub.id) {
            continue;
        }
        let today = now.date_naive();
        let after_window = now.time() >= chrono::NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let due = match &sub.retry_state {
            Some(state) if state.attempt_date.is_none() || state.attempt_date == Some(today) => {
                state.consecutive_failures < 5 && now >= state.retry_at
            }
            _ => {
                after_window
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
            model.subscription_updates.insert(sub.id);
            model.automatic_subscription_updates.insert(sub.id);
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
    use crate::config::profile::{GeoAutoUpdate, Profile, Subscription, SubscriptionAutoUpdate};
    use crate::test_helpers::*;
    use crossterm::event::{KeyCode, KeyEvent};
    use uuid::Uuid;

    #[test]
    fn geo_auto_update_due_without_previous_check() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Every1d;

        let effects = handle_tick_at(&mut model, after_update_window());

        assert!(effects.contains(&Effect::DownloadGeo));
        assert!(model.geo_updating);
        assert!(model.geo_last_attempt_at.is_some());
    }

    #[test]
    fn geo_auto_update_respects_interval_and_retries_after_interval() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Every1d;
        let now = after_update_window();
        model.geo_last_checked_at = Some(now - chrono::Duration::try_hours(23).unwrap());
        assert!(!geo_update_due(&model, now));

        model.geo_last_checked_at = Some(now - chrono::Duration::try_hours(25).unwrap());
        assert!(geo_update_due(&model, now));

        model.geo_last_attempt_at = Some(now - chrono::Duration::try_hours(1).unwrap());
        assert!(!geo_update_due(&model, now));
        model.geo_last_attempt_at = Some(now - chrono::Duration::try_hours(25).unwrap());
        assert!(geo_update_due(&model, now));
    }

    #[test]
    fn geo_auto_update_honors_retry_deadline_before_normal_interval() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Every1d;
        let now = after_update_window();
        model.geo_last_checked_at = Some(now - chrono::Duration::days(2));
        model.geo_retry_state = Some(crate::geo::GeoRetryState {
            consecutive_failures: 2,
            retry_at: now + chrono::Duration::minutes(5),
            attempt_date: None,
        });

        assert!(!geo_update_due(&model, now));
        assert!(!geo_update_due(&model, now + chrono::Duration::minutes(4)));
        assert!(geo_update_due(&model, now + chrono::Duration::minutes(5)));
    }

    #[test]
    fn service_retry_is_independent_from_region_schedule() {
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Connected;
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Off;
        model.config.settings.geo_routing.service_routes.insert(
            crate::config::profile::RoutedService::Telegram,
            crate::config::profile::ServiceRoute::Proxy,
        );
        model.service_retry_states.insert(
            crate::config::profile::RoutedService::Telegram,
            crate::geo::GeoRetryState {
                consecutive_failures: 2,
                retry_at: Local::now() - chrono::Duration::seconds(1),
                attempt_date: None,
            },
        );

        let effects = handle_tick(&mut model);

        assert!(effects.contains(&Effect::RetryServiceRuleSets {
            services: vec![crate::config::profile::RoutedService::Telegram],
        }));
        assert!(!effects.contains(&Effect::DownloadGeo));
    }

    #[test]
    fn global_auto_update_checks_enabled_services_only() {
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Connected;
        model
            .config
            .settings
            .geo_routing
            .set_region(GeoRegion::Global);
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Every1d;
        model.config.settings.geo_routing.service_routes.insert(
            RoutedService::Telegram,
            crate::config::profile::ServiceRoute::Proxy,
        );

        let effects = handle_tick_at(&mut model, after_update_window());
        assert!(!effects.contains(&Effect::DownloadGeo));
        assert!(effects.contains(&Effect::RetryServiceRuleSets {
            services: vec![RoutedService::Telegram],
        }));
    }

    #[test]
    fn global_auto_update_skips_fresh_service_check() {
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Connected;
        model
            .config
            .settings
            .geo_routing
            .set_region(GeoRegion::Global);
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Every1d;
        model.config.settings.geo_routing.service_routes.insert(
            RoutedService::Telegram,
            crate::config::profile::ServiceRoute::Proxy,
        );
        model
            .service_checked_at
            .insert(RoutedService::Telegram, Local::now());

        let effects = handle_tick(&mut model);
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::RetryServiceRuleSets { .. }))
        );
    }

    #[test]
    fn geo_auto_update_waits_for_vpn_when_kill_switch_is_enabled() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Every1d;
        model.config.settings.kill_switch = true;
        let now = after_update_window();

        model.connection = ConnectionState::Connecting;
        assert!(!geo_update_due(&model, now));

        model.connection = ConnectionState::ConnectPending;
        assert!(!geo_update_due(&model, now));

        model.connection = ConnectionState::Connected;
        assert!(geo_update_due(&model, now));
    }

    #[test]
    fn geo_auto_update_skips_off_global_and_in_flight() {
        let mut model = model_with_profiles(vec![]);
        let now = Local::now();
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        assert!(!geo_update_due(&model, now));

        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Every1d;
        model
            .config
            .settings
            .geo_routing
            .set_region(GeoRegion::Global);
        assert!(!geo_update_due(&model, now));

        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.geo_updating = true;
        assert!(!geo_update_due(&model, now));
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
    fn a_frozen_daemon_stops_scheduling_but_keeps_running_work() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.config.subscriptions.push(Subscription {
            id: uuid::Uuid::new_v4(),
            name: "Sub".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: Some(Local::now() - chrono::Duration::hours(25)),
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        model.connection = ConnectionState::Connected;
        model.restart_required = true;

        let effects = handle_tick(&mut model);

        assert!(!effects.iter().any(|effect| matches!(
            effect,
            Effect::DownloadGeo
                | Effect::RetryServiceRuleSets { .. }
                | Effect::UpdateSubscription { .. }
        )));
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::FetchTrafficStats { .. })),
            "an already-connected tunnel must keep reporting traffic"
        );
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
        assert!(model.subscription_updates.contains(&sub_id));
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
            retry_state: Some(crate::config::profile::SubscriptionRetryState {
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
        assert!(model.subscription_updates.contains(&sub_id));
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
        assert!(!model.subscription_updates.contains(&sub_id));

        model.connection = ConnectionState::ConnectPending;
        assert!(check_due_subscriptions_at(&mut model, now).is_empty());
        assert!(!model.subscription_updates.contains(&sub_id));

        model.connection = ConnectionState::Connected;
        assert_eq!(
            check_due_subscriptions_at(&mut model, now),
            vec![Effect::UpdateSubscription { id: sub_id }]
        );
        assert!(model.subscription_updates.contains(&sub_id));
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
        assert!(!model.subscription_updates.contains(&sub_id));
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
        assert!(!model.subscription_updates.contains(&sub_id));
    }

    #[test]
    fn tick_dispatches_up_to_4_tests_from_pending() {
        let profiles = sample_profiles();
        // Need 5 profiles — add two more manually.
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
        // Enqueue all 5.
        let key = KeyEvent::new(KeyCode::Char('T'), crossterm::event::KeyModifiers::NONE);
        let _ = update(&mut model, Msg::Key(key));
        assert_eq!(model.pending_tests.len(), 5);
        // Tick should dispatch first 4.
        let effects = update(&mut model, Msg::Tick);
        let test_effects: Vec<_> = effects
            .iter()
            .filter(|e| matches!(e, Effect::TestProfile { .. }))
            .collect();
        assert_eq!(test_effects.len(), 4);
        assert_eq!(model.testing_profiles.len(), 4);
        assert_eq!(model.pending_tests.len(), 1, "one still waiting");
    }

    #[test]
    fn automatic_service_update_can_run_directly_without_kill_switch() {
        let mut model = model_with_profiles(vec![]);
        model
            .config
            .settings
            .geo_routing
            .set_region(GeoRegion::Global);
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Every1d;
        model.config.settings.geo_routing.service_routes.insert(
            RoutedService::Steam,
            crate::config::profile::ServiceRoute::Direct,
        );
        let now = after_update_window();
        model
            .service_next_updates
            .insert(RoutedService::Steam, now.date_naive());

        let effects = handle_tick_at(&mut model, now);
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::RetryServiceRuleSets { services }
                if services == &vec![RoutedService::Steam]
        )));
    }
}

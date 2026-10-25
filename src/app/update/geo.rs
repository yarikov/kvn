use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model, StatusKey, UpdateTrigger};
use crate::config::profile::{GeoAutoUpdate, RoutingFallback};
use crate::geo::{RuleSet, RuleSetOutcome, RuleSetReport, RuleSetStates};

use crate::app::update::connection::queue_connect;
use crate::app::update::status::{DownloadKind, append_download_hint, push_status};

pub(in crate::app::update) fn start_rule_set_download(
    model: &mut Model,
    rule_sets: Vec<RuleSet>,
    trigger: UpdateTrigger,
    only_missing: bool,
) -> Option<Effect> {
    let rule_sets: Vec<RuleSet> = rule_sets
        .into_iter()
        .filter(|rule_set| !model.rule_set_updates.contains_key(&rule_set.key()))
        .collect();
    if rule_sets.is_empty() {
        return None;
    }
    for rule_set in &rule_sets {
        model.rule_set_updates.insert(rule_set.key(), trigger);
    }
    let auto_update_on = model.config.settings.geo_routing.auto_update != GeoAutoUpdate::Off;
    Some(Effect::DownloadRuleSets {
        rule_sets,
        only_missing,
        record_schedule: trigger == UpdateTrigger::Automatic || (only_missing && auto_update_on),
    })
}

pub(in crate::app::update) fn handle_rule_sets_updated(
    model: &mut Model,
    reports: Vec<RuleSetReport>,
    states: RuleSetStates,
    epoch: u64,
) -> Vec<Effect> {
    for report in &reports {
        model.rule_set_updates.remove(&report.key);
    }
    model.apply_rule_set_states(states, epoch);
    let updated: Vec<&RuleSetReport> = reports
        .iter()
        .filter(|report| report.outcome == RuleSetOutcome::Updated)
        .collect();
    let failures: Vec<String> = reports.iter().filter_map(failure_text).collect();

    let mut effects = Vec::new();
    for report in &updated {
        let text = format!("Updated: {}", report.tag);
        effects.push(Effect::AppendAppLog {
            level: "INFO".to_string(),
            message: text.clone(),
        });
        model.logs.push_back(text);
    }
    let status = if !failures.is_empty() {
        AppStatus::Error(format!("Geo update failed: {}", failures.join("; ")))
    } else if !updated.is_empty() {
        AppStatus::Info("Geo databases updated".into())
    } else {
        AppStatus::Info("Geo databases up to date".into())
    };
    push_status(&mut effects, model, StatusKey::Geo, status);
    if !failures.is_empty() {
        append_download_hint(&mut effects, model, DownloadKind::Geo);
    }

    if model.connection == ConnectionState::Connected {
        match model.routing_fallback {
            Some(fallback) => effects.extend(leave_routing_fallback(model, fallback, &reports)),
            None => reconnect_for_updated_rule_sets(model, &updated),
        }
    }
    effects.push(Effect::BroadcastState);
    effects
}

fn failure_text(report: &RuleSetReport) -> Option<String> {
    match &report.outcome {
        RuleSetOutcome::Failed(message) => Some(format!("{}: {message}", report.tag)),
        _ => None,
    }
}

fn reconnect_for_updated_rule_sets(model: &mut Model, updated: &[&RuleSetReport]) {
    let required = model.active_rule_sets();
    let routed_update = updated
        .iter()
        .any(|report| required.iter().any(|rule_set| rule_set.key() == report.key));
    if routed_update
        && let Some(active_id) = model.active_profile_id
        && queue_connect(model, active_id)
    {
        model
            .logs
            .push_back("Reconnecting to apply new geo databases".into());
    }
}

pub(in crate::app::update) fn fetch_skipped_rule_sets(model: &mut Model) -> Vec<Effect> {
    let skipped: Vec<RuleSet> = model
        .active_rule_sets()
        .into_iter()
        .filter(|rule_set| {
            !model
                .rule_set_states
                .get(&rule_set.key())
                .is_some_and(|state| state.present)
        })
        .collect();
    if skipped.is_empty() {
        return Vec::new();
    }
    let mut effects = Vec::new();
    let tags: Vec<&str> = skipped.iter().map(|rule_set| &*rule_set.tag).collect();
    push_status(
        &mut effects,
        model,
        StatusKey::Geo,
        AppStatus::Info(format!("Not routed until downloaded: {}", tags.join(", "))),
    );
    effects.extend(start_rule_set_download(
        model,
        skipped,
        UpdateTrigger::Manual,
        true,
    ));
    effects
}

pub(in crate::app::update) fn fetch_region_rule_sets(
    model: &mut Model,
    fallback: RoutingFallback,
) -> Vec<Effect> {
    let mut effects = Vec::new();
    let mode = model
        .routing
        .mode_name(model.config.settings.geo_routing.mode(&model.routing));
    let consequence = match fallback {
        RoutingFallback::Global => "running in Global while they download through the VPN",
        RoutingFallback::Direct => "nothing uses the VPN until they download",
    };
    push_status(
        &mut effects,
        model,
        StatusKey::Geo,
        AppStatus::Info(format!("{mode} rule-sets are missing — {consequence}")),
    );
    let required = model.active_rule_sets();
    effects.extend(start_rule_set_download(
        model,
        required,
        UpdateTrigger::Manual,
        true,
    ));
    effects
}

fn leave_routing_fallback(
    model: &mut Model,
    fallback: RoutingFallback,
    reports: &[RuleSetReport],
) -> Vec<Effect> {
    let required = model.required_rule_sets();
    if required
        .iter()
        .any(|rule_set| model.rule_set_updates.contains_key(&rule_set.key()))
    {
        return Vec::new();
    }
    let missing: Vec<RuleSet> = required
        .into_iter()
        .filter(|rule_set| {
            !model
                .rule_set_states
                .get(&rule_set.key())
                .is_some_and(|state| state.present)
        })
        .collect();
    if missing.is_empty() {
        if let Some(active_id) = model.active_profile_id {
            queue_connect(model, active_id);
        }
        return Vec::new();
    }
    let failures: Vec<String> = reports
        .iter()
        .filter(|report| missing.iter().any(|rule_set| rule_set.key() == report.key))
        .filter_map(failure_text)
        .collect();
    if failures.is_empty() {
        return Vec::new();
    }
    let mut effects = Vec::new();
    let mode = model
        .routing
        .mode_name(model.config.settings.geo_routing.mode(&model.routing));
    let consequence = match fallback {
        RoutingFallback::Global => "running in Global",
        RoutingFallback::Direct => "nothing uses the VPN",
    };
    push_status(
        &mut effects,
        model,
        StatusKey::Geo,
        AppStatus::Error(format!(
            "{mode} unavailable — {consequence}: {}",
            failures.join("; ")
        )),
    );
    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::msg::Msg;
    use crate::app::update::update;
    use crate::config::profile::{GeoRegion, Profile, RetryState};
    use crate::geo::RuleSetState;
    use crate::test_helpers::*;
    use chrono::Local;

    fn ru_rule_sets() -> Vec<RuleSet> {
        crate::test_helpers::region_rule_sets(GeoRegion::Ru)
    }

    fn report(rule_set: &RuleSet, outcome: RuleSetOutcome) -> RuleSetReport {
        RuleSetReport {
            key: rule_set.key(),
            tag: rule_set.tag.to_string(),
            outcome,
        }
    }

    fn present(rule_sets: &[RuleSet], present: bool) -> RuleSetStates {
        rule_sets
            .iter()
            .map(|rule_set| {
                let state = RuleSetState {
                    present,
                    checked_at: present.then(Local::now),
                    ..RuleSetState::default()
                };
                (rule_set.key(), state)
            })
            .collect()
    }

    fn rule_sets_updated(reports: Vec<RuleSetReport>, states: RuleSetStates) -> Msg {
        Msg::RuleSetsUpdated {
            reports,
            states,
            epoch: 0,
        }
    }

    fn retry_state(consecutive_failures: u32) -> RetryState {
        RetryState {
            consecutive_failures,
            retry_at: Local::now(),
            attempt_date: None,
        }
    }

    fn connected(mode: &str) -> Model {
        let a = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let b = Profile::new_vless("B".into(), "2.2.2.2".into(), 443, "u2".into());
        let active_id = a.id;
        let mut model = model_with_profiles(vec![a, b]);
        model
            .routing
            .add_region_modes(crate::config::profile::GeoRegion::Ru);
        model.config.settings.geo_routing.set_mode(mode);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(active_id);
        model
    }

    #[test]
    fn states_from_before_a_schedule_reset_keep_the_reset_schedule() {
        let mut model = model_with_profiles(vec![]);
        model.geo_schedule_epoch = 2;
        let geoip = ru_rule_sets()[1].clone();
        let reset_next = Some(Local::now().date_naive());
        model.rule_set_states.insert(
            geoip.key(),
            RuleSetState {
                next_update: reset_next,
                ..RuleSetState::default()
            },
        );
        let checked_at = Local::now();
        let stale = RuleSetState {
            present: true,
            checked_at: Some(checked_at),
            retry: Some(retry_state(3)),
            next_update: None,
        };

        let geosite = ru_rule_sets()[0].clone();

        update(
            &mut model,
            Msg::RuleSetsUpdated {
                reports: vec![report(&geoip, RuleSetOutcome::UpToDate)],
                states: RuleSetStates::from([(geoip.key(), stale.clone()), (geosite.key(), stale)]),
                epoch: 1,
            },
        );

        let state = &model.rule_set_states[&geoip.key()];
        assert_eq!(state.next_update, reset_next);
        assert!(state.retry.is_none());
        assert_eq!(state.checked_at, Some(checked_at));
        let unknown_before = &model.rule_set_states[&geosite.key()];
        assert!(unknown_before.retry.is_none());
        assert_eq!(unknown_before.checked_at, Some(checked_at));
    }

    #[test]
    fn states_of_the_current_schedule_update_the_schedule() {
        let mut model = model_with_profiles(vec![]);
        model.geo_schedule_epoch = 2;
        let geoip = ru_rule_sets()[1].clone();
        let failed = RuleSetState {
            retry: Some(retry_state(1)),
            ..RuleSetState::default()
        };

        update(
            &mut model,
            Msg::RuleSetsUpdated {
                reports: vec![report(&geoip, RuleSetOutcome::Failed("down".into()))],
                states: RuleSetStates::from([(geoip.key(), failed.clone())]),
                epoch: 2,
            },
        );

        assert_eq!(model.rule_set_states[&geoip.key()], failed);
    }

    #[test]
    fn refreshed_states_reach_the_model() {
        let mut model = model_with_profiles(vec![]);
        let states = present(&ru_rule_sets(), true);

        let effects = update(
            &mut model,
            Msg::RuleSetStatesRefreshed {
                states: states.clone(),
                epoch: 0,
            },
        );

        assert_eq!(model.rule_set_states, states);
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn an_update_logs_each_updated_rule_set_and_ends_its_download() {
        let mut model = model_with_profiles(vec![]);
        let [geosite, geoip] = <[RuleSet; 2]>::try_from(ru_rule_sets()).unwrap();
        model
            .rule_set_updates
            .insert(geoip.key(), UpdateTrigger::Manual);

        let effects = update(
            &mut model,
            rule_sets_updated(
                vec![
                    report(&geosite, RuleSetOutcome::UpToDate),
                    report(&geoip, RuleSetOutcome::Updated),
                ],
                RuleSetStates::new(),
            ),
        );

        assert!(!model.geo_updating());
        assert_eq!(
            effects,
            vec![
                app_log_info("Updated: geoip-ru"),
                app_log_info("Geo databases updated"),
                Effect::BroadcastState
            ]
        );
    }

    #[test]
    fn an_up_to_date_check_broadcasts_state() {
        let mut model = model_with_profiles(vec![]);
        let effects = update(
            &mut model,
            rule_sets_updated(
                vec![report(&ru_rule_sets()[0], RuleSetOutcome::UpToDate)],
                RuleSetStates::new(),
            ),
        );
        assert_eq!(
            effects,
            vec![
                app_log_info("Geo databases up to date"),
                Effect::BroadcastState
            ]
        );
    }

    #[test]
    fn a_failed_update_reports_each_rule_set_and_the_download_hint() {
        let mut model = model_with_profiles(vec![]);
        let geoip = ru_rule_sets()[1].clone();
        model
            .rule_set_updates
            .insert(geoip.key(), UpdateTrigger::Automatic);

        let effects = update(
            &mut model,
            rule_sets_updated(
                vec![report(&geoip, RuleSetOutcome::Failed("net fail".into()))],
                RuleSetStates::new(),
            ),
        );

        assert!(!model.geo_updating());
        assert_eq!(
            effects,
            vec![
                app_log_error("Geo update failed: geoip-ru: net fail"),
                Effect::AppendAppLog {
                    level: "WARN".into(),
                    message:
                        "VPN is disconnected. Try connecting to VPN and retrying the geo download."
                            .into(),
                },
                Effect::BroadcastState,
            ]
        );
    }

    #[test]
    fn an_update_of_a_routed_rule_set_reconnects_the_active_profile_not_the_cursor() {
        let mut model = connected("bypass_ru");
        let active_id = model.active_profile_id;
        model.select_next();

        update(
            &mut model,
            rule_sets_updated(
                vec![report(&ru_rule_sets()[1], RuleSetOutcome::Updated)],
                RuleSetStates::new(),
            ),
        );

        assert_eq!(model.connecting_profile_id, active_id);
    }

    #[test]
    fn an_update_of_a_rule_set_the_mode_does_not_route_keeps_the_connection() {
        let mut model = connected("global");

        update(
            &mut model,
            rule_sets_updated(
                vec![report(&ru_rule_sets()[1], RuleSetOutcome::Updated)],
                RuleSetStates::new(),
            ),
        );

        assert_eq!(model.connection, ConnectionState::Connected);
    }

    #[test]
    fn a_download_skips_rule_sets_that_are_already_downloading() {
        let mut model = model_with_profiles(vec![]);
        let [geosite, geoip] = <[RuleSet; 2]>::try_from(ru_rule_sets()).unwrap();
        model
            .rule_set_updates
            .insert(geoip.key(), UpdateTrigger::Automatic);

        let effect = start_rule_set_download(
            &mut model,
            vec![geosite.clone(), geoip.clone()],
            UpdateTrigger::Manual,
            false,
        );

        assert_eq!(
            effect,
            Some(Effect::DownloadRuleSets {
                rule_sets: vec![geosite.clone()],
                only_missing: false,
                record_schedule: false,
            })
        );
        assert_eq!(
            model.rule_set_updates[&geoip.key()],
            UpdateTrigger::Automatic
        );
        assert_eq!(
            model.rule_set_updates[&geosite.key()],
            UpdateTrigger::Manual
        );
        assert_eq!(
            start_rule_set_download(
                &mut model,
                vec![geosite.clone(), geoip.clone()],
                UpdateTrigger::Manual,
                false
            ),
            None
        );
    }

    #[test]
    fn a_region_change_during_a_scheduled_download_keeps_the_download_in_flight() {
        let mut model = model_with_profiles(vec![]);
        model
            .routing
            .add_region_modes(crate::config::profile::GeoRegion::Ru);
        let ru = ru_rule_sets();
        for rule_set in &ru {
            model
                .rule_set_updates
                .insert(rule_set.key(), UpdateTrigger::Automatic);
        }

        let effects = crate::app::update::routing::commit_geo_region(&mut model, GeoRegion::Cn);
        let cn = crate::test_helpers::region_rule_sets(GeoRegion::Cn);
        assert!(effects.contains(&Effect::DownloadRuleSets {
            rule_sets: cn.clone(),
            only_missing: true,
            record_schedule: false,
        }));

        update(
            &mut model,
            rule_sets_updated(
                ru.iter()
                    .map(|rule_set| report(rule_set, RuleSetOutcome::UpToDate))
                    .collect(),
                present(&ru, true),
            ),
        );

        assert!(model.geo_updating());
        assert!(
            cn.iter()
                .all(|rule_set| model.rule_set_updates.contains_key(&rule_set.key()))
        );
    }

    #[test]
    fn a_late_result_for_the_previous_region_leaves_the_current_one_alone() {
        let mut model = model_with_profiles(vec![]);
        model
            .routing
            .add_region_modes(crate::config::profile::GeoRegion::Cn);
        let cn = crate::test_helpers::region_rule_sets(GeoRegion::Cn);
        let cn_states = present(&cn, false);
        model.rule_set_states = cn_states.clone();
        let checked_before = model.selectable_rule_sets_checked_at();

        update(
            &mut model,
            rule_sets_updated(
                vec![report(&ru_rule_sets()[1], RuleSetOutcome::Updated)],
                present(&ru_rule_sets()[1..], true),
            ),
        );

        assert!(
            cn.iter().all(
                |rule_set| model.rule_set_states[&rule_set.key()] == cn_states[&rule_set.key()]
            )
        );
        assert_eq!(model.selectable_rule_sets_checked_at(), checked_before);
    }

    fn connected_in_fallback(downloading: bool) -> (Model, Vec<Effect>) {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model
            .routing
            .add_region_modes(crate::config::profile::GeoRegion::Ru);
        model.config.settings.geo_routing.set_mode("bypass_ru");
        model.connection = ConnectionState::ConnectPending;
        if downloading {
            for rule_set in ru_rule_sets() {
                model
                    .rule_set_updates
                    .insert(rule_set.key(), UpdateTrigger::Automatic);
            }
        }
        let effects = update(
            &mut model,
            Msg::Connected {
                pid: 1,
                profile_id,
                attempt_id: 0,
                routing_fallback: Some(RoutingFallback::Global),
            },
        );
        (model, effects)
    }

    fn region_failed() -> Msg {
        rule_sets_updated(
            ru_rule_sets()
                .iter()
                .map(|rule_set| report(rule_set, RuleSetOutcome::Failed("net fail".into())))
                .collect(),
            present(&ru_rule_sets(), false),
        )
    }

    #[test]
    fn fallback_connection_downloads_its_missing_rule_sets_through_the_tunnel() {
        let (model, effects) = connected_in_fallback(false);
        assert_eq!(model.routing_fallback, Some(RoutingFallback::Global));
        assert!(effects.contains(&Effect::DownloadRuleSets {
            rule_sets: ru_rule_sets(),
            only_missing: true,
            record_schedule: false,
        }));
        assert!(model.geo_updating());
        assert_eq!(
            model.status_text(),
            "Bypass RU rule-sets are missing — running in Global while they download through the VPN"
        );
    }

    #[test]
    fn direct_fallback_says_nothing_uses_the_vpn() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model
            .routing
            .add_region_modes(crate::config::profile::GeoRegion::Ru);
        model.config.settings.geo_routing.set_mode("only_ru");
        model.connection = ConnectionState::ConnectPending;
        update(
            &mut model,
            Msg::Connected {
                pid: 1,
                profile_id,
                attempt_id: 0,
                routing_fallback: Some(RoutingFallback::Direct),
            },
        );
        assert_eq!(
            model.status_text(),
            "Only RU rule-sets are missing — nothing uses the VPN until they download"
        );

        update(&mut model, region_failed());
        assert_eq!(
            model.status_text(),
            "Only RU unavailable — nothing uses the VPN: geosite-category-ru: net fail; geoip-ru: net fail"
        );
    }

    #[test]
    fn fallback_connection_waits_for_a_download_of_its_rule_sets_already_running() {
        let (mut model, effects) = connected_in_fallback(true);
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::DownloadRuleSets { .. }))
        );

        let effects = update(&mut model, region_failed());

        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::DownloadRuleSets { .. }))
        );
        assert_eq!(model.connection, ConnectionState::Connected);
        assert!(
            model
                .status_text()
                .starts_with("Bypass RU unavailable — running in Global: ")
        );
    }

    #[test]
    fn fallback_connection_reconnects_once_every_rule_set_is_present() {
        let (mut model, _) = connected_in_fallback(false);
        let [geosite, geoip] = <[RuleSet; 2]>::try_from(ru_rule_sets()).unwrap();

        update(
            &mut model,
            rule_sets_updated(
                vec![report(&geosite, RuleSetOutcome::Updated)],
                present(std::slice::from_ref(&geosite), true),
            ),
        );
        assert_eq!(model.connection, ConnectionState::Connected);

        update(
            &mut model,
            rule_sets_updated(
                vec![report(&geoip, RuleSetOutcome::Updated)],
                present(std::slice::from_ref(&geoip), true),
            ),
        );
        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, model.active_profile_id);
    }

    #[test]
    fn a_connection_names_and_downloads_the_optional_rule_sets_it_skipped() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model
            .routing
            .add_region_modes(crate::config::profile::GeoRegion::Ru);
        model.config.settings.geo_routing.set_mode("bypass_ru");
        model
            .routing
            .adopt_service_routes(&serde_json::json!({ "steam": "direct" }));
        model.apply_rule_set_states(present(&ru_rule_sets(), true), model.geo_schedule_epoch);
        model.connection = ConnectionState::ConnectPending;
        let steam: Vec<RuleSet> = ["geosite-steam", "geoip-steam"]
            .map(|tag| model.routing.rule_set(tag).unwrap())
            .to_vec();

        let effects = update(
            &mut model,
            Msg::Connected {
                pid: 1,
                profile_id,
                attempt_id: 0,
                routing_fallback: None,
            },
        );

        assert!(effects.contains(&Effect::DownloadRuleSets {
            rule_sets: steam,
            only_missing: true,
            record_schedule: false,
        }));
        assert!(effects.contains(&app_log_info(
            "Not routed until downloaded: geosite-steam, geoip-steam"
        )));
    }

    #[test]
    fn fallback_connection_does_not_wait_for_an_optional_rule_set() {
        let (mut model, _) = connected_in_fallback(false);
        let [geosite, geoip] = <[RuleSet; 2]>::try_from(ru_rule_sets()).unwrap();
        model
            .routing
            .modes
            .iter_mut()
            .find(|mode| mode.id == "bypass_ru")
            .unwrap()
            .rules[1]
            .required = false;

        update(
            &mut model,
            rule_sets_updated(
                vec![
                    report(&geosite, RuleSetOutcome::Failed("net fail".into())),
                    report(&geoip, RuleSetOutcome::Updated),
                ],
                present(std::slice::from_ref(&geoip), true),
            ),
        );

        assert_eq!(model.connection, ConnectionState::Connecting);
    }
}

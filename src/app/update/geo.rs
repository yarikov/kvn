use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model, StatusKey};
use crate::app::msg::GeoResult;
use crate::config::profile::RoutingFallback;

use crate::app::update::connection::queue_connect;
use crate::app::update::status::{DownloadKind, append_download_hint, push_status};

pub(in crate::app::update) fn handle_geo_result(
    model: &mut Model,
    result: GeoResult,
    region_ready: bool,
) -> Vec<Effect> {
    let pending_geo_reconnect = std::mem::take(&mut model.pending_geo_reconnect);
    let failure = match &result {
        GeoResult::Error { message, .. } => Some(message.clone()),
        _ => None,
    };
    model.geo_updating = pending_geo_reconnect && model.pending_service_reconnect;
    model.geo_automatic_update = false;
    let mut effects = match result {
        GeoResult::Updated {
            parts,
            checked_at,
            service_checked_at,
            schedule,
            warnings,
        } => {
            model.service_checked_at = service_checked_at;
            if let Some(schedule) = schedule {
                model.apply_geo_schedule(schedule);
            }
            model.geo_last_updated = Some(checked_at.format("%d %b %H:%M").to_string());
            model.geo_last_checked_at = Some(checked_at);
            let mut log_effects = Vec::new();
            for part in &parts {
                let text = format!("Updated: {}", part);
                log_effects.push(Effect::AppendAppLog {
                    level: "INFO".to_string(),
                    message: text.clone(),
                });
                model.logs.push_back(text);
            }
            for warning in &warnings {
                log_effects.push(Effect::AppendAppLog {
                    level: "ERROR".to_string(),
                    message: format!("Geo batch partial failure: {warning}"),
                });
            }
            if !warnings.is_empty() {
                append_download_hint(&mut log_effects, model, DownloadKind::Geo);
            }
            let status = if warnings.is_empty() {
                AppStatus::Info("Geo databases updated".into())
            } else {
                AppStatus::Error(format!(
                    "Geo update partially failed: {}",
                    warnings.join("; ")
                ))
            };
            push_status(&mut log_effects, model, StatusKey::Geo, status);
            if !pending_geo_reconnect
                && model.connection == ConnectionState::Connected
                && let Some(active_id) = model.active_profile_id
                && queue_connect(model, active_id)
            {
                model
                    .logs
                    .push_back("Reconnecting to apply new geo databases".into());
            }
            log_effects
        }
        GeoResult::UpToDate {
            checked_at,
            service_checked_at,
            schedule,
            warnings,
        } => {
            model.service_checked_at = service_checked_at;
            if let Some(schedule) = schedule {
                model.apply_geo_schedule(schedule);
            }
            model.geo_last_checked_at = checked_at;
            if let Some(checked_at) = checked_at {
                model.geo_last_updated = Some(checked_at.format("%d %b %H:%M").to_string());
            }
            let mut effects = Vec::new();
            let status = if warnings.is_empty() {
                AppStatus::Info("Geo databases up to date".into())
            } else {
                AppStatus::Error(format!(
                    "Service rule-set update failed: {}",
                    warnings.join("; ")
                ))
            };
            push_status(&mut effects, model, StatusKey::Geo, status);
            if !warnings.is_empty() {
                append_download_hint(&mut effects, model, DownloadKind::Geo);
            }
            effects
        }
        GeoResult::Error {
            message,
            service_checked_at,
            schedule,
            updated_parts,
        } => {
            model.service_checked_at = service_checked_at;
            if let Some(schedule) = schedule {
                model.apply_geo_schedule(schedule);
            }
            let mut effects = Vec::new();
            let has_updates = !updated_parts.is_empty();
            push_status(
                &mut effects,
                model,
                StatusKey::Geo,
                crate::app::model::AppStatus::Error(format!("Geo update failed: {message}")),
            );
            append_download_hint(&mut effects, model, DownloadKind::Geo);
            for part in updated_parts {
                effects.push(Effect::AppendAppLog {
                    level: "INFO".to_string(),
                    message: format!("Updated: {part}"),
                });
            }
            if !pending_geo_reconnect
                && has_updates
                && model.connection == ConnectionState::Connected
                && let Some(active_id) = model.active_profile_id
            {
                queue_connect(model, active_id);
            }
            effects
        }
    };
    if pending_geo_reconnect
        && !model.pending_service_reconnect
        && model.connection == ConnectionState::Connected
        && let Some(active_id) = model.active_profile_id
    {
        queue_connect(model, active_id);
    }
    if let Some(fallback) = model.routing_fallback
        && model.connection == ConnectionState::Connected
    {
        effects.extend(leave_routing_fallback(
            model,
            fallback,
            region_ready,
            failure,
        ));
    }
    effects.push(Effect::BroadcastState);
    effects
}

pub(in crate::app::update) fn fetch_region_rule_sets(
    model: &mut Model,
    fallback: RoutingFallback,
) -> Vec<Effect> {
    let mut effects = Vec::new();
    let mode = model.config.settings.geo_routing.mode();
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
    if model.geo_updating {
        model.region_download_deferred = true;
    } else {
        effects.extend(start_region_rule_sets_download(model));
    }
    effects
}

fn leave_routing_fallback(
    model: &mut Model,
    fallback: RoutingFallback,
    region_ready: bool,
    failure: Option<String>,
) -> Vec<Effect> {
    if region_ready {
        if let Some(active_id) = model.active_profile_id {
            queue_connect(model, active_id);
        }
        return Vec::new();
    }
    if let Some(effects) = start_deferred_region_download(model) {
        return effects;
    }
    match failure {
        None => Vec::new(),
        Some(message) => {
            let mut effects = Vec::new();
            let mode = model.config.settings.geo_routing.mode();
            let consequence = match fallback {
                RoutingFallback::Global => "running in Global",
                RoutingFallback::Direct => "nothing uses the VPN",
            };
            push_status(
                &mut effects,
                model,
                StatusKey::Geo,
                AppStatus::Error(format!("{mode} unavailable — {consequence}: {message}")),
            );
            effects
        }
    }
}

pub(in crate::app::update) fn start_deferred_region_download(
    model: &mut Model,
) -> Option<Vec<Effect>> {
    let deferred = std::mem::take(&mut model.region_download_deferred);
    (deferred && model.routing_fallback.is_some() && model.connection == ConnectionState::Connected)
        .then(|| start_region_rule_sets_download(model))
}

fn start_region_rule_sets_download(model: &mut Model) -> Vec<Effect> {
    model.geo_updating = true;
    model.geo_last_attempt_at = Some(chrono::Local::now());
    vec![Effect::DownloadGeoIfMissing]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::msg::{GeoSchedule, Msg};
    use crate::app::update::update;
    use crate::config::profile::{GeoRegion, Profile, RoutedService, RoutingMode};
    use crate::geo::GeoRetryState;
    use crate::test_helpers::*;
    use chrono::Local;
    use std::collections::HashMap;

    fn stale_retry_state() -> GeoRetryState {
        GeoRetryState {
            consecutive_failures: 3,
            retry_at: Local::now(),
            attempt_date: None,
        }
    }

    #[test]
    fn geo_result_from_before_a_schedule_reset_keeps_the_reset_schedule() {
        let mut model = model_with_profiles(vec![]);
        model.geo_schedule_epoch = 2;
        let reset_next = Some(Local::now().date_naive());
        model.geo_next_update = reset_next;
        let checked_at = Local::now();

        update(
            &mut model,
            Msg::GeoUpdated {
                result: GeoResult::UpToDate {
                    checked_at: Some(checked_at),
                    service_checked_at: Default::default(),
                    schedule: Some(GeoSchedule {
                        epoch: 1,
                        retry_state: Some(stale_retry_state()),
                        service_retry_states: HashMap::from([(
                            RoutedService::Steam,
                            stale_retry_state(),
                        )]),
                        ..Default::default()
                    }),
                    warnings: Vec::new(),
                },
                region_ready: false,
            },
        );

        assert_eq!(model.geo_next_update, reset_next);
        assert!(model.geo_retry_state.is_none());
        assert!(model.service_retry_states.is_empty());
        assert_eq!(model.geo_last_checked_at, Some(checked_at));
    }

    #[test]
    fn geo_result_of_the_current_schedule_updates_the_schedule() {
        let mut model = model_with_profiles(vec![]);
        model.geo_schedule_epoch = 2;
        let retry_state = stale_retry_state();

        update(
            &mut model,
            Msg::GeoUpdated {
                result: GeoResult::Error {
                    message: "unavailable".into(),
                    service_checked_at: Default::default(),
                    schedule: Some(GeoSchedule {
                        epoch: 2,
                        retry_state: Some(retry_state),
                        ..Default::default()
                    }),
                    updated_parts: Vec::new(),
                },
                region_ready: false,
            },
        );

        assert_eq!(model.geo_retry_state, Some(retry_state));
    }

    #[test]
    fn geo_last_updated_message_updates_model() {
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = None;
        let effects = update(
            &mut model,
            Msg::GeoMetadataRefreshed {
                last_updated: Some("2026-06-15 08:00".to_string()),
                last_checked_at: None,
                service_checked_at: Default::default(),
                schedule: None,
            },
        );
        assert_eq!(model.geo_last_updated, Some("2026-06-15 08:00".to_string()));
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn geo_result_updated_broadcasts_state() {
        let mut model = model_with_profiles(vec![]);
        model.geo_updating = true;
        let effects = update(
            &mut model,
            Msg::GeoUpdated {
                result: GeoResult::Updated {
                    parts: vec!["geoip".into()],
                    checked_at: Local::now(),
                    service_checked_at: Default::default(),
                    schedule: None,
                    warnings: Vec::new(),
                },
                region_ready: false,
            },
        );
        assert!(!model.geo_updating);
        assert_eq!(
            effects,
            vec![
                app_log_info("Updated: geoip"),
                app_log_info("Geo databases updated"),
                Effect::BroadcastState
            ]
        );
    }

    #[test]
    fn geo_update_queues_active_profile_not_cursor() {
        let a = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let b = Profile::new_vless("B".into(), "2.2.2.2".into(), 443, "u2".into());
        let active_id = a.id;
        let mut model = model_with_profiles(vec![a, b]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(active_id);
        model.select_next();

        update(
            &mut model,
            Msg::GeoUpdated {
                result: GeoResult::Updated {
                    parts: vec!["geoip".into()],
                    checked_at: Local::now(),
                    service_checked_at: Default::default(),
                    schedule: None,
                    warnings: Vec::new(),
                },
                region_ready: false,
            },
        );

        assert_eq!(model.connecting_profile_id, Some(active_id));
    }

    #[test]
    fn geo_result_up_to_date_broadcasts_state() {
        let mut model = model_with_profiles(vec![]);
        model.geo_updating = true;
        let effects = update(
            &mut model,
            Msg::GeoUpdated {
                result: GeoResult::UpToDate {
                    checked_at: Some(Local::now()),
                    service_checked_at: Default::default(),
                    schedule: None,
                    warnings: Vec::new(),
                },
                region_ready: false,
            },
        );
        assert!(!model.geo_updating);
        assert_eq!(
            effects,
            vec![
                app_log_info("Geo databases up to date"),
                Effect::BroadcastState
            ]
        );
    }

    fn connected_in_fallback(geo_updating: bool, steam_routed: bool) -> (Model, Vec<Effect>) {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model
            .config
            .settings
            .geo_routing
            .set_mode(RoutingMode::Bypass(GeoRegion::Ru));
        if steam_routed {
            model.config.settings.geo_routing.service_routes.insert(
                RoutedService::Steam,
                crate::config::profile::ServiceRoute::Direct,
            );
        }
        model.connection = ConnectionState::ConnectPending;
        model.geo_updating = geo_updating;
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

    fn geo_error() -> Msg {
        Msg::GeoUpdated {
            result: GeoResult::Error {
                message: "net fail".into(),
                service_checked_at: Default::default(),
                schedule: None,
                updated_parts: Vec::new(),
            },
            region_ready: false,
        }
    }

    #[test]
    fn fallback_connection_downloads_region_rule_sets_through_the_tunnel() {
        let (model, effects) = connected_in_fallback(false, false);
        assert_eq!(model.routing_fallback, Some(RoutingFallback::Global));
        assert!(model.geo_updating);
        assert!(effects.contains(&Effect::DownloadGeoIfMissing));
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
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model
            .config
            .settings
            .geo_routing
            .set_mode(RoutingMode::Only(GeoRegion::Ru));
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

        update(&mut model, geo_error());
        assert_eq!(
            model.status_text(),
            "Only RU unavailable — nothing uses the VPN: net fail"
        );
    }

    #[test]
    fn fallback_connection_retries_a_running_download_once_it_fails() {
        let (mut model, effects) = connected_in_fallback(true, false);
        assert!(!effects.contains(&Effect::DownloadGeoIfMissing));

        let effects = update(&mut model, geo_error());
        assert!(effects.contains(&Effect::DownloadGeoIfMissing));
        assert!(model.geo_updating);

        let effects = update(&mut model, geo_error());
        assert!(!effects.contains(&Effect::DownloadGeoIfMissing));
        assert_eq!(model.connection, ConnectionState::Connected);
        assert_eq!(
            model.status_text(),
            "Bypass RU unavailable — running in Global: net fail"
        );
    }

    #[test]
    fn fallback_connection_reconnects_once_region_rule_sets_are_present() {
        let (mut model, _) = connected_in_fallback(false, false);
        update(&mut model, geo_up_to_date(true));
        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, model.active_profile_id);
    }

    #[test]
    fn fallback_connection_stays_in_global_while_region_rule_sets_are_absent() {
        let (mut model, _) = connected_in_fallback(false, false);
        let effects = update(&mut model, geo_up_to_date(false));
        assert_eq!(model.connection, ConnectionState::Connected);
        assert!(!effects.contains(&Effect::DownloadGeoIfMissing));
    }

    #[test]
    fn fallback_connection_leaves_service_rule_sets_to_the_reconnect() {
        let (_, effects) = connected_in_fallback(false, true);
        assert!(!effects.contains(&Effect::DownloadServiceRuleSetsIfMissing));
        assert!(effects.contains(&Effect::DownloadGeoIfMissing));
    }

    #[test]
    fn fallback_connection_starts_its_deferred_download_after_service_rule_sets() {
        let (mut model, _) = connected_in_fallback(true, false);
        let effects = update(
            &mut model,
            Msg::ServiceRuleSetsReady {
                checked_at: Default::default(),
                schedule: None,
                updated_parts: Vec::new(),
                errors: Vec::new(),
            },
        );
        assert!(effects.contains(&Effect::DownloadGeoIfMissing));
        assert!(model.geo_updating);
    }

    fn geo_up_to_date(region_ready: bool) -> Msg {
        Msg::GeoUpdated {
            result: GeoResult::UpToDate {
                checked_at: Some(Local::now()),
                service_checked_at: Default::default(),
                schedule: None,
                warnings: Vec::new(),
            },
            region_ready,
        }
    }

    #[test]
    fn geo_result_error_broadcasts_state() {
        let mut model = model_with_profiles(vec![]);
        model.geo_updating = true;
        let retry_state = crate::geo::GeoRetryState {
            consecutive_failures: 1,
            retry_at: Local::now() + chrono::Duration::minutes(1),
            attempt_date: None,
        };
        let effects = update(
            &mut model,
            Msg::GeoUpdated {
                result: GeoResult::Error {
                    message: "net fail".into(),
                    service_checked_at: Default::default(),
                    schedule: Some(GeoSchedule {
                        retry_state: Some(retry_state),
                        ..Default::default()
                    }),
                    updated_parts: Vec::new(),
                },
                region_ready: false,
            },
        );
        assert!(!model.geo_updating);
        assert_eq!(model.geo_retry_state, Some(retry_state));
        assert_eq!(
            effects,
            vec![
                app_log_error("Geo update failed: net fail"),
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
}

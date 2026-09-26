use std::collections::HashMap;

use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model, RoutingSettingsDraft};
use crate::config::profile::{GeoRegion, RoutedService, RoutingMode};

use crate::app::update::connection::queue_connect;
use crate::app::update::status::{
    DownloadKind, append_download_hint, download_allowed, push_download_blocked, push_status,
};

pub(in crate::app::update) fn on_service_rule_sets_ready(
    model: &mut Model,
    retry_states: HashMap<RoutedService, crate::geo::GeoRetryState>,
    checked_at: HashMap<RoutedService, chrono::DateTime<chrono::Local>>,
    next_updates: HashMap<RoutedService, chrono::NaiveDate>,
    updated_parts: Vec<String>,
    errors: Vec<String>,
) -> Vec<Effect> {
    model.geo_updating = model.pending_geo_reconnect;
    model.service_retry_states = retry_states;
    model.service_checked_at = checked_at;
    model.service_next_updates = next_updates;
    let mut effects = Vec::new();
    for part in updated_parts {
        effects.push(Effect::AppendAppLog {
            level: "INFO".to_string(),
            message: format!("Updated: {part}"),
        });
    }
    if !errors.is_empty() {
        push_status(
            &mut effects,
            model,
            AppStatus::Error(format!(
                "Service rule-set update failed: {}",
                errors.join("; ")
            )),
        );
        append_download_hint(&mut effects, model, DownloadKind::Geo);
    }
    if !model.pending_service_reconnect {
        if !effects.is_empty() {
            effects.push(Effect::BroadcastState);
        }
        return effects;
    }
    model.pending_service_reconnect = false;
    if model.pending_geo_reconnect {
        effects.push(Effect::BroadcastState);
        return effects;
    }
    if model.connection != ConnectionState::Connected {
        // Disconnected while the download ran — the files are on
        // disk and apply on whatever connect happens next.
        effects.push(Effect::BroadcastState);
        return effects;
    }
    effects.push(Effect::BroadcastState);
    // Reconnect the ACTIVE profile explicitly — never the cursor's
    // row, which may have moved since the routing change.
    let active = model
        .active_profile_id
        .and_then(|id| model.config.profiles.iter().find(|p| p.id == id).cloned());
    if let Some(profile) = active {
        queue_connect(model, profile.id);
        push_status(
            &mut effects,
            model,
            AppStatus::Info("Service routing changed — reconnecting".into()),
        );
    } else {
        push_status(
            &mut effects,
            model,
            AppStatus::Info("Service routing changed — applies after reconnect".into()),
        );
    }
    effects
}

/// Commit a routing-mode change: shared by the routing overlay's Enter key
/// and the `SetRoutingMode` IPC command so both run identical logic.
/// Rejects modes unavailable for the current geo region.
pub(in crate::app::update) fn commit_routing_mode(
    model: &mut Model,
    mode: RoutingMode,
) -> Vec<Effect> {
    let region = model.config.settings.geo_routing.current_region;
    if !RoutingMode::available(region).contains(&mode) {
        let mut effects = vec![];
        push_status(
            &mut effects,
            model,
            AppStatus::Error(format!(
                "Routing mode change failed: {mode} is unavailable for region {}",
                region.map(|r| r.code_upper()).unwrap_or("GLOBAL")
            )),
        );
        return effects;
    }
    let changed = model.config.settings.geo_routing.mode() != mode;
    model.config.settings.geo_routing.set_mode(mode);
    let mut effects =
        super::onboarding::outcome(model, super::onboarding::Trigger::RoutingCommitted);
    effects.push(Effect::SaveConfig);
    let reconnecting = if changed && model.connection == ConnectionState::Connected {
        model
            .active_profile_id
            .is_some_and(|active_id| queue_connect(model, active_id))
    } else {
        false
    };
    let status = if reconnecting {
        format!("Routing mode changed: {mode} — reconnecting")
    } else {
        format!("Routing mode changed: {mode}")
    };
    push_status(&mut effects, model, AppStatus::Info(status));
    effects
}

pub(in crate::app::update) fn commit_routing_settings(
    model: &mut Model,
    draft: RoutingSettingsDraft,
) -> Vec<Effect> {
    if !RoutingMode::available(Some(draft.region)).contains(&draft.mode) {
        let mut effects = vec![];
        push_status(
            &mut effects,
            model,
            AppStatus::Error(format!(
                "Routing mode change failed: {} is unavailable for region {}",
                draft.mode,
                draft.region.code_upper()
            )),
        );
        return effects;
    }

    let old_region = model.config.settings.geo_routing.current_region;
    let old_mode = model.config.settings.geo_routing.mode();
    let old_routes = model.config.settings.geo_routing.service_routes.clone();
    let region_changed = old_region != Some(draft.region);
    let mode_changed = old_mode != draft.mode;
    let service_routes_changed = old_routes != draft.service_routes;
    if !region_changed && !mode_changed && !service_routes_changed {
        return vec![];
    }

    if region_changed && let Some(region) = old_region {
        model
            .config
            .settings
            .geo_routing
            .selected_region_modes
            .insert(region, old_mode);
    }
    model.config.settings.geo_routing.set_region(draft.region);
    model.config.settings.geo_routing.set_mode(draft.mode);
    model.config.settings.geo_routing.service_routes = draft.service_routes;

    let connection = model.connection;
    let mut effects = vec![Effect::SaveConfig, Effect::BroadcastState];
    push_status(
        &mut effects,
        model,
        AppStatus::Info("Routing settings changed".into()),
    );

    if region_changed {
        effects.push(Effect::RefreshGeoLastUpdated);
        if draft.region != GeoRegion::Global {
            if download_allowed(model) {
                model.geo_updating = true;
                model.geo_last_attempt_at = Some(chrono::Local::now());
                if connection == ConnectionState::Connected && service_routes_changed {
                    model.pending_geo_reconnect = true;
                }
                effects.push(Effect::DownloadGeoIfMissing);
            } else {
                push_download_blocked(&mut effects, model, DownloadKind::Geo);
            }
        }
    }

    if service_routes_changed {
        match connection {
            ConnectionState::Connected => {
                model.pending_service_reconnect = true;
                effects.push(Effect::DownloadServiceRuleSetsIfMissing);
            }
            ConnectionState::Connecting | ConnectionState::ConnectPending => {
                push_status(
                    &mut effects,
                    model,
                    AppStatus::Info("Routing settings changed — applies after reconnect".into()),
                );
            }
            _ if !model
                .config
                .settings
                .geo_routing
                .enabled_services()
                .is_empty() =>
            {
                if download_allowed(model) {
                    effects.push(Effect::DownloadServiceRuleSetsIfMissing);
                } else {
                    push_download_blocked(&mut effects, model, DownloadKind::Geo);
                }
            }
            _ => {}
        }
    }

    if connection == ConnectionState::Connected && !service_routes_changed {
        if let Some(active_id) = model.active_profile_id {
            queue_connect(model, active_id);
        }
    } else if connection == ConnectionState::Idle
        && region_changed
        && model.config.settings.auto_connect
        && let Some(profile_id) = model.config.settings.last_connected_profile
        && let Some(idx) = model
            .config
            .profiles
            .iter()
            .position(|profile| profile.id == profile_id)
    {
        model.selected = crate::app::model::row_for_profile(&model.config, idx);
        queue_connect(model, profile_id);
    }

    effects
}

/// Commit a geo-region switch: shared by the region overlay's Enter key and
/// the `SetGeoRegion` IPC command. Persists the old region's routing mode,
/// restores the new region's stored mode, kicks off missing-database
/// downloads, and reconnects/auto-connects as the overlay does.
pub(in crate::app::update) fn commit_geo_region(
    model: &mut Model,
    region: GeoRegion,
) -> Vec<Effect> {
    let old_region = model.config.settings.geo_routing.current_region;
    let old_mode = model.config.settings.geo_routing.mode();
    let changed = old_region != Some(region);
    model.config.settings.geo_routing.set_region(region);
    let mut effects =
        super::onboarding::outcome(model, super::onboarding::Trigger::RegionCommitted);
    effects.push(Effect::SaveConfig);
    if changed {
        effects.push(Effect::RefreshGeoLastUpdated);
    }
    push_status(
        &mut effects,
        model,
        AppStatus::Info(format!("Geo region selected: {}", region.as_str())),
    );

    // If the region changed and is not Global, check whether geo databases
    // are present and download them automatically if they are missing.
    if changed && region != GeoRegion::Global {
        if download_allowed(model) {
            model.geo_updating = true;
            model.geo_last_attempt_at = Some(chrono::Local::now());
            push_status(
                &mut effects,
                model,
                AppStatus::Info("Checking geo databases...".to_string()),
            );
            effects.push(Effect::DownloadGeoIfMissing);
        } else {
            push_download_blocked(&mut effects, model, DownloadKind::Geo);
        }
    }

    // Persist the previously active routing mode under the old region
    // and restore the mode stored for the newly selected region.
    if changed {
        if let Some(old_region) = old_region {
            model
                .config
                .settings
                .geo_routing
                .selected_region_modes
                .insert(old_region, old_mode);
        }
        let new_mode = model.config.settings.geo_routing.mode();
        if new_mode != old_mode {
            push_status(
                &mut effects,
                model,
                AppStatus::Info(format!("Routing mode changed: {new_mode}")),
            );
        }
    }

    // Trigger auto-connect immediately after picking a region
    // so the user does not have to restart the app.
    if model.connection == ConnectionState::Idle
        && model.config.settings.auto_connect
        && let Some(profile_id) = model.config.settings.last_connected_profile
        && let Some(idx) = model
            .config
            .profiles
            .iter()
            .position(|p| p.id == profile_id)
    {
        model.selected = crate::app::model::row_for_profile(&model.config, idx);
        queue_connect(model, profile_id);
        if let Some(profile) = model.config.profiles.get(idx) {
            push_status(
                &mut effects,
                model,
                AppStatus::Info(format!("Auto-connecting to {}…", profile.name)),
            );
        }
    }

    if changed
        && model.connection == ConnectionState::Connected
        && let Some(active_id) = model.active_profile_id
        && queue_connect(model, active_id)
    {
        model.logs.push_back("Region changed — reconnecting".into());
    }
    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::msg::{GeoResult, Msg};
    use crate::app::update::tick::handle_tick;
    use crate::app::update::update;
    use crate::config::profile::Profile;
    use crate::test_helpers::*;
    use chrono::Local;

    #[test]
    fn service_rule_sets_ready_reconnects_active_profile_not_cursor() {
        let a = Profile::new_vless("A".into(), "e".into(), 1, "u".into());
        let b = Profile::new_vless("B".into(), "e".into(), 2, "u".into());
        let active_id = a.id;
        let mut model = model_with_profiles(vec![a, b]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(active_id);
        model.pending_service_reconnect = true;
        // Cursor rests on profile B — the reconnect must still target A.
        model.select_next();
        update(
            &mut model,
            Msg::ServiceRuleSetsReady {
                retry_states: Default::default(),
                checked_at: Default::default(),
                next_updates: Default::default(),
                updated_parts: Vec::new(),
                errors: Vec::new(),
            },
        );
        assert!(!model.pending_service_reconnect, "flag is consumed");
        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(active_id));
        let tick_effects = handle_tick(&mut model);
        let connect_id = tick_effects.iter().find_map(|e| match e {
            Effect::Connect { profile, .. } => Some(profile.id),
            _ => None,
        });
        assert_eq!(
            connect_id,
            Some(active_id),
            "must reconnect the active profile, not the cursor's"
        );
    }

    #[test]
    fn routing_reconnect_waits_for_geo_and_service_downloads_in_any_order() {
        fn geo_ready() -> Msg {
            Msg::GeoUpdated(GeoResult::UpToDate {
                checked_at: Some(Local::now()),
                retry_state: None,
                service_retry_states: Default::default(),
                service_checked_at: Default::default(),
                next_update: None,
                service_next_updates: Default::default(),
                warnings: Vec::new(),
            })
        }

        fn services_ready() -> Msg {
            Msg::ServiceRuleSetsReady {
                retry_states: Default::default(),
                checked_at: Default::default(),
                next_updates: Default::default(),
                updated_parts: Vec::new(),
                errors: Vec::new(),
            }
        }

        for geo_first in [true, false] {
            let profile = Profile::new_vless("A".into(), "e".into(), 1, "u".into());
            let active_id = profile.id;
            let mut model = model_with_profiles(vec![profile]);
            model.connection = ConnectionState::Connected;
            model.active_profile_id = Some(active_id);
            let mut service_routes = std::collections::HashMap::new();
            service_routes.insert(
                RoutedService::Steam,
                crate::config::profile::ServiceRoute::Proxy,
            );
            let effects = commit_routing_settings(
                &mut model,
                RoutingSettingsDraft {
                    region: GeoRegion::Ru,
                    mode: RoutingMode::Global,
                    service_routes,
                },
            );

            assert!(effects.contains(&Effect::DownloadGeoIfMissing));
            assert!(effects.contains(&Effect::DownloadServiceRuleSetsIfMissing));
            assert!(model.pending_geo_reconnect);
            assert!(model.pending_service_reconnect);

            if geo_first {
                update(&mut model, geo_ready());
                assert_eq!(model.connection, ConnectionState::Connected);
                assert!(model.geo_updating);
                update(&mut model, services_ready());
            } else {
                update(&mut model, services_ready());
                assert_eq!(model.connection, ConnectionState::Connected);
                assert!(model.geo_updating);
                update(&mut model, geo_ready());
            }

            assert_eq!(model.connection, ConnectionState::Connecting);
            assert_eq!(model.connecting_profile_id, Some(active_id));
            assert!(!model.pending_geo_reconnect);
            assert!(!model.pending_service_reconnect);
            assert!(!model.geo_updating);
        }
    }

    #[test]
    fn service_rule_sets_ready_without_pending_commit_is_noop() {
        // The post-connect backstop download also reports readiness; it must
        // not trigger a reconnect loop.
        let profile = Profile::new_vless("A".into(), "e".into(), 1, "u".into());
        let mut model = model_with_profiles(vec![profile.clone()]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile.id);
        let effects = update(
            &mut model,
            Msg::ServiceRuleSetsReady {
                retry_states: Default::default(),
                checked_at: Default::default(),
                next_updates: Default::default(),
                updated_parts: Vec::new(),
                errors: Vec::new(),
            },
        );
        assert!(effects.is_empty());
        assert_eq!(model.connection, ConnectionState::Connected);
    }

    #[test]
    fn service_rule_sets_ready_after_disconnect_clears_flag_without_connect() {
        let profile = Profile::new_vless("A".into(), "e".into(), 1, "u".into());
        let mut model = model_with_profiles(vec![profile.clone()]);
        model.connection = ConnectionState::Idle;
        model.active_profile_id = Some(profile.id);
        model.pending_service_reconnect = true;
        let effects = update(
            &mut model,
            Msg::ServiceRuleSetsReady {
                retry_states: Default::default(),
                checked_at: Default::default(),
                next_updates: Default::default(),
                updated_parts: Vec::new(),
                errors: Vec::new(),
            },
        );
        assert!(!model.pending_service_reconnect);
        assert!(!effects.iter().any(|e| matches!(e, Effect::Connect { .. })));
        assert_eq!(model.connection, ConnectionState::Idle);
    }

    #[test]
    fn service_rule_sets_ready_with_missing_active_profile_does_not_flip_state() {
        let profile = Profile::new_vless("A".into(), "e".into(), 1, "u".into());
        let mut model = model_with_profiles(vec![profile]);
        model.connection = ConnectionState::Connected;
        // Active profile id points at a profile that no longer exists.
        model.active_profile_id = Some(uuid::Uuid::new_v4());
        model.pending_service_reconnect = true;
        let effects = update(
            &mut model,
            Msg::ServiceRuleSetsReady {
                retry_states: Default::default(),
                checked_at: Default::default(),
                next_updates: Default::default(),
                updated_parts: Vec::new(),
                errors: Vec::new(),
            },
        );
        assert!(!model.pending_service_reconnect);
        assert!(!effects.iter().any(|e| matches!(e, Effect::Connect { .. })));
        // Never drop to Connecting without a resolvable target — that path
        // ends in Idle-with-live-tunnel on the next Tick.
        assert_eq!(model.connection, ConnectionState::Connected);
    }
}

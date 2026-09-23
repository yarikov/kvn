use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model};
use crate::config::profile::Profile;
use uuid::Uuid;

use crate::app::update::connection::queue_connect;
use crate::app::update::status::push_status;

pub(crate) fn handle_config_reloaded(
    model: &mut Model,
    result: Result<crate::config::profile::Config, crate::app::msg::IpcError>,
) -> Vec<Effect> {
    match result {
        Ok(mut config) => {
            let old_settings = model.config.settings.clone();
            let kill_switch_ignored = config.settings.kill_switch != old_settings.kill_switch;
            // The persisted flag is not the source of truth for the live
            // firewall. Only ApplyKillSwitch may change it after the helper
            // succeeds, so an editor reload must not create config/systemd
            // drift.
            config.settings.kill_switch = old_settings.kill_switch;
            let runtime_settings_changed =
                connection_settings_changed(&old_settings, &config.settings);
            let service_routes_changed = old_settings.geo_routing.service_routes
                != config.settings.geo_routing.service_routes;
            let active_profile_changed = model.active_profile_id.is_some_and(|id| {
                profile_runtime_changed_for_id(&model.config.profiles, &config.profiles, id)
            });
            let connecting_profile_changed = model.connecting_profile_id.is_some_and(|id| {
                profile_runtime_changed_for_id(&model.config.profiles, &config.profiles, id)
            });
            let active_missing = model.connection == ConnectionState::Connected
                && model
                    .active_profile_id
                    .is_some_and(|id| !config.profiles.iter().any(|profile| profile.id == id));
            let connecting_missing = matches!(
                model.connection,
                ConnectionState::Connecting | ConnectionState::ConnectPending
            ) && model
                .connecting_profile_id
                .is_some_and(|id| !config.profiles.iter().any(|profile| profile.id == id));
            let region_changed = model.config.settings.geo_routing.current_region
                != config.settings.geo_routing.current_region;
            model.replace_config_preserving_selection(config);
            model.config_persistence_blocked = false;
            let mut effects = vec![Effect::BroadcastState];
            if active_missing || connecting_missing {
                effects.push(Effect::Disconnect);
            } else {
                let runtime_changed = runtime_settings_changed
                    || match model.connection {
                        ConnectionState::Connected => active_profile_changed,
                        ConnectionState::Connecting | ConnectionState::ConnectPending => {
                            connecting_profile_changed
                        }
                        _ => false,
                    };
                if runtime_changed {
                    match model.connection {
                        ConnectionState::Connected if service_routes_changed => {
                            model.pending_service_reconnect = true;
                            effects.push(Effect::DownloadServiceRuleSetsIfMissing);
                        }
                        ConnectionState::Connected => {
                            if let Some(id) = model.active_profile_id {
                                queue_connect(model, id);
                            }
                        }
                        ConnectionState::ConnectPending => {
                            if let Some(id) = model.connecting_profile_id {
                                queue_connect(model, id);
                            }
                        }
                        // A queued attempt has not captured its Profile and
                        // Settings yet; the next Tick reads the new config.
                        ConnectionState::Connecting | ConnectionState::Idle => {}
                    }
                }
            }
            if region_changed {
                model.geo_last_updated = None;
                model.geo_last_checked_at = None;
                model.geo_last_attempt_at = None;
                effects.push(Effect::RefreshGeoLastUpdated);
            }
            let reconnecting = model.connection == ConnectionState::Connecting
                && (active_profile_changed
                    || connecting_profile_changed
                    || runtime_settings_changed);
            let status = if kill_switch_ignored && reconnecting {
                "Kill switch edit ignored (use K); configuration changed — reconnecting"
            } else if kill_switch_ignored
                && model.pending_service_reconnect
                && service_routes_changed
            {
                "Kill switch edit ignored (use K); configuration changed — updating rule-sets"
            } else if kill_switch_ignored {
                "Kill switch edit ignored — use K to change it"
            } else if reconnecting {
                "Configuration changed — reconnecting"
            } else if model.pending_service_reconnect && service_routes_changed {
                "Configuration changed — updating rule-sets"
            } else {
                "Profiles reloaded"
            };
            push_status(&mut effects, model, AppStatus::Info(status.into()));
            effects
        }
        Err(e) => {
            let mut effects = vec![Effect::BroadcastState];
            push_status(
                &mut effects,
                model,
                AppStatus::Error(format!("Failed to reload: {}", e)),
            );
            effects
        }
    }
}

fn profile_runtime_changed_for_id(old: &[Profile], new: &[Profile], id: Uuid) -> bool {
    let old = old.iter().find(|profile| profile.id == id);
    let new = new.iter().find(|profile| profile.id == id);
    match (old, new) {
        (Some(old), Some(new)) => profile_runtime_changed(old, new),
        _ => false,
    }
}

pub(in crate::app::update) fn profile_runtime_changed(old: &Profile, new: &Profile) -> bool {
    old.address != new.address || old.port != new.port || old.config != new.config
}

fn connection_settings_changed(
    old: &crate::config::profile::Settings,
    new: &crate::config::profile::Settings,
) -> bool {
    old.tun_interface != new.tun_interface
        || old.dns != new.dns
        || old.geo_routing.mode() != new.geo_routing.mode()
        || old.geo_routing.service_routes != new.geo_routing.service_routes
        || old.logs.level != new.logs.level
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::msg::Msg;
    use crate::app::update::tick::handle_tick;
    use crate::app::update::update;
    use crate::config::profile::GeoRegion;
    use crate::test_helpers::*;

    #[test]
    fn config_reloaded_updates_model() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        let config = model.config.clone();
        model.config_persistence_blocked = true;
        let effects = update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));
        assert!(!model.config_persistence_blocked);
        assert_eq!(
            effects,
            vec![Effect::BroadcastState, app_log_info("Profiles reloaded")]
        );
    }

    #[test]
    fn config_reloaded_preserves_selected_profile() {
        let mut model = model_with_profiles(vec![
            Profile::new_vless(
                "A".to_string(),
                "1.1.1.1".to_string(),
                443,
                "u1".to_string(),
            ),
            Profile::new_vless(
                "B".to_string(),
                "2.2.2.2".to_string(),
                443,
                "u2".to_string(),
            ),
        ]);
        model.selected = 1;
        let selected_id = model.selected_profile().unwrap().id;
        let mut config = model.config.clone();
        config.profiles[1].name = "B edited".to_string();

        update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));

        assert_eq!(model.selected, 1);
        assert_eq!(model.selected_profile().unwrap().id, selected_id);
        assert_eq!(model.selected_profile().unwrap().name, "B edited");
    }

    #[test]
    fn config_reloaded_error_updates_status() {
        let mut model = model_with_profiles(vec![]);
        let effects = update(
            &mut model,
            Msg::ConfigReloaded(Box::new(Err(crate::app::msg::IpcError::new("parse error")))),
        );
        assert_eq!(
            effects,
            vec![
                Effect::BroadcastState,
                app_log_error("Failed to reload: parse error")
            ]
        );
    }

    #[test]
    fn config_reload_disconnects_when_active_profile_was_removed() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile_id);
        let mut config = model.config.clone();
        config.profiles.clear();

        let effects = update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));

        assert!(effects.contains(&Effect::Disconnect));
    }

    #[test]
    fn config_reload_cancels_missing_queued_connection() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        assert!(queue_connect(&mut model, profile_id));
        let mut config = model.config.clone();
        config.profiles.clear();

        let effects = update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));

        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(profile_id));
        assert!(effects.contains(&Effect::Disconnect));
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::Connect { .. }))
        );
    }

    #[test]
    fn config_reload_disconnects_missing_connect_pending_profile() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        assert!(queue_connect(&mut model, profile_id));
        let connect_effects = handle_tick(&mut model);
        assert!(
            connect_effects
                .iter()
                .any(|effect| matches!(effect, Effect::Connect { .. }))
        );
        model.connection = ConnectionState::ConnectPending;
        let mut config = model.config.clone();
        config.profiles.clear();

        let effects = update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));

        assert_eq!(model.connecting_profile_id, Some(profile_id));
        assert!(effects.contains(&Effect::Disconnect));
    }

    #[test]
    fn config_reload_keeps_connect_pending_profile_with_same_id() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        assert!(queue_connect(&mut model, profile_id));
        model.connection = ConnectionState::ConnectPending;
        let mut config = model.config.clone();
        config.profiles[0].name = "A renamed".into();

        let effects = update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));

        assert!(!effects.contains(&Effect::Disconnect));
        assert_eq!(model.connecting_profile_id, Some(profile_id));
    }

    #[test]
    fn config_reload_reconnects_changed_active_profile() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile_id);
        let mut config = model.config.clone();
        config.profiles[0].address = "2.2.2.2".into();

        let effects = update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));

        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(profile_id));
        assert_eq!(model.status_text(), "Configuration changed — reconnecting");
        assert!(!effects.contains(&Effect::Disconnect));
    }

    #[test]
    fn config_reload_does_not_reconnect_for_profile_metadata() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile_id);
        let mut config = model.config.clone();
        config.profiles[0].name = "Renamed".into();
        config.profiles[0].tags.push("metadata".into());

        let effects = update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));

        assert_eq!(model.connection, ConnectionState::Connected);
        assert_eq!(model.connecting_profile_id, None);
        assert_eq!(model.status_text(), "Profiles reloaded");
        assert!(!effects.contains(&Effect::Disconnect));
    }

    #[test]
    fn config_reload_reconnects_for_runtime_settings_only() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile_id);
        let mut config = model.config.clone();
        config.settings.tun_interface = "tun9".into();

        update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));

        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(profile_id));
    }

    #[test]
    fn connection_settings_comparison_covers_singbox_inputs() {
        use crate::config::profile::{DnsStrategy, GeoRegion, RoutedService, ServiceRoute};

        let original = crate::config::profile::Settings::default();
        let mut changed = original.clone();
        changed.dns.strategy = DnsStrategy::OnlyIpv6;
        assert!(connection_settings_changed(&original, &changed));

        let mut changed = original.clone();
        changed.logs.level = "debug".into();
        assert!(connection_settings_changed(&original, &changed));

        let mut changed = original.clone();
        changed.geo_routing.set_region(GeoRegion::Ru);
        changed
            .geo_routing
            .set_mode(crate::config::profile::RoutingMode::Only(GeoRegion::Ru));
        assert!(connection_settings_changed(&original, &changed));

        let mut changed = original.clone();
        changed
            .geo_routing
            .service_routes
            .insert(RoutedService::Steam, ServiceRoute::Proxy);
        assert!(connection_settings_changed(&original, &changed));
    }

    #[test]
    fn config_reload_ignores_ui_settings_and_external_kill_switch_value() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile_id);
        let mut config = model.config.clone();
        config.settings.theme = "gruvbox".into();
        config.settings.auto_connect = !config.settings.auto_connect;
        config.settings.kill_switch = !config.settings.kill_switch;

        update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));

        assert_eq!(model.connection, ConnectionState::Connected);
        assert!(!model.config.settings.kill_switch);
        assert_eq!(
            model.status_text(),
            "Kill switch edit ignored — use K to change it"
        );
    }

    #[test]
    fn config_reload_reports_ignored_kill_switch_alongside_reconnect() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile_id);
        let mut config = model.config.clone();
        config.settings.kill_switch = true;
        config.settings.tun_interface = "tun9".into();

        let effects = update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));

        assert_eq!(model.connection, ConnectionState::Connecting);
        assert!(!model.config.settings.kill_switch);
        assert_eq!(
            model.status_text(),
            "Kill switch edit ignored (use K); configuration changed — reconnecting"
        );
        assert!(effects.contains(&app_log_info(
            "Kill switch edit ignored (use K); configuration changed — reconnecting"
        )));
    }

    #[test]
    fn config_reload_requeues_changed_connect_pending_attempt() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        assert!(queue_connect(&mut model, profile_id));
        model.connection = ConnectionState::ConnectPending;
        let old_attempt = model.connect_attempt_id;
        let mut config = model.config.clone();
        config.profiles[0].port = 8443;

        update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));

        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connect_attempt_id, old_attempt + 1);
        assert_eq!(model.connecting_profile_id, Some(profile_id));
    }

    #[test]
    fn config_reload_defers_service_route_reconnect_until_assets_ready() {
        use crate::config::profile::{RoutedService, ServiceRoute};

        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile_id);
        let mut config = model.config.clone();
        config
            .settings
            .geo_routing
            .service_routes
            .insert(RoutedService::Steam, ServiceRoute::Proxy);

        let effects = update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));

        assert_eq!(model.connection, ConnectionState::Connected);
        assert!(model.pending_service_reconnect);
        assert!(effects.contains(&Effect::DownloadServiceRuleSetsIfMissing));
    }
}

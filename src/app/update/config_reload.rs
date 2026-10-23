use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model, StatusKey};
use crate::config::profile::Profile;
use uuid::Uuid;

use crate::app::update::connection::reconnect_with_new_settings;
use crate::app::update::routing::download_missing_selectable_rule_sets;
use crate::app::update::status::{
    DownloadKind, download_allowed, push_download_blocked, push_status,
};
use crate::config::routing::RoutingConfig;
use crate::route_plan::RoutePlan;

pub(crate) fn handle_config_reloaded(
    model: &mut Model,
    result: Result<crate::config::profile::Config, crate::app::msg::IpcError>,
) -> Vec<Effect> {
    match result {
        Ok(mut config) => {
            let old_settings = model.config.settings.clone();
            let kill_switch_ignored = config.settings.kill_switch != old_settings.kill_switch;
            config.settings.kill_switch = old_settings.kill_switch;
            let runtime_settings_changed =
                connection_settings_changed(&old_settings, &config.settings, &model.routing);
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
            model.unsaved_state_merge_base = None;
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
                        ConnectionState::Connected | ConnectionState::ConnectPending => {
                            reconnect_with_new_settings(model);
                        }
                        ConnectionState::Connecting | ConnectionState::Idle => {}
                    }
                }
            }
            if region_changed {
                effects.push(Effect::RefreshRuleSetStates);
            }
            let reconnecting = model.connection == ConnectionState::Connecting
                && (active_profile_changed
                    || connecting_profile_changed
                    || runtime_settings_changed);
            let status = if kill_switch_ignored && reconnecting {
                "Kill switch edit ignored (use K); configuration changed — reconnecting"
            } else if kill_switch_ignored {
                "Kill switch edit ignored — use K to change it"
            } else if reconnecting {
                "Configuration changed — reconnecting"
            } else {
                "Configuration reloaded"
            };
            push_status(
                &mut effects,
                model,
                StatusKey::Config,
                AppStatus::Info(status.into()),
            );
            effects
        }
        Err(e) => {
            let mut effects = vec![Effect::BroadcastState];
            push_status(
                &mut effects,
                model,
                StatusKey::Config,
                AppStatus::Error(format!("Configuration reload failed: {}", e)),
            );
            effects
        }
    }
}

pub(crate) fn handle_routing_reloaded(
    model: &mut Model,
    result: Result<RoutingConfig, crate::app::msg::IpcError>,
) -> Vec<Effect> {
    let mut effects = vec![Effect::BroadcastState];
    let routing = match result {
        Ok(routing) => routing,
        Err(e) => {
            push_status(
                &mut effects,
                model,
                StatusKey::Config,
                AppStatus::Error(format!("Routing configuration reload failed: {e}")),
            );
            return effects;
        }
    };
    let old_plan = RoutePlan::for_settings(&model.config.settings, &model.routing);
    let old_selectable = model.selectable_rule_sets();
    model.routing = routing;
    if model.selectable_rule_sets() != old_selectable {
        effects.push(Effect::RefreshRuleSetStates);
        if download_allowed(model) {
            effects.extend(download_missing_selectable_rule_sets(model));
        } else {
            push_download_blocked(&mut effects, model, DownloadKind::Geo);
        }
    }
    let new_plan = RoutePlan::for_settings(&model.config.settings, &model.routing);
    let awaits_download = new_plan
        .rule_sets()
        .filter(|rule_set| !old_plan.rule_sets().any(|old| old.url == rule_set.url))
        .any(|rule_set| downloads(&effects, rule_set));
    if new_plan != old_plan && !awaits_download && reconnect_with_new_settings(model) {
        model
            .logs
            .push_back("Routing configuration changed — reconnecting".into());
    }
    effects
}

fn downloads(effects: &[Effect], rule_set: &crate::geo::RuleSet) -> bool {
    effects.iter().any(|effect| {
        matches!(effect, Effect::DownloadRuleSets { rule_sets, .. }
            if rule_sets.iter().any(|queued| queued.key() == rule_set.key()))
    })
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
    old.address != new.address
        || old.port != new.port
        || old.config != new.config
        || old.share_link_params != new.share_link_params
}

fn connection_settings_changed(
    old: &crate::config::profile::Settings,
    new: &crate::config::profile::Settings,
    routing: &RoutingConfig,
) -> bool {
    old.tun_interface != new.tun_interface
        || old.dns != new.dns
        || old.geo_routing.mode(routing) != new.geo_routing.mode(routing)
        || old.logs.level != new.logs.level
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::msg::Msg;
    use crate::app::update::connection::queue_connect;
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
        model.unsaved_state_merge_base = Some(config.clone());
        let effects = update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));
        assert!(!model.config_persistence_blocked);
        assert_eq!(model.unsaved_state_merge_base, None);
        assert_eq!(
            effects,
            vec![
                Effect::BroadcastState,
                app_log_info("Configuration reloaded")
            ]
        );
    }

    fn reload_routing(model: &mut Model, routing: RoutingConfig) -> Vec<Effect> {
        update(model, Msg::RoutingReloaded(Box::new(Ok(routing))))
    }

    #[test]
    fn a_reloaded_rule_set_url_downloads_before_it_reconnects() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model
            .config
            .settings
            .geo_routing
            .set_mode(crate::config::profile::RoutingMode::Bypass(GeoRegion::Ru));
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile_id);
        let mut routing = RoutingConfig::default();
        routing.rule_sets.get_mut("geoip-ru").unwrap().url =
            "https://mirror.example/geoip-ru.srs".into();

        let effects = reload_routing(&mut model, routing.clone());

        assert_eq!(model.routing, routing);
        assert!(effects.contains(&Effect::RefreshRuleSetStates));
        assert!(downloads_rule_sets(&effects));
        assert_eq!(model.connection, ConnectionState::Connected);
    }

    #[test]
    fn a_reloaded_rule_set_kind_reconnects_at_once() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model
            .config
            .settings
            .geo_routing
            .set_mode(crate::config::profile::RoutingMode::Bypass(GeoRegion::Ru));
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile_id);
        let mut routing = RoutingConfig::default();
        routing.rule_sets.get_mut("geoip-ru").unwrap().kind =
            crate::config::routing::RuleSetKind::Geosite;

        reload_routing(&mut model, routing);

        assert_eq!(model.connection, ConnectionState::Connecting);
    }

    #[test]
    fn a_reloaded_rule_set_no_mode_routes_changes_nothing_else() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        let mut routing = RoutingConfig::default();
        routing.rule_sets.insert(
            "ads".into(),
            crate::config::routing::RuleSetSource {
                kind: crate::config::routing::RuleSetKind::Geosite,
                url: "https://example.com/ads.srs".into(),
            },
        );

        let effects = reload_routing(&mut model, routing.clone());

        assert_eq!(model.routing, routing);
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn a_failed_routing_reload_keeps_the_loaded_rule_sets() {
        let mut model = model_with_profiles(vec![]);
        model.routing.rule_sets.remove("geoip-ir");
        let before = model.routing.clone();

        let effects = update(
            &mut model,
            Msg::RoutingReloaded(Box::new(Err(crate::app::msg::IpcError::new("bad url")))),
        );

        assert_eq!(model.routing, before);
        assert_eq!(
            effects,
            vec![
                Effect::BroadcastState,
                app_log_error("Routing configuration reload failed: bad url"),
            ]
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
                app_log_error("Configuration reload failed: parse error")
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
    fn config_reload_reconnects_when_share_link_params_change() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile_id);
        let mut config = model.config.clone();
        config.profiles[0]
            .share_link_params
            .insert("encryption".into(), "mlkem768".into());

        update(&mut model, Msg::ConfigReloaded(Box::new(Ok(config))));

        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(profile_id));
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
        assert_eq!(model.status_text(), "Configuration reloaded");
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
        let original = crate::config::profile::Settings::default();
        let mut changed = original.clone();
        changed.dns.fakeip_enabled = true;
        assert!(connection_settings_changed(
            &original,
            &changed,
            &RoutingConfig::default()
        ));

        let mut changed = original.clone();
        changed.logs.level = "debug".into();
        assert!(connection_settings_changed(
            &original,
            &changed,
            &RoutingConfig::default()
        ));

        let mut changed = original.clone();
        changed.geo_routing.set_region(GeoRegion::Ru);
        changed
            .geo_routing
            .set_mode(crate::config::profile::RoutingMode::Only(GeoRegion::Ru));
        assert!(connection_settings_changed(
            &original,
            &changed,
            &RoutingConfig::default()
        ));
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
}

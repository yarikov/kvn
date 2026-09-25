use crate::app::effect::Effect;
use crate::app::model::{Model, Overlay};
use crate::config::profile::GeoRegion;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::update::key::settings_menu::{finish_settings_overlay, return_to_settings_menu};
use crate::app::update::routing::{commit_geo_region, commit_routing_mode};

pub(in crate::app::update) fn handle_routing_mode(model: &mut Model, key: KeyEvent) -> Vec<Effect> {
    let available = model.config.settings.geo_routing.available_modes();
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            crate::ui::nav::select_next(&mut model.routing_selected, available.len());
        }
        KeyCode::Char('k') | KeyCode::Up => {
            crate::ui::nav::select_prev(&mut model.routing_selected);
        }
        KeyCode::Char('G') => {
            crate::ui::nav::select_last(&mut model.routing_selected, available.len());
        }
        KeyCode::Enter => {
            if let Some(&mode) = available.get(model.routing_selected) {
                finish_settings_overlay(model);
                return commit_routing_mode(model, mode);
            }
        }
        KeyCode::Backspace if return_to_settings_menu(model) => {}
        // Like the region picker, the tour's mode step ends only by choosing.
        KeyCode::Char('q') | KeyCode::Esc
            if model.onboarding.awaiting != Some(crate::onboarding::OnboardingStep::Routing) =>
        {
            model.settings_menu_return = None;
            model.overlay = Overlay::None;
        }
        _ => {}
    }
    vec![]
}

pub(in crate::app::update) fn handle_geo_region(model: &mut Model, key: KeyEvent) -> Vec<Effect> {
    let regions = &GeoRegion::ALL;
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            crate::ui::nav::select_next(&mut model.geo_region_selected, regions.len());
        }
        KeyCode::Char('k') | KeyCode::Up => {
            crate::ui::nav::select_prev(&mut model.geo_region_selected);
        }
        KeyCode::Char('G') => {
            crate::ui::nav::select_last(&mut model.geo_region_selected, regions.len());
        }
        KeyCode::Enter => {
            if let Some(&region) = regions.get(model.geo_region_selected) {
                finish_settings_overlay(model);
                return commit_geo_region(model, region);
            }
        }
        KeyCode::Backspace if return_to_settings_menu(model) => {}
        // Unescapable without a region, and unescapable during the tour even
        // with one: the tour's region step ends only by choosing a region.
        KeyCode::Char('q') | KeyCode::Esc
            if model.config.settings.geo_routing.current_region.is_some()
                && model.onboarding.awaiting != Some(crate::onboarding::OnboardingStep::Region) =>
        {
            model.settings_menu_return = None;
            model.overlay = Overlay::None;
        }
        _ => {}
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::ConnectionState;
    use crate::app::update::key::ipc::handle_ipc_command;
    use crate::config::profile::{Profile, RoutingMode};
    use crate::test_helpers::*;

    #[test]
    fn routing_mode_navigates() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 0;

        let _ = handle_routing_mode(&mut model, key('j'));
        assert_eq!(model.routing_selected, 1);
        let _ = handle_routing_mode(&mut model, key('j'));
        assert_eq!(model.routing_selected, 2);
        let _ = handle_routing_mode(&mut model, key('j'));
        assert_eq!(model.routing_selected, 2); // clamp

        let _ = handle_routing_mode(&mut model, key('k'));
        assert_eq!(model.routing_selected, 1);
        let _ = handle_routing_mode(&mut model, key('g'));
        assert_eq!(model.routing_selected, 1);
        let _ = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::GoFirst);
        assert_eq!(model.routing_selected, 0);
        let _ = handle_routing_mode(&mut model, key('G'));
        assert_eq!(model.routing_selected, 2);
    }

    #[test]
    fn routing_mode_enter_changes_mode() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 2; // OnlyRu

        let effects = handle_routing_mode(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            model.config.settings.geo_routing.mode(),
            RoutingMode::Only(GeoRegion::Ru)
        );
        assert_eq!(model.overlay, Overlay::None);
        assert!(model.status_text().contains("Only RU"));
        assert_eq!(
            effects,
            vec![Effect::SaveConfig, app_log_info("Routing mode: Only RU")]
        );
    }

    #[test]
    fn routing_mode_change_queues_active_profile_not_cursor() {
        let a = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let b = Profile::new_vless("B".into(), "2.2.2.2".into(), 443, "u2".into());
        let active_id = a.id;
        let mut model = model_with_profiles(vec![a, b]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(active_id);
        model.select_next();
        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 1;

        handle_routing_mode(&mut model, KeyEvent::from(KeyCode::Enter));

        assert_eq!(model.connecting_profile_id, Some(active_id));
    }

    #[test]
    fn routing_mode_esc_cancels() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 2;
        let effects = handle_routing_mode(&mut model, KeyEvent::from(KeyCode::Esc));
        assert_eq!(model.overlay, Overlay::None);
        assert!(effects.is_empty());
    }

    #[test]
    fn geo_region_navigates() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::GeoRegions;
        model.geo_region_selected = 0;

        let _ = handle_geo_region(&mut model, key('j'));
        assert_eq!(model.geo_region_selected, 1);
        let _ = handle_geo_region(&mut model, key('j'));
        assert_eq!(model.geo_region_selected, 2);
        let _ = handle_geo_region(&mut model, key('j'));
        assert_eq!(model.geo_region_selected, 3);
        let _ = handle_geo_region(&mut model, key('j'));
        assert_eq!(model.geo_region_selected, 3); // clamp

        let _ = handle_geo_region(&mut model, key('k'));
        assert_eq!(model.geo_region_selected, 2);
        let _ = handle_geo_region(&mut model, key('g'));
        assert_eq!(model.geo_region_selected, 2);
        let _ = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::GoFirst);
        assert_eq!(model.geo_region_selected, 0);
        let _ = handle_geo_region(&mut model, key('G'));
        assert_eq!(model.geo_region_selected, 3);
    }

    #[test]
    fn geo_region_enter_changes_region() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::GeoRegions;
        model.geo_region_selected = 1; // Cn

        let effects = handle_geo_region(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            model.config.settings.geo_routing.current_region,
            Some(GeoRegion::Cn)
        );
        assert_eq!(model.overlay, Overlay::None);
        assert!(model.logs.iter().any(|l| l.contains("Geo region: cn")));
        assert_eq!(
            effects,
            vec![
                Effect::SaveConfig,
                Effect::RefreshGeoLastUpdated,
                app_log_info("Geo region: cn"),
                app_log_info("Checking geo databases..."),
                Effect::DownloadGeoIfMissing,
            ]
        );
    }

    #[test]
    fn geo_region_change_queues_active_profile_not_cursor() {
        let a = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let b = Profile::new_vless("B".into(), "2.2.2.2".into(), 443, "u2".into());
        let active_id = a.id;
        let mut model = model_with_profiles(vec![a, b]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.config.settings.auto_connect = true;
        model.config.settings.last_connected_profile = Some(model.config.profiles[1].id);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(active_id);
        model.select_next();
        model.overlay = Overlay::GeoRegions;
        model.geo_region_selected = 1;

        handle_geo_region(&mut model, KeyEvent::from(KeyCode::Enter));

        assert_eq!(model.connecting_profile_id, Some(active_id));
    }

    #[test]
    fn geo_region_esc_blocked_when_none() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::GeoRegions;

        let effects = handle_geo_region(&mut model, KeyEvent::from(KeyCode::Esc));
        assert_eq!(model.overlay, Overlay::GeoRegions);
        assert!(effects.is_empty());
    }

    #[test]
    fn geo_region_esc_allowed_when_some() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::GeoRegions;
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);

        let effects = handle_geo_region(&mut model, KeyEvent::from(KeyCode::Esc));
        assert_eq!(model.overlay, Overlay::None);
        assert!(effects.is_empty());
    }

    #[test]
    fn geo_region_change_resets_incompatible_routing_mode() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model
            .config
            .settings
            .geo_routing
            .set_mode(RoutingMode::Only(GeoRegion::Ru));
        model.overlay = Overlay::GeoRegions;
        model.geo_region_selected = 3; // Global

        let effects = handle_geo_region(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            model.config.settings.geo_routing.current_region,
            Some(GeoRegion::Global)
        );
        assert_eq!(
            model.config.settings.geo_routing.mode(),
            RoutingMode::Global
        );
        assert_eq!(
            model
                .config
                .settings
                .geo_routing
                .selected_region_modes
                .get(&GeoRegion::Ru)
                .copied()
                .unwrap_or(RoutingMode::Global),
            RoutingMode::Only(GeoRegion::Ru),
            "previous region's routing mode should be preserved"
        );
        assert_eq!(
            effects,
            vec![
                Effect::SaveConfig,
                Effect::RefreshGeoLastUpdated,
                app_log_info("Geo region: global"),
                app_log_info("Routing mode: Global")
            ]
        );
    }

    #[test]
    fn routing_mode_persists_per_region() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model
            .config
            .settings
            .geo_routing
            .set_mode(RoutingMode::Bypass(GeoRegion::Ru));
        model.overlay = Overlay::GeoRegions;
        model.geo_region_selected = 1; // Cn

        // Switch to Cn: routing mode falls back to Global.
        let effects = handle_geo_region(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            model.config.settings.geo_routing.current_region,
            Some(GeoRegion::Cn)
        );
        assert_eq!(
            model.config.settings.geo_routing.mode(),
            RoutingMode::Global
        );
        assert!(effects.contains(&Effect::DownloadGeoIfMissing));

        // Switch back to Ru: routing mode is restored to BypassRu.
        model.overlay = Overlay::GeoRegions;
        model.geo_region_selected = 0; // Ru
        let effects = handle_geo_region(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            model.config.settings.geo_routing.current_region,
            Some(GeoRegion::Ru)
        );
        assert_eq!(
            model.config.settings.geo_routing.mode(),
            RoutingMode::Bypass(GeoRegion::Ru)
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::AppendAppLog { message, .. } if message.contains("Routing mode: Bypass RU"))));
    }

    #[test]
    fn routing_mode_change_is_stored_per_region() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 1; // BypassRu

        handle_routing_mode(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            model.config.settings.geo_routing.mode(),
            RoutingMode::Bypass(GeoRegion::Ru)
        );
        assert_eq!(
            model
                .config
                .settings
                .geo_routing
                .selected_region_modes
                .get(&GeoRegion::Ru)
                .copied()
                .unwrap_or(RoutingMode::Global),
            RoutingMode::Bypass(GeoRegion::Ru)
        );
    }

    #[test]
    fn geo_region_triggers_auto_connect_after_selection() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Auto".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        let id = model.config.profiles[0].id;
        model.config.settings.auto_connect = true;
        model.config.settings.last_connected_profile = Some(id);
        model.overlay = Overlay::GeoRegions;
        model.geo_region_selected = 0; // Ru

        let effects = handle_geo_region(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            model.config.settings.geo_routing.current_region,
            Some(GeoRegion::Ru)
        );
        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(id));
        assert_eq!(model.selected, 0);
        assert!(model.status_text().contains("Auto-connecting"));
        assert_eq!(
            effects,
            vec![
                Effect::SaveConfig,
                Effect::RefreshGeoLastUpdated,
                app_log_info("Geo region: ru"),
                app_log_info("Checking geo databases..."),
                Effect::DownloadGeoIfMissing,
                app_log_info("Auto-connecting to Auto…")
            ]
        );
    }

    #[test]
    fn geo_region_same_region_does_not_refresh_last_updated() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Cn);
        model.overlay = Overlay::GeoRegions;
        model.geo_region_selected = 1; // Cn

        let effects = handle_geo_region(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            model.config.settings.geo_routing.current_region,
            Some(GeoRegion::Cn)
        );
        assert!(!effects.contains(&Effect::RefreshGeoLastUpdated));
    }

    #[test]
    fn geo_region_global_does_not_trigger_geo_download() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.overlay = Overlay::GeoRegions;
        model.geo_region_selected = 3; // Global

        let effects = handle_geo_region(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            model.config.settings.geo_routing.current_region,
            Some(GeoRegion::Global)
        );
        assert!(!effects.contains(&Effect::DownloadGeoIfMissing));
        assert!(!model.status_text().contains("Checking geo databases"));
    }
}

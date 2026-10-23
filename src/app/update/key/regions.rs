use crate::app::effect::Effect;
use crate::app::model::{Model, Overlay};
use crate::config::profile::GeoRegion;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::update::routing::{commit_geo_region, commit_routing_mode};

pub(in crate::app::update) fn handle_routing_mode(model: &mut Model, key: KeyEvent) -> Vec<Effect> {
    let count = model.routing.mode_ids().len();
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            crate::ui::nav::select_next(&mut model.routing_selected, count);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            crate::ui::nav::select_prev(&mut model.routing_selected);
        }
        KeyCode::Char('G') => {
            crate::ui::nav::select_last(&mut model.routing_selected, count);
        }
        KeyCode::Enter => {
            if let Some(id) = model
                .routing
                .mode_ids()
                .get(model.routing_selected)
                .map(|id| id.to_string())
            {
                model.overlay = Overlay::None;
                return commit_routing_mode(model, &id);
            }
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
                model.overlay = Overlay::None;
                return commit_geo_region(model, region);
            }
        }
        _ => {}
    }
    vec![]
}

pub(in crate::app::update) fn open_routing_mode(model: &mut Model) {
    model.overlay = Overlay::RoutingMode;
    let selected = model.config.settings.geo_routing.mode(&model.routing);
    model.routing_selected = model
        .routing
        .mode_ids()
        .iter()
        .position(|id| *id == selected)
        .unwrap_or(0);
}

pub(in crate::app::update) fn open_geo_region(model: &mut Model) {
    model.overlay = Overlay::GeoRegions;
    model.geo_region_selected = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::ConnectionState;
    use crate::app::update::key::ipc::handle_ipc_command;
    use crate::config::profile::Profile;
    use crate::test_helpers::*;

    fn model_with_ru_modes(profiles: Vec<Profile>) -> Model {
        let mut model = model_with_profiles(profiles);
        model.routing.add_region_modes(GeoRegion::Ru);
        model
    }

    #[test]
    fn routing_mode_navigates() {
        let mut model = model_with_ru_modes(vec![]);
        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 0;

        let _ = handle_routing_mode(&mut model, key('j'));
        assert_eq!(model.routing_selected, 1);
        let _ = handle_routing_mode(&mut model, key('j'));
        assert_eq!(model.routing_selected, 2);
        let _ = handle_routing_mode(&mut model, key('j'));
        assert_eq!(model.routing_selected, 2);

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
        let mut model = model_with_ru_modes(vec![]);
        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 2;

        let effects = handle_routing_mode(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            model.config.settings.geo_routing.mode(&model.routing),
            "only_ru"
        );
        assert_eq!(model.overlay, Overlay::None);
        assert!(model.status_text().contains("Only RU"));
        assert_eq!(
            effects,
            vec![
                Effect::SaveConfig,
                app_log_info("Routing mode changed: Only RU")
            ]
        );
    }

    #[test]
    fn routing_mode_change_queues_active_profile_not_cursor() {
        let a = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let b = Profile::new_vless("B".into(), "2.2.2.2".into(), 443, "u2".into());
        let active_id = a.id;
        let mut model = model_with_ru_modes(vec![a, b]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(active_id);
        model.select_next();
        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 1;

        handle_routing_mode(&mut model, KeyEvent::from(KeyCode::Enter));

        assert_eq!(model.connecting_profile_id, Some(active_id));
    }

    #[test]
    fn geo_region_navigates() {
        let mut model = model_with_ru_modes(vec![]);
        model.overlay = Overlay::GeoRegions;
        model.geo_region_selected = 0;

        let _ = handle_geo_region(&mut model, key('j'));
        assert_eq!(model.geo_region_selected, 1);
        let _ = handle_geo_region(&mut model, key('j'));
        assert_eq!(model.geo_region_selected, 2);
        let _ = handle_geo_region(&mut model, key('j'));
        assert_eq!(model.geo_region_selected, 3);
        let _ = handle_geo_region(&mut model, key('j'));
        assert_eq!(model.geo_region_selected, 3);

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
    fn geo_region_enter_adds_the_region_modes_and_downloads_their_rule_sets() {
        let mut model = model_with_ru_modes(vec![]);
        model.overlay = Overlay::GeoRegions;
        model.geo_region_selected = 1;

        let effects = handle_geo_region(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            model.routing.mode_ids(),
            ["global", "bypass_ru", "only_ru", "bypass_cn", "only_cn"]
        );
        assert_eq!(model.config.settings.geo_routing.mode, "global");
        assert_eq!(model.overlay, Overlay::None);
        assert_eq!(
            effects,
            vec![
                app_log_info("Geo region selected: cn"),
                Effect::AddRegionModes {
                    region: GeoRegion::Cn
                },
                Effect::RefreshRuleSetStates,
                app_log_info("Checking geo databases…"),
                Effect::DownloadRuleSets {
                    rule_sets: [
                        region_rule_sets(GeoRegion::Ru),
                        region_rule_sets(GeoRegion::Cn)
                    ]
                    .concat(),
                    only_missing: true,
                    record_schedule: false,
                },
            ]
        );
    }

    #[test]
    fn a_region_whose_modes_exist_changes_nothing() {
        for selected in [0, 3] {
            let mut model = model_with_ru_modes(vec![]);
            model.config.settings.geo_routing.set_mode("only_ru");
            let routing = model.routing.clone();
            model.overlay = Overlay::GeoRegions;
            model.geo_region_selected = selected;

            let effects = handle_geo_region(&mut model, KeyEvent::from(KeyCode::Enter));

            assert_eq!(model.routing, routing);
            assert_eq!(model.config.settings.geo_routing.mode, "only_ru");
            assert!(!effects.contains(&Effect::SaveConfig));
            assert!(!downloads_rule_sets(&effects));
            assert!(!model.status_text().contains("Checking geo databases"));
        }
    }

    #[test]
    fn a_mode_routing_json_does_not_declare_is_refused() {
        let mut model = model_with_ru_modes(vec![]);
        model.routing.modes.retain(|mode| mode.id != "only_ru");

        let effects = commit_routing_mode(&mut model, "only_ru");

        assert!(!effects.contains(&Effect::SaveConfig));
        assert_eq!(model.config.settings.geo_routing.mode, "global");
        assert!(
            model
                .status_text()
                .contains("only_ru is not a mode of routing.json")
        );
    }

    #[test]
    fn a_mode_change_reports_the_name_routing_json_gives() {
        let mut model = model_with_ru_modes(vec![]);
        model
            .routing
            .modes
            .iter_mut()
            .find(|mode| mode.id == "bypass_ru")
            .unwrap()
            .name = "Home".into();

        let effects = commit_routing_mode(&mut model, "bypass_ru");

        assert!(effects.contains(&app_log_info("Routing mode changed: Home")));
    }

    #[test]
    fn the_mode_picker_lists_every_mode_of_routing_json() {
        let mut model = model_with_ru_modes(vec![]);
        model.routing.add_region_modes(GeoRegion::Ir);
        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 4;

        handle_routing_mode(&mut model, KeyEvent::from(KeyCode::Enter));

        assert_eq!(model.config.settings.geo_routing.mode, "only_ir");
    }

    #[test]
    fn routing_mode_picker_opens_on_the_current_mode() {
        let mut model = model_with_ru_modes(vec![]);
        model.config.settings.geo_routing.set_mode("bypass_ru");
        open_routing_mode(&mut model);
        assert_eq!(model.overlay, Overlay::RoutingMode);
        assert_eq!(model.routing_selected, 1);
    }

    #[test]
    fn region_picker_opens_on_the_first_region() {
        let mut model = model_with_ru_modes(vec![]);
        model.geo_region_selected = 2;
        open_geo_region(&mut model);
        assert_eq!(model.overlay, Overlay::GeoRegions);
        assert_eq!(model.geo_region_selected, 0);
    }
}

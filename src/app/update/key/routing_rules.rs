use crate::app::effect::Effect;
use crate::app::model::{AppStatus, Model, RoutingSettingsRow, StatusKey};
use crate::app::update::status::push_status;

use super::reorder::Direction;

pub(in crate::app::update) fn move_selected_rule(
    model: &mut Model,
    direction: Direction,
) -> Vec<Effect> {
    let RoutingSettingsRow::Rule(index) = model.routing_settings_row() else {
        return vec![];
    };
    if model.routing_error.is_some() {
        return invalid_routing_error(model);
    }
    let rules = model.shown_mode_rules();
    let Some(neighbor) = (match direction {
        Direction::Up => index.checked_sub(1),
        Direction::Down => Some(index + 1),
    })
    .filter(|&neighbor| neighbor < rules.len()) else {
        return vec![];
    };
    let mut reordered = rules.to_vec();
    reordered.swap(index, neighbor);
    model.draft_shown_mode_rules(reordered);
    model.settings_menu_selected = neighbor + 1;
    vec![]
}

pub(in crate::app::update) fn cycle_selected_rule_route(
    model: &mut Model,
    forward: bool,
) -> Vec<Effect> {
    let RoutingSettingsRow::Rule(index) = model.routing_settings_row() else {
        return vec![];
    };
    if model.routing_error.is_some() {
        return invalid_routing_error(model);
    }
    let mut rules = model.shown_mode_rules().to_vec();
    rules[index].route = rules[index].route.cycled(forward);
    model.draft_shown_mode_rules(rules);
    vec![]
}

fn invalid_routing_error(model: &mut Model) -> Vec<Effect> {
    let mut effects = vec![];
    push_status(
        &mut effects,
        model,
        StatusKey::Routing,
        AppStatus::Error(
            "Rules cannot be changed while routing.json is invalid; fix it with e".into(),
        ),
    );
    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::routing::RuleRoute;
    use crate::test_helpers::*;

    fn routing_page(mode: &str) -> Model {
        let mut model = model_with_profiles(vec![]);
        model
            .routing
            .add_region_modes(crate::config::profile::GeoRegion::Ru);
        model.config.settings.geo_routing.set_mode(mode);
        model.overlay =
            crate::app::model::Overlay::SettingsMenu(crate::app::model::SettingsMenuPage::Routing);
        model
    }

    fn saved_rules(model: &Model, mode: &str) -> Vec<crate::config::routing::ModeRule> {
        model.routing.mode(mode).unwrap().rules.clone()
    }

    #[test]
    fn a_rule_moves_within_its_mode_and_the_selection_follows() {
        let mut model = routing_page("bypass_ru");
        let saved = saved_rules(&model, "bypass_ru");
        model.settings_menu_selected = 1;

        assert!(move_selected_rule(&mut model, Direction::Down).is_empty());

        let mut moved = saved.clone();
        moved.swap(0, 1);
        assert_eq!(model.shown_mode_rules(), moved);
        assert_eq!(model.settings_menu_selected, 2);
        assert_eq!(saved_rules(&model, "bypass_ru"), saved);

        move_selected_rule(&mut model, Direction::Up);
        assert_eq!(model.shown_mode_rules(), saved);
        assert_eq!(model.settings_menu_selected, 1);
        assert_eq!(model.routing_settings_draft, None);
    }

    #[test]
    fn nothing_moves_from_the_mode_row_final_or_past_an_edge() {
        let mut model = routing_page("bypass_ru");
        let rules = model.shown_mode_rules().len();
        for (selected, direction) in [
            (0, Direction::Down),
            (1, Direction::Up),
            (rules, Direction::Down),
            (rules + 1, Direction::Up),
        ] {
            model.settings_menu_selected = selected;

            move_selected_rule(&mut model, direction);

            assert_eq!(model.routing_settings_draft, None);
            assert_eq!(model.settings_menu_selected, selected);
        }
    }

    #[test]
    fn each_mode_keeps_its_own_rule_order_and_the_order_it_was_moved_from() {
        let mut model = routing_page("bypass_ru");
        let saved = saved_rules(&model, "bypass_ru");
        model.settings_menu_selected = 1;
        move_selected_rule(&mut model, Direction::Down);
        let moved = model.shown_mode_rules().to_vec();
        model
            .routing
            .modes
            .iter_mut()
            .find(|mode| mode.id == "bypass_ru")
            .unwrap()
            .rules
            .reverse();
        move_selected_rule(&mut model, Direction::Down);
        model.routing_settings_draft.as_mut().unwrap().mode = Some("only_ru".into());
        model.settings_menu_selected = 1;
        move_selected_rule(&mut model, Direction::Down);

        let orders = &model.routing_settings_draft.as_ref().unwrap().mode_rules;
        assert_eq!(
            orders.keys().map(String::as_str).collect::<Vec<_>>(),
            ["bypass_ru", "only_ru"]
        );
        let mut twice = moved;
        twice.swap(1, 2);
        assert_eq!(orders["bypass_ru"].rules, twice);
        assert_eq!(orders["bypass_ru"].saved, saved);
        assert_eq!(orders["only_ru"].saved, saved_rules(&model, "only_ru"));
    }

    #[test]
    fn a_route_cycles_through_every_target_back_to_the_saved_one() {
        let mut model = routing_page("bypass_ru");
        model.settings_menu_selected = 2;
        let route = |model: &Model| model.shown_mode_rules()[1].route;

        let mut forward = vec![];
        for _ in 0..4 {
            cycle_selected_rule_route(&mut model, true);
            forward.push(route(&model));
        }
        assert_eq!(
            forward,
            [
                RuleRoute::Block,
                RuleRoute::Disabled,
                RuleRoute::Proxy,
                RuleRoute::Direct
            ]
        );
        assert_eq!(model.routing_settings_draft, None);

        cycle_selected_rule_route(&mut model, false);
        assert_eq!(route(&model), RuleRoute::Proxy);
        assert_eq!(model.settings_menu_selected, 2);
        assert_eq!(saved_rules(&model, "bypass_ru")[1].route, RuleRoute::Direct);
    }

    #[test]
    fn a_route_does_not_change_on_the_mode_row_or_final() {
        let mut model = routing_page("bypass_ru");
        let rules = model.shown_mode_rules().len();
        for selected in [0, rules + 1] {
            model.settings_menu_selected = selected;

            cycle_selected_rule_route(&mut model, true);

            assert_eq!(model.routing_settings_draft, None);
        }
    }

    #[test]
    fn rules_of_an_invalid_routing_json_do_not_change() {
        for change in [
            |model: &mut Model| move_selected_rule(model, Direction::Down),
            |model: &mut Model| cycle_selected_rule_route(model, true),
        ] {
            let mut model = routing_page("bypass_ru");
            model.routing_error = Some("broken".into());
            model.settings_menu_selected = 1;

            let effects = change(&mut model);

            assert_eq!(model.routing_settings_draft, None);
            assert!(model.status_is_error());
            assert!(!effects.is_empty());
        }
    }
}

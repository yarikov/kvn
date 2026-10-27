use crate::app::effect::Effect;
use crate::app::model::{
    AppStatus, Model, RoutingSettingsRow, RuleOrderDraft, SourceRow, StatusKey, row_for_profile,
    row_for_subscription_header,
};
use crate::app::update::status::push_status;
use crate::config::profile::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app::update) enum Direction {
    Up,
    Down,
}

pub(in crate::app::update) fn move_selected_source(
    model: &mut Model,
    direction: Direction,
) -> Vec<Effect> {
    let moved_to_row = match model.selected_row() {
        Some(SourceRow::StandaloneProfile(profile_idx))
        | Some(SourceRow::SubscriptionProfile { profile_idx, .. }) => {
            move_profile(&mut model.config, profile_idx, direction)
        }
        Some(SourceRow::SubscriptionHeader(sub_idx)) => {
            move_subscription(&mut model.config, sub_idx, direction)
        }
        None => None,
    };
    match moved_to_row {
        Some(row) => {
            model.selected = row;
            vec![Effect::SaveConfig]
        }
        None => vec![],
    }
}

fn move_profile(config: &mut Config, profile_idx: usize, direction: Direction) -> Option<usize> {
    let group = config.profiles[profile_idx].subscription_id;
    let mut group_members = config
        .profiles
        .iter()
        .enumerate()
        .filter(|(_, profile)| profile.subscription_id == group)
        .map(|(idx, _)| idx);
    let neighbor = match direction {
        Direction::Up => group_members.rev().find(|&idx| idx < profile_idx)?,
        Direction::Down => group_members.find(|&idx| idx > profile_idx)?,
    };
    config.profiles.swap(profile_idx, neighbor);
    Some(row_for_profile(config, neighbor))
}

fn move_subscription(config: &mut Config, sub_idx: usize, direction: Direction) -> Option<usize> {
    let neighbor = match direction {
        Direction::Up => sub_idx.checked_sub(1)?,
        Direction::Down => sub_idx + 1,
    };
    if neighbor >= config.subscriptions.len() {
        return None;
    }
    config.subscriptions.swap(sub_idx, neighbor);
    Some(row_for_subscription_header(config, neighbor))
}

pub(in crate::app::update) fn move_selected_rule(
    model: &mut Model,
    direction: Direction,
) -> Vec<Effect> {
    let RoutingSettingsRow::Rule(index) = model.routing_settings_row() else {
        return vec![];
    };
    if model.routing_error.is_some() {
        let mut effects = vec![];
        push_status(
            &mut effects,
            model,
            StatusKey::Routing,
            AppStatus::Error(
                "Rules cannot be moved while routing.json is invalid; fix it with e".into(),
            ),
        );
        return effects;
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
    let mode = model.shown_routing_mode().to_string();
    let saved = match model.shown_rule_order_draft() {
        Some(draft) => draft.saved.clone(),
        None => model
            .routing
            .mode(&mode)
            .map_or_else(Vec::new, |definition| definition.rules.clone()),
    };
    let orders = &mut model
        .routing_settings_draft
        .get_or_insert_default()
        .rule_orders;
    if reordered == saved {
        orders.remove(&mode);
    } else {
        orders.insert(
            mode,
            RuleOrderDraft {
                saved,
                rules: reordered,
            },
        );
    }
    model.drop_empty_routing_settings_draft();
    model.settings_menu_selected = neighbor + 1;
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::{Profile, Subscription};
    use crate::test_helpers::*;
    use uuid::Uuid;

    fn subscription(name: &str) -> Subscription {
        Subscription {
            name: name.into(),
            ..subscription_with_hwid(false, None)
        }
    }

    fn profile(name: &str, subscription_id: Option<Uuid>) -> Profile {
        let mut profile = Profile::new_vless(name.into(), "h".into(), 1, Uuid::new_v4().into());
        profile.subscription_id = subscription_id;
        profile
    }

    fn grouped_model() -> Model {
        let first = subscription("S1");
        let second = subscription("S2");
        let mut model = model_with_profiles(vec![
            profile("P1", Some(first.id)),
            profile("A", None),
            profile("P2", Some(first.id)),
            profile("B", None),
            profile("Q1", Some(second.id)),
        ]);
        model.config.subscriptions = vec![first, second];
        model
    }

    fn row_names(model: &Model) -> Vec<&str> {
        model
            .source_rows()
            .into_iter()
            .map(|row| match row {
                SourceRow::StandaloneProfile(idx)
                | SourceRow::SubscriptionProfile {
                    profile_idx: idx, ..
                } => model.config.profiles[idx].name.as_str(),
                SourceRow::SubscriptionHeader(idx) => model.config.subscriptions[idx].name.as_str(),
            })
            .collect()
    }

    fn selected_name(model: &Model) -> &str {
        row_names(model)[model.selected]
    }

    #[test]
    fn standalone_profile_moves_down_and_keeps_selection() {
        let mut model = grouped_model();
        model.selected = 0;

        let effects = move_selected_source(&mut model, Direction::Down);

        assert_eq!(effects, vec![Effect::SaveConfig]);
        assert_eq!(row_names(&model), ["B", "A", "S1", "P1", "P2", "S2", "Q1"]);
        assert_eq!(selected_name(&model), "A");
    }

    #[test]
    fn last_standalone_profile_does_not_enter_a_subscription() {
        let mut model = grouped_model();
        model.selected = 1;

        let effects = move_selected_source(&mut model, Direction::Down);

        assert!(effects.is_empty());
        assert_eq!(row_names(&model), ["A", "B", "S1", "P1", "P2", "S2", "Q1"]);
    }

    #[test]
    fn subscription_profile_moves_within_its_subscription() {
        let mut model = grouped_model();
        model.selected = 4;

        let effects = move_selected_source(&mut model, Direction::Up);

        assert_eq!(effects, vec![Effect::SaveConfig]);
        assert_eq!(row_names(&model), ["A", "B", "S1", "P2", "P1", "S2", "Q1"]);
        assert_eq!(selected_name(&model), "P2");
    }

    #[test]
    fn first_subscription_profile_does_not_leave_its_subscription() {
        let mut model = grouped_model();
        model.selected = 3;

        assert!(move_selected_source(&mut model, Direction::Up).is_empty());
        model.selected = 6;
        assert!(move_selected_source(&mut model, Direction::Up).is_empty());
        assert_eq!(row_names(&model), ["A", "B", "S1", "P1", "P2", "S2", "Q1"]);
    }

    #[test]
    fn subscription_moves_up_with_its_profiles() {
        let mut model = grouped_model();
        model.selected = 5;

        let effects = move_selected_source(&mut model, Direction::Up);

        assert_eq!(effects, vec![Effect::SaveConfig]);
        assert_eq!(row_names(&model), ["A", "B", "S2", "Q1", "S1", "P1", "P2"]);
        assert_eq!(selected_name(&model), "S2");
    }

    #[test]
    fn first_subscription_stays_below_standalone_profiles() {
        let mut model = grouped_model();
        model.selected = 2;

        assert!(move_selected_source(&mut model, Direction::Up).is_empty());
        model.selected = 5;
        assert!(move_selected_source(&mut model, Direction::Down).is_empty());
        assert_eq!(row_names(&model), ["A", "B", "S1", "P1", "P2", "S2", "Q1"]);
    }

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

        let orders = &model.routing_settings_draft.as_ref().unwrap().rule_orders;
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
    fn rules_of_an_invalid_routing_json_do_not_move() {
        let mut model = routing_page("bypass_ru");
        model.routing_error = Some("broken".into());
        model.settings_menu_selected = 1;

        let effects = move_selected_rule(&mut model, Direction::Down);

        assert_eq!(model.routing_settings_draft, None);
        assert!(model.status_is_error());
        assert!(!effects.is_empty());
    }
}

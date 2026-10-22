use crate::app::effect::Effect;
use crate::app::model::{Model, SourceRow, row_for_profile, row_for_subscription_header};
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
}

use crate::app::effect::Effect;
use crate::app::model::{AppStatus, Model, Overlay};
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::update::key::sources::is_connection_affected;
use crate::app::update::status::push_status;

pub(in crate::app::update) fn handle_confirm_delete(
    model: &mut Model,
    key: KeyEvent,
) -> Vec<Effect> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => {
            if is_connection_affected(model) {
                model.overlay = Overlay::None;
                let mut effects = vec![];
                push_status(
                    &mut effects,
                    model,
                    AppStatus::Error("Disconnect before deleting".into()),
                );
                return effects;
            }
            model.overlay = Overlay::None;
            let row = model.selected_row();
            match row {
                Some(crate::app::model::SourceRow::StandaloneProfile(_))
                | Some(crate::app::model::SourceRow::SubscriptionProfile { .. }) => {
                    let name = model.selected_profile().map(|p| p.name.clone());
                    model.delete_selected();
                    let mut effects = vec![Effect::SaveConfig];
                    if let Some(name) = name {
                        push_status(
                            &mut effects,
                            model,
                            crate::app::model::AppStatus::Info(format!(
                                "Profile '{}' deleted",
                                name
                            )),
                        );
                    }
                    return effects;
                }
                Some(crate::app::model::SourceRow::SubscriptionHeader(_)) => {
                    let name = model.selected_subscription().map(|s| s.name.clone());
                    model.delete_selected();
                    let mut effects = vec![Effect::SaveConfig];
                    if let Some(name) = name {
                        push_status(
                            &mut effects,
                            model,
                            crate::app::model::AppStatus::Info(format!(
                                "Subscription '{}' deleted",
                                name
                            )),
                        );
                    }
                    return effects;
                }
                _ => {}
            }
        }
        KeyCode::Char('n') | KeyCode::Char('q') | KeyCode::Esc => {
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
    use crate::config::profile::{Profile, Subscription, SubscriptionAutoUpdate};
    use crate::test_helpers::*;
    use uuid::Uuid;

    #[test]
    fn confirm_delete_yes() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.overlay = Overlay::ConfirmDelete;
        let effects = handle_confirm_delete(&mut model, key('y'));
        assert!(model.config.profiles.is_empty());
        assert_eq!(model.overlay, Overlay::None);
        assert_eq!(
            effects,
            vec![Effect::SaveConfig, app_log_info("Profile 'A' deleted")]
        );
    }

    #[test]
    fn confirm_delete_no() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.overlay = Overlay::ConfirmDelete;
        let effects = handle_confirm_delete(&mut model, key('n'));
        assert_eq!(model.config.profiles.len(), 1);
        assert_eq!(model.overlay, Overlay::None);
        assert!(effects.is_empty());
    }

    #[test]
    fn confirm_delete_subscription_removes_subscription_and_profiles() {
        let sub_id = Uuid::new_v4();
        let mut existing = Profile::new_vless(
            "Old".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        );
        existing.subscription_id = Some(sub_id);
        let mut model = model_with_profiles(vec![existing]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Sub".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        // source_rows: [SubscriptionHeader(0), SubscriptionProfile { sub_idx: 0, profile_idx: 0 }]
        model.selected = 0;
        model.overlay = Overlay::ConfirmDelete;

        let effects = handle_confirm_delete(&mut model, KeyEvent::from(KeyCode::Enter));

        assert!(model.config.subscriptions.is_empty());
        assert!(model.config.profiles.is_empty());
        assert_eq!(model.selected, 0);
        assert_eq!(
            effects,
            vec![
                Effect::SaveConfig,
                app_log_info("Subscription 'Sub' deleted")
            ]
        );
    }

    #[test]
    fn confirm_delete_rechecks_connection_target() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            "u".into(),
        )]);
        model.overlay = Overlay::ConfirmDelete;
        model.connection = ConnectionState::ConnectPending;
        model.connecting_profile_id = Some(model.config.profiles[0].id);

        let effects = handle_confirm_delete(&mut model, enter());

        assert_eq!(model.config.profiles.len(), 1);
        assert_eq!(model.overlay, Overlay::None);
        assert!(model.status_text().contains("Disconnect before"));
        assert!(!effects.contains(&Effect::SaveConfig));
    }

    // ---- handle_confirm_delete gaps ----

    #[test]
    fn confirm_delete_enter_on_profile_deletes() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            "u".into(),
        )]);
        model.overlay = Overlay::ConfirmDelete;
        let effects = handle_confirm_delete(&mut model, enter());
        assert!(model.config.profiles.is_empty());
        assert_eq!(model.overlay, Overlay::None);
        assert!(effects.contains(&Effect::SaveConfig));
    }

    #[test]
    fn confirm_delete_y_on_subscription_header_deletes_sub() {
        let sub_id = Uuid::new_v4();
        let mut profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u".into());
        profile.subscription_id = Some(sub_id);
        let mut model = model_with_profiles(vec![profile]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Example".into(),
            url: "http://e".into(),
            auto_update: SubscriptionAutoUpdate::Off,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        model.overlay = Overlay::ConfirmDelete;
        model.selected = crate::app::model::row_for_subscription_header(&model.config, 0);
        let effects = handle_confirm_delete(&mut model, key('y'));
        assert!(model.config.subscriptions.is_empty());
        assert!(model.config.profiles.is_empty());
        assert_eq!(model.overlay, Overlay::None);
        assert!(effects.iter().any(
            |e| matches!(e, Effect::AppendAppLog { message, .. } if message.contains("Example"))
        ));
    }

    #[test]
    fn confirm_delete_esc_cancels() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            "u".into(),
        )]);
        model.overlay = Overlay::ConfirmDelete;
        let effects = handle_confirm_delete(&mut model, esc());
        assert_eq!(model.config.profiles.len(), 1);
        assert_eq!(model.overlay, Overlay::None);
        assert!(effects.is_empty());
    }

    #[test]
    fn confirm_delete_q_cancels() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            "u".into(),
        )]);
        model.overlay = Overlay::ConfirmDelete;

        let effects = handle_confirm_delete(&mut model, key('q'));

        assert_eq!(model.config.profiles.len(), 1);
        assert_eq!(model.overlay, Overlay::None);
        assert!(effects.is_empty());
    }

    #[test]
    fn confirm_delete_unknown_key_noop() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            "u".into(),
        )]);
        model.overlay = Overlay::ConfirmDelete;
        let effects = handle_confirm_delete(&mut model, key('z'));
        assert_eq!(model.config.profiles.len(), 1);
        // Unknown key leaves overlay open.
        assert_eq!(model.overlay, Overlay::ConfirmDelete);
        assert!(effects.is_empty());
    }
}

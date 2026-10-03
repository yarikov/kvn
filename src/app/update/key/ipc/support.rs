use crate::app::effect::Effect;
use crate::app::model::{HelpContext, HelpState, Model, Overlay};
use crate::app::msg::SupportPromptResolution;

pub(super) fn check_prompt(model: &mut Model) -> Vec<Effect> {
    if model.overlay == Overlay::None
        && model.onboarding.is_complete()
        && model.support_prompt.is_due(chrono::Utc::now())
    {
        model.support_selected = 0;
        model.overlay = Overlay::Support;
    }
    vec![]
}

pub(super) fn resolve_prompt(
    model: &mut Model,
    resolution: SupportPromptResolution,
) -> Vec<Effect> {
    if model.overlay != Overlay::Support {
        return vec![];
    }
    let previous = model.support_prompt.clone();
    match resolution {
        SupportPromptResolution::RemindLater => {
            model.support_prompt.remind_later(chrono::Utc::now());
        }
        SupportPromptResolution::Supported => {
            model.support_prompt.supported(chrono::Utc::now());
        }
        SupportPromptResolution::Dismiss => {
            model.support_prompt.dismiss();
        }
    }
    model.overlay = Overlay::None;
    vec![Effect::PersistSupportPrompt {
        previous,
        reopen_prompt: true,
    }]
}

pub(super) fn dismiss_prompt(model: &mut Model) -> Vec<Effect> {
    let previous = model.support_prompt.clone();
    model.support_prompt.dismiss();
    let closed_prompt = matches!(
        model.overlay,
        Overlay::Support
            | Overlay::Help(HelpState {
                context: HelpContext::Support,
                ..
            })
    );
    if closed_prompt {
        model.overlay = Overlay::None;
    }
    vec![Effect::PersistSupportPrompt {
        previous,
        reopen_prompt: closed_prompt,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::msg::IpcCommand;
    use crate::app::update::key::ipc::handle_ipc_command;

    #[test]
    fn support_prompt_check_requires_finished_onboarding_and_due_deadline() {
        let mut model = crate::test_helpers::model_with_profiles(vec![]);
        model.support_prompt.next_show_at = Some(chrono::Utc::now() - chrono::Duration::seconds(1));

        let effects = handle_ipc_command(&mut model, IpcCommand::CheckSupportPrompt);
        assert_eq!(model.overlay, Overlay::None);
        assert_eq!(effects, vec![Effect::BroadcastState]);

        model.onboarding.state.complete(chrono::Utc::now());
        let effects = handle_ipc_command(&mut model, IpcCommand::CheckSupportPrompt);
        assert_eq!(model.overlay, Overlay::Support);
        assert_eq!(model.support_selected, 0);
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn support_prompt_resolutions_persist_and_close() {
        use crate::app::msg::SupportPromptResolution;

        let mut model = crate::test_helpers::model_with_profiles(vec![]);
        model.overlay = Overlay::Support;
        model.support_prompt.next_show_at = Some(chrono::Utc::now() - chrono::Duration::days(1));
        let previous = model.support_prompt.clone();
        let effects = handle_ipc_command(
            &mut model,
            IpcCommand::ResolveSupportPrompt {
                resolution: SupportPromptResolution::RemindLater,
            },
        );
        assert_eq!(model.overlay, Overlay::None);
        assert!(
            model.support_prompt.next_show_at
                > Some(chrono::Utc::now() + chrono::Duration::days(29))
        );
        assert_eq!(
            effects,
            vec![
                Effect::PersistSupportPrompt {
                    previous,
                    reopen_prompt: true,
                },
                Effect::BroadcastState
            ]
        );

        model.overlay = Overlay::Support;
        let effects = handle_ipc_command(
            &mut model,
            IpcCommand::ResolveSupportPrompt {
                resolution: SupportPromptResolution::Dismiss,
            },
        );
        assert!(model.support_prompt.dismissed);
        assert_eq!(model.support_prompt.next_show_at, None);
        assert!(matches!(effects[0], Effect::PersistSupportPrompt { .. }));
    }

    #[test]
    fn dismiss_command_closes_open_prompt_and_persists() {
        let mut model = crate::test_helpers::model_with_profiles(vec![]);
        model.overlay = Overlay::Support;
        model.support_prompt.next_show_at = Some(chrono::Utc::now());
        let previous = model.support_prompt.clone();

        let effects = handle_ipc_command(&mut model, IpcCommand::DismissSupportPrompt);

        assert_eq!(model.overlay, Overlay::None);
        assert!(model.support_prompt.dismissed);
        assert_eq!(
            effects,
            vec![
                Effect::PersistSupportPrompt {
                    previous,
                    reopen_prompt: true,
                },
                Effect::BroadcastState
            ]
        );
    }

    #[test]
    fn dismiss_command_closes_help_that_would_restore_the_prompt() {
        let mut model = crate::test_helpers::model_with_profiles(vec![]);
        model.overlay = Overlay::Help(HelpState {
            context: HelpContext::Support,
            selected: 0,
        });

        let effects = handle_ipc_command(&mut model, IpcCommand::DismissSupportPrompt);

        assert_eq!(model.overlay, Overlay::None);
        assert!(matches!(
            effects[0],
            Effect::PersistSupportPrompt {
                reopen_prompt: true,
                ..
            }
        ));
    }

    #[test]
    fn dismiss_command_leaves_other_overlays_open() {
        let mut model = crate::test_helpers::model_with_profiles(vec![]);
        model.overlay = Overlay::DnsSettings;

        let effects = handle_ipc_command(&mut model, IpcCommand::DismissSupportPrompt);

        assert_eq!(model.overlay, Overlay::DnsSettings);
        assert!(model.support_prompt.dismissed);
        assert!(matches!(
            effects[0],
            Effect::PersistSupportPrompt {
                reopen_prompt: false,
                ..
            }
        ));
    }

    #[test]
    fn supported_prompt_is_recorded_and_closed() {
        use crate::app::msg::SupportPromptResolution;

        let mut model = crate::test_helpers::model_with_profiles(vec![]);
        model.overlay = Overlay::Support;
        let effects = handle_ipc_command(
            &mut model,
            IpcCommand::ResolveSupportPrompt {
                resolution: SupportPromptResolution::Supported,
            },
        );

        assert_eq!(model.overlay, Overlay::None);
        assert!(model.support_prompt.supported_at.is_some());
        assert_eq!(model.support_prompt.next_show_at, None);
        assert!(matches!(effects[0], Effect::PersistSupportPrompt { .. }));
    }
}

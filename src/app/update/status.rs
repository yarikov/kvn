use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model};
use crate::app::msg::CopiedTarget;

/// Set the application status (pure, in-memory) and return an effect that
/// appends the same message to the on-disk log file.
pub(in crate::app::update) fn set_status(model: &mut Model, status: AppStatus) -> Option<Effect> {
    let (level, text) = match &status {
        AppStatus::Info(text) => ("INFO", text),
        AppStatus::Error(text) => ("ERROR", text),
    };
    let effect = (!text.is_empty()).then(|| Effect::AppendAppLog {
        level: level.to_string(),
        message: text.clone(),
    });
    model.set_status(status);
    effect
}

pub(in crate::app::update) fn push_status(
    effects: &mut Vec<Effect>,
    model: &mut Model,
    status: AppStatus,
) {
    if let Some(e) = set_status(model, status) {
        effects.push(e);
    }
}

#[derive(Clone, Copy)]
pub(in crate::app::update) enum DownloadKind {
    Geo,
    Subscription,
}

pub(in crate::app::update) fn download_allowed(model: &Model) -> bool {
    !model.config.settings.kill_switch || model.connection == ConnectionState::Connected
}

pub(in crate::app::update) fn push_download_blocked(
    effects: &mut Vec<Effect>,
    model: &mut Model,
    kind: DownloadKind,
) {
    let message = match kind {
        DownloadKind::Geo => {
            "Geo download failed: blocked by kill switch; connect to VPN and retry"
        }
        DownloadKind::Subscription => {
            "Subscription update failed: blocked by kill switch; connect to VPN and retry"
        }
    };
    push_status(effects, model, AppStatus::Error(message.into()));
}

pub(in crate::app::update) fn append_download_hint(
    effects: &mut Vec<Effect>,
    model: &Model,
    kind: DownloadKind,
) {
    if model.connection == ConnectionState::Connected {
        return;
    }
    let message = match (model.config.settings.kill_switch, kind) {
        (true, DownloadKind::Geo) => {
            "Kill switch is enabled and VPN is disconnected. Reconnect VPN and retry the geo download."
        }
        (true, DownloadKind::Subscription) => {
            "Kill switch is enabled and VPN is disconnected. Reconnect VPN and retry the subscription update."
        }
        (false, DownloadKind::Geo) => {
            "VPN is disconnected. Try connecting to VPN and retrying the geo download."
        }
        (false, DownloadKind::Subscription) => {
            "VPN is disconnected. Try connecting to VPN and retrying the subscription update."
        }
    };
    effects.push(Effect::AppendAppLog {
        level: "WARN".to_string(),
        message: message.into(),
    });
}

pub(in crate::app::update) fn handle_copied_status(
    model: &mut Model,
    target: CopiedTarget,
) -> Vec<Effect> {
    let msg = match target {
        CopiedTarget::Profile { name } => format!("Profile copied: {name}"),
        CopiedTarget::Subscription { name } => format!("Subscription copied: {name}"),
        CopiedTarget::Logs => "Logs copied".to_string(),
        CopiedTarget::Command => "Command copied".to_string(),
    };
    let mut effects = Vec::new();
    push_status(&mut effects, model, crate::app::model::AppStatus::Info(msg));
    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::model_with_profiles;

    #[test]
    fn copied_status_matches_the_target() {
        let cases = [
            (
                CopiedTarget::Profile {
                    name: "Alpha".into(),
                },
                "Profile copied: Alpha",
            ),
            (
                CopiedTarget::Subscription {
                    name: "Beta".into(),
                },
                "Subscription copied: Beta",
            ),
            (CopiedTarget::Logs, "Logs copied"),
            (CopiedTarget::Command, "Command copied"),
        ];

        for (target, expected) in cases {
            let mut model = model_with_profiles(vec![]);
            let effects = handle_copied_status(&mut model, target);

            assert_eq!(model.status_text(), expected);
            assert_eq!(
                effects,
                vec![Effect::AppendAppLog {
                    level: "INFO".into(),
                    message: expected.into(),
                }]
            );
        }
    }
}

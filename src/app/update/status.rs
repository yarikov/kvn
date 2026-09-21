use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model};

/// Set the application status (pure, in-memory) and return an effect that
/// appends the same message to the on-disk log file.
pub(in crate::app::update) fn set_status(model: &mut Model, status: AppStatus) -> Option<Effect> {
    let text = status.text();
    let effect = if text.is_empty() {
        None
    } else {
        let level = match &status {
            AppStatus::Info(_) => "INFO",
            AppStatus::Error(_) => "ERROR",
        };
        Some(Effect::AppendAppLog {
            level: level.to_string(),
            message: text.to_string(),
        })
    };
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
            "Geo download is blocked by the kill switch. Connect to VPN and retry."
        }
        DownloadKind::Subscription => {
            "Subscription update is blocked by the kill switch. Connect to VPN and retry."
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
    name: String,
    count: usize,
) -> Vec<Effect> {
    let msg = if name == "log" && count > 1 {
        format!("Copied {count} logs")
    } else if count <= 1 {
        format!("Copied: {name}")
    } else {
        format!("Copied {count} links from {name}")
    };
    let mut effects = Vec::new();
    push_status(&mut effects, model, crate::app::model::AppStatus::Info(msg));
    effects
}

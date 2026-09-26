use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model};
use uuid::Uuid;

use crate::app::update::connection::queue_connect;
use crate::app::update::status::push_status;

pub(super) fn connect_profile(model: &mut Model, profile_id: Uuid) -> Vec<Effect> {
    let profile_name = model
        .config
        .profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .map(|profile| profile.name.clone());
    let mut effects = vec![];
    match profile_name {
        Some(profile_name) => {
            push_status(
                &mut effects,
                model,
                AppStatus::Info(format!("Connecting to {profile_name}…")),
            );
            queue_connect(model, profile_id);
        }
        None => push_status(
            &mut effects,
            model,
            AppStatus::Error("Connection failed: profile no longer exists".into()),
        ),
    }
    effects
}

pub(super) fn disconnect(model: &mut Model) -> Vec<Effect> {
    match model.connection {
        ConnectionState::Connected
        | ConnectionState::Connecting
        | ConnectionState::ConnectPending => vec![Effect::Disconnect],
        ConnectionState::Idle if matches!(model.status, Some(AppStatus::Error(_))) => {
            let mut effects = vec![];
            push_status(&mut effects, model, AppStatus::Info("Disconnected".into()));
            effects
        }
        ConnectionState::Idle => vec![],
    }
}

pub(super) fn reconnect(model: &mut Model) -> Vec<Effect> {
    let profile_id = match model.connection {
        ConnectionState::Connected => model.active_profile_id,
        ConnectionState::Connecting | ConnectionState::ConnectPending => {
            model.connecting_profile_id
        }
        ConnectionState::Idle => {
            let mut effects = vec![];
            push_status(
                &mut effects,
                model,
                AppStatus::Error("Reconnect failed: VPN is disconnected".into()),
            );
            return effects;
        }
    };
    connect_known_profile(
        model,
        profile_id,
        "Reconnecting to",
        "Reconnect failed: profile no longer exists",
    )
}

pub(super) fn toggle(model: &mut Model) -> Vec<Effect> {
    match model.connection {
        ConnectionState::Connected
        | ConnectionState::Connecting
        | ConnectionState::ConnectPending => vec![Effect::Disconnect],
        ConnectionState::Idle => connect_known_profile(
            model,
            model.config.settings.last_connected_profile,
            "Connecting to",
            "Connection failed: no previous profile; run `kvn connect <name>` first",
        ),
    }
}

fn connect_known_profile(
    model: &mut Model,
    profile_id: Option<Uuid>,
    verb: &str,
    missing: &str,
) -> Vec<Effect> {
    let target = profile_id
        .and_then(|id| model.config.profiles.iter().find(|p| p.id == id))
        .map(|profile| (profile.id, profile.name.clone()));
    let mut effects = vec![];
    match target {
        Some((profile_id, profile_name)) => {
            push_status(
                &mut effects,
                model,
                AppStatus::Info(format!("{verb} {profile_name}…")),
            );
            queue_connect(model, profile_id);
        }
        None => push_status(&mut effects, model, AppStatus::Error(missing.into())),
    }
    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::update::key::ipc::handle_ipc_command;
    use crate::config::profile::Profile;
    use crate::test_helpers::*;

    #[test]
    fn ipc_command_disconnect_when_connected() {
        let (mut model, _) = connected_model();
        let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::Disconnect);
        assert_eq!(effects, vec![Effect::Disconnect, Effect::BroadcastState]);
    }

    #[test]
    fn ipc_command_disconnect_when_idle_is_noop() {
        let mut model = model_with_profiles(vec![]);
        let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::Disconnect);
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn ipc_command_disconnect_cancels_in_progress_connection() {
        for connection in [ConnectionState::Connecting, ConnectionState::ConnectPending] {
            let mut model = model_with_profiles(vec![]);
            model.connection = connection;
            let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::Disconnect);
            assert_eq!(effects, vec![Effect::Disconnect, Effect::BroadcastState]);
        }
    }

    #[test]
    fn ipc_command_reconnect_when_connected() {
        let (mut model, a_id) = connected_model();
        let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::Reconnect);
        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(a_id));
        assert_eq!(model.connect_attempt_id, 1);
        assert_eq!(
            effects,
            vec![app_log_info("Reconnecting to A…"), Effect::BroadcastState]
        );
    }

    #[test]
    fn ipc_command_reconnect_restarts_in_progress_connection() {
        for connection in [ConnectionState::Connecting, ConnectionState::ConnectPending] {
            let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
            let profile_id = profile.id;
            let mut model = model_with_profiles(vec![profile]);
            model.connection = connection;
            model.connecting_profile_id = Some(profile_id);
            model.connect_attempt_id = 7;

            let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::Reconnect);

            assert_eq!(model.connection, ConnectionState::Connecting);
            assert_eq!(model.connecting_profile_id, Some(profile_id));
            assert_eq!(model.connect_attempt_id, 8);
            assert_eq!(model.status_text(), "Reconnecting to A…");
            assert_eq!(
                effects,
                vec![app_log_info("Reconnecting to A…"), Effect::BroadcastState]
            );
        }
    }

    #[test]
    fn ipc_command_reconnect_when_idle_reports_error() {
        let mut model = model_with_profiles(vec![]);
        let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::Reconnect);
        assert_eq!(
            effects,
            vec![
                app_log_error("Reconnect failed: VPN is disconnected"),
                Effect::BroadcastState,
            ]
        );
        assert_eq!(model.connection, ConnectionState::Idle);
        assert_eq!(model.status_text(), "Reconnect failed: VPN is disconnected");
    }

    #[test]
    fn ipc_command_toggle_disconnects_or_cancels_non_idle_connection() {
        for connection in [
            ConnectionState::Connected,
            ConnectionState::Connecting,
            ConnectionState::ConnectPending,
        ] {
            let mut model = model_with_profiles(vec![]);
            model.connection = connection;
            let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::Toggle);
            assert_eq!(effects, vec![Effect::Disconnect, Effect::BroadcastState]);
        }
    }

    #[test]
    fn ipc_command_toggle_connects_last_successful_profile() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.config.settings.last_connected_profile = Some(profile_id);

        let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::Toggle);

        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(profile_id));
        assert_eq!(model.status_text(), "Connecting to A…");
        assert_eq!(
            effects,
            vec![app_log_info("Connecting to A…"), Effect::BroadcastState]
        );
    }

    #[test]
    fn ipc_command_toggle_without_last_profile_reports_error() {
        let mut model = model_with_profiles(vec![]);

        let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::Toggle);

        assert_eq!(model.connection, ConnectionState::Idle);
        assert_eq!(
            effects,
            vec![
                app_log_error(
                    "Connection failed: no previous profile; run `kvn connect <name>` first"
                ),
                Effect::BroadcastState,
            ]
        );
    }

    #[test]
    fn ipc_connect_profile_switches_or_reconnects_without_disconnecting() {
        let first = Profile::new_vless("A".into(), "a".into(), 1, "u".into());
        let second = Profile::new_vless("B".into(), "b".into(), 2, "v".into());
        let mut model = model_with_profiles(vec![first.clone(), second.clone()]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(first.id);

        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::ConnectProfile {
                profile_id: second.id,
            },
        );
        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(second.id));
        assert_eq!(model.status_text(), "Connecting to B…");
        assert_eq!(
            effects,
            vec![
                Effect::AppendAppLog {
                    level: "INFO".into(),
                    message: "Connecting to B…".into(),
                },
                Effect::BroadcastState,
            ]
        );
        assert!(!effects.contains(&Effect::Disconnect));

        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(first.id);
        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::ConnectProfile {
                profile_id: first.id,
            },
        );
        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(first.id));
        assert_eq!(model.status_text(), "Connecting to A…");
        assert_eq!(
            effects,
            vec![
                Effect::AppendAppLog {
                    level: "INFO".into(),
                    message: "Connecting to A…".into(),
                },
                Effect::BroadcastState,
            ]
        );
        assert!(!effects.contains(&Effect::Disconnect));
    }

    #[test]
    fn ipc_connect_profile_rejects_unknown_uuid() {
        let profile = Profile::new_vless("A".into(), "a".into(), 1, "u".into());
        let mut model = model_with_profiles(vec![profile.clone()]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile.id);
        handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::ConnectProfile {
                profile_id: uuid::Uuid::new_v4(),
            },
        );
        assert_eq!(model.connection, ConnectionState::Connected);
        assert_eq!(model.connecting_profile_id, None);
    }
}

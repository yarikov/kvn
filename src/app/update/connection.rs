use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model, TrafficStats};
use uuid::Uuid;

use crate::app::update::status::push_status;

pub(in crate::app::update) fn on_connected(
    model: &mut Model,
    pid: u32,
    profile_id: Uuid,
    attempt_id: u64,
) -> Vec<Effect> {
    if attempt_id != model.connect_attempt_id {
        return vec![];
    }
    model.singbox_pid = Some(pid);
    model.connection = ConnectionState::Connected;
    model.connecting_profile_id = None;
    // Fresh sing-box → fresh counters. Drop the previous sample so the
    // first delta is computed against zero rather than a stale value.
    reset_traffic_state(model);
    let mut effects = vec![Effect::WriteState];
    // The tunnel is up — fetch rule-sets for enabled service routes
    // through it if any are still missing (they apply on the next
    // reconnect).
    if !model
        .config
        .settings
        .geo_routing
        .enabled_services()
        .is_empty()
    {
        effects.push(Effect::DownloadServiceRuleSetsIfMissing);
    }
    // Attribute the connection to the profile that actually connected
    // (carried in the message) — never to the cursor's row, which may
    // have moved since the connect was issued.
    model.active_profile_id = Some(profile_id);
    let profile_name = model
        .config
        .profiles
        .iter()
        .find(|p| p.id == profile_id)
        .map(|p| p.name.clone());
    push_status(
        &mut effects,
        model,
        AppStatus::Info(match profile_name {
            Some(name) => format!("Connected to {}", name),
            // Profile deleted while the connect was in flight.
            None => "Connected".to_string(),
        }),
    );
    // Persist last connected profile for auto-connect on next startup.
    if model.config.settings.last_connected_profile != Some(profile_id) {
        model.config.settings.last_connected_profile = Some(profile_id);
        effects.push(Effect::SaveConfig);
    }
    effects
}

pub(in crate::app::update) fn on_connect_failed(
    model: &mut Model,
    attempt_id: u64,
    error: crate::app::msg::IpcError,
) -> Vec<Effect> {
    if attempt_id != model.connect_attempt_id {
        return vec![];
    }
    clear_connection(model);
    let mut effects = vec![Effect::BroadcastState];
    push_status(
        &mut effects,
        model,
        AppStatus::Error(format!("Connection failed: {}", error)),
    );
    effects
}

pub(in crate::app::update) fn on_singbox_exited(
    model: &mut Model,
    attempt_id: u64,
    code: Option<i32>,
    signal: Option<i32>,
) -> Vec<Effect> {
    if attempt_id != model.connect_attempt_id {
        return vec![];
    }
    // Invalidate any Connected/traffic reply that was already queued
    // by the worker for the process that just exited.
    model.connect_attempt_id = model.connect_attempt_id.wrapping_add(1);
    clear_connection(model);

    let reason = match (code, signal) {
        (Some(code), _) => format!("sing-box exited unexpectedly (code {code})"),
        (None, Some(signal)) => {
            format!("sing-box terminated unexpectedly (signal {signal})")
        }
        (None, None) => "sing-box exited unexpectedly".to_string(),
    };
    let mut effects = vec![
        Effect::WriteState,
        Effect::RevokeKillSwitchExceptions,
        Effect::BroadcastState,
    ];
    push_status(&mut effects, model, AppStatus::Error(reason));
    effects
}

pub(in crate::app::update) fn on_system_resumed(model: &mut Model) -> Vec<Effect> {
    if model.connection != ConnectionState::Connected
        || !model
            .active_profile_id
            .is_some_and(|id| queue_connect(model, id))
    {
        return vec![];
    }
    let mut effects = vec![];
    push_status(
        &mut effects,
        model,
        AppStatus::Info("Resumed — reconnecting…".into()),
    );
    effects
}

fn clear_connection(model: &mut Model) {
    model.connection = ConnectionState::Idle;
    model.connecting_profile_id = None;
    model.singbox_pid = None;
    model.active_profile_id = None;
    reset_traffic_state(model);
}

fn reset_traffic_state(model: &mut Model) {
    model.traffic = TrafficStats::default();
    model.last_traffic_sample_at_ms = 0;
    model.traffic_request_id = 0;
    model.last_traffic_response_id = 0;
    model.last_traffic_fetch_at = None;
}

pub(in crate::app::update) fn handle_kill_switch_applied(
    model: &mut Model,
    enabled: bool,
    error: Option<crate::app::msg::IpcError>,
) -> Vec<Effect> {
    if model.kill_switch_pending != Some(enabled) {
        return Vec::new();
    }
    model.kill_switch_pending = None;
    let mut effects = Vec::new();
    match error {
        None => {
            model.config.settings.kill_switch = enabled;
            push_status(
                &mut effects,
                model,
                AppStatus::Info(format!(
                    "Kill switch {}",
                    if enabled { "enabled" } else { "disabled" }
                )),
            );
            effects.push(Effect::SaveConfig);
            effects.push(Effect::BroadcastState);
        }
        Some(err) => {
            push_status(
                &mut effects,
                model,
                AppStatus::Error(format!("Kill switch: {}", err)),
            );
            effects.push(Effect::BroadcastState);
        }
    }
    effects
}

pub(in crate::app::update) fn handle_auto_connect_polkit_checked(
    model: &mut Model,
    error: Option<crate::app::msg::IpcError>,
) -> Vec<Effect> {
    if !model.auto_connect_pending {
        return Vec::new();
    }
    model.auto_connect_pending = false;
    let mut effects = Vec::new();
    match error {
        None => {
            model.config.settings.auto_connect = true;
            push_status(
                &mut effects,
                model,
                AppStatus::Info("Auto-connect enabled".into()),
            );
            effects.push(Effect::SaveConfig);
        }
        Some(err) => {
            push_status(
                &mut effects,
                model,
                AppStatus::Error(format!("Auto-connect not enabled: {err}")),
            );
        }
    }
    effects.push(Effect::BroadcastState);
    effects
}

pub(in crate::app::update) fn queue_connect(model: &mut Model, profile_id: Uuid) -> bool {
    if model
        .config
        .profiles
        .iter()
        .any(|profile| profile.id == profile_id)
    {
        model.connecting_profile_id = Some(profile_id);
        model.connect_attempt_id = model.connect_attempt_id.wrapping_add(1);
        model.connection = ConnectionState::Connecting;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::Overlay;
    use crate::app::msg::Msg;
    use crate::app::update::key::ipc::handle_ipc_command;
    use crate::app::update::key::sources::handle_sources;
    use crate::app::update::tick::handle_tick;
    use crate::app::update::update;
    use crate::config::profile::Profile;
    use crate::test_helpers::*;
    use std::time::Instant;

    #[test]
    fn connect_failed_sets_status_error() {
        let mut model = Model::test_new(crate::config::profile::Config::default());
        model.connection = ConnectionState::ConnectPending;
        model.connect_attempt_id = 7;
        model.singbox_pid = Some(99);
        model.active_profile_id = Some(Uuid::new_v4());
        model.traffic_request_id = 4;
        model.last_traffic_response_id = 3;
        let effects = update(
            &mut model,
            Msg::ConnectFailed {
                attempt_id: 7,
                error: crate::app::msg::IpcError::new("timeout"),
            },
        );
        assert_eq!(model.connection, ConnectionState::Idle);
        assert_eq!(model.singbox_pid, None);
        assert_eq!(model.active_profile_id, None);
        assert_eq!(model.traffic_request_id, 0);
        assert_eq!(model.last_traffic_response_id, 0);
        assert!(model.status.is_error());
        assert!(model.status.text().contains("Connection failed: timeout"));
        assert_eq!(
            effects,
            vec![
                Effect::BroadcastState,
                app_log_error("Connection failed: timeout")
            ]
        );
    }

    #[test]
    fn stale_connect_failed_does_not_override_newer_connection() {
        let profile_id = Uuid::new_v4();
        let mut model = Model::test_new(crate::config::profile::Config::default());
        model.connection = ConnectionState::Connected;
        model.connect_attempt_id = 2;
        model.active_profile_id = Some(profile_id);
        model.singbox_pid = Some(42);

        let effects = update(
            &mut model,
            Msg::ConnectFailed {
                attempt_id: 1,
                error: crate::app::msg::IpcError::new("old timeout"),
            },
        );

        assert!(effects.is_empty());
        assert_eq!(model.connection, ConnectionState::Connected);
        assert_eq!(model.active_profile_id, Some(profile_id));
        assert_eq!(model.singbox_pid, Some(42));
    }

    #[test]
    fn a_frozen_daemon_still_handles_the_tunnel_dying() {
        let profile_id = uuid::Uuid::new_v4();
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Connected;
        model.connect_attempt_id = 3;
        model.active_profile_id = Some(profile_id);
        model.singbox_pid = Some(99);
        model.restart_required = true;

        update(
            &mut model,
            Msg::SingBoxExited {
                attempt_id: 3,
                code: Some(1),
                signal: None,
            },
        );

        assert_eq!(
            model.connection,
            ConnectionState::Idle,
            "a frozen daemon must not keep reporting a tunnel that is gone"
        );
        assert_eq!(model.active_profile_id, None);
        assert_eq!(model.singbox_pid, None);
    }

    #[test]
    fn singbox_exit_clears_connection_and_invalidates_attempt() {
        let profile_id = Uuid::new_v4();
        let mut model = Model::test_new(crate::config::profile::Config::default());
        model.connection = ConnectionState::Connected;
        model.connect_attempt_id = 7;
        model.connecting_profile_id = Some(profile_id);
        model.active_profile_id = Some(profile_id);
        model.singbox_pid = Some(99);
        model.traffic.up_total = 123;
        model.traffic_request_id = 4;
        model.last_traffic_response_id = 3;

        let effects = update(
            &mut model,
            Msg::SingBoxExited {
                attempt_id: 7,
                code: Some(17),
                signal: None,
            },
        );

        assert_eq!(model.connection, ConnectionState::Idle);
        assert_eq!(model.connect_attempt_id, 8);
        assert_eq!(model.connecting_profile_id, None);
        assert_eq!(model.active_profile_id, None);
        assert_eq!(model.singbox_pid, None);
        assert_eq!(model.traffic, TrafficStats::default());
        assert_eq!(model.traffic_request_id, 0);
        assert_eq!(model.last_traffic_response_id, 0);
        assert_eq!(
            model.status.text(),
            "sing-box exited unexpectedly (code 17)"
        );
        assert_eq!(
            effects,
            vec![
                Effect::WriteState,
                Effect::RevokeKillSwitchExceptions,
                Effect::BroadcastState,
                app_log_error("sing-box exited unexpectedly (code 17)"),
            ]
        );
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::Connect { .. }))
        );
    }

    #[test]
    fn singbox_exit_reports_signal() {
        let mut model = Model::test_new(crate::config::profile::Config::default());
        model.connection = ConnectionState::ConnectPending;
        model.connect_attempt_id = 3;

        update(
            &mut model,
            Msg::SingBoxExited {
                attempt_id: 3,
                code: None,
                signal: Some(9),
            },
        );

        assert_eq!(
            model.status.text(),
            "sing-box terminated unexpectedly (signal 9)"
        );
    }

    #[test]
    fn stale_singbox_exit_does_not_override_newer_connection() {
        let profile_id = Uuid::new_v4();
        let mut model = Model::test_new(crate::config::profile::Config::default());
        model.connection = ConnectionState::Connected;
        model.connect_attempt_id = 8;
        model.active_profile_id = Some(profile_id);
        model.singbox_pid = Some(100);

        let effects = update(
            &mut model,
            Msg::SingBoxExited {
                attempt_id: 7,
                code: Some(1),
                signal: None,
            },
        );

        assert!(effects.is_empty());
        assert_eq!(model.connection, ConnectionState::Connected);
        assert_eq!(model.connect_attempt_id, 8);
        assert_eq!(model.active_profile_id, Some(profile_id));
        assert_eq!(model.singbox_pid, Some(100));
    }

    #[test]
    fn stale_connected_does_not_override_newer_attempt() {
        let mut model = Model::test_new(crate::config::profile::Config::default());
        model.connection = ConnectionState::ConnectPending;
        model.connect_attempt_id = 2;

        let effects = update(
            &mut model,
            Msg::Connected {
                pid: 42,
                profile_id: Uuid::new_v4(),
                attempt_id: 1,
            },
        );

        assert!(effects.is_empty());
        assert_eq!(model.connection, ConnectionState::ConnectPending);
        assert_eq!(model.singbox_pid, None);
        assert_eq!(model.active_profile_id, None);
    }

    #[test]
    fn queued_connect_carries_new_attempt_id() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            "u1".into(),
        )]);
        let profile_id = model.config.profiles[0].id;

        assert!(queue_connect(&mut model, profile_id));
        assert_eq!(model.connect_attempt_id, 1);
        let effects = handle_tick(&mut model);

        assert_eq!(model.connecting_profile_id, Some(profile_id));
        assert!(matches!(
            effects.as_slice(),
            [Effect::Connect { attempt_id: 1, .. }]
        ));
    }

    #[test]
    fn connected_clears_pending() {
        let mut model = Model::test_new(crate::config::profile::Config::default());
        model.connection = ConnectionState::ConnectPending;
        let id = uuid::Uuid::new_v4();
        let effects = update(
            &mut model,
            Msg::Connected {
                pid: 12345,
                profile_id: id,
                attempt_id: 0,
            },
        );
        assert_eq!(model.connection, ConnectionState::Connected);
        assert_eq!(model.overlay, Overlay::None);
        // The connection is attributed to the carried id even when the
        // profile is no longer in the list (deleted mid-connect).
        assert_eq!(model.active_profile_id, Some(id));
        assert_eq!(
            effects,
            vec![
                Effect::WriteState,
                app_log_info("Connected"),
                Effect::SaveConfig
            ]
        );
    }

    #[test]
    fn connected_does_not_close_open_overlay() {
        let mut model = Model::test_new(crate::config::profile::Config::default());
        model.connection = ConnectionState::Connecting;
        model.overlay = Overlay::ConfirmDelete;

        update(
            &mut model,
            Msg::Connected {
                pid: 12345,
                profile_id: uuid::Uuid::new_v4(),
                attempt_id: 0,
            },
        );

        assert_eq!(model.connection, ConnectionState::Connected);
        assert_eq!(model.overlay, Overlay::ConfirmDelete);
    }

    #[test]
    fn connected_attributes_to_carried_profile_not_cursor() {
        let a = Profile::new_vless(
            "A".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        );
        let b = Profile::new_vless(
            "B".to_string(),
            "2.2.2.2".to_string(),
            443,
            "u2".to_string(),
        );
        let a_id = a.id;
        let mut model = model_with_profiles(vec![a, b]);
        model.connection = ConnectionState::ConnectPending;
        // Cursor rests on B while A's connect completes.
        model.select_next();
        let effects = update(
            &mut model,
            Msg::Connected {
                pid: 1,
                profile_id: a_id,
                attempt_id: 0,
            },
        );
        assert_eq!(model.active_profile_id, Some(a_id));
        assert_eq!(model.config.settings.last_connected_profile, Some(a_id));
        assert!(effects.contains(&app_log_info("Connected to A")));
    }

    #[test]
    fn system_resumed_reconnects_active_profile_not_cursor() {
        let a = Profile::new_vless(
            "A".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        );
        let b = Profile::new_vless(
            "B".to_string(),
            "2.2.2.2".to_string(),
            443,
            "u2".to_string(),
        );
        let a_id = a.id;
        let mut model = model_with_profiles(vec![a, b]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(a_id);
        model.select_next();

        let effects = update(&mut model, Msg::SystemResumed);

        assert_eq!(model.connecting_profile_id, Some(a_id));
        assert!(effects.contains(&app_log_info("Resumed — reconnecting…")));
        let tick_effects = handle_tick(&mut model);
        assert!(
            tick_effects.iter().any(
                |effect| matches!(effect, Effect::Connect { profile, .. } if profile.id == a_id)
            )
        );
    }

    #[test]
    fn system_resumed_without_resolvable_active_profile_is_noop() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(Uuid::new_v4());

        let effects = update(&mut model, Msg::SystemResumed);

        assert!(effects.is_empty());
    }

    #[test]
    fn connected_fetches_service_rule_sets_only_when_enabled() {
        use crate::config::profile::{RoutedService, ServiceRoute};

        let mut model = Model::test_new(crate::config::profile::Config::default());
        model.connection = ConnectionState::ConnectPending;
        let effects = update(
            &mut model,
            Msg::Connected {
                pid: 1,
                profile_id: uuid::Uuid::new_v4(),
                attempt_id: 0,
            },
        );
        assert!(
            !effects.contains(&Effect::DownloadServiceRuleSetsIfMissing),
            "all service routes are disabled by default — no fetch"
        );

        let mut model = Model::test_new(crate::config::profile::Config::default());
        model
            .config
            .settings
            .geo_routing
            .service_routes
            .insert(RoutedService::Telegram, ServiceRoute::Proxy);
        model.connection = ConnectionState::ConnectPending;
        let effects = update(
            &mut model,
            Msg::Connected {
                pid: 1,
                profile_id: uuid::Uuid::new_v4(),
                attempt_id: 0,
            },
        );
        assert!(effects.contains(&Effect::DownloadServiceRuleSetsIfMissing));

        // Explicitly-disabled entries don't count as enabled.
        let mut model = Model::test_new(crate::config::profile::Config::default());
        model
            .config
            .settings
            .geo_routing
            .service_routes
            .insert(RoutedService::Steam, ServiceRoute::Disabled);
        model.connection = ConnectionState::ConnectPending;
        let effects = update(
            &mut model,
            Msg::Connected {
                pid: 1,
                profile_id: uuid::Uuid::new_v4(),
                attempt_id: 0,
            },
        );
        assert!(!effects.contains(&Effect::DownloadServiceRuleSetsIfMissing));
    }

    #[test]
    fn connected_saves_last_profile() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.connection = ConnectionState::ConnectPending;
        let id = model.config.profiles[0].id;
        let effects = update(
            &mut model,
            Msg::Connected {
                pid: 12345,
                profile_id: id,
                attempt_id: 0,
            },
        );
        assert_eq!(model.connection, ConnectionState::Connected);
        assert_eq!(
            model.config.settings.last_connected_profile,
            Some(model.config.profiles[0].id)
        );
        assert_eq!(
            effects,
            vec![
                Effect::WriteState,
                app_log_info("Connected to A"),
                Effect::SaveConfig
            ]
        );
    }

    #[test]
    fn auto_connect_polkit_failure_keeps_it_disabled_and_logs_error() {
        let mut model = model_with_profiles(vec![]);
        handle_sources(&mut model, key('a'));

        let effects = update(
            &mut model,
            Msg::AutoConnectPolkitChecked {
                error: Some(crate::app::msg::IpcError::new(
                    "reboot to activate the `kvn-tui` group",
                )),
            },
        );

        let message = "Auto-connect not enabled: reboot to activate the `kvn-tui` group";
        assert!(!model.config.settings.auto_connect);
        assert!(!model.auto_connect_pending);
        assert_eq!(model.status, AppStatus::Error(message.into()));
        assert_eq!(
            effects,
            vec![
                Effect::AppendAppLog {
                    level: "ERROR".into(),
                    message: message.into(),
                },
                Effect::BroadcastState,
            ]
        );
    }

    #[test]
    fn stale_auto_connect_polkit_result_is_ignored() {
        let mut model = model_with_profiles(vec![]);
        assert!(update(&mut model, Msg::AutoConnectPolkitChecked { error: None }).is_empty());

        handle_sources(&mut model, key('a'));
        let mut effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::SetAutoConnect { enabled: false },
        );
        effects.retain(|effect| *effect != Effect::BroadcastState);
        assert!(effects.is_empty());
        assert!(!model.auto_connect_pending);
        assert!(update(&mut model, Msg::AutoConnectPolkitChecked { error: None }).is_empty());
        assert!(!model.config.settings.auto_connect);
    }

    #[test]
    fn kill_switch_applied_success_flips_and_saves() {
        let mut model = model_with_profiles(vec![]);
        model.kill_switch_pending = Some(true);
        let effects = update(
            &mut model,
            Msg::KillSwitchApplied {
                enabled: true,
                error: None,
            },
        );
        assert!(model.config.settings.kill_switch);
        assert_eq!(model.kill_switch_pending, None);
        assert!(model.status.text().contains("enabled"));
        assert_eq!(
            effects,
            vec![
                app_log_info("Kill switch enabled"),
                Effect::SaveConfig,
                Effect::BroadcastState,
            ]
        );
    }

    #[test]
    fn kill_switch_applied_error_keeps_bool_unchanged() {
        let mut model = model_with_profiles(vec![]);
        model.kill_switch_pending = Some(true);
        let effects = update(
            &mut model,
            Msg::KillSwitchApplied {
                enabled: true,
                error: Some(crate::app::msg::IpcError::new("helper missing")),
            },
        );
        assert!(!model.config.settings.kill_switch);
        assert_eq!(model.kill_switch_pending, None);
        assert!(model.status.text().contains("helper missing"));
        assert!(!effects.iter().any(|e| matches!(e, Effect::SaveConfig)));
    }

    #[test]
    fn stale_kill_switch_result_is_ignored() {
        let mut model = model_with_profiles(vec![]);
        model.kill_switch_pending = Some(true);
        let status = model.status.clone();

        let effects = update(
            &mut model,
            Msg::KillSwitchApplied {
                enabled: false,
                error: None,
            },
        );

        assert!(effects.is_empty());
        assert!(!model.config.settings.kill_switch);
        assert_eq!(model.kill_switch_pending, Some(true));
        assert_eq!(model.status, status);
    }

    #[test]
    fn kill_switch_disable_uses_pending_false() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.kill_switch = true;

        let effects = handle_sources(&mut model, key('K'));

        assert_eq!(model.kill_switch_pending, Some(false));
        assert!(effects.contains(&Effect::ApplyKillSwitch { enabled: false }));
    }

    #[test]
    fn connected_resets_traffic_state() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            "u".into(),
        )]);
        model.traffic.up_total = 9999;
        model.traffic.down_total = 8888;
        model.traffic.up_rate_bps = 100;
        model.last_traffic_sample_at_ms = 1_000;
        model.last_traffic_fetch_at = Some(Instant::now());
        model.traffic_request_id = 4;
        model.last_traffic_response_id = 3;
        let id = model.config.profiles[0].id;
        let _ = update(
            &mut model,
            Msg::Connected {
                pid: 1234,
                profile_id: id,
                attempt_id: 0,
            },
        );
        assert_eq!(model.traffic, TrafficStats::default());
        assert_eq!(model.last_traffic_sample_at_ms, 0);
        assert!(model.last_traffic_fetch_at.is_none());
        assert_eq!(model.traffic_request_id, 0);
        assert_eq!(model.last_traffic_response_id, 0);
    }
}

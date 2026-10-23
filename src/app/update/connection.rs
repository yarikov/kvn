use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model, StatusKey, TrafficStats};
use crate::app::msg::ConnectRetryReason;
use crate::config::profile::RoutingFallback;
use uuid::Uuid;

use crate::app::update::status::push_status;

pub(in crate::app::update) fn on_connected(
    model: &mut Model,
    pid: u32,
    profile_id: Uuid,
    attempt_id: u64,
    routing_fallback: Option<RoutingFallback>,
) -> Vec<Effect> {
    if attempt_id != model.connect_attempt_id {
        return vec![];
    }
    model.singbox_pid = Some(pid);
    model.routing_fallback = routing_fallback;
    model.connection = ConnectionState::Connected;
    model.connecting_profile_id = None;
    reset_traffic_state(model);
    let mut effects = crate::app::update::onboarding::outcome(
        model,
        crate::app::update::onboarding::Trigger::TunnelUp,
    );
    effects.push(Effect::WriteState);
    model.active_profile_id = Some(profile_id);
    let profile = model.config.profiles.iter().find(|p| p.id == profile_id);
    let profile_name = profile.map(|p| p.name.clone());
    let fallback_settings = (routing_fallback == Some(RoutingFallback::Global))
        .then(|| model.config.settings.with_global_routing());
    let running_settings = fallback_settings.as_ref().unwrap_or(&model.config.settings);
    let warnings: Vec<String> = profile
        .map(|p| {
            [
                crate::singbox::config::dns_bypass_warning(p, running_settings, &model.routing),
                crate::singbox::config::certificate_pin_warning(p),
                crate::singbox::config::ech_dns_warning(p, running_settings),
            ]
            .into_iter()
            .flatten()
            .collect()
        })
        .unwrap_or_default();
    push_status(
        &mut effects,
        model,
        StatusKey::Connection,
        AppStatus::Info(match profile_name {
            Some(name) => format!("Connected to {}", name),
            None => "Connected".to_string(),
        }),
    );
    match routing_fallback {
        Some(fallback) => effects.extend(crate::app::update::geo::fetch_region_rule_sets(
            model, fallback,
        )),
        None => effects.extend(crate::app::update::geo::fetch_skipped_rule_sets(model)),
    }
    for (index, warning) in warnings.into_iter().enumerate() {
        push_status(
            &mut effects,
            model,
            StatusKey::ConnectionWarning(index),
            AppStatus::Error(warning),
        );
    }
    if model.config.settings.last_connected_profile != Some(profile_id) {
        model.config.settings.last_connected_profile = Some(profile_id);
        effects.push(Effect::PersistConfirmedState);
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
        StatusKey::Connection,
        AppStatus::Error(format!("Connection failed: {}", error)),
    );
    effects
}

pub(in crate::app::update) fn on_connect_retrying(
    model: &mut Model,
    attempt_id: u64,
    reason: ConnectRetryReason,
    limit: std::time::Duration,
) -> Vec<Effect> {
    if attempt_id != model.connect_attempt_id {
        return vec![];
    }
    let limit = limit.as_secs();
    let status = match reason {
        ConnectRetryReason::BinaryBeingWritten => {
            format!("The sing-box binary is being written; retrying for up to {limit} seconds")
        }
        ConnectRetryReason::TunNotPermitted => format!(
            "sing-box is not allowed to create the TUN interface yet; retrying for up to {limit} seconds"
        ),
    };
    let mut effects = vec![Effect::BroadcastState];
    push_status(
        &mut effects,
        model,
        StatusKey::Connection,
        AppStatus::Info(status),
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
    push_status(
        &mut effects,
        model,
        StatusKey::Connection,
        AppStatus::Error(reason),
    );
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
        StatusKey::Connection,
        AppStatus::Info("VPN resumed — reconnecting…".into()),
    );
    effects
}

fn clear_connection(model: &mut Model) {
    model.connection = ConnectionState::Idle;
    model.connecting_profile_id = None;
    model.singbox_pid = None;
    model.active_profile_id = None;
    model.routing_fallback = None;
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
                StatusKey::KillSwitch,
                AppStatus::Info(format!(
                    "Kill switch {}",
                    if enabled { "enabled" } else { "disabled" }
                )),
            );
            effects.push(Effect::PersistConfirmedState);
            effects.push(Effect::BroadcastState);
        }
        Some(err) => {
            push_status(
                &mut effects,
                model,
                StatusKey::KillSwitch,
                AppStatus::Error(format!("Kill switch failed: {}", err)),
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
                StatusKey::AutoConnect,
                AppStatus::Info("Auto-connect enabled".into()),
            );
            effects.push(Effect::SaveConfig);
        }
        Some(err) => {
            push_status(
                &mut effects,
                model,
                StatusKey::AutoConnect,
                AppStatus::Error(format!("Auto-connect failed: {err}")),
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

pub(in crate::app::update) fn reconnect_with_new_settings(model: &mut Model) -> bool {
    let profile_id = match model.connection {
        ConnectionState::Connected => model.active_profile_id,
        ConnectionState::ConnectPending => model.connecting_profile_id,
        ConnectionState::Connecting | ConnectionState::Idle => None,
    };
    profile_id.is_some_and(|id| queue_connect(model, id))
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
    fn reconnect_with_new_settings_restarts_only_a_live_or_pending_connection() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        for (state, restarts) in [
            (ConnectionState::Connected, true),
            (ConnectionState::ConnectPending, true),
            (ConnectionState::Connecting, false),
            (ConnectionState::Idle, false),
        ] {
            let mut model = model_with_profiles(vec![profile.clone()]);
            model.connection = state;
            model.active_profile_id = Some(profile_id);
            model.connecting_profile_id = Some(profile_id);
            let attempt = model.connect_attempt_id;

            assert_eq!(
                reconnect_with_new_settings(&mut model),
                restarts,
                "{state:?}"
            );
            let expected_attempt = if restarts {
                attempt.wrapping_add(1)
            } else {
                attempt
            };
            assert_eq!(model.connect_attempt_id, expected_attempt, "{state:?}");
        }
    }

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
        assert!(model.status_is_error());
        assert!(model.status_text().contains("Connection failed: timeout"));
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
    fn connect_retry_is_reported_with_its_reason_for_the_current_attempt_only() {
        let mut model = Model::test_new(crate::config::profile::Config::default());
        model.connection = ConnectionState::ConnectPending;
        model.connect_attempt_id = 3;
        let retrying = |attempt_id, reason| Msg::ConnectRetrying {
            attempt_id,
            reason,
            limit: std::time::Duration::from_secs(90),
        };

        assert!(update(&mut model, retrying(2, ConnectRetryReason::TunNotPermitted)).is_empty());
        assert!(model.status.is_none());

        for (reason, message) in [
            (
                ConnectRetryReason::BinaryBeingWritten,
                "The sing-box binary is being written; retrying for up to 90 seconds",
            ),
            (
                ConnectRetryReason::TunNotPermitted,
                "sing-box is not allowed to create the TUN interface yet; retrying for up to 90 seconds",
            ),
        ] {
            let effects = update(&mut model, retrying(3, reason));
            assert_eq!(model.connection, ConnectionState::ConnectPending);
            assert_eq!(model.status_text(), message);
            assert_eq!(effects, vec![Effect::BroadcastState, app_log_info(message)]);
        }
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
            model.status_text(),
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
            model.status_text(),
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
                routing_fallback: None,
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
                routing_fallback: None,
            },
        );
        assert_eq!(model.connection, ConnectionState::Connected);
        assert_eq!(model.overlay, Overlay::None);
        assert_eq!(model.active_profile_id, Some(id));
        assert_eq!(
            effects,
            vec![
                Effect::WriteState,
                app_log_info("Connected"),
                Effect::PersistConfirmedState
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
                routing_fallback: None,
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
        model.select_next();
        let effects = update(
            &mut model,
            Msg::Connected {
                pid: 1,
                profile_id: a_id,
                attempt_id: 0,
                routing_fallback: None,
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
        assert!(effects.contains(&app_log_info("VPN resumed — reconnecting…")));
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
    fn fallback_connection_warns_about_dns_as_global_routing_runs_it() {
        use crate::config::profile::{CustomDnsPreset, DnsServer, ProtocolConfig};

        let profile = Profile {
            config: ProtocolConfig::Http(Default::default()),
            ..Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into())
        };
        let profile_id = profile.id;
        let dns_warned = |routing_fallback: Option<RoutingFallback>| {
            let mut model = model_with_profiles(vec![profile.clone()]);
            model
                .routing
                .add_region_modes(crate::config::profile::GeoRegion::Ru);
            let settings = &mut model.config.settings;
            settings.dns.custom_presets = vec![CustomDnsPreset {
                name: "home".to_string(),
                servers: vec![DnsServer::Udp {
                    tag: "plain".to_string(),
                    server: "8.8.8.8".to_string(),
                    server_port: None,
                }],
                rules: Vec::new(),
                final_server: "plain".to_string(),
            }];
            settings.dns.current_preset = "home".to_string();
            settings.geo_routing.set_mode("only_ru");
            model.connection = ConnectionState::ConnectPending;
            update(
                &mut model,
                Msg::Connected {
                    pid: 1,
                    profile_id,
                    attempt_id: 0,
                    routing_fallback,
                },
            )
            .iter()
            .any(|effect| {
                matches!(effect, Effect::AppendAppLog { message, .. } if message.contains("\"plain\" (UDP)"))
            })
        };
        assert!(dns_warned(Some(RoutingFallback::Global)));
        assert!(!dns_warned(Some(RoutingFallback::Direct)));
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
                routing_fallback: None,
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
                Effect::PersistConfirmedState
            ]
        );
    }

    #[test]
    fn connected_warns_when_the_ech_config_is_looked_up_over_plain_dns() {
        use crate::config::profile::{EchSettings, ProtocolConfig, TlsCommon, TrojanConfig};
        let mut profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        profile.config = ProtocolConfig::Trojan(TrojanConfig {
            password: "pw".into(),
            tls: TlsCommon {
                server_name: Some("ech.example".into()),
                ech: Some(EchSettings {
                    enabled: true,
                    config: Vec::new(),
                }),
                ..TlsCommon::default()
            },
            ..TrojanConfig::default()
        });
        let mut model = model_with_profiles(vec![profile.clone()]);
        model.config.settings.dns.current_preset = "system_local".into();
        let warning =
            crate::singbox::config::ech_dns_warning(&profile, &model.config.settings).unwrap();

        on_connected(&mut model, 1, profile.id, 0, None);

        assert_eq!(model.status, Some(AppStatus::Error(warning)));
    }

    #[test]
    fn connected_warns_when_the_certificate_pin_is_not_checked() {
        let mut profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        profile
            .share_link_params
            .insert("pinSHA256".into(), "AB:CD".into());
        let mut model = model_with_profiles(vec![profile.clone()]);
        let warning = crate::singbox::config::certificate_pin_warning(&profile).unwrap();

        let effects = on_connected(&mut model, 1, profile.id, 0, None);

        assert_eq!(model.status, Some(AppStatus::Error(warning.clone())));
        assert!(effects.contains(&app_log_error(&warning)));
    }

    #[test]
    fn connect_warnings_do_not_replace_the_connected_status_event() {
        let mut profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        profile
            .share_link_params
            .insert("pinSHA256".into(), "AB:CD".into());
        let mut model = model_with_profiles(vec![profile.clone()]);

        on_connected(&mut model, 1, profile.id, 0, None);

        let keys: Vec<_> = model.status_events.iter().map(|event| event.key).collect();
        assert_eq!(
            keys,
            vec![StatusKey::Connection, StatusKey::ConnectionWarning(0)]
        );
    }

    #[test]
    fn connected_warns_when_dns_bypasses_the_tunnel() {
        use crate::config::profile::{CustomDnsPreset, DnsServer, ProtocolConfig};
        let mut profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        profile.config = ProtocolConfig::Http(Default::default());
        let mut model = model_with_profiles(vec![profile.clone()]);
        let dns = &mut model.config.settings.dns;
        dns.custom_presets = vec![CustomDnsPreset {
            name: "plain".into(),
            servers: vec![DnsServer::Udp {
                tag: "plain".into(),
                server: "8.8.8.8".into(),
                server_port: None,
            }],
            rules: vec![],
            final_server: "plain".into(),
        }];
        dns.current_preset = "plain".into();
        let warning = crate::singbox::config::dns_bypass_warning(
            &profile,
            &model.config.settings,
            &model.routing,
        )
        .unwrap();
        let effects = on_connected(&mut model, 1, profile.id, 0, None);
        assert_eq!(model.connection, ConnectionState::Connected);
        assert_eq!(model.status, Some(AppStatus::Error(warning.clone())));
        assert!(effects.contains(&app_log_error(&warning)));
    }

    #[test]
    fn auto_connect_polkit_failure_keeps_it_disabled_and_logs_error() {
        let mut model = model_with_profiles(vec![]);
        handle_sources(&mut model, key('A'));

        let effects = update(
            &mut model,
            Msg::AutoConnectPolkitChecked {
                error: Some(crate::app::msg::IpcError::new(
                    "reboot to activate the `kvn-tui` group",
                )),
            },
        );

        let message = "Auto-connect failed: reboot to activate the `kvn-tui` group";
        assert!(!model.config.settings.auto_connect);
        assert!(!model.auto_connect_pending);
        assert_eq!(model.status, Some(AppStatus::Error(message.into())));
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

        handle_sources(&mut model, key('A'));
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
        assert!(model.status_text().contains("enabled"));
        assert_eq!(
            effects,
            vec![
                app_log_info("Kill switch enabled"),
                Effect::PersistConfirmedState,
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
        assert!(model.status_text().contains("helper missing"));
        assert!(!effects.contains(&Effect::PersistConfirmedState));
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

        handle_sources(&mut model, key('K'));
        let effects = crate::app::update::key::handle_key(&mut model, key('y'));

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
                routing_fallback: None,
            },
        );
        assert_eq!(model.traffic, TrafficStats::default());
        assert_eq!(model.last_traffic_sample_at_ms, 0);
        assert!(model.last_traffic_fetch_at.is_none());
        assert_eq!(model.traffic_request_id, 0);
        assert_eq!(model.last_traffic_response_id, 0);
    }
}

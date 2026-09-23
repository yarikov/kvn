mod config_io;
mod connection;
mod effect;
mod geo;
mod process_slot;
mod profile_test;
mod subscription;
mod traffic;

use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result};

use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model};
use crate::app::msg::{IpcCommand, LogSessionOffsets, Msg, StateSnapshot};
use crate::app::update::update;
use crate::ipc::{IpcServer, cleanup_socket};

use config_io::{commit_config_change, persist_config_unless_frozen};
use effect::execute_daemon_effect;
use process_slot::{ProcessSlot, lock_process_slot, spawn_ticker};

struct DaemonShared {
    process_slot: Arc<Mutex<ProcessSlot>>,
    connect_coordinator: Arc<Mutex<()>>,
    singbox_log_pruned_at: Arc<Mutex<Option<Instant>>>,
}

/// Run the daemon main loop.
pub fn run(mut model: Model) -> Result<()> {
    if let Err(error) = crate::config::recovery::maintain() {
        tracing::warn!("Failed to maintain config recovery files: {error:#}");
    }
    let (tx, rx) = channel::<Msg>();
    let ipc_server = IpcServer::bind(tx.clone())?;

    let app_log_path = crate::paths::app_log_path();
    if let Err(error) = crate::services::log_tailer::prune_log_to_lines(
        &app_log_path,
        model.config.settings.logs.line_retention.app,
    ) {
        tracing::warn!(
            "Failed to enforce log line limit for {:?}: {}",
            app_log_path,
            error
        );
    }
    let singbox_log_path = crate::paths::singbox_log_path();
    let singbox_log_pruned_at = match crate::services::log_tailer::prune_log_to_lines(
        &singbox_log_path,
        model.config.settings.logs.line_retention.singbox,
    ) {
        Ok(()) => Some(Instant::now()),
        Err(error) => {
            tracing::warn!(
                "Failed to enforce log line limit for {:?}: {}",
                singbox_log_path,
                error
            );
            None
        }
    };
    let log_session_offsets = LogSessionOffsets {
        app: log_file_len(crate::paths::app_log_path()),
        singbox: log_file_len(crate::paths::singbox_log_path()),
    };

    spawn_suspend_watcher(tx.clone());
    if let Err(e) = spawn_signal_handler(tx.clone()) {
        tracing::warn!("Failed to install signal handler: {e}");
    }

    reconcile_kill_switch_state(&mut model);
    reconcile_auto_connect_state(&mut model);

    let shared = DaemonShared {
        process_slot: Arc::new(Mutex::new(ProcessSlot {
            attempt_id: model.connect_attempt_id,
            handle: None,
        })),
        connect_coordinator: Arc::new(Mutex::new(())),
        singbox_log_pruned_at: Arc::new(Mutex::new(singbox_log_pruned_at)),
    };
    spawn_ticker(tx.clone(), Arc::downgrade(&shared.process_slot));

    let result = run_loop(
        &mut model,
        rx,
        &tx,
        &shared,
        &ipc_server,
        log_session_offsets,
    );

    // Cleanup
    let _coordinator = shared
        .connect_coordinator
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some(mut handle) = lock_process_slot(&shared.process_slot).handle.take()
        && let Err(e) = handle.kill_and_wait()
    {
        tracing::warn!("Failed to stop sing-box on exit: {}", e);
    }
    if model.config.settings.kill_switch
        && let Err(e) = crate::services::killswitch::revoke()
    {
        tracing::warn!("Failed to flush kill switch handshake set on exit: {}", e);
    }
    cleanup_socket();

    result
}

fn run_loop(
    model: &mut Model,
    rx: std::sync::mpsc::Receiver<Msg>,
    tx: &Sender<Msg>,
    shared: &DaemonShared,
    ipc_server: &IpcServer,
    log_session_offsets: LogSessionOffsets,
) -> Result<()> {
    loop {
        let msg = rx.recv()?;
        let response_to = match &msg {
            Msg::IpcRequest { request_id, .. } => Some(*request_id),
            _ => None,
        };
        let response_error = match &msg {
            Msg::IpcRequest { command, .. }
                if model.restart_required
                    && matches!(
                        command,
                        IpcCommand::ConnectProfile { .. }
                            | IpcCommand::Disconnect
                            | IpcCommand::Reconnect
                            | IpcCommand::Toggle
                    ) =>
            {
                Some("Restart the kvn daemon to finish the upgrade".into())
            }
            _ => None,
        };
        let config_before = model.config.clone();
        let support_prompt_before = model.support_prompt.clone();
        let mut effects = update(model, msg);
        if effects
            .iter()
            .any(|effect| matches!(effect, Effect::SaveConfig))
        {
            let edited = model.config.clone();
            match persist_config_unless_frozen(model, &config_before, &edited) {
                Ok(config) => {
                    model.replace_config_preserving_selection(config);
                    effects.retain(|effect| !matches!(effect, Effect::SaveConfig));
                }
                Err(error) => {
                    model.replace_config_preserving_selection(config_before);
                    model.support_prompt = support_prompt_before;
                    let message = match crate::config::save_conflict_config(&edited) {
                        Ok(path) => format!(
                            "Failed to save config: {error:#}; unsaved version preserved at {}",
                            path.display()
                        ),
                        Err(save_error) => format!(
                            "Failed to save config: {error:#}; failed to preserve unsaved version: {save_error:#}"
                        ),
                    };
                    model.set_status(AppStatus::Error(message.clone()));
                    crate::services::log_tailer::append_app_log("ERROR", &message);
                    effects.retain(|effect| {
                        matches!(effect, Effect::BroadcastState | Effect::AppendAppLog { .. })
                    });
                    if !effects
                        .iter()
                        .any(|effect| matches!(effect, Effect::BroadcastState))
                    {
                        effects.push(Effect::BroadcastState);
                    }
                }
            }
        }
        // `queue_connect` advances the generation before the next Tick emits
        // `Effect::Connect`. Publish that invalidation immediately so an old
        // worker cannot install its process during the intervening 250 ms.
        lock_process_slot(&shared.process_slot).attempt_id = model.connect_attempt_id;
        let mut should_broadcast = false;

        for effect in &effects {
            if matches!(
                effect,
                Effect::Connect { .. }
                    | Effect::Disconnect
                    | Effect::DownloadGeo
                    | Effect::RetryServiceRuleSets { .. }
                    | Effect::ResetGeoUpdateSchedules
                    | Effect::WriteState
                    | Effect::SaveConfig
                    | Effect::PersistSupportPrompt { .. }
                    | Effect::CommitEditedConfig { .. }
                    | Effect::UpdateSubscription { .. }
                    | Effect::BroadcastState
                    | Effect::ApplyKillSwitch { .. }
                    | Effect::CheckAutoConnectPolkit
            ) {
                should_broadcast = true;
            }
        }

        for effect in effects {
            execute_daemon_effect(effect, tx, model, shared)?;
        }

        if model.should_quit {
            break;
        }

        if should_broadcast {
            ipc_server.broadcast(&build_snapshot(
                model,
                log_session_offsets,
                ipc_server.tui_sessions(),
                response_to,
                response_error,
            ));
        }
    }
    Ok(())
}

/// At daemon startup, align `settings.kill_switch` with the actual systemd
/// unit state. systemd is the source of truth — if a user disabled the unit
/// manually or never installed the helper, the persisted bool would otherwise
/// drift and the TUI would render `[KS]` against an open firewall.
fn reconcile_kill_switch_state(model: &mut Model) {
    let active = match crate::services::killswitch::is_active() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("Failed to query kill switch unit state: {}", e);
            return;
        }
    };
    if model.config.settings.kill_switch != active {
        tracing::info!(
            "Reconciling kill switch state: config={}, systemd={}",
            model.config.settings.kill_switch,
            active
        );
        let base = model.config.clone();
        let mut edited = base.clone();
        edited.settings.kill_switch = active;
        match commit_config_change(model, &base, &edited) {
            Ok(config) => model.replace_config_preserving_selection(config),
            Err(e) => {
                tracing::warn!("Failed to persist reconciled kill switch state: {}", e);
            }
        }
    }
}

fn reconcile_auto_connect_state(model: &mut Model) {
    if !model.config.settings.auto_connect || !crate::doctor::polkit_authorization_denied() {
        return;
    }
    tracing::info!("Disabling auto-connect: passwordless polkit is not set up");
    let base = model.config.clone();
    let mut edited = base.clone();
    edited.settings.auto_connect = false;
    match commit_config_change(model, &base, &edited) {
        Ok(config) => model.replace_config_preserving_selection(config),
        Err(e) => {
            tracing::warn!("Failed to persist disabled auto-connect: {}", e);
            model.config.settings.auto_connect = false;
        }
    }
    cancel_startup_auto_connect(model);
}

fn cancel_startup_auto_connect(model: &mut Model) {
    if model.connection == ConnectionState::Connecting {
        model.connection = ConnectionState::Idle;
        model.connecting_profile_id = None;
    }
    model.status = AppStatus::Info(
        "Auto-connect disabled: passwordless polkit is not set up; run `sudo kvn setup --polkit`"
            .into(),
    );
}

pub(crate) fn build_snapshot(
    model: &Model,
    log_session_offsets: LogSessionOffsets,
    tui_sessions: usize,
    response_to: Option<uuid::Uuid>,
    response_error: Option<String>,
) -> StateSnapshot {
    StateSnapshot {
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        ipc_version: crate::ipc::IPC_VERSION,
        tui_sessions,
        restart_required: model.restart_required,
        response_to,
        response_error,
        connection: model.connection,
        status: model.status.text().to_string(),
        status_is_error: matches!(model.status, AppStatus::Error(_)),
        status_revision: model.status_revision,
        singbox_pid: model.singbox_pid,
        active_profile_id: model.active_profile_id.map(|id| id.to_string()),
        selected: model.selected,
        routing_selected: model.routing_selected,
        geo_region_selected: model.geo_region_selected,
        dns_selected: model.dns_selected,
        dns_preset_draft: model.dns_preset_draft,
        dns_strategy_draft: model.dns_strategy_draft.clone(),
        dns_fakeip_draft: model.dns_fakeip_draft,
        theme_selected: model.theme_selected,
        theme_draft: model.theme_draft.clone(),
        service_routing_selected: model.service_routing_selected,
        service_routing_draft: model.service_routing_draft.clone(),
        settings_menu_return: model.settings_menu_return,
        settings_menu_selected: model.settings_menu_selected,
        routing_settings_draft: model.routing_settings_draft.clone(),
        connection_settings_draft: model.connection_settings_draft,
        interface_settings_draft: model.interface_settings_draft,
        geo_updating: model.geo_updating,
        geo_last_updated: model.geo_last_updated.clone(),
        geo_last_checked_at: model.geo_last_checked_at,
        service_checked_at: model.service_checked_at.clone(),
        overlay: model.overlay,
        main_pane_focus: model.main_pane_focus,
        profiles: model.config.profiles.clone(),
        subscriptions: model.config.subscriptions.clone(),
        settings: model.config.settings.clone(),
        traffic: model.traffic.clone(),
        log_session_offsets: Some(log_session_offsets),
        profile_latencies: model
            .profile_latencies
            .iter()
            .map(|(id, ms)| (id.to_string(), *ms))
            .collect(),
        testing_profiles: model
            .testing_profiles
            .iter()
            .map(|id| id.to_string())
            .collect(),
    }
}

fn log_file_len(path: std::path::PathBuf) -> u64 {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

fn spawn_suspend_watcher(tx: Sender<Msg>) {
    thread::spawn(move || {
        crate::services::suspend::listen_blocking(tx);
    });
}

/// Translate SIGTERM/SIGINT into an `IpcCommand::Quit` message so the daemon
/// can run its normal cleanup path (kill sing-box, remove socket) instead of
/// being torn down mid-flight by the kernel. Only the first signal is acted
/// on; further signals fall through to the default disposition.
fn spawn_signal_handler(tx: Sender<Msg>) -> Result<()> {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::iterator::Signals;
    let mut signals =
        Signals::new([SIGTERM, SIGINT]).context("Failed to register SIGTERM/SIGINT handler")?;
    thread::spawn(move || {
        if let Some(sig) = signals.forever().next() {
            tracing::info!("Received signal {sig}, shutting down");
            let _ = tx.send(Msg::IpcCommand(IpcCommand::Quit));
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::cancel_startup_auto_connect;

    #[test]
    fn startup_auto_connect_is_cancelled_when_polkit_is_missing() {
        use crate::app::model::{AppStatus, ConnectionState};

        let mut model = crate::app::model::Model::test_new(Default::default());
        model.connection = ConnectionState::Connecting;
        model.connecting_profile_id = Some(uuid::Uuid::new_v4());

        cancel_startup_auto_connect(&mut model);

        assert_eq!(model.connection, ConnectionState::Idle);
        assert!(model.connecting_profile_id.is_none());
        assert!(matches!(
            &model.status,
            AppStatus::Info(text) if text.contains("sudo kvn setup --polkit")
        ));
    }
}

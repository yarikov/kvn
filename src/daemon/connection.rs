use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::app::model::{AppStatus, ConnectionState, Model, Overlay, TrafficStats};
use crate::app::msg::{IpcError, Msg};
use crate::config::profile::{Profile, Settings};

use super::DaemonShared;
use super::process_slot::{ProcessSlot, is_current_attempt, lock_process_slot};

const LOG_PRUNE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const SING_BOX_UPDATE_TIMEOUT: Duration = Duration::from_secs(300);

fn wait_for_sing_box_update(slot: &Arc<Mutex<ProcessSlot>>, attempt_id: u64) {
    let Some(binary) = crate::singbox::runner::binary_path() else {
        return;
    };
    let sing_box = [binary];
    if !crate::pacman::transaction_wrote_any(&sing_box) {
        return;
    }
    tracing::info!("Waiting for the pacman transaction updating sing-box to finish");
    let settled =
        crate::pacman::wait_until_transaction_settles(&sing_box, SING_BOX_UPDATE_TIMEOUT, || {
            is_current_attempt(slot, attempt_id)
        });
    if !settled && is_current_attempt(slot, attempt_id) {
        tracing::warn!("The sing-box update is still running; starting sing-box anyway");
    }
}

pub(super) fn connect(
    tx: &Sender<Msg>,
    model: &mut Model,
    shared: &DaemonShared,
    profile: Profile,
    settings: Settings,
    attempt_id: u64,
) {
    let previous = {
        let mut slot = lock_process_slot(&shared.process_slot);
        slot.attempt_id = attempt_id;
        slot.handle.take()
    };
    if let Some(mut handle) = previous
        && let Err(e) = handle.kill_and_wait()
    {
        tracing::warn!("Failed to stop sing-box process: {}", e);
    }
    model.connection = ConnectionState::ConnectPending;
    let tx = tx.clone();
    let slot = shared.process_slot.clone();
    let coordinator = shared.connect_coordinator.clone();
    let log_pruned_at = shared.singbox_log_pruned_at.clone();
    thread::spawn(move || {
        let _coordinator = coordinator.lock().unwrap_or_else(|p| p.into_inner());
        if !is_current_attempt(&slot, attempt_id) {
            return;
        }
        wait_for_sing_box_update(&slot, attempt_id);
        if !is_current_attempt(&slot, attempt_id) {
            return;
        }
        if !is_current_attempt(&slot, attempt_id) {
            return;
        }
        let now = Instant::now();
        let mut last_pruned = log_pruned_at.lock().unwrap_or_else(|p| p.into_inner());
        if log_prune_due(*last_pruned, now) {
            let log_path = crate::paths::singbox_log_path();
            match crate::services::log_tailer::prune_log_to_lines(
                &log_path,
                settings.logs.line_retention.singbox,
            ) {
                Ok(()) => *last_pruned = Some(now),
                Err(error) => tracing::warn!(
                    "Failed to enforce log line limit for {:?}: {}",
                    log_path,
                    error
                ),
            }
        }
        drop(last_pruned);
        let cancelled = || !is_current_attempt(&slot, attempt_id);
        match crate::singbox::runner::start(&profile, &settings, &cancelled) {
            Ok(handle) => {
                let pid = handle.pid;
                let stale_handle = {
                    let mut slot = lock_process_slot(&slot);
                    if slot.attempt_id == attempt_id {
                        slot.handle = Some(handle);
                        None
                    } else {
                        Some(handle)
                    }
                };
                if let Some(mut handle) = stale_handle {
                    let _ = handle.kill_and_wait();
                    return;
                }
                let _ = tx.send(Msg::Connected {
                    pid,
                    profile_id: profile.id,
                    attempt_id,
                });
            }
            Err(_) if cancelled() => {}
            Err(e) => {
                let _ = tx.send(Msg::ConnectFailed {
                    attempt_id,
                    error: IpcError::from(e),
                });
            }
        }
    });
}

pub(super) fn disconnect(model: &mut Model, shared: &DaemonShared) {
    model.connect_attempt_id = model.connect_attempt_id.wrapping_add(1);
    {
        let mut slot = lock_process_slot(&shared.process_slot);
        slot.attempt_id = model.connect_attempt_id;
    }
    let previous = {
        let mut slot = lock_process_slot(&shared.process_slot);
        slot.handle.take()
    };
    if let Some(mut handle) = previous
        && let Err(e) = handle.kill_and_wait()
    {
        tracing::warn!("Failed to stop sing-box process: {}", e);
    }
    model.connection = ConnectionState::Idle;
    model.active_profile_id = None;
    model.connecting_profile_id = None;
    model.singbox_pid = None;
    model.traffic = TrafficStats::default();
    model.last_traffic_sample_at_ms = 0;
    model.traffic_request_id = 0;
    model.last_traffic_response_id = 0;
    model.last_traffic_fetch_at = None;
    model.set_status(AppStatus::Info("Disconnected".into()));
    model.overlay = Overlay::None;
    crate::services::waybar::write_state(model);
    if model.config.settings.kill_switch
        && let Err(e) = crate::services::killswitch::revoke()
    {
        tracing::warn!("Failed to flush kill switch handshake set: {}", e);
    }
}

pub(super) fn revoke_kill_switch_exceptions(model: &Model) {
    if model.config.settings.kill_switch
        && let Err(e) = crate::services::killswitch::revoke()
    {
        tracing::warn!(
            "Failed to flush kill switch handshake set after sing-box exit: {}",
            e
        );
    }
}

pub(super) fn apply_kill_switch(tx: &Sender<Msg>, enabled: bool) {
    let tx = tx.clone();
    thread::spawn(move || {
        let error = crate::services::killswitch::apply(enabled)
            .err()
            .map(IpcError::from);
        let _ = tx.send(Msg::KillSwitchApplied { enabled, error });
    });
}

pub(super) fn check_integration_setup(tx: &Sender<Msg>) {
    let tx = tx.clone();
    thread::spawn(move || {
        let setup = crate::doctor::integration_setup(std::path::Path::new("/"));
        let _ = tx.send(Msg::IntegrationSetupChecked(setup));
    });
}

pub(super) fn check_auto_connect_polkit(tx: &Sender<Msg>) {
    let tx = tx.clone();
    thread::spawn(move || {
        let error = crate::doctor::polkit_readiness().err().map(IpcError::from);
        let _ = tx.send(Msg::AutoConnectPolkitChecked { error });
    });
}

fn log_prune_due(last_pruned: Option<Instant>, now: Instant) -> bool {
    last_pruned.is_none_or(|last| now.duration_since(last) >= LOG_PRUNE_INTERVAL)
}

#[cfg(test)]
mod tests {
    use super::log_prune_due;
    use std::time::{Duration, Instant};

    #[test]
    fn log_prune_is_due_initially_and_after_twenty_four_hours() {
        let start = Instant::now();
        assert!(log_prune_due(None, start));
        assert!(!log_prune_due(
            Some(start),
            start + Duration::from_secs(24 * 60 * 60 - 1)
        ));
        assert!(log_prune_due(
            Some(start),
            start + Duration::from_secs(24 * 60 * 60)
        ));
    }
}

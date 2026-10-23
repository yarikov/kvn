use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use crate::app::model::{AppStatus, ConnectionState, Model, Overlay, StatusKey, TrafficStats};
use crate::app::msg::{ConnectRetryReason, IpcError, Msg};
use crate::config::profile::{Profile, Settings};
use crate::singbox::runner::NotReady;

use super::DaemonShared;
use super::process_slot::{is_current_attempt, lock_process_slot, stop_retired};
use crate::singbox::process_handle::STOP_GRACE_PERIOD;

const LOG_PRUNE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const NOT_READY_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const BINARY_WRITE_RETRY_LIMIT: Duration = Duration::from_secs(15);
const TUN_PERMISSION_RETRY_LIMIT: Duration = Duration::from_mins(2);
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(250);

pub(super) fn connect(
    tx: &Sender<Msg>,
    model: &mut Model,
    shared: &DaemonShared,
    profile: Profile,
    settings: Settings,
    attempt_id: u64,
) {
    {
        let mut slot = lock_process_slot(&shared.process_slot);
        slot.attempt_id = attempt_id;
        slot.retire_handle();
    }
    model.connection = ConnectionState::ConnectPending;
    let tx = tx.clone();
    let slot = shared.process_slot.clone();
    let coordinator = shared.connect_coordinator.clone();
    let log_pruned_at = shared.singbox_log_pruned_at.clone();
    thread::spawn(move || {
        let _coordinator = coordinator.lock().unwrap_or_else(|p| p.into_inner());
        stop_retired(&slot);
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
        match start_retrying_until_ready(&tx, &profile, &settings, attempt_id, &cancelled) {
            Ok(crate::singbox::runner::StartedSingBox {
                handle,
                routing_fallback,
            }) => {
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
                    if let Err(e) = handle.stop(STOP_GRACE_PERIOD) {
                        tracing::warn!("Failed to stop superseded sing-box process: {e:#}");
                    }
                    return;
                }
                let _ = tx.send(Msg::Connected {
                    pid,
                    profile_id: profile.id,
                    attempt_id,
                    routing_fallback,
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

fn start_retrying_until_ready(
    tx: &Sender<Msg>,
    profile: &Profile,
    settings: &Settings,
    attempt_id: u64,
    cancelled: &dyn Fn() -> bool,
) -> anyhow::Result<crate::singbox::runner::StartedSingBox> {
    let mut budget = RetryBudget::default();
    loop {
        let error = match crate::singbox::runner::start(profile, settings, cancelled) {
            Ok(started) => return Ok(started),
            Err(error) => error,
        };
        if cancelled() {
            return Err(error);
        }
        let Some(not_ready) = crate::singbox::runner::not_ready(&error) else {
            return Err(error);
        };
        tracing::info!("sing-box is not ready yet ({not_ready:?}): {error:#}");
        match budget.step(not_ready, Instant::now()) {
            RetryStep::GiveUp => return Err(give_up(error, not_ready)),
            RetryStep::FirstRetry => {
                let _ = tx.send(Msg::ConnectRetrying {
                    attempt_id,
                    reason: retry_reason(not_ready),
                    limit: retry_limit(not_ready),
                });
            }
            RetryStep::Retry => {}
        }
        if !sleep_unless_cancelled(NOT_READY_RETRY_INTERVAL, cancelled) {
            return Err(error);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryStep {
    FirstRetry,
    Retry,
    GiveUp,
}

#[derive(Default)]
struct RetryBudget {
    binary_busy_since: Option<Instant>,
    tun_denied_since: Option<Instant>,
}

impl RetryBudget {
    fn step(&mut self, reason: NotReady, now: Instant) -> RetryStep {
        let limit = retry_limit(reason);
        let since = match reason {
            NotReady::BinaryBeingWritten => &mut self.binary_busy_since,
            NotReady::TunNotPermitted => &mut self.tun_denied_since,
        };
        match *since {
            None => {
                *since = Some(now);
                RetryStep::FirstRetry
            }
            Some(first) if now.duration_since(first) >= limit => RetryStep::GiveUp,
            Some(_) => RetryStep::Retry,
        }
    }
}

fn retry_limit(not_ready: NotReady) -> Duration {
    match not_ready {
        NotReady::BinaryBeingWritten => BINARY_WRITE_RETRY_LIMIT,
        NotReady::TunNotPermitted => TUN_PERMISSION_RETRY_LIMIT,
    }
}

fn retry_reason(not_ready: NotReady) -> ConnectRetryReason {
    match not_ready {
        NotReady::BinaryBeingWritten => ConnectRetryReason::BinaryBeingWritten,
        NotReady::TunNotPermitted => ConnectRetryReason::TunNotPermitted,
    }
}

fn give_up(error: anyhow::Error, reason: NotReady) -> anyhow::Error {
    match reason {
        NotReady::BinaryBeingWritten => error,
        NotReady::TunNotPermitted => error.context(format!(
            "sing-box was still not allowed to create the TUN interface after {} seconds; \
             run `kvn doctor`",
            TUN_PERMISSION_RETRY_LIMIT.as_secs()
        )),
    }
}

fn sleep_unless_cancelled(duration: Duration, cancelled: &dyn Fn() -> bool) -> bool {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if cancelled() {
            return false;
        }
        thread::sleep(CANCEL_POLL_INTERVAL);
    }
    true
}

pub(super) fn disconnect(model: &mut Model, shared: &DaemonShared) {
    model.connect_attempt_id = model.connect_attempt_id.wrapping_add(1);
    let retired = {
        let mut slot = lock_process_slot(&shared.process_slot);
        slot.attempt_id = model.connect_attempt_id;
        slot.retire_handle()
    };
    if retired {
        let slot = shared.process_slot.clone();
        let coordinator = shared.connect_coordinator.clone();
        thread::spawn(move || {
            let _coordinator = coordinator.lock().unwrap_or_else(|p| p.into_inner());
            stop_retired(&slot);
        });
    }
    model.connection = ConnectionState::Idle;
    model.active_profile_id = None;
    model.connecting_profile_id = None;
    model.singbox_pid = None;
    model.routing_fallback = None;
    model.traffic = TrafficStats::default();
    model.last_traffic_sample_at_ms = 0;
    model.traffic_request_id = 0;
    model.last_traffic_response_id = 0;
    model.last_traffic_fetch_at = None;
    model.set_status(
        StatusKey::Connection,
        AppStatus::Info("Disconnected".into()),
    );
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
    use super::{
        BINARY_WRITE_RETRY_LIMIT, NotReady, RetryBudget, RetryStep, TUN_PERMISSION_RETRY_LIMIT,
        log_prune_due,
    };
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

    #[test]
    fn retry_budget_counts_each_reason_from_its_own_first_failure() {
        let start = Instant::now();
        let mut budget = RetryBudget::default();

        assert_eq!(
            budget.step(NotReady::TunNotPermitted, start),
            RetryStep::FirstRetry
        );
        let later = start + Duration::from_secs(20);
        assert_eq!(
            budget.step(NotReady::BinaryBeingWritten, later),
            RetryStep::FirstRetry
        );
        assert_eq!(
            budget.step(
                NotReady::BinaryBeingWritten,
                later + Duration::from_secs(10)
            ),
            RetryStep::Retry
        );
        assert_eq!(
            budget.step(
                NotReady::BinaryBeingWritten,
                later + BINARY_WRITE_RETRY_LIMIT
            ),
            RetryStep::GiveUp
        );
        assert_eq!(
            budget.step(NotReady::TunNotPermitted, later),
            RetryStep::Retry
        );
        assert_eq!(
            budget.step(
                NotReady::TunNotPermitted,
                start + TUN_PERMISSION_RETRY_LIMIT
            ),
            RetryStep::GiveUp
        );
    }
}

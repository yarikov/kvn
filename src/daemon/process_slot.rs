use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::Duration;

use crate::app::msg::Msg;
use crate::singbox::process_handle::{ProcessHandle, STOP_GRACE_PERIOD};

pub(super) struct ProcessSlot {
    pub(super) attempt_id: u64,
    pub(super) handle: Option<ProcessHandle>,
    pub(super) retired: Vec<ProcessHandle>,
}

impl ProcessSlot {
    pub(super) fn new(attempt_id: u64) -> Self {
        Self {
            attempt_id,
            handle: None,
            retired: Vec::new(),
        }
    }

    pub(super) fn retire_handle(&mut self) -> bool {
        let Some(handle) = self.handle.take() else {
            return false;
        };
        self.retired.push(handle);
        true
    }
}

pub(super) fn stop_retired(process_slot: &Arc<Mutex<ProcessSlot>>) {
    let retired = std::mem::take(&mut lock_process_slot(process_slot).retired);
    for mut handle in retired {
        if let Err(e) = handle.stop(STOP_GRACE_PERIOD) {
            tracing::warn!("Failed to stop sing-box process {}: {e:#}", handle.pid);
        }
    }
}

pub(super) fn spawn_ticker(tx: Sender<Msg>, process_slot: Weak<Mutex<ProcessSlot>>) {
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(250));
            let Some(process_slot) = process_slot.upgrade() else {
                break;
            };
            if let Some(msg) = poll_process_exit(&process_slot)
                && tx.send(msg).is_err()
            {
                break;
            }
            if tx.send(Msg::Tick).is_err() {
                break;
            }
        }
    });
}

pub(super) fn poll_process_exit(process_slot: &Arc<Mutex<ProcessSlot>>) -> Option<Msg> {
    use std::os::unix::process::ExitStatusExt;

    let mut slot = lock_process_slot(process_slot);
    let status = match slot.handle.as_mut()?.try_wait() {
        Ok(Some(status)) => status,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!("Failed to monitor sing-box process: {e:#}");
            return None;
        }
    };
    let attempt_id = slot.attempt_id;
    slot.handle.take();
    Some(Msg::SingBoxExited {
        attempt_id,
        code: status.code(),
        signal: status.signal(),
    })
}

pub(super) fn lock_process_slot(
    slot: &Arc<Mutex<ProcessSlot>>,
) -> std::sync::MutexGuard<'_, ProcessSlot> {
    slot.lock().unwrap_or_else(|p| p.into_inner())
}

pub(super) fn is_current_attempt(slot: &Arc<Mutex<ProcessSlot>>, attempt_id: u64) -> bool {
    lock_process_slot(slot).attempt_id == attempt_id
}

#[cfg(test)]
mod tests {
    use super::{ProcessSlot, lock_process_slot, poll_process_exit, stop_retired};
    use crate::app::msg::Msg;
    use crate::singbox::process_handle::ProcessHandle;
    use std::sync::{Arc, Mutex};

    #[test]
    fn process_poll_reports_exit_once_and_removes_handle() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let child = std::process::Command::new("sh")
            .args(["-c", "exit 7"])
            .spawn()
            .unwrap();
        let slot = Arc::new(Mutex::new(ProcessSlot {
            handle: Some(ProcessHandle::new(child, 0)),
            ..ProcessSlot::new(42)
        }));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let msg = loop {
            if let Some(msg) = poll_process_exit(&slot) {
                break msg;
            }
            assert!(std::time::Instant::now() < deadline, "child did not exit");
            std::thread::yield_now();
        };

        assert!(matches!(
            msg,
            Msg::SingBoxExited {
                attempt_id: 42,
                code: Some(7),
                signal: None,
            }
        ));
        assert!(lock_process_slot(&slot).handle.is_none());
        assert!(poll_process_exit(&slot).is_none());
    }

    #[test]
    fn process_poll_ignores_intentionally_removed_handle() {
        let slot = Arc::new(Mutex::new(ProcessSlot::new(1)));
        assert!(poll_process_exit(&slot).is_none());
    }

    #[test]
    fn retired_handle_is_stopped_and_released() {
        use std::io::{BufRead, Read};

        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let mut child = std::process::Command::new("sh")
            .args([
                "-c",
                "trap 'echo stopped; exit' TERM; echo ready; while :; do sleep 0.05; done",
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        stdout.read_line(&mut ready).unwrap();
        let slot = Arc::new(Mutex::new(ProcessSlot {
            handle: Some(ProcessHandle::new(child, 0)),
            ..ProcessSlot::new(1)
        }));

        assert!(lock_process_slot(&slot).retire_handle());
        assert!(!lock_process_slot(&slot).retire_handle());
        assert!(poll_process_exit(&slot).is_none());

        stop_retired(&slot);
        assert!(lock_process_slot(&slot).retired.is_empty());
        let mut output = String::new();
        stdout.read_to_string(&mut output).unwrap();
        assert_eq!(output, "stopped\n");
    }
}

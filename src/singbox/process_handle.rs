use anyhow::{Context, Result};
use std::process::ExitStatus;
use std::time::{Duration, Instant};

pub const STOP_GRACE_PERIOD: Duration = Duration::from_secs(12);
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub struct ProcessHandle {
    child: std::process::Child,
    pub pid: u32,
    pub clash_api_port: u16,
}

impl ProcessHandle {
    pub fn new(child: std::process::Child, clash_api_port: u16) -> Self {
        let pid = child.id();
        Self {
            child,
            pid,
            clash_api_port,
        }
    }

    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .context("Failed to check sing-box process status")
    }

    pub fn stop(&mut self, grace_period: Duration) -> Result<()> {
        if self.try_wait()?.is_some() || self.stopped_by_sigterm(grace_period)? {
            return Ok(());
        }
        self.child
            .kill()
            .context("Failed to kill sing-box process")?;
        if let Err(e) = self.child.wait() {
            tracing::warn!("Failed to wait for sing-box process: {}", e);
        }
        Ok(())
    }

    fn stopped_by_sigterm(&mut self, grace_period: Duration) -> Result<bool> {
        let terminated_at = Instant::now();
        // NOTE: the child is not reaped yet, so the kernel keeps its PID
        // reserved and the signal cannot reach a process that reused it.
        if let Err(e) = signal_process(self.pid, libc::SIGTERM) {
            tracing::warn!("{e:#}; killing it");
            return Ok(false);
        }
        if !self.wait_for_exit(grace_period)? {
            tracing::warn!(
                "sing-box did not stop within {}s of SIGTERM; killing it",
                grace_period.as_secs()
            );
            return Ok(false);
        }
        tracing::info!(
            "sing-box stopped {} ms after SIGTERM",
            terminated_at.elapsed().as_millis()
        );
        Ok(true)
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.try_wait()?.is_some() {
                return Ok(true);
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(false);
            }
            std::thread::sleep(EXIT_POLL_INTERVAL.min(deadline - now));
        }
    }
}

pub fn signal_process(pid: u32, signal: libc::c_int) -> Result<()> {
    let target = libc::pid_t::try_from(pid).context("sing-box PID exceeds pid_t")?;
    #[allow(unsafe_code)]
    let result = unsafe { libc::kill(target, signal) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
            .with_context(|| format!("Failed to send signal {signal} to sing-box PID {pid}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_handle_lifecycle() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let child = std::process::Command::new("sleep")
            .arg("10")
            .spawn()
            .unwrap();
        let pid = child.id();
        let mut handle = ProcessHandle::new(child, 41390);
        assert_eq!(handle.pid, pid);
        assert_eq!(handle.clash_api_port, 41390);
        handle.stop(STOP_GRACE_PERIOD).unwrap();
    }

    #[test]
    fn stop_terminates_with_sigterm_first() {
        use std::os::unix::process::ExitStatusExt;

        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let child = std::process::Command::new("sleep")
            .arg("10")
            .spawn()
            .unwrap();
        let mut handle = ProcessHandle::new(child, 0);
        handle.stop(STOP_GRACE_PERIOD).unwrap();
        let status = handle.child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }

    #[test]
    fn stop_kills_a_process_that_ignores_sigterm() {
        use std::os::unix::process::ExitStatusExt;

        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let mut child = std::process::Command::new("sh")
            .args([
                "-c",
                "trap '' TERM; echo ready; while :; do sleep 0.05; done",
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(child.stdout.take().unwrap()),
            &mut ready,
        )
        .unwrap();
        let mut handle = ProcessHandle::new(child, 0);
        handle.stop(Duration::from_millis(200)).unwrap();
        let status = handle.child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }

    #[test]
    fn signal_process_reports_a_rejected_signal() {
        let error = signal_process(std::process::id(), -1).unwrap_err();
        assert!(
            format!("{error:#}").contains("Failed to send signal -1"),
            "{error:#}"
        );
    }

    #[test]
    fn try_wait_reports_natural_exit() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let child = std::process::Command::new("sh")
            .args(["-c", "exit 7"])
            .spawn()
            .unwrap();
        let mut handle = ProcessHandle::new(child, 0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let status = loop {
            if let Some(status) = handle.try_wait().unwrap() {
                break status;
            }
            assert!(std::time::Instant::now() < deadline, "child did not exit");
            std::thread::yield_now();
        };
        assert_eq!(status.code(), Some(7));
    }
}

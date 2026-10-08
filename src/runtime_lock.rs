use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

const RETRY_INTERVAL: Duration = Duration::from_millis(25);

pub(crate) struct RuntimeLock {
    file: File,
}

impl RuntimeLock {
    pub(crate) fn try_acquire_within(path: &Path, timeout: Duration) -> Result<Option<Self>> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("Failed to open lock {}", path.display()))?;
        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Some(Self { file })),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(RETRY_INTERVAL);
                }
                Err(TryLockError::WouldBlock) => return Ok(None),
                Err(TryLockError::Error(error)) => {
                    return Err(error)
                        .with_context(|| format!("Failed to lock {}", path.display()));
                }
            }
        }
    }
}

impl Drop for RuntimeLock {
    fn drop(&mut self) {
        // close() alone can leave the lock held by a concurrently forked child
        // until exec closes its inherited CLOEXEC descriptor. Release it
        // explicitly when the holder finishes, regardless of those copies.
        if let Err(error) = self.file.unlock() {
            tracing::warn!(%error, "Failed to release runtime lock");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_lock_is_refused_until_released() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.lock");

        let held = RuntimeLock::try_acquire_within(&path, Duration::ZERO)
            .unwrap()
            .unwrap();
        assert!(
            RuntimeLock::try_acquire_within(&path, Duration::ZERO)
                .unwrap()
                .is_none()
        );

        drop(held);
        assert!(
            RuntimeLock::try_acquire_within(&path, Duration::ZERO)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_waiting_acquirer_gets_the_lock_once_the_holder_releases_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.lock");

        let held = RuntimeLock::try_acquire_within(&path, Duration::ZERO)
            .unwrap()
            .unwrap();
        let releasing = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            drop(held);
        });

        assert!(
            RuntimeLock::try_acquire_within(&path, Duration::from_secs(5))
                .unwrap()
                .is_some()
        );
        releasing.join().unwrap();
    }
}

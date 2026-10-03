use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const LOCK_PATH: &str = "/var/lib/pacman/db.lck";
const POLL_INTERVAL: Duration = Duration::from_secs(1);

pub fn transaction_wrote_any(paths: &[impl AsRef<Path>]) -> bool {
    let transaction_started = fs::metadata(LOCK_PATH)
        .and_then(|lock| lock.modified())
        .ok();
    let files_written = paths.iter().map(|path| status_changed_at(path.as_ref()));
    wrote_since(transaction_started, files_written)
}

pub fn wait_until_transaction_settles(
    paths: &[impl AsRef<Path>],
    timeout: Duration,
    mut still_wanted: impl FnMut() -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    while transaction_wrote_any(paths) {
        if Instant::now() >= deadline || !still_wanted() {
            return false;
        }
        thread::sleep(POLL_INTERVAL);
    }
    true
}

fn status_changed_at(path: &Path) -> Option<SystemTime> {
    let metadata = fs::metadata(path).ok()?;
    let seconds = u64::try_from(metadata.ctime()).ok()?;
    let nanoseconds = u32::try_from(metadata.ctime_nsec()).ok()?;
    UNIX_EPOCH.checked_add(Duration::new(seconds, nanoseconds))
}

fn wrote_since(
    transaction_started: Option<SystemTime>,
    files_written: impl IntoIterator<Item = Option<SystemTime>>,
) -> bool {
    transaction_started.is_some_and(|started| {
        files_written
            .into_iter()
            .flatten()
            .any(|written| written >= started)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_files_written_since_the_transaction_started_count() {
        let started = UNIX_EPOCH + Duration::from_secs(1_000);
        let before = Some(started - Duration::from_secs(1));
        let after = Some(started + Duration::from_secs(1));

        assert!(!wrote_since(None, [after]));
        assert!(!wrote_since(Some(started), [before, None]));
        assert!(wrote_since(Some(started), [before, after]));
        assert!(wrote_since(Some(started), [Some(started)]));
    }

    #[test]
    fn status_change_time_is_read_from_the_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(status_changed_at(file.path()).is_some());
        assert!(status_changed_at(&file.path().join("missing")).is_none());
    }
}

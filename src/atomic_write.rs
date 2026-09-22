use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result};

/// Write `data` to `dest` atomically and durably.
///
/// Writes to `<dest>.tmp`, fsyncs the data to disk, renames over `dest`, then
/// fsyncs the parent directory so the rename itself survives a crash. Without
/// the parent-dir fsync, on ext4 with the default `data=ordered` the rename
/// metadata can be lost after a power cut even though the file contents are
/// persisted — leaving the user with an empty or stale `dest`.
pub fn write(dest: &Path, data: &[u8]) -> Result<()> {
    write_inner(dest, data, None)
}

/// Atomically write `data` only while the destination still has the bytes the
/// caller previously read. The comparison happens after the temporary file is
/// durable and immediately before rename.
pub fn write_if_unchanged(dest: &Path, data: &[u8], expected: Option<&[u8]>) -> Result<()> {
    write_inner(dest, data, Some(expected))
}

fn write_inner(dest: &Path, data: &[u8], expected: Option<Option<&[u8]>>) -> Result<()> {
    let dir = dest
        .parent()
        .with_context(|| format!("Atomic write: dest {:?} has no parent", dest))?;
    let name = dest
        .file_name()
        .with_context(|| format!("Atomic write: dest {:?} has no file name", dest))?;
    let mut temp = tempfile::Builder::new()
        .prefix(&format!(".{}.", name.to_string_lossy()))
        .suffix(".tmp")
        .tempfile_in(dir)
        .with_context(|| format!("Failed to create temp file next to {:?}", dest))?;

    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .with_context(|| format!("Failed to chmod temp file {:?}", temp.path()))?;
    temp.write_all(data)
        .with_context(|| format!("Failed to write temp file {:?}", temp.path()))?;
    temp.as_file()
        .sync_all()
        .with_context(|| format!("Failed to fsync temp file {:?}", temp.path()))?;

    if let Some(expected) = expected {
        let actual = match fs::read(dest) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).context("Failed to verify destination revision");
            }
        };
        if actual.as_deref() != expected {
            anyhow::bail!("destination changed since it was read");
        }
    }

    temp.persist(dest)
        .map_err(|error| error.error)
        .with_context(|| format!("Failed to publish temp file as {:?}", dest))?;

    // Persist the rename itself. Best-effort: some filesystems (tmpfs, certain
    // FUSE mounts) return errors here even though the rename is safe in
    // practice — we don't want to fail the whole save for that.
    match fs::File::open(dir) {
        Ok(handle) => {
            if let Err(e) = handle.sync_all() {
                tracing::warn!("fsync of parent dir {:?} failed: {}", dir, e);
            }
        }
        Err(e) => {
            tracing::warn!("open of parent dir {:?} for fsync failed: {}", dir, e);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_persists_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.json");
        write(&path, b"hello world").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello world");
    }

    #[test]
    fn atomic_write_removes_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.json");
        write(&path, b"payload").unwrap();
        assert_eq!(entry_names(dir.path()), vec!["file.json".to_string()]);
    }

    fn entry_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn concurrent_writers_never_publish_each_others_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        write(&path, b"seed").unwrap();
        let payloads: Vec<Vec<u8>> = (0..8)
            .map(|writer| vec![b'a' + writer as u8; 4096])
            .collect();

        std::thread::scope(|scope| {
            for payload in &payloads {
                let path = path.clone();
                scope.spawn(move || {
                    for _ in 0..20 {
                        write(&path, payload).unwrap();
                    }
                });
            }
        });

        let published = fs::read(&path).unwrap();
        assert!(
            payloads.contains(&published),
            "destination holds bytes no writer wrote"
        );
        assert_eq!(entry_names(dir.path()), vec!["profiles.json".to_string()]);
    }

    #[test]
    fn atomic_write_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.json");
        fs::write(&path, b"old").unwrap();
        write(&path, b"new").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    }

    #[test]
    fn atomic_write_removes_temp_file_when_rename_fails() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("destination");
        fs::create_dir(&destination).unwrap();

        assert!(write(&destination, b"payload").is_err());
        assert_eq!(entry_names(dir.path()), vec!["destination".to_string()]);
    }

    #[test]
    fn atomic_write_fails_when_parent_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing/file.json");
        assert!(write(&path, b"data").is_err());
    }

    #[test]
    fn atomic_write_sets_0600_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        write(&path, b"secret").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {:o}", mode);
    }

    #[test]
    fn atomic_write_tightens_permissions_on_existing_loose_file() {
        // Upgrade path: a pre-existing file written by an older build with
        // umask-derived 0644 must end up at 0600 after the next save.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write(&path, b"new").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {:o}", mode);
    }

    #[test]
    fn conditional_write_rejects_changed_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        fs::write(&path, b"newer").unwrap();
        assert!(write_if_unchanged(&path, b"ours", Some(b"older")).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"newer");
        assert!(!dir.path().join("profiles.json.tmp").exists());
    }

    #[test]
    fn conditional_write_accepts_matching_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        fs::write(&path, b"old").unwrap();
        write_if_unchanged(&path, b"new", Some(b"old")).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
    }
}

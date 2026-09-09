//! Ordered, resumable migrations shipped by the installed package.
//!
//! This follows Omarchy's proven model: migration scripts are immutable
//! package payloads, the successful transaction gets per-user marker files,
//! and the first failure stops the chain so a later invocation resumes from
//! its private journal and candidate.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, ensure};

const INSTALLED_DIR: &str = "/usr/lib/kvn-tui/migrations";
const BASELINE_PATH: &str = "/var/lib/kvn-tui/migration-baseline";
const SESSION_STATE_NAME: &str = "migration-session.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    pub id: String,
    pub path: PathBuf,
    pub summary: String,
}

#[derive(Debug, Clone)]
struct Store {
    migrations_dir: PathBuf,
    baseline_path: PathBuf,
    state_dir: PathBuf,
    enforce_package_ownership: bool,
}

impl Store {
    fn installed() -> Result<Self> {
        let state = dirs::state_dir()
            .context("failed to determine XDG state directory")?
            .join("kvn-tui/migrations");
        Ok(Self {
            migrations_dir: PathBuf::from(INSTALLED_DIR),
            baseline_path: PathBuf::from(BASELINE_PATH),
            state_dir: state,
            enforce_package_ownership: true,
        })
    }

    #[cfg(test)]
    fn fixture(root: &Path) -> Self {
        Self {
            migrations_dir: root.join("installed"),
            baseline_path: root.join("baseline"),
            state_dir: root.join("state"),
            enforce_package_ownership: false,
        }
    }

    fn pending(&self) -> Result<Vec<Migration>> {
        let migrations = self.discover()?;
        let baseline = if self.enforce_package_ownership {
            read_packaged_marker_set(&self.baseline_path)?
        } else {
            read_marker_set(&self.baseline_path)?
        };
        Ok(migrations
            .into_iter()
            .filter(|migration| {
                !baseline.contains(&migration.id) && !self.state_dir.join(&migration.id).is_file()
            })
            .collect())
    }

    fn discover(&self) -> Result<Vec<Migration>> {
        let dir_meta = match fs::symlink_metadata(&self.migrations_dir) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).context("failed to inspect migration directory"),
        };
        ensure!(
            dir_meta.file_type().is_dir(),
            "migration path is not a directory"
        );
        if self.enforce_package_ownership {
            validate_packaged_path(&self.migrations_dir, &dir_meta)?;
        }

        let mut migrations = Vec::new();
        for entry in
            fs::read_dir(&self.migrations_dir).context("failed to read installed migrations")?
        {
            let entry = entry.context("failed to read migration entry")?;
            let path = entry.path();
            let Some(id) = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            if !id.ends_with(".sh") {
                continue;
            }
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("failed to inspect migration {id}"))?;
            ensure!(
                metadata.file_type().is_file(),
                "migration {id} is not a regular file"
            );
            if self.enforce_package_ownership {
                validate_packaged_path(&path, &metadata)?;
            }
            ensure!(
                metadata.permissions().mode() & 0o111 != 0,
                "migration {id} is not executable"
            );
            let contents = fs::read_to_string(&path)
                .with_context(|| format!("failed to read migration {id}"))?;
            migrations.push(Migration {
                id: id.clone(),
                path,
                summary: migration_summary(&contents)
                    .unwrap_or_else(|| id.trim_end_matches(".sh").to_string()),
            });
        }
        migrations.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(migrations)
    }

    fn mark_applied(&self, migration: &Migration) -> Result<()> {
        fs::create_dir_all(&self.state_dir)
            .context("failed to create migration state directory")?;
        crate::atomic_write::write(&self.state_dir.join(&migration.id), b"applied\n")
            .with_context(|| format!("failed to mark migration {} as applied", migration.id))
    }
}

fn validate_packaged_path(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    ensure!(
        metadata.uid() == 0,
        "{} is not owned by root",
        path.display()
    );
    ensure!(
        metadata.permissions().mode() & 0o022 == 0,
        "{} is writable by group or other users",
        path.display()
    );
    Ok(())
}

fn migration_summary(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        line.strip_prefix("# kvn:summary=")
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
            .map(str::to_string)
    })
}

fn read_marker_set(path: &Path) -> Result<HashSet<String>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(HashSet::new()),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn read_packaged_marker_set(path: &Path) -> Result<HashSet<String>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };
    ensure!(
        metadata.file_type().is_file(),
        "{} is not a regular file",
        path.display()
    );
    validate_packaged_path(path, &metadata)?;
    read_marker_set(path)
}

pub fn pending() -> Result<Vec<Migration>> {
    Store::installed()?.pending()
}

/// Run every pending migration. Returns true when at least one script ran.
pub fn run_pending_interactive() -> Result<bool> {
    let store = Store::installed()?;
    let pending = store.pending()?;
    let profile_migration_required = profile_migration_required()?;
    let session_pending = load_session()?.is_some();
    if pending.is_empty() && !pacman_is_running() && !profile_migration_required && !session_pending
    {
        return Ok(false);
    }
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "kvn migrations or a package transaction require an interactive terminal; run `kvn migrate` there"
    );
    run_interactive(&store)
}

fn run_interactive(store: &Store) -> Result<bool> {
    let _lock = MigrationLock::acquire()?;
    wait_for_pacman()?;
    let pending = store.pending()?;
    let profile_migration_required = profile_migration_required()?;
    let existing = load_session()?;
    if pending.is_empty() && !profile_migration_required && existing.is_none() {
        return Ok(false);
    }
    run_transaction(store, pending, existing, prompt_retry)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum SessionPhase {
    Initializing,
    Running,
    Failed,
    Finalizing,
    Swapped,
    StartingDaemon,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct MigrationRecord {
    id: String,
    summary: String,
    digest: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct MigrationSession {
    id: String,
    phase: SessionPhase,
    manifest: Vec<MigrationRecord>,
    completed: Vec<String>,
    backup_path: Option<PathBuf>,
    candidate_path: Option<PathBuf>,
    #[serde(default)]
    connection_captured: bool,
    reconnect_profile: Option<uuid::Uuid>,
    summary: String,
    error: Option<String>,
}

impl MigrationSession {
    fn ui_status(&self) -> crate::app::model::MigrationStatus {
        use crate::app::model::MigrationPhase;
        crate::app::model::MigrationStatus {
            session_id: self.id.clone(),
            phase: match self.phase {
                SessionPhase::Failed => MigrationPhase::Failed,
                SessionPhase::Finalizing | SessionPhase::Swapped | SessionPhase::StartingDaemon => {
                    MigrationPhase::Finalizing
                }
                SessionPhase::Initializing | SessionPhase::Running => MigrationPhase::Running,
            },
            completed: self.completed.len(),
            total: self.manifest.len(),
            summary: self.summary.clone(),
            error: self.error.clone(),
        }
    }
}

pub(crate) fn load_ui_status() -> Result<Option<crate::app::model::MigrationStatus>> {
    Ok(load_session()?.map(|session| session.ui_status()))
}

fn session_state_path() -> Result<PathBuf> {
    Ok(dirs::state_dir()
        .context("failed to determine XDG state directory")?
        .join("kvn-tui")
        .join(SESSION_STATE_NAME))
}

fn save_session(session: &MigrationSession) -> Result<()> {
    let path = session_state_path()?;
    fs::create_dir_all(
        path.parent()
            .context("migration session path has no parent")?,
    )?;
    crate::atomic_write::write(&path, &serde_json::to_vec_pretty(session)?)
        .with_context(|| format!("failed to save migration session at {}", path.display()))
}

fn load_session() -> Result<Option<MigrationSession>> {
    let path = session_state_path()?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("failed to inspect migration session"),
    };
    ensure!(
        metadata.file_type().is_file(),
        "migration session is not a regular file"
    );
    #[allow(unsafe_code)]
    let uid = unsafe { libc::getuid() };
    ensure!(
        metadata.uid() == uid,
        "migration session is not owned by the current user"
    );
    ensure!(
        metadata.permissions().mode() & 0o077 == 0,
        "migration session exposes private paths to other users"
    );
    Ok(Some(
        serde_json::from_slice(&fs::read(&path)?).context("failed to parse migration session")?,
    ))
}

fn clear_session() -> Result<()> {
    let path = session_state_path()?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("failed to remove migration session"),
    }
}

fn migration_digest(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    let hash = bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    Ok(format!("{hash:016x}"))
}

fn manifest(migrations: &[Migration]) -> Result<Vec<MigrationRecord>> {
    migrations
        .iter()
        .map(|migration| {
            Ok(MigrationRecord {
                id: migration.id.clone(),
                summary: migration.summary.clone(),
                digest: migration_digest(&migration.path)?,
            })
        })
        .collect()
}

fn run_transaction<F>(
    store: &Store,
    pending: Vec<Migration>,
    existing: Option<MigrationSession>,
    mut retry: F,
) -> Result<bool>
where
    F: FnMut() -> Result<bool>,
{
    let expected_manifest = manifest(&pending)?;
    let mut session = existing.unwrap_or_else(|| MigrationSession {
        id: uuid::Uuid::new_v4().to_string(),
        phase: SessionPhase::Initializing,
        manifest: expected_manifest.clone(),
        completed: Vec::new(),
        backup_path: None,
        candidate_path: None,
        connection_captured: false,
        reconnect_profile: None,
        summary: "Preparing migration workspace…".into(),
        error: None,
    });

    if !matches!(
        session.phase,
        SessionPhase::Swapped | SessionPhase::StartingDaemon
    ) && session.manifest != expected_manifest
    {
        abandon_candidate(&session)?;
        session.phase = SessionPhase::Initializing;
        session.manifest = expected_manifest;
        session.completed.clear();
        session.backup_path = None;
        session.candidate_path = None;
        session.error = None;
        session.summary = "Migration set changed; rebuilding candidate…".into();
    }
    save_session(&session)?;

    let mut daemon = attach_daemon_for_migration(&mut session)?;
    let result = (|| -> Result<bool> {
        recover_cutover_phase(&mut session)?;
        if matches!(
            session.phase,
            SessionPhase::Swapped | SessionPhase::StartingDaemon
        ) {
            return finish_daemon_handoff(store, &pending, &mut session, daemon.take());
        }

        let workspace_invalid = workspace_needs_rebuild(&session)?;
        if workspace_invalid || source_changed(&session)? {
            abandon_candidate(&session)?;
            session.phase = SessionPhase::Initializing;
            session.completed.clear();
            session.backup_path = None;
            session.candidate_path = None;
            session.error = None;
            session.summary = if workspace_invalid {
                "Migration workspace is incomplete; rebuilding candidate…".into()
            } else {
                "profiles.json changed; rebuilding migration candidate…".into()
            };
            save_session(&session)?;
            daemon = attach_daemon_for_migration(&mut session)?;
        }

        if session.backup_path.is_none() {
            create_workspace(&mut session)?;
        }
        session.phase = SessionPhase::Running;
        session.error = None;
        save_session(&session)?;
        send_migration_status(&mut daemon, &session, false)?;

        for migration in &pending {
            if session.completed.iter().any(|id| id == &migration.id) {
                continue;
            }
            loop {
                session.summary = migration.summary.clone();
                session.phase = SessionPhase::Running;
                session.error = None;
                save_session(&session)?;
                send_migration_status(&mut daemon, &session, false)?;
                println!(
                    "[{}/{}] {}",
                    session.completed.len() + 1,
                    pending.len(),
                    migration.summary
                );
                let mut command = Command::new("/usr/bin/bash");
                command
                    .args(["-euo", "pipefail"])
                    .arg(&migration.path)
                    .env("KVN_MIGRATION_SESSION_ID", &session.id)
                    .stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit());
                if let Some(candidate) = &session.candidate_path {
                    command.env("KVN_MIGRATION_PROFILES_PATH", candidate);
                }
                let status = command
                    .status()
                    .with_context(|| format!("failed to start migration {}", migration.id))?;
                if status.success() {
                    session.completed.push(migration.id.clone());
                    save_session(&session)?;
                    println!("      Done\n");
                    break;
                }
                let message = format!("Migration {} failed with {status}", migration.id);
                session.phase = SessionPhase::Failed;
                session.error = Some(message.clone());
                save_session(&session)?;
                send_migration_status(&mut daemon, &session, false)?;
                eprintln!("\n{message}.");
                if !retry()? {
                    anyhow::bail!("migration stopped; fix the problem and run `kvn migrate`");
                }
            }
        }

        if let Some(candidate) = &session.candidate_path {
            crate::config::migrate_candidate_at(candidate)?;
        }
        validate_candidate(&session)?;
        verify_source(&session)?;
        session.phase = SessionPhase::Finalizing;
        session.summary = "Switching to the migrated configuration…".into();
        save_session(&session)?;
        send_migration_status(&mut daemon, &session, false)?;
        stop_daemon_for_cutover(&mut daemon, &session)?;
        swap_candidate(&mut session)?;
        finish_daemon_handoff(store, &pending, &mut session, None)
    })();
    if let Err(error) = &result {
        record_session_failure(&mut session, &mut daemon, error);
    }
    result
}

fn record_session_failure(
    session: &mut MigrationSession,
    daemon: &mut Option<crate::ipc::IpcClient>,
    error: &anyhow::Error,
) {
    // Once the files have been exchanged, preserve the recovery phase: the
    // next invocation must finish the cutover rather than rerun scripts.
    if matches!(
        session.phase,
        SessionPhase::Swapped | SessionPhase::StartingDaemon
    ) {
        return;
    }
    if session.phase == SessionPhase::Failed && session.error.is_some() {
        let _ = save_session(session);
        let _ = send_migration_status(daemon, session, false);
        return;
    }
    session.phase = SessionPhase::Failed;
    session.summary = "Migration paused".into();
    session.error = Some(format!("{error:#}"));
    let _ = save_session(session);
    let _ = send_migration_status(daemon, session, false);
}

#[cfg(test)]
fn run_after_package_idle<W, F>(store: &Store, wait: W, retry: F) -> Result<bool>
where
    W: FnOnce() -> Result<()>,
    F: FnMut() -> Result<bool>,
{
    wait()?;
    // The package transaction may have installed additional scripts after the
    // initial probe, so never execute that stale snapshot.
    let pending = store.pending()?;
    if pending.is_empty() {
        return Ok(false);
    }
    run_queue(store, pending, retry)
}

#[cfg(test)]
fn run_queue<F>(store: &Store, pending: Vec<Migration>, mut retry: F) -> Result<bool>
where
    F: FnMut() -> Result<bool>,
{
    let total = pending.len();
    println!("kvn has {total} pending migration(s).\n");
    let mut completed = 0;
    run_queue_segment(store, &pending, total, &mut completed, &mut retry)?;
    println!("All kvn migrations completed.");
    Ok(!pending.is_empty())
}

#[cfg(test)]
fn run_queue_segment<F>(
    store: &Store,
    migrations: &[Migration],
    total: usize,
    completed: &mut usize,
    retry: &mut F,
) -> Result<()>
where
    F: FnMut() -> Result<bool>,
{
    for migration in migrations {
        loop {
            println!("[{}/{}] {}", *completed + 1, total, migration.summary);
            let status = Command::new("/usr/bin/bash")
                .args(["-euo", "pipefail"])
                .arg(&migration.path)
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()
                .with_context(|| format!("failed to start migration {}", migration.id))?;
            if status.success() {
                store.mark_applied(migration)?;
                *completed += 1;
                println!("      Done\n");
                break;
            }
            eprintln!("\nMigration {} failed with {status}.", migration.id);
            if !retry()? {
                anyhow::bail!("migration stopped; fix the problem and run `kvn migrate`");
            }
        }
    }
    Ok(())
}

fn attach_daemon_for_migration(
    session: &mut MigrationSession,
) -> Result<Option<crate::ipc::IpcClient>> {
    if !crate::ipc::is_daemon_running() {
        if !session.connection_captured {
            session.connection_captured = true;
            save_session(session)?;
        }
        return Ok(None);
    }
    let mut client = crate::ipc::IpcClient::connect()
        .context("failed to connect to the daemon before migration")?;
    client.send(&crate::app::msg::IpcCommand::Attach)?;
    let snapshot = client
        .read_snapshot_value(Duration::from_secs(2))
        .context("daemon did not provide its state before migration")?;
    ensure!(
        snapshot
            .get("migration_protocol_version")
            .and_then(serde_json::Value::as_u64)
            == Some(u64::from(crate::ipc::MIGRATION_PROTOCOL_VERSION)),
        "the running daemon predates transactional migrations; follow the upgrade guide first"
    );
    if !session.connection_captured {
        session.reconnect_profile = reconnect_profile_from_snapshot(&snapshot);
        session.connection_captured = true;
        save_session(session)?;
    }
    send_migration_status(&mut Some(client), session, true)
}

fn send_migration_status(
    client: &mut Option<crate::ipc::IpcClient>,
    session: &MigrationSession,
    begin: bool,
) -> Result<Option<crate::ipc::IpcClient>> {
    let Some(mut client) = client.take() else {
        return Ok(None);
    };
    let status = session.ui_status();
    let command = if begin {
        crate::app::msg::IpcCommand::MigrationBegin { status }
    } else {
        crate::app::msg::IpcCommand::MigrationProgress { status }
    };
    client.send(&command)?;
    let snapshot = client
        .read_snapshot(Duration::from_secs(2))
        .context("daemon did not acknowledge migration state")?;
    ensure!(
        snapshot
            .migration
            .as_ref()
            .map(|state| state.session_id.as_str())
            == Some(session.id.as_str()),
        "daemon rejected migration session"
    );
    Ok(Some(client))
}

fn create_workspace(session: &mut MigrationSession) -> Result<()> {
    let live = crate::paths::profiles_path().context("failed to determine profiles path")?;
    let contents = match fs::read(&live) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("failed to read profiles.json for migration"),
    };
    let config_dir = live.parent().context("profiles.json has no parent")?;
    let recovery_dir = config_dir.join("recovery");
    fs::create_dir_all(&recovery_dir)?;
    fs::set_permissions(&recovery_dir, fs::Permissions::from_mode(0o700))?;
    let stamp = chrono::Local::now().format("%Y%m%dT%H%M%S%6f");
    let backup = recovery_dir.join(format!(
        "profiles.json.before-migration-{stamp}-{}.json",
        session.id
    ));
    let candidate = config_dir.join(format!(".profiles.json.migrating-{}", session.id));
    ensure!(
        !backup.exists() && !candidate.exists(),
        "refusing to overwrite an existing migration artifact"
    );
    crate::atomic_write::write(&backup, &contents)?;
    crate::atomic_write::write(&candidate, &fs::read(&backup)?)?;
    session.backup_path = Some(backup.clone());
    session.candidate_path = Some(candidate);
    save_session(session)?;
    println!("Saved profiles.json backup at {}", backup.display());
    Ok(())
}

fn source_changed(session: &MigrationSession) -> Result<bool> {
    let (Some(backup), Some(_)) = (&session.backup_path, &session.candidate_path) else {
        return Ok(false);
    };
    let live = crate::paths::profiles_path().context("failed to determine profiles path")?;
    Ok(fs::read(live)? != fs::read(backup)?)
}

fn workspace_needs_rebuild(session: &MigrationSession) -> Result<bool> {
    let paths = [
        session.backup_path.as_ref(),
        session.candidate_path.as_ref(),
    ];
    if paths.iter().all(Option::is_none) {
        return Ok(false);
    }
    let [Some(backup), Some(candidate)] = paths else {
        return Ok(true);
    };
    let is_regular_file = |path: &Path| match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_file()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    };
    Ok(!is_regular_file(backup)? || !is_regular_file(candidate)?)
}

fn verify_source(session: &MigrationSession) -> Result<()> {
    ensure!(
        !source_changed(session)?,
        "profiles.json changed during migration; rerun `kvn migrate` to rebuild the candidate"
    );
    Ok(())
}

fn validate_candidate(session: &MigrationSession) -> Result<()> {
    if let Some(candidate) = &session.candidate_path {
        let config = crate::config::load_config_at_read_only(candidate)
            .context("migration candidate is invalid")?;
        config
            .validate()
            .context("migration candidate is invalid")?;
    }
    Ok(())
}

fn abandon_candidate(session: &MigrationSession) -> Result<()> {
    let Some(candidate) = &session.candidate_path else {
        return Ok(());
    };
    if !candidate.exists() {
        return Ok(());
    }
    let abandoned = candidate.with_file_name(format!(
        "profiles.json.abandoned-migration-{}-{}.json",
        chrono::Local::now().format("%Y%m%dT%H%M%S%6f"),
        session.id
    ));
    ensure!(
        !abandoned.exists(),
        "refusing to overwrite an abandoned migration candidate"
    );
    fs::rename(candidate, abandoned).context("failed to preserve abandoned migration candidate")
}

fn stop_daemon_for_cutover(
    client: &mut Option<crate::ipc::IpcClient>,
    session: &MigrationSession,
) -> Result<()> {
    let Some(mut client) = client.take() else {
        return Ok(());
    };
    client.send(&crate::app::msg::IpcCommand::MigrationStopDaemon {
        session_id: session.id.clone(),
    })?;
    drop(client);
    ensure!(
        crate::ipc::wait_for_daemon_exit(Duration::from_secs(5)),
        "daemon did not stop within 5s; config was not switched"
    );
    Ok(())
}

fn atomic_exchange(left: &Path, right: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let left = CString::new(left.as_os_str().as_bytes())?;
    let right = CString::new(right.as_os_str().as_bytes())?;
    #[allow(unsafe_code)]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    ensure!(
        result == 0,
        "atomic profiles.json exchange failed: {}",
        io::Error::last_os_error()
    );
    Ok(())
}

fn sync_parent(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }
}

fn swap_candidate(session: &mut MigrationSession) -> Result<()> {
    let Some(candidate) = &session.candidate_path else {
        session.phase = SessionPhase::StartingDaemon;
        save_session(session)?;
        return Ok(());
    };
    let live = crate::paths::profiles_path().context("failed to determine profiles path")?;
    atomic_exchange(&live, candidate)?;
    sync_parent(&live);
    session.phase = SessionPhase::Swapped;
    save_session(session)?;
    session.phase = SessionPhase::StartingDaemon;
    save_session(session)
}

fn recover_cutover_phase(session: &mut MigrationSession) -> Result<()> {
    let live = crate::paths::profiles_path().context("failed to determine profiles path")?;
    if session.phase == SessionPhase::Finalizing
        && let (Some(candidate), Some(backup)) = (&session.candidate_path, &session.backup_path)
        && candidate.exists()
        && fs::read(candidate)? == fs::read(backup)?
        && fs::read(&live)? != fs::read(backup)?
    {
        session.phase = SessionPhase::Swapped;
        save_session(session)?;
    }
    if session.phase == SessionPhase::Swapped {
        // After RENAME_EXCHANGE the candidate path temporarily contains the
        // previous live config. Keep it until the new daemon and VPN are up;
        // the durable recovery copy is the backup created before any script.
        session.phase = SessionPhase::StartingDaemon;
        save_session(session)?;
    }
    Ok(())
}

fn finish_daemon_handoff(
    store: &Store,
    pending: &[Migration],
    session: &mut MigrationSession,
    mut daemon: Option<crate::ipc::IpcClient>,
) -> Result<bool> {
    if !crate::ipc::is_daemon_running() {
        crate::start_current_daemon().context("failed to start updated daemon")?;
        ensure!(
            crate::ipc::wait_for_daemon(Duration::from_secs(5)),
            "updated daemon did not start within 5s"
        );
    }
    if daemon.is_none() {
        daemon = attach_daemon_for_migration(session)?;
    }
    for migration in pending {
        store.mark_applied(migration)?;
    }
    if let Some(client) = &mut daemon {
        client.send(&crate::app::msg::IpcCommand::MigrationEnd {
            session_id: session.id.clone(),
        })?;
        let snapshot = client
            .read_snapshot(Duration::from_secs(2))
            .context("updated daemon did not acknowledge migration completion")?;
        ensure!(
            snapshot.migration.is_none(),
            "updated daemon did not release migration mode"
        );
    }
    restore_connection(session.reconnect_profile)?;
    remove_previous_live_candidate(session)?;
    // Keep the journal until the previous connection is restored as well.
    // If reconnecting fails, `kvn migrate` can retry the handoff instead of
    // silently reporting a completed transaction with the VPN left idle.
    clear_session()?;
    println!("All kvn migrations completed.");
    Ok(true)
}

fn remove_previous_live_candidate(session: &MigrationSession) -> Result<()> {
    let Some(candidate) = &session.candidate_path else {
        return Ok(());
    };
    match fs::remove_file(candidate) {
        Ok(()) => {
            sync_parent(candidate);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("failed to remove temporary pre-migration config"),
    }
}

fn restore_connection(profile_id: Option<uuid::Uuid>) -> Result<()> {
    let Some(profile_id) = profile_id else {
        return Ok(());
    };
    let mut client = crate::ipc::IpcClient::connect()?;
    client.send(&crate::app::msg::IpcCommand::Attach)?;
    let initial = client.read_snapshot(Duration::from_secs(2))?;
    if !initial
        .profiles
        .iter()
        .any(|profile| profile.id == profile_id)
    {
        eprintln!("WARNING: previous VPN profile no longer exists; skipping reconnect.");
        return Ok(());
    }
    client.send(&crate::app::msg::IpcCommand::ConnectProfile { profile_id })?;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        ensure!(
            !remaining.is_zero(),
            "updated daemon did not restore VPN within 30s"
        );
        let snapshot = client.read_snapshot(remaining)?;
        if snapshot.connection == crate::app::model::ConnectionState::Connected
            && snapshot.active_profile_id.as_deref() == Some(profile_id.to_string().as_str())
        {
            return Ok(());
        }
        ensure!(
            !snapshot.status_is_error,
            "updated daemon could not restore VPN: {}",
            snapshot.status
        );
    }
}

pub(crate) fn reconnect_profile_from_snapshot(value: &serde_json::Value) -> Option<uuid::Uuid> {
    if value.get("connection").and_then(serde_json::Value::as_str) != Some("Connected") {
        return None;
    }
    value
        .get("active_profile_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            value
                .pointer("/settings/last_connected_profile")
                .and_then(serde_json::Value::as_str)
        })
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
}

fn prompt_retry() -> Result<bool> {
    let mut answer = String::new();
    loop {
        print!("[r] Retry  [q] Close: ");
        io::stdout().flush()?;
        answer.clear();
        io::stdin().read_line(&mut answer)?;
        match answer.trim().to_ascii_lowercase().as_str() {
            "r" | "retry" => return Ok(true),
            "q" | "quit" | "close" | "" => return Ok(false),
            _ => {}
        }
    }
}

fn wait_for_pacman() -> Result<()> {
    let lock = Path::new("/var/lib/pacman/db.lck");
    if !pacman_is_running() {
        return Ok(());
    }
    println!("Waiting for the pacman transaction to finish before running migrations...");
    for _ in 0..900 {
        if !lock.exists() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    anyhow::bail!("pacman transaction is still running; retry with `kvn migrate`")
}

fn pacman_is_running() -> bool {
    Path::new("/var/lib/pacman/db.lck").exists()
}

pub(crate) fn package_transaction_active() -> bool {
    pacman_is_running()
}

struct MigrationLock {
    _file: File,
}

impl MigrationLock {
    fn acquire() -> Result<Self> {
        let path = crate::paths::ensure_runtime_dir()?.join("migrate.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        #[allow(unsafe_code)]
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        ensure!(result == 0, "another kvn migration is already running");
        Ok(Self { _file: file })
    }
}

/// Refuse to start a new daemon while migrations are pending. Existing old
/// daemons are deliberately left alone by the package upgrade itself.
pub fn block_daemon_if_pending() -> Result<bool> {
    if pacman_is_running() {
        let message = "A package transaction is active; kvn daemon startup is deferred";
        eprintln!("{message}");
        notify_message_once("pacman-transaction", message);
        return Ok(true);
    }
    if load_session()?.is_some_and(|session| session.phase == SessionPhase::StartingDaemon) {
        return Ok(false);
    }
    let pending = match pending() {
        Ok(pending) => pending,
        Err(error) => {
            let message =
                format!("kvn migration state could not be inspected: {error:#}. Run: kvn doctor");
            tracing::error!("{message}");
            eprintln!("{message}");
            notify_message_once("state-error", &message);
            return Ok(true);
        }
    };
    let profile_migration_required = match profile_migration_required() {
        Ok(required) => required,
        Err(error) => {
            let message =
                format!("kvn profile schema could not be inspected: {error:#}. Run: kvn doctor");
            tracing::error!("{message}");
            eprintln!("{message}");
            notify_message_once("profile-schema-state-error", &message);
            return Ok(true);
        }
    };
    if pending.is_empty() && !profile_migration_required {
        return Ok(false);
    }
    let message = if profile_migration_required && pending.is_empty() {
        "A kvn profiles.json schema migration is required. Run `kvn migrate` or open the TUI to finish it"
            .to_string()
    } else {
        format!(
            "{} kvn migration(s) are pending. Open a terminal and run: kvn migrate",
            pending.len()
        )
    };
    tracing::error!("{message}");
    eprintln!("{message}");
    if pending.is_empty() {
        notify_message_once("profile-schema", &message);
    } else {
        notify_once(&pending, &message);
    }
    Ok(true)
}

fn notify_once(pending: &[Migration], message: &str) {
    let Some(last) = pending.last() else { return };
    notify_message_once(&last.id, message);
}

fn notify_message_once(id: &str, message: &str) {
    let Some(state_dir) = dirs::state_dir() else {
        return;
    };
    let marker = state_dir.join("kvn-tui/migration-notifications").join(id);
    if marker.exists() {
        return;
    }
    if matches!(
        Command::new("notify-send")
            .args(["--urgency=critical", "kvn requires migration", message])
            .status(),
        Ok(status) if status.success()
    ) {
        if let Some(parent) = marker.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = crate::atomic_write::write(&marker, b"notified\n");
    }
}

pub fn print_pending() -> Result<bool> {
    let pending = pending()?;
    let profile_migration_required = profile_migration_required()?;
    let package_transaction = package_transaction_active();
    let session = load_session()?;
    if pending.is_empty()
        && !profile_migration_required
        && !package_transaction
        && session.is_none()
    {
        println!("No pending kvn migrations.");
    }
    for migration in &pending {
        println!("{}\t{}", migration.id, migration.summary);
    }
    if profile_migration_required {
        println!("profile-schema\tMigrate profiles.json to the current schema");
    }
    if package_transaction {
        println!("pacman-transaction\tWait for the active package transaction to finish");
    }
    if let Some(session) = &session {
        println!(
            "{SESSION_STATE_NAME}\t{:?}: {} ({}/{})",
            session.phase,
            session.summary,
            session.completed.len(),
            session.manifest.len()
        );
    }
    Ok(!pending.is_empty()
        || profile_migration_required
        || package_transaction
        || session.is_some())
}

pub(crate) fn session_diagnostic() -> Result<Option<String>> {
    Ok(load_session()?.map(|session| {
        format!(
            "transactional migration {:?}: {} ({}/{})",
            session.phase,
            session.summary,
            session.completed.len(),
            session.manifest.len()
        )
    }))
}

pub fn run_command() -> Result<()> {
    run_pending_interactive().map(|_| ())
}

pub fn prepare_profile_migration(target_version: u32) -> Result<()> {
    let candidate = migration_candidate_from_environment()?
        .context("`kvn config migrate` is an internal migration step; run `kvn migrate` instead")?;
    if crate::config::migrate_candidate_to_at(&candidate, target_version)? {
        println!("Migrated candidate profiles.json.");
    } else {
        println!("Candidate profiles.json already uses the current schema.");
    }
    Ok(())
}

fn migration_candidate_from_environment() -> Result<Option<PathBuf>> {
    let Some(path) = std::env::var_os("KVN_MIGRATION_PROFILES_PATH").map(PathBuf::from) else {
        return Ok(None);
    };
    let session_id = std::env::var("KVN_MIGRATION_SESSION_ID")
        .context("KVN_MIGRATION_SESSION_ID is required for a migration candidate")?;
    let session = load_session()?.context("no active migration session")?;
    ensure!(
        session.id == session_id,
        "migration session ID does not match"
    );
    ensure!(
        session.candidate_path.as_ref() == Some(&path),
        "migration candidate path does not match the active session"
    );
    Ok(Some(path))
}

pub fn profile_migration_required() -> Result<bool> {
    let config = crate::paths::profiles_path().context("failed to determine profiles path")?;
    crate::config::schema_migration_required_at(&config)
}

pub fn run_update() -> Result<()> {
    let executable = PathBuf::from("/usr/bin/kvn");
    ensure!(
        Command::new("pacman")
            .args(["-Qq", "kvn-tui-bin"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success()),
        "`kvn update` supports the AUR package kvn-tui-bin only"
    );
    let helper = ["yay", "paru"]
        .into_iter()
        .find(|name| command_exists(name))
        .context("neither yay nor paru is installed")?;
    let status = Command::new(helper)
        .args(["-S", "--needed", "kvn-tui-bin"])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("failed to start {helper}"))?;
    ensure!(status.success(), "{helper} exited with {status}");

    let status = Command::new(&executable)
        .arg("migrate")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to start the updated migration runner")?;
    ensure!(
        status.success(),
        "updated migration runner exited with {status}"
    );
    println!("Update complete. Starting kvn...");
    use std::os::unix::process::CommandExt;
    let error = Command::new(&executable).exec();
    Err(error).context("failed to start the updated kvn")
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            dir.join(name)
                .metadata()
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(dir: &Path, id: &str, summary: &str) {
        script_body(dir, id, summary, "exit 0");
    }

    fn script_body(dir: &Path, id: &str, summary: &str, body: &str) {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(id);
        fs::write(
            &path,
            format!("#!/bin/bash\n# kvn:summary={summary}\n{body}\n"),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn pending_is_ordered_and_filters_baseline_and_markers() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        script(&store.migrations_dir, "200-second.sh", "Second");
        script(&store.migrations_dir, "100-first.sh", "First");
        script(&store.migrations_dir, "300-third.sh", "Third");
        fs::write(&store.baseline_path, "100-first.sh\n").unwrap();
        fs::create_dir_all(&store.state_dir).unwrap();
        fs::write(store.state_dir.join("300-third.sh"), "applied\n").unwrap();

        let pending = store.pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "200-second.sh");
        assert_eq!(pending[0].summary, "Second");
    }

    #[test]
    fn missing_directory_has_no_pending_migrations() {
        let root = tempfile::tempdir().unwrap();
        assert!(Store::fixture(root.path()).pending().unwrap().is_empty());
    }

    #[test]
    fn rejects_non_regular_migration() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        fs::create_dir_all(store.migrations_dir.join("100-bad.sh")).unwrap();
        assert!(
            store
                .pending()
                .unwrap_err()
                .to_string()
                .contains("not a regular file")
        );
    }

    #[test]
    fn summary_falls_back_to_filename() {
        assert_eq!(migration_summary("#!/bin/bash\nexit 0"), None);
        assert_eq!(
            migration_summary("# kvn:summary= Refresh files "),
            Some("Refresh files".into())
        );
    }

    #[test]
    fn migration_workspace_preserves_exact_source_and_uses_private_files() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let config_root = root.path().join("config");
        let state_root = root.path().join("state");
        let _config = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", &state_root);
        let profiles = crate::paths::profiles_path().unwrap();
        fs::create_dir_all(profiles.parent().unwrap()).unwrap();
        fs::write(&profiles, b"{  exact bytes  }\n").unwrap();

        let mut session = MigrationSession {
            id: uuid::Uuid::new_v4().to_string(),
            phase: SessionPhase::Initializing,
            manifest: vec![],
            completed: vec![],
            backup_path: None,
            candidate_path: None,
            connection_captured: false,
            reconnect_profile: None,
            summary: String::new(),
            error: None,
        };
        create_workspace(&mut session).unwrap();
        let backup = session.backup_path.as_ref().unwrap();
        let candidate = session.candidate_path.as_ref().unwrap();
        assert_eq!(fs::read(backup).unwrap(), b"{  exact bytes  }\n");
        assert_eq!(fs::read(candidate).unwrap(), b"{  exact bytes  }\n");
        assert_eq!(
            fs::metadata(backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(candidate).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(backup.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_ne!(backup, candidate);
    }

    #[test]
    fn mark_applied_removes_migration_from_pending() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        script(&store.migrations_dir, "100-first.sh", "First");
        let migration = store.pending().unwrap().remove(0);
        store.mark_applied(&migration).unwrap();
        assert!(store.pending().unwrap().is_empty());
    }

    #[test]
    fn successful_queue_runs_in_order_and_marks_every_script() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        let trace = root.path().join("trace");
        script_body(
            &store.migrations_dir,
            "200-second.sh",
            "Second",
            &format!("echo second >>'{}'", trace.display()),
        );
        script_body(
            &store.migrations_dir,
            "100-first.sh",
            "First",
            &format!("echo first >>'{}'", trace.display()),
        );

        let pending = store.pending().unwrap();
        assert!(run_queue(&store, pending, || Ok(false)).unwrap());
        assert_eq!(fs::read_to_string(trace).unwrap(), "first\nsecond\n");
        assert!(store.pending().unwrap().is_empty());
    }

    #[test]
    fn queue_is_rediscovered_after_package_wait() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        script(&store.migrations_dir, "100-first.sh", "First");

        assert!(
            run_after_package_idle(
                &store,
                || {
                    script(&store.migrations_dir, "200-installed-later.sh", "Later");
                    Ok(())
                },
                || Ok(false),
            )
            .unwrap()
        );
        assert!(store.pending().unwrap().is_empty());
    }

    #[test]
    fn failed_queue_stops_without_marker() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        script_body(&store.migrations_dir, "100-fail.sh", "Fail", "exit 9");

        let error = run_queue(&store, store.pending().unwrap(), || Ok(false)).unwrap_err();
        assert!(error.to_string().contains("migration stopped"));
        assert_eq!(store.pending().unwrap().len(), 1);
    }

    #[test]
    fn retry_reexecutes_failed_script_then_marks_it() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        let attempt = root.path().join("attempt");
        script_body(
            &store.migrations_dir,
            "100-retry.sh",
            "Retry",
            &format!(
                "if [[ ! -e '{}' ]]; then touch '{}'; exit 1; fi",
                attempt.display(),
                attempt.display()
            ),
        );
        let mut prompts = 0;
        assert!(
            run_queue(&store, store.pending().unwrap(), || {
                prompts += 1;
                Ok(true)
            })
            .unwrap()
        );
        assert_eq!(prompts, 1);
        assert!(store.pending().unwrap().is_empty());
    }

    #[test]
    fn discovery_ignores_non_scripts_and_rejects_non_executable_scripts() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        fs::create_dir_all(&store.migrations_dir).unwrap();
        fs::write(store.migrations_dir.join("notes.txt"), "ignored").unwrap();
        assert!(store.pending().unwrap().is_empty());

        let path = store.migrations_dir.join("100-disabled.sh");
        fs::write(&path, "#!/bin/bash\nexit 0\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            store
                .pending()
                .unwrap_err()
                .to_string()
                .contains("not executable")
        );
    }

    #[test]
    fn marker_set_handles_missing_empty_and_duplicate_lines() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("markers");
        assert!(read_marker_set(&path).unwrap().is_empty());
        fs::write(&path, "\na.sh\na.sh\n b.sh \n").unwrap();
        assert_eq!(read_marker_set(&path).unwrap().len(), 2);
    }

    fn workspace_session() -> MigrationSession {
        MigrationSession {
            id: uuid::Uuid::new_v4().to_string(),
            phase: SessionPhase::Initializing,
            manifest: vec![],
            completed: vec![],
            backup_path: None,
            candidate_path: None,
            connection_captured: false,
            reconnect_profile: None,
            summary: String::new(),
            error: None,
        }
    }

    #[test]
    fn cutover_keeps_previous_config_only_until_handoff_completes() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let _config = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", root.path());
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", root.path());
        let live = crate::paths::profiles_path().unwrap();
        fs::create_dir_all(live.parent().unwrap()).unwrap();
        fs::write(&live, b"old config").unwrap();
        let mut session = workspace_session();
        create_workspace(&mut session).unwrap();
        fs::write(session.candidate_path.as_ref().unwrap(), b"migrated config").unwrap();

        swap_candidate(&mut session).unwrap();

        assert_eq!(fs::read(&live).unwrap(), b"migrated config");
        assert_eq!(
            fs::read(session.candidate_path.as_ref().unwrap()).unwrap(),
            b"old config"
        );
        assert_eq!(
            fs::read(session.backup_path.as_ref().unwrap()).unwrap(),
            b"old config"
        );
        assert_eq!(session.phase, SessionPhase::StartingDaemon);

        remove_previous_live_candidate(&session).unwrap();
        assert!(!session.candidate_path.as_ref().unwrap().exists());
        assert_eq!(
            fs::read(session.backup_path.as_ref().unwrap()).unwrap(),
            b"old config"
        );
    }

    #[test]
    fn cutover_recovery_detects_exchange_before_journal_update() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let _config = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", root.path());
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", root.path());
        let live = crate::paths::profiles_path().unwrap();
        fs::create_dir_all(live.parent().unwrap()).unwrap();
        fs::write(&live, b"old config").unwrap();
        let mut session = workspace_session();
        create_workspace(&mut session).unwrap();
        let candidate = session.candidate_path.as_ref().unwrap().clone();
        fs::write(&candidate, b"migrated config").unwrap();
        session.phase = SessionPhase::Finalizing;

        atomic_exchange(&live, &candidate).unwrap();
        recover_cutover_phase(&mut session).unwrap();

        assert_eq!(fs::read(&live).unwrap(), b"migrated config");
        assert_eq!(
            fs::read(session.candidate_path.as_ref().unwrap()).unwrap(),
            b"old config"
        );
        assert_eq!(session.phase, SessionPhase::StartingDaemon);
    }

    #[test]
    fn cutover_recovery_does_not_guess_when_candidate_is_unchanged() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let _config = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", root.path());
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", root.path());
        let live = crate::paths::profiles_path().unwrap();
        fs::create_dir_all(live.parent().unwrap()).unwrap();
        fs::write(&live, b"same config").unwrap();
        let mut session = workspace_session();
        create_workspace(&mut session).unwrap();
        session.phase = SessionPhase::Finalizing;

        recover_cutover_phase(&mut session).unwrap();

        assert_eq!(session.phase, SessionPhase::Finalizing);
        assert!(session.candidate_path.as_ref().unwrap().exists());
    }

    #[test]
    fn session_journal_is_private_round_trips_and_maps_ui_phases() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", root.path());
        let mut session = workspace_session();
        session.summary = "First step".into();
        session.manifest = vec![MigrationRecord {
            id: "100-first.sh".into(),
            summary: "First".into(),
            digest: "abc".into(),
        }];
        save_session(&session).unwrap();

        let path = session_state_path().unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let loaded = load_session().unwrap().unwrap();
        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.summary, "First step");
        assert_eq!(
            loaded.ui_status().phase,
            crate::app::model::MigrationPhase::Running
        );

        session.phase = SessionPhase::Failed;
        assert_eq!(
            session.ui_status().phase,
            crate::app::model::MigrationPhase::Failed
        );
        for phase in [
            SessionPhase::Finalizing,
            SessionPhase::Swapped,
            SessionPhase::StartingDaemon,
        ] {
            session.phase = phase;
            assert_eq!(
                session.ui_status().phase,
                crate::app::model::MigrationPhase::Finalizing
            );
        }
        clear_session().unwrap();
        assert!(load_session().unwrap().is_none());
    }

    #[test]
    fn migration_manifest_detects_script_content_changes() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        script_body(&store.migrations_dir, "100-first.sh", "First", "echo one");
        let first = manifest(&store.discover().unwrap()).unwrap();

        script_body(&store.migrations_dir, "100-first.sh", "First", "echo two");
        let second = manifest(&store.discover().unwrap()).unwrap();

        assert_ne!(first[0].digest, second[0].digest);
        assert_eq!(first[0].id, second[0].id);
    }

    #[test]
    fn changed_source_rebuild_preserves_abandoned_candidate() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let _config = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", root.path());
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", root.path());
        let live = crate::paths::profiles_path().unwrap();
        fs::create_dir_all(live.parent().unwrap()).unwrap();
        crate::config::save_config_at(&live, &crate::config::profile::Config::default()).unwrap();
        let mut session = workspace_session();
        create_workspace(&mut session).unwrap();
        validate_candidate(&session).unwrap();
        assert!(!source_changed(&session).unwrap());

        let candidate = session.candidate_path.as_ref().unwrap().clone();
        fs::write(&live, b"externally changed").unwrap();
        assert!(source_changed(&session).unwrap());
        assert!(verify_source(&session).is_err());
        abandon_candidate(&session).unwrap();
        assert!(!candidate.exists());
        assert!(fs::read_dir(live.parent().unwrap()).unwrap().any(|entry| {
            entry.is_ok_and(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("profiles.json.abandoned-migration-")
            })
        }));
    }

    #[test]
    fn incomplete_or_unsafe_workspace_requires_rebuild() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let _config = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", root.path());
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", root.path());
        let live = crate::paths::profiles_path().unwrap();
        fs::create_dir_all(live.parent().unwrap()).unwrap();
        fs::write(&live, b"config").unwrap();
        let mut session = workspace_session();

        assert!(!workspace_needs_rebuild(&session).unwrap());
        session.backup_path = Some(root.path().join("only-backup"));
        assert!(workspace_needs_rebuild(&session).unwrap());

        session.backup_path = None;
        create_workspace(&mut session).unwrap();
        assert!(!workspace_needs_rebuild(&session).unwrap());
        fs::remove_file(session.candidate_path.as_ref().unwrap()).unwrap();
        assert!(workspace_needs_rebuild(&session).unwrap());
    }

    #[test]
    fn internal_profile_step_uses_only_the_matching_candidate() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let _config = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", root.path());
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", root.path());
        let live = crate::paths::profiles_path().unwrap();
        fs::create_dir_all(live.parent().unwrap()).unwrap();
        fs::write(&live, r#"{"schema_version":4,"profiles":[],"settings":{}}"#).unwrap();
        let mut session = workspace_session();
        create_workspace(&mut session).unwrap();
        save_session(&session).unwrap();
        let candidate = session.candidate_path.as_ref().unwrap();
        let _candidate =
            crate::test_helpers::EnvVarGuard::set("KVN_MIGRATION_PROFILES_PATH", candidate);
        let _session =
            crate::test_helpers::EnvVarGuard::set("KVN_MIGRATION_SESSION_ID", &session.id);

        prepare_profile_migration(5).unwrap();
        assert_eq!(
            crate::config::load_config_at_read_only(candidate)
                .unwrap()
                .schema_version,
            5
        );
        assert_eq!(
            fs::read_to_string(&live).unwrap(),
            r#"{"schema_version":4,"profiles":[],"settings":{}}"#
        );
    }

    #[test]
    fn migration_lock_rejects_a_concurrent_runner() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", root.path());

        let first = MigrationLock::acquire().unwrap();
        assert!(MigrationLock::acquire().is_err());
        drop(first);
        assert!(MigrationLock::acquire().is_ok());
    }

    #[test]
    fn generic_failure_is_persisted_for_doctor_and_overlay() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", root.path());
        let mut session = workspace_session();
        let mut daemon = None;

        record_session_failure(
            &mut session,
            &mut daemon,
            &anyhow::anyhow!("candidate validation failed"),
        );

        assert_eq!(session.phase, SessionPhase::Failed);
        assert_eq!(session.summary, "Migration paused");
        assert!(
            session
                .error
                .as_deref()
                .unwrap()
                .contains("candidate validation")
        );
        assert_eq!(load_session().unwrap().unwrap().phase, SessionPhase::Failed);
        assert!(
            session_diagnostic()
                .unwrap()
                .unwrap()
                .contains("Migration paused")
        );
    }
}

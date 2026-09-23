//! Ordered migrations shipped by the installed package.
//!
//! Migration scripts are immutable package payloads, each successful script
//! gets a per-user marker file, and the first failure stops the chain so a
//! later invocation resumes from the script that failed.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, ensure};

const INSTALLED_DIR: &str = "/usr/lib/kvn/migrations";
const BASELINE_PATH: &str = "/var/lib/kvn/migration-baseline";
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(2);
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
            .join("kvn/migrations");
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

    fn mark_applied_id(&self, id: &str) -> Result<()> {
        ensure!(
            id.ends_with(".sh") && !id.contains('/') && !id.contains('\\'),
            "invalid migration marker ID"
        );
        fs::create_dir_all(&self.state_dir)
            .context("failed to create migration state directory")?;
        crate::atomic_write::write(&self.state_dir.join(id), b"applied\n")
            .with_context(|| format!("failed to mark migration {id} as applied"))
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

/// Run every pending migration. Returns true when at least one script ran or
/// the profile schema was migrated.
pub fn run_pending_interactive() -> Result<bool> {
    let store = Store::installed()?;
    if store.pending()?.is_empty() && !pacman_is_running() && !profile_migration_required()? {
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
    run_migrations(store, prompt_retry)
}

fn run_migrations<F>(store: &Store, retry: F) -> Result<bool>
where
    F: FnMut() -> Result<bool>,
{
    wait_for_pacman()?;
    let pending = store.pending()?;
    if pending.is_empty() && !profile_migration_required()? {
        return Ok(false);
    }
    backup_profiles()?;
    let candidate = Candidate::create()?;
    run_queue(&pending, candidate.as_ref().map(Candidate::path), retry)?;
    let published = match &candidate {
        Some(candidate) => {
            candidate.migrate_to_current()?;
            candidate.publish()?
        }
        None => false,
    };
    for migration in &pending {
        store.mark_applied_id(&migration.id)?;
    }
    let changed = !pending.is_empty() || published;
    if changed {
        hand_off_to_daemon();
    }
    Ok(changed)
}

fn run_queue<F>(pending: &[Migration], profiles: Option<&Path>, mut retry: F) -> Result<()>
where
    F: FnMut() -> Result<bool>,
{
    for (index, migration) in pending.iter().enumerate() {
        loop {
            println!("[{}/{}] {}", index + 1, pending.len(), migration.summary);
            let mut command = Command::new("/usr/bin/bash");
            command
                .args(["-euo", "pipefail"])
                .arg(&migration.path)
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            match profiles {
                Some(profiles) => command.env("KVN_MIGRATION_PROFILES_PATH", profiles),
                None => command.env_remove("KVN_MIGRATION_PROFILES_PATH"),
            };
            let status = command
                .status()
                .with_context(|| format!("failed to start migration {}", migration.id))?;
            if status.success() {
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

fn backup_profiles() -> Result<Option<PathBuf>> {
    let path = crate::paths::profiles_path().context("failed to determine profiles path")?;
    let contents = match fs::read(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let backup = crate::config::recovery::preserve(
        crate::config::recovery::RecoveryKind::Migration,
        &contents,
    )
    .context("failed to back up profiles.json before migrating")?;
    println!("Saved a profiles.json backup to {}", backup.display());
    Ok(Some(backup))
}

struct Candidate {
    live: PathBuf,
    path: PathBuf,
    original: Vec<u8>,
}

impl Candidate {
    fn create() -> Result<Option<Self>> {
        let live = crate::paths::profiles_path().context("failed to determine profiles path")?;
        let original = match fs::read(&live) {
            Ok(original) => original,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", live.display()));
            }
        };
        let name = live
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("profiles.json");
        let path = live.with_file_name(format!(".{name}.migrating"));
        crate::atomic_write::write(&path, &original)
            .with_context(|| format!("failed to create {}", path.display()))?;
        Ok(Some(Self {
            live,
            path,
            original,
        }))
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn migrate_to_current(&self) -> Result<bool> {
        crate::config::migrate_config_at(&self.path)
    }

    fn publish(&self) -> Result<bool> {
        let migrated = fs::read(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        if migrated == self.original {
            self.remove();
            return Ok(false);
        }
        crate::config::load_config_at_read_only(&self.path)
            .context("migrated profiles.json is invalid")?
            .validate()
            .context("migrated profiles.json is invalid")?;
        crate::atomic_write::write_if_unchanged(&self.live, &migrated, Some(&self.original))
            .with_context(|| format!("failed to update {}", self.live.display()))?;
        self.remove();
        Ok(true)
    }

    fn remove(&self) {
        if let Err(error) = fs::remove_file(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(
                "Failed to remove migration candidate {:?}: {}",
                self.path,
                error
            );
        }
    }
}

fn hand_off_to_daemon() {
    let Ok(mut client) = crate::ipc::IpcClient::connect() else {
        return;
    };
    if attached_tui_sessions(&mut client).is_some_and(|sessions| sessions > 0)
        && client
            .send(&crate::app::msg::IpcCommand::RestartRequired)
            .is_ok()
    {
        println!("An open kvn window is asking for the daemon restart.");
        return;
    }
    drop(client);
    crate::systemd::restart_daemon_unit();
}

fn attached_tui_sessions(client: &mut crate::ipc::IpcClient) -> Option<u64> {
    client.send(&crate::app::msg::IpcCommand::Attach).ok()?;
    client
        .read_snapshot_value(SNAPSHOT_TIMEOUT)
        .ok()?
        .get("tui_sessions")
        .and_then(serde_json::Value::as_u64)
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
        let path = crate::paths::ensure_kvn_runtime_dir()?.join("migrate.lock");
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

impl Drop for MigrationLock {
    fn drop(&mut self) {
        // close() alone can leave the lock held by a concurrently forked child
        // until exec closes its inherited CLOEXEC descriptor. Release it
        // explicitly when the runner finishes, regardless of those copies.
        #[allow(unsafe_code)]
        let result = unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
        if result != 0 {
            tracing::warn!(error = %io::Error::last_os_error(), "Failed to release migration lock");
        }
    }
}
/// Refuse to start a new daemon on unmigrated state. An active package
/// transaction is not one: nothing is pending until its payload is installed.
pub fn block_daemon_if_pending() -> Result<bool> {
    report_blocking_state(pending(), profile_migration_required())
}

fn report_blocking_state(
    pending: Result<Vec<Migration>>,
    profile_migration_required: Result<bool>,
) -> Result<bool> {
    let pending = match pending {
        Ok(pending) => pending,
        Err(error) => {
            let message =
                format!("kvn migration state could not be inspected: {error:#}. Run: kvn doctor");
            tracing::error!("{message}");
            eprintln!("{message}");
            return Ok(true);
        }
    };
    let profile_migration_required = match profile_migration_required {
        Ok(required) => required,
        Err(error) => {
            let message =
                format!("kvn profile schema could not be inspected: {error:#}. Run: kvn doctor");
            tracing::error!("{message}");
            eprintln!("{message}");
            return Ok(true);
        }
    };
    if pending.is_empty() && !profile_migration_required {
        return Ok(false);
    }
    let message = if profile_migration_required && pending.is_empty() {
        "A kvn profiles.json schema migration is required. Launch `kvn` in a terminal to apply it"
            .to_string()
    } else {
        format!(
            "{} kvn migration(s) are pending. Launch `kvn` in a terminal to apply them",
            pending.len()
        )
    };
    tracing::error!("{message}");
    eprintln!("{message}");
    Ok(true)
}

pub fn print_pending() -> Result<bool> {
    Ok(report_pending(
        &pending()?,
        profile_migration_required()?,
        package_transaction_active(),
    ))
}

fn report_pending(
    pending: &[Migration],
    profile_migration_required: bool,
    package_transaction: bool,
) -> bool {
    if pending.is_empty() && !profile_migration_required && !package_transaction {
        println!("No pending kvn migrations.");
    }
    for migration in pending {
        println!("{}\t{}", migration.id, migration.summary);
    }
    if profile_migration_required {
        println!("profile-schema\tMigrate profiles.json to the current schema");
    }
    if package_transaction {
        println!("pacman-transaction\tWait for the active package transaction to finish");
    }
    !pending.is_empty() || profile_migration_required || package_transaction
}

pub fn run_command() -> Result<()> {
    run_pending_interactive().map(|_| ())
}

pub fn prepare_profile_migration(target_version: u32) -> Result<()> {
    let path = migration_profiles_path_from_environment()
        .context("`kvn config migrate` is an internal migration step; run `kvn migrate` instead")?;
    if crate::config::migrate_config_to_at(&path, target_version)? {
        println!("Migrated profiles.json to schema {target_version}.");
    } else {
        println!("profiles.json already uses schema {target_version}.");
    }
    Ok(())
}

fn migration_profiles_path_from_environment() -> Option<PathBuf> {
    std::env::var_os("KVN_MIGRATION_PROFILES_PATH")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn profile_migration_required() -> Result<bool> {
    let config = crate::paths::profiles_path().context("failed to determine profiles path")?;
    crate::config::schema_migration_required_at(&config)
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

    const PACKAGE_HOOKS: [&str; 2] = [
        include_str!("../pkg/arch/kvn-tui.install"),
        include_str!("../pkg/aur/kvn-tui.install"),
    ];

    fn seed_baseline_fixture(
        hook: &str,
        installed: &str,
        preexisting_state: bool,
    ) -> (HashSet<String>, String) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        for (id, version) in [("100-first.sh", "0.31.0"), ("200-second.sh", "0.35.0")] {
            script_body(
                &store.migrations_dir,
                id,
                id,
                &format!("# kvn:introduced={version}\nexit 0"),
            );
        }
        let state = root.path().join("machine-state");
        if preexisting_state {
            fs::create_dir_all(&state).unwrap();
        }
        let hook = hook
            .replace("/var/lib/kvn", state.to_str().unwrap())
            .replace(
                "/usr/lib/kvn/migrations",
                store.migrations_dir.to_str().unwrap(),
            );
        // CI need not have pacman/vercmp; these fixtures use plain
        // numeric releases for which sort -V has the same ordering.
        let command = format!(
            "{hook}\n{}",
            r#"
vercmp() {
    if [[ $1 == "$2" ]]; then echo 0
    elif [[ $(printf '%s\n' "$1" "$2" | sort -V | head -n1) == "$1" ]]; then echo -1
    else echo 1
    fi
}
seed_migration_baseline "$1"
print_migration_notice "$1"
"#
        );
        let output = Command::new("bash")
            .args(["-euc", &command, "kvn-install-test", installed])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let baseline = read_marker_set(&state.join("migration-baseline")).unwrap();
        (
            baseline,
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    }

    #[test]
    fn package_baseline_preserves_migrations_when_skipping_framework_release() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        for hook in PACKAGE_HOOKS {
            for installed in ["", "0.29.0", "0.30.0", "0.31.0"] {
                let (baseline, stdout) = seed_baseline_fixture(hook, installed, false);
                let expected: HashSet<String> = match installed {
                    "" => ["100-first.sh", "200-second.sh"]
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                    "0.31.0" => ["100-first.sh"].into_iter().map(str::to_string).collect(),
                    _ => HashSet::new(),
                };
                assert_eq!(baseline, expected, "upgrading from {installed}");
                if !installed.is_empty() {
                    assert!(stdout.contains("kvn has new migrations"));
                }
            }
        }
    }

    #[test]
    fn package_baseline_stays_empty_when_a_rename_reinstalls_over_existing_state() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        for hook in PACKAGE_HOOKS {
            let (baseline, _) = seed_baseline_fixture(hook, "", true);
            assert!(
                baseline.is_empty(),
                "a post_install over an existing state directory must leave every migration pending"
            );
        }
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

    fn always_retry() -> Result<bool> {
        Ok(true)
    }

    fn never_retry() -> Result<bool> {
        Ok(false)
    }

    struct ProfilesFixture {
        _dir: tempfile::TempDir,
        _config: crate::test_helpers::EnvVarGuard,
        _state: crate::test_helpers::EnvVarGuard,
        _runtime: crate::test_helpers::EnvVarGuard,
        path: PathBuf,
    }

    fn profiles_fixture(contents: Option<&[u8]>) -> ProfilesFixture {
        let dir = tempfile::tempdir().unwrap();
        let config =
            crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path().join("config"));
        let state =
            crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", dir.path().join("state"));
        let runtime_dir = dir.path().join("runtime");
        fs::create_dir_all(&runtime_dir).unwrap();
        fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", &runtime_dir);
        assert!(!crate::ipc::is_daemon_running());
        let path = crate::paths::profiles_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        if let Some(contents) = contents {
            fs::write(&path, contents).unwrap();
        }
        ProfilesFixture {
            _dir: dir,
            _config: config,
            _state: state,
            _runtime: runtime,
            path,
        }
    }

    #[test]
    fn the_queue_runs_in_bytewise_filename_order() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let fixture = profiles_fixture(None);
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        let trace = root.path().join("trace");
        for id in ["300-third.sh", "100-first.sh", "200-second.sh"] {
            script_body(
                &store.migrations_dir,
                id,
                id,
                &format!("echo {id} >>{}", trace.display()),
            );
        }

        let pending = store.pending().unwrap();
        run_queue(&pending, None, never_retry).unwrap();

        assert_eq!(
            fs::read_to_string(&trace).unwrap(),
            "100-first.sh\n200-second.sh\n300-third.sh\n"
        );
        drop(fixture);
    }

    #[test]
    fn migration_scripts_receive_the_candidate_never_the_live_config() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let fixture = profiles_fixture(Some(b"{  original  }"));
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        let seen = root.path().join("seen");
        script_body(
            &store.migrations_dir,
            "100-first.sh",
            "First",
            &format!(
                "printf '%s' \"${{KVN_MIGRATION_PROFILES_PATH:?}}\" >{0}\nprintf 'edited' >\"${{KVN_MIGRATION_PROFILES_PATH}}\"",
                seen.display()
            ),
        );
        let candidate = Candidate::create().unwrap().unwrap();

        run_queue(
            &store.pending().unwrap(),
            Some(candidate.path()),
            never_retry,
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&seen).unwrap(),
            candidate.path().to_str().unwrap()
        );
        assert_ne!(candidate.path(), fixture.path);
        assert_eq!(fs::read(&fixture.path).unwrap(), b"{  original  }");
    }

    #[test]
    fn a_queue_that_fails_midway_leaves_the_live_config_untouched() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let fixture =
            profiles_fixture(Some(br#"{"schema_version":4,"profiles":[],"settings":{}}"#));
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        let candidate = Candidate::create().unwrap().unwrap();
        script_body(
            &store.migrations_dir,
            "100-first.sh",
            "First",
            "kvn-config-migrate",
        );
        script_body(&store.migrations_dir, "200-second.sh", "Second", "exit 1");
        let migrate = store.migrations_dir.join("kvn-config-migrate");
        fs::write(
            &migrate,
            format!(
                "#!/bin/bash\nprintf '%s' '{}' >\"${{KVN_MIGRATION_PROFILES_PATH:?}}\"\n",
                format_args!(
                    r#"{{"schema_version":{},"profiles":[],"settings":{{}}}}"#,
                    crate::config::profile::CURRENT_SCHEMA_VERSION
                )
            ),
        )
        .unwrap();
        fs::set_permissions(&migrate, fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!(
            "{}:{}",
            store.migrations_dir.display(),
            std::env::var("PATH").unwrap()
        );
        let _path = crate::test_helpers::EnvVarGuard::set("PATH", path);

        assert!(
            run_queue(
                &store.pending().unwrap(),
                Some(candidate.path()),
                never_retry
            )
            .is_err()
        );

        assert_eq!(
            fs::read(&fixture.path).unwrap(),
            br#"{"schema_version":4,"profiles":[],"settings":{}}"#
        );
        assert!(crate::config::schema_migration_required_at(&fixture.path).unwrap());
    }

    #[test]
    fn publishing_replaces_the_live_config_once_and_removes_the_candidate() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let fixture =
            profiles_fixture(Some(br#"{"schema_version":4,"profiles":[],"settings":{}}"#));
        let candidate = Candidate::create().unwrap().unwrap();

        assert!(candidate.migrate_to_current().unwrap());
        assert!(crate::config::schema_migration_required_at(&fixture.path).unwrap());

        assert!(candidate.publish().unwrap());
        assert!(!crate::config::schema_migration_required_at(&fixture.path).unwrap());
        assert!(!candidate.path().exists());
    }

    #[test]
    fn publishing_is_a_no_op_when_no_migration_changed_the_candidate() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let fixture = profiles_fixture(Some(b"{  untouched  }"));
        let candidate = Candidate::create().unwrap().unwrap();

        assert!(!candidate.publish().unwrap());
        assert_eq!(fs::read(&fixture.path).unwrap(), b"{  untouched  }");
        assert!(!candidate.path().exists());
    }

    #[test]
    fn publishing_refuses_when_the_live_config_changed_during_the_run() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let fixture =
            profiles_fixture(Some(br#"{"schema_version":4,"profiles":[],"settings":{}}"#));
        let candidate = Candidate::create().unwrap().unwrap();
        candidate.migrate_to_current().unwrap();

        fs::write(
            &fixture.path,
            br#"{"schema_version":4,"profiles":[],"settings":{}} "#,
        )
        .unwrap();

        let error = candidate.publish().unwrap_err().to_string();
        assert!(error.contains("failed to update"));
        assert_eq!(
            fs::read(&fixture.path).unwrap(),
            br#"{"schema_version":4,"profiles":[],"settings":{}} "#
        );
    }

    #[test]
    fn there_is_no_candidate_without_a_config_file() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let _fixture = profiles_fixture(None);
        assert!(Candidate::create().unwrap().is_none());
    }

    #[test]
    fn a_successful_run_marks_the_whole_queue() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let fixture =
            profiles_fixture(Some(br#"{"schema_version":4,"profiles":[],"settings":{}}"#));
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        script(&store.migrations_dir, "100-first.sh", "First");
        script(&store.migrations_dir, "200-second.sh", "Second");

        assert!(run_migrations(&store, never_retry).unwrap());

        assert!(store.pending().unwrap().is_empty());
        assert!(!crate::config::schema_migration_required_at(&fixture.path).unwrap());
        assert!(
            !fixture
                .path
                .with_file_name(".profiles.json.migrating")
                .exists()
        );
    }

    #[test]
    fn a_failed_run_marks_nothing_and_replays_the_whole_queue() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let _fixture = profiles_fixture(None);
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        let attempts = root.path().join("attempts");
        script(&store.migrations_dir, "100-first.sh", "First");
        script_body(
            &store.migrations_dir,
            "200-second.sh",
            "Second",
            &format!(
                "echo x >>{0}\n[[ $(grep -c x {0}) -ge 2 ]]",
                attempts.display()
            ),
        );
        script(&store.migrations_dir, "300-third.sh", "Third");

        let error = run_migrations(&store, never_retry).unwrap_err();
        assert!(error.to_string().contains("kvn migrate"));
        assert_eq!(
            store
                .pending()
                .unwrap()
                .into_iter()
                .map(|migration| migration.id)
                .collect::<Vec<_>>(),
            ["100-first.sh", "200-second.sh", "300-third.sh"],
            "a partial run must mark nothing"
        );

        run_migrations(&store, never_retry).unwrap();
        assert!(store.pending().unwrap().is_empty());
    }

    #[test]
    fn retrying_reruns_only_the_failed_migration() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let _fixture = profiles_fixture(None);
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        let attempts = root.path().join("attempts");
        script_body(
            &store.migrations_dir,
            "100-first.sh",
            "First",
            &format!(
                "echo x >>{0}\n[[ $(grep -c x {0}) -ge 3 ]]",
                attempts.display()
            ),
        );

        run_queue(&store.pending().unwrap(), None, always_retry).unwrap();

        assert_eq!(fs::read_to_string(&attempts).unwrap().lines().count(), 3);
    }

    #[test]
    fn backup_copies_the_exact_bytes_and_skips_a_missing_config() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let fixture = profiles_fixture(None);
        assert!(backup_profiles().unwrap().is_none());

        fs::write(&fixture.path, b"{  exact bytes  }\n").unwrap();
        let backup = backup_profiles().unwrap().unwrap();
        assert_eq!(fs::read(&backup).unwrap(), b"{  exact bytes  }\n");
        assert_eq!(fs::read(&fixture.path).unwrap(), b"{  exact bytes  }\n");
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            backup.parent().unwrap(),
            fixture.path.parent().unwrap().join("recovery")
        );
    }

    #[test]
    fn config_migrate_step_requires_the_runner_environment() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let _cleared = crate::test_helpers::EnvVarGuard::remove("KVN_MIGRATION_PROFILES_PATH");
        let error = prepare_profile_migration(crate::config::profile::CURRENT_SCHEMA_VERSION)
            .unwrap_err()
            .to_string();
        assert!(error.contains("kvn migrate"));
    }

    #[test]
    fn config_migrate_step_upgrades_the_path_it_is_given() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        fs::write(&path, r#"{"schema_version":4,"profiles":[],"settings":{}}"#).unwrap();
        let _set = crate::test_helpers::EnvVarGuard::set("KVN_MIGRATION_PROFILES_PATH", &path);

        prepare_profile_migration(crate::config::profile::CURRENT_SCHEMA_VERSION).unwrap();

        assert!(!crate::config::schema_migration_required_at(&path).unwrap());
    }

    #[test]
    fn only_one_runner_holds_the_migration_lock() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", dir.path());

        let held = MigrationLock::acquire().unwrap();
        let contended = MigrationLock::acquire();
        assert!(
            contended
                .err()
                .map(|error| error.to_string())
                .is_some_and(|error| error.contains("already running"))
        );

        drop(held);
        MigrationLock::acquire().unwrap();
    }

    fn sample_migration() -> Migration {
        Migration {
            id: "1700000000-sample.sh".into(),
            path: PathBuf::from("/usr/lib/kvn/migrations/1700000000-sample.sh"),
            summary: "Sample".into(),
        }
    }

    #[test]
    fn pending_listing_reports_scripts_an_outdated_schema_and_a_transaction() {
        assert!(!report_pending(&[], false, false));
        assert!(report_pending(&[sample_migration()], false, false));
        assert!(report_pending(&[], true, false));
        assert!(report_pending(&[], false, true));
    }

    #[test]
    fn an_outdated_schema_is_detected_from_the_profiles_file() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let fixture =
            profiles_fixture(Some(br#"{"schema_version":4,"profiles":[],"settings":{}}"#));
        assert!(profile_migration_required().unwrap());

        fs::write(
            &fixture.path,
            format!(
                r#"{{"schema_version":{},"profiles":[],"settings":{{}}}}"#,
                crate::config::profile::CURRENT_SCHEMA_VERSION
            ),
        )
        .unwrap();
        assert!(!profile_migration_required().unwrap());
    }

    #[test]
    fn a_pending_schema_migration_blocks_daemon_startup() {
        assert!(!report_blocking_state(Ok(Vec::new()), Ok(false)).unwrap());
        assert!(report_blocking_state(Ok(Vec::new()), Ok(true)).unwrap());
        assert!(report_blocking_state(Ok(vec![sample_migration()]), Ok(false)).unwrap());
        assert!(
            report_blocking_state(Err(anyhow::anyhow!("unreadable store")), Ok(false)).unwrap()
        );
        assert!(
            report_blocking_state(Ok(Vec::new()), Err(anyhow::anyhow!("unreadable config")))
                .unwrap()
        );
    }
}

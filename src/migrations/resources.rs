//! Download-only preparation. Never run downloaded code or change the live installation.

use super::*;
use semver::{Version, VersionReq};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GitResource {
    id: String,
    url: String,
    commit: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    when: Option<ResourceCondition>,
    #[serde(default)]
    git: Vec<GitResource>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceCondition {
    omarchy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<VersionReq>,
}

fn applies_to(manifest: &ResourceManifest, omarchy_version: Option<&str>) -> Result<bool> {
    let Some(condition) = &manifest.when else {
        return Ok(true);
    };
    let Some(version) = omarchy_version else {
        return Ok(false);
    };
    match &condition.version {
        None => Ok(true),
        Some(requirement) => {
            let version = parse_omarchy_version(version).context(
                "unrecognized Omarchy version; cannot determine whether migration resources apply",
            )?;
            Ok(requirement.matches(&version))
        }
    }
}

fn parse_omarchy_version(version: &str) -> Option<Version> {
    let version = version.trim();
    let version = version.strip_prefix("Omarchy ").unwrap_or(version);
    let version = version.strip_prefix('v').unwrap_or(version);
    let version = version
        .rsplit_once('-')
        .filter(|(_, release)| {
            release
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        })
        .map_or(version, |(version, _)| version);
    Version::parse(version).ok()
}

fn detect_omarchy_version() -> Result<Option<String>> {
    let output = match Command::new("omarchy")
        .arg("version")
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).context("failed to detect Omarchy for migration resources");
        }
    };
    ensure!(
        output.status.success(),
        "omarchy version failed; cannot determine whether plugin resources apply"
    );
    let version = String::from_utf8(output.stdout).context("invalid Omarchy version output")?;
    Ok(Some(version.trim().to_owned()))
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct PreparedMigration {
    pub migration_id: String,
    pub directory: PathBuf,
    manifest: ResourceManifest,
    digest: String,
}

fn root() -> Result<PathBuf> {
    Ok(dirs::state_dir()
        .context("failed to determine XDG state directory")?
        .join("kvn-tui/migration-resources"))
}

pub(super) fn manifest_path(migration: &Migration) -> PathBuf {
    migration.path.with_extension("resources.json")
}

pub(super) fn manifest_digest(migration: &Migration) -> Result<Option<String>> {
    let path = manifest_path(migration);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("failed to inspect resource manifest"),
        Ok(metadata) => {
            ensure!(
                metadata.is_file(),
                "resource manifest is not a regular file"
            );
            migration_digest(&path).map(Some)
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn validate_manifest(manifest: &ResourceManifest) -> Result<()> {
    if let Some(condition) = &manifest.when {
        ensure!(
            condition.omarchy,
            "migration resource condition `omarchy` must be true"
        );
    }
    let mut ids = HashSet::new();
    for resource in &manifest.git {
        ensure!(valid_id(&resource.id), "invalid migration resource ID");
        ensure!(ids.insert(&resource.id), "duplicate migration resource ID");
        let url = url::Url::parse(&resource.url).context("invalid Git resource URL")?;
        ensure!(
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "Git resources require an HTTPS URL without credentials, query or fragment"
        );
        ensure!(
            matches!(resource.commit.len(), 40 | 64)
                && resource.commit.bytes().all(|b| b.is_ascii_hexdigit()),
            "Git resource must pin a full commit SHA"
        );
    }
    Ok(())
}

fn read_manifest(store: &Store, migration: &Migration) -> Result<ResourceManifest> {
    let path = manifest_path(migration);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ResourceManifest::default());
        }
        Err(error) => return Err(error).context("failed to inspect resource manifest"),
    };
    ensure!(
        metadata.is_file(),
        "resource manifest is not a regular file"
    );
    if store.enforce_package_ownership {
        validate_packaged_path(&path, &metadata)?;
    }
    let manifest: ResourceManifest =
        serde_json::from_slice(&fs::read(path)?).context("invalid migration resource manifest")?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn git() -> Command {
    let mut command = Command::new("git");
    // Preparation must not execute user-configured hooks, filters or URL rewrites.
    let git_env: Vec<_> = std::env::vars_os()
        .filter(|(name, _)| name.to_string_lossy().starts_with("GIT_"))
        .map(|(name, _)| name)
        .collect();
    for name in git_env {
        command.env_remove(name);
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.https.allow=always",
            "-c",
            "submodule.recurse=false",
            "-c",
            "http.lowSpeedLimit=1",
            "-c",
            "http.lowSpeedTime=30",
        ])
        .stdin(Stdio::null());
    command
}

fn git_output(path: &Path, args: &[&str]) -> Result<String> {
    let output = git()
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .context("Git is required to prepare migration resources; install git and retry")?;
    ensure!(
        output.status.success(),
        "Git resource validation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn validate_checkout(path: &Path, resource: &GitResource) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "resource is not a directory"
    );
    ensure!(
        fs::symlink_metadata(path.join(".git"))?.is_dir(),
        "resource is not an independent Git checkout"
    );
    ensure!(
        git_output(path, &["rev-parse", "HEAD"])? == resource.commit.to_ascii_lowercase(),
        "prepared Git commit changed"
    );
    ensure!(
        git_output(path, &["remote", "get-url", "origin"])? == resource.url,
        "prepared Git origin changed"
    );
    ensure!(
        git_output(
            path,
            &[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--ignored"
            ]
        )?
        .is_empty(),
        "prepared Git checkout is dirty"
    );
    let tree = git_output(path, &["ls-tree", "-r", "HEAD"])?;
    ensure!(
        !tree.lines().any(|line| line.starts_with("160000 ")),
        "migration resources cannot require submodules"
    );
    let attributes = git()
        .arg("-C")
        .arg(path)
        .args([
            "grep",
            "-l",
            "-E",
            "filter[[:space:]]*=[[:space:]]*lfs",
            "HEAD",
            "--",
            "*.gitattributes",
        ])
        .output()?;
    ensure!(
        attributes.status.code() == Some(1),
        "migration resources cannot require Git LFS (or attribute inspection failed)"
    );
    Ok(())
}

fn private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "resource directory must not be a symlink"
    );
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub(super) fn prepare(store: &Store, pending: &[Migration]) -> Result<Vec<PreparedMigration>> {
    let result = prepare_downloads(store, pending);
    if let Err(error) = &result {
        // This is deliberately separate from migration-session.json: a failed
        // download must not freeze the daemon or suppress VPN auto-connect.
        let _ = save_preparation_status(&format!(
            "Resource preparation failed: {error:#}. Run kvn migrate to retry."
        ));
    }
    result
}

fn prepare_downloads(store: &Store, pending: &[Migration]) -> Result<Vec<PreparedMigration>> {
    prepare_downloads_with_detection(store, pending, detect_omarchy_version)
}

fn prepare_downloads_with_detection(
    store: &Store,
    pending: &[Migration],
    detect: impl FnOnce() -> Result<Option<String>>,
) -> Result<Vec<PreparedMigration>> {
    // Validate the entire queue before making any network requests.
    let manifests = pending
        .iter()
        .map(|migration| read_manifest(store, migration))
        .collect::<Result<Vec<_>>>()?;
    // Inspect the environment once, before any Git command or cache creation.
    // A resource-free/unconditional queue never needs an Omarchy executable.
    let omarchy_version = if manifests
        .iter()
        .any(|manifest| manifest.when.is_some() && !manifest.git.is_empty())
    {
        detect()?
    } else {
        None
    };
    let applicable = manifests
        .iter()
        .map(|manifest| applies_to(manifest, omarchy_version.as_deref()))
        .collect::<Result<Vec<_>>>()?;
    let mut prepared = Vec::new();
    for ((migration, manifest), applies) in pending.iter().zip(manifests).zip(applicable) {
        if manifest.git.is_empty() || !applies {
            continue;
        }
        let digest = migration_digest(&manifest_path(migration))?;
        let resource_root = root()?;
        private_directory(&resource_root)?;
        let directory = resource_root.join(format!("{}-{digest}", migration.id));
        private_directory(&directory)?;
        // SIGKILL/power loss bypasses TempDir's destructor. Only remove our
        // reserved-prefix partial downloads, never published checkouts.
        for child in fs::read_dir(&directory)? {
            let child = child?;
            let name = child.file_name();
            if name
                .to_str()
                .is_some_and(|name| name.starts_with(".download-"))
                && child.file_type()?.is_dir()
            {
                fs::remove_dir_all(child.path())?;
            }
        }
        let entry = PreparedMigration {
            migration_id: migration.id.clone(),
            directory,
            manifest,
            digest,
        };
        for resource in &entry.manifest.git {
            let destination = entry.directory.join(&resource.id);
            match fs::symlink_metadata(&destination) {
                Ok(_) => {
                    validate_checkout(&destination, resource).context(
                        "prepared resource changed; move it aside and retry kvn migrate",
                    )?;
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("failed to inspect prepared resource"),
            }
            let summary = format!("Preparing {}: {}…", migration.id, resource.id);
            save_preparation_status(&summary)?;
            println!("{summary}");
            let temporary = tempfile::Builder::new()
                .prefix(".download-")
                .tempdir_in(&entry.directory)?;
            let checkout = temporary.path().join("checkout");
            let status = git()
                .args(["clone", "--no-checkout", "--no-recurse-submodules", "--"])
                .arg(&resource.url)
                .arg(&checkout)
                .status()
                .context("Git is required to prepare migration resources; install git and retry")?;
            ensure!(
                status.success(),
                "Git download failed before migration mode; connect VPN if needed and retry kvn migrate"
            );
            git_output(&checkout, &["checkout", "--detach", &resource.commit])?;
            validate_checkout(&checkout, resource)?;
            fs::rename(checkout, &destination)?;
            sync_parent(&destination)?;
        }
        // Only a fully downloaded and verified set is published as ready.
        crate::atomic_write::write(
            &entry.directory.join("ready.json"),
            &serde_json::to_vec(&entry)?,
        )?;
        prepared.push(entry);
    }
    Ok(prepared)
}

pub(super) fn verify(prepared: &[PreparedMigration]) -> Result<()> {
    for entry in prepared {
        validate_manifest(&entry.manifest)?;
        validate_directory(entry, &root()?)?;
        let bytes = fs::read(entry.directory.join("ready.json"))
            .context("prepared resource metadata unavailable; restore the cache before retrying the active migration")?;
        let ready: PreparedMigration =
            serde_json::from_slice(&bytes).context("invalid prepared resource metadata")?;
        ensure!(ready == *entry, "prepared resource manifest changed");
        for resource in &entry.manifest.git {
            validate_checkout(&entry.directory.join(&resource.id), resource)
                .context("prepared resources unavailable; migration paused without downloading")?;
        }
    }
    Ok(())
}

pub(super) fn cleanup(prepared: &[PreparedMigration]) -> Result<()> {
    let root = root()?;
    for entry in prepared {
        validate_directory(entry, &root)?;
        match fs::symlink_metadata(&entry.directory) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            Ok(metadata) => ensure!(
                metadata.is_dir(),
                "resource directory must not be a symlink"
            ),
        }
        fs::remove_dir_all(&entry.directory)
            .context("failed to remove completed migration resources")?;
    }
    Ok(())
}

fn validate_directory(entry: &PreparedMigration, resource_root: &Path) -> Result<()> {
    ensure!(
        entry.migration_id.len() > 3
            && entry.migration_id.ends_with(".sh")
            && entry
                .migration_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')),
        "invalid prepared migration ID"
    );
    ensure!(
        entry.digest.len() == 16 && entry.digest.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid resource digest"
    );
    ensure!(
        entry.directory == resource_root.join(format!("{}-{}", entry.migration_id, entry.digest)),
        "resource directory outside its migration storage"
    );
    if let Ok(metadata) = fs::symlink_metadata(resource_root) {
        ensure!(metadata.is_dir(), "resource storage must not be a symlink");
    }
    if let Ok(metadata) = fs::symlink_metadata(&entry.directory) {
        ensure!(
            metadata.is_dir(),
            "resource directory must not be a symlink"
        );
    }
    Ok(())
}

fn save_preparation_status(summary: &str) -> Result<()> {
    let root = root()?;
    private_directory(&root)?;
    crate::atomic_write::write(
        &root.join("preparation.json"),
        &serde_json::to_vec(summary)?,
    )
}

pub(super) fn preparation_diagnostic() -> Result<Option<String>> {
    match fs::read(root()?.join("preparation.json")) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("failed to read resource preparation status"),
    }
}

pub(super) fn clear_preparation_status() -> Result<()> {
    match fs::remove_file(root()?.join("preparation.json")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("failed to clear resource preparation status"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omarchy_condition_uses_an_optional_semver_requirement() {
        let conditional: ResourceManifest = serde_json::from_str(
            r#"{"when":{"omarchy":true,"version":">=4.0.0, <5.0.0"},"git":[]}"#,
        )
        .unwrap();
        for version in [None, Some("3.9.9"), Some("5.0.0"), Some("40.0.0")] {
            assert!(!applies_to(&conditional, version).unwrap());
        }
        for version in ["4.0.0", "4.0.3-1", "4.9.9"] {
            assert!(applies_to(&conditional, Some(version)).unwrap());
        }
        assert!(applies_to(&conditional, Some("unrecognized")).is_err());
        assert!(!applies_to(&conditional, Some("4.0.0-beta.1")).unwrap());
        assert!(applies_to(&ResourceManifest::default(), None).unwrap());
        let any_omarchy: ResourceManifest =
            serde_json::from_str(r#"{"when":{"omarchy":true},"git":[]}"#).unwrap();
        assert!(!applies_to(&any_omarchy, None).unwrap());
        assert!(applies_to(&any_omarchy, Some("unrecognized")).unwrap());
        assert!(
            serde_json::from_str::<ResourceManifest>(r#"{"when":"omarchy_4","git":[]}"#).is_err()
        );
        for manifest in [
            r#"{"when":{"omarchy":false},"git":[]}"#,
            r#"{"when":{"version":"^4.0.0"},"git":[]}"#,
            r#"{"when":{"omarchy":true,"version":"not-semver"},"git":[]}"#,
            r#"{"when":{"omarchy":true,"unknown":1},"git":[]}"#,
        ] {
            let parsed = serde_json::from_str::<ResourceManifest>(manifest);
            assert!(parsed.is_err() || validate_manifest(&parsed.unwrap()).is_err());
        }
        assert_eq!(
            serde_json::to_value(conditional).unwrap()["when"],
            serde_json::json!({"omarchy": true, "version": ">=4.0.0, <5.0.0"})
        );
    }

    #[test]
    fn parses_omarchy_semver_and_ignores_numeric_package_release() {
        for (input, expected) in [
            ("4.0.0", "4.0.0"),
            (" 4.2.0-1\n", "4.2.0"),
            ("4.2.0-1.2", "4.2.0"),
            ("v4.0.0-beta.1", "4.0.0-beta.1"),
            ("Omarchy 4.0.0", "4.0.0"),
            ("Omarchy v4.0.0", "4.0.0"),
        ] {
            assert_eq!(parse_omarchy_version(input).unwrap().to_string(), expected);
        }
        for version in ["", "unknown", "error 404", "4.invalid", "release 2026.09"] {
            assert_eq!(parse_omarchy_version(version), None);
        }
    }

    #[test]
    fn detects_missing_omarchy_and_reports_broken_version_commands() {
        let _lock = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let _path = crate::test_helpers::EnvVarGuard::set("PATH", scratch.path());
        assert_eq!(detect_omarchy_version().unwrap(), None);
        let command = scratch.path().join("omarchy");
        for (script, expected) in [
            ("#!/bin/sh\nprintf '4.0.0-1\\n'\n", "4.0.0-1"),
            ("#!/bin/sh\nprintf '3.4.0\\n'\n", "3.4.0"),
        ] {
            fs::write(&command, script).unwrap();
            fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(detect_omarchy_version().unwrap().as_deref(), Some(expected));
        }
        fs::write(&command, "#!/bin/sh\nexit 1\n").unwrap();
        assert!(detect_omarchy_version().is_err());
    }

    fn conditional_fixture(scratch: &Path) -> (Store, Migration) {
        let store = Store::fixture(scratch);
        fs::create_dir_all(&store.migrations_dir).unwrap();
        let migration = Migration {
            id: "200-omarchy-plugin.sh".into(),
            path: store.migrations_dir.join("200-omarchy-plugin.sh"),
            summary: "Plugin".into(),
        };
        fs::write(&migration.path, b"#!/bin/bash\nexit 0\n").unwrap();
        let manifest = ResourceManifest {
            when: Some(ResourceCondition {
                omarchy: true,
                version: Some(VersionReq::parse(">=4.0.0, <5.0.0").unwrap()),
            }),
            git: vec![GitResource {
                id: "omakvn".into(),
                url: "https://example.invalid/plugin.git".into(),
                commit: "a".repeat(40),
            }],
        };
        fs::write(
            manifest_path(&migration),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        (store, migration)
    }

    #[test]
    fn other_desktops_skip_before_git_or_cache_creation() {
        let _lock = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", scratch.path());
        // No Git executable is available: a skipped resource must not need it.
        let _path = crate::test_helpers::EnvVarGuard::set("PATH", scratch.path());
        let (store, migration) = conditional_fixture(scratch.path());
        for version in [None, Some("3.4.0"), Some("5.0.0")] {
            let prepared =
                prepare_downloads_with_detection(&store, std::slice::from_ref(&migration), || {
                    Ok(version.map(str::to_owned))
                })
                .unwrap();
            assert!(prepared.is_empty());
            assert!(!root().unwrap().exists());
            assert!(load_ui_status().unwrap().is_none());
        }
        // Exercise actual missing-command detection on plain Arch as well.
        assert!(prepare(&store, &[migration]).unwrap().is_empty());
        assert!(!root().unwrap().exists());
    }

    #[test]
    fn detection_failure_prevents_downloads() {
        let scratch = tempfile::tempdir().unwrap();
        let (store, migration) = conditional_fixture(scratch.path());
        assert!(
            prepare_downloads_with_detection(&store, &[migration], || {
                anyhow::bail!("version probe failed")
            })
            .is_err()
        );
    }

    #[test]
    fn empty_resources_do_not_probe_omarchy() {
        let scratch = tempfile::tempdir().unwrap();
        let store = Store::fixture(scratch.path());
        assert!(
            prepare_downloads_with_detection(&store, &[], || {
                panic!("an empty queue must not probe Omarchy")
            })
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn resources_require_pinned_https_and_unique_safe_ids() {
        let good = GitResource {
            id: "omakvn".into(),
            url: "https://example.com/plugin.git".into(),
            commit: "a".repeat(40),
        };
        assert!(
            validate_manifest(&ResourceManifest {
                when: None,
                git: vec![good.clone()]
            })
            .is_ok()
        );
        for (url, commit, id) in [
            (
                "https://user:secret@example.com/repo",
                good.commit.as_str(),
                "plugin",
            ),
            ("file:///tmp/repo", good.commit.as_str(), "plugin"),
            (good.url.as_str(), "main", "plugin"),
            (good.url.as_str(), good.commit.as_str(), "../plugin"),
        ] {
            assert!(
                validate_manifest(&ResourceManifest {
                    when: None,
                    git: vec![GitResource {
                        id: id.into(),
                        url: url.into(),
                        commit: commit.into()
                    }]
                })
                .is_err()
            );
        }
        assert!(
            validate_manifest(&ResourceManifest {
                when: None,
                git: vec![good.clone(), good]
            })
            .is_err()
        );
    }

    #[test]
    fn resource_free_migration_needs_no_git_or_storage() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::fixture(root.path());
        let migration = Migration {
            id: "100-local.sh".into(),
            path: root.path().join("100-local.sh"),
            summary: "Local".into(),
        };
        assert!(prepare(&store, &[migration]).unwrap().is_empty());
    }

    fn cached_fixture(scratch: &Path) -> (Store, Migration, PathBuf) {
        let store = Store::fixture(scratch);
        fs::create_dir_all(&store.migrations_dir).unwrap();
        let migration = Migration {
            id: "100-v0.31-plugin.sh".into(),
            path: store.migrations_dir.join("100-v0.31-plugin.sh"),
            summary: "Plugin".into(),
        };
        fs::write(&migration.path, "#!/bin/bash\nexit 0\n").unwrap();
        let checkout = scratch.join("checkout");
        assert!(
            git()
                .args(["init", "-q"])
                .arg(&checkout)
                .status()
                .unwrap()
                .success()
        );
        fs::write(checkout.join("manifest.json"), r#"{"id":"yarikov.omakvn"}"#).unwrap();
        git_output(&checkout, &["add", "."]).unwrap();
        git_output(
            &checkout,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-qm",
                "Fixture",
            ],
        )
        .unwrap();
        let resource = GitResource {
            id: "omakvn".into(),
            url: "https://example.invalid/plugin.git".into(),
            commit: git_output(&checkout, &["rev-parse", "HEAD"]).unwrap(),
        };
        git_output(&checkout, &["remote", "add", "origin", &resource.url]).unwrap();
        let manifest = ResourceManifest {
            when: None,
            git: vec![resource],
        };
        fs::write(
            manifest_path(&migration),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let digest = migration_digest(&manifest_path(&migration)).unwrap();
        let directory = root().unwrap().join(format!("{}-{digest}", migration.id));
        fs::create_dir_all(&directory).unwrap();
        fs::rename(checkout, directory.join("omakvn")).unwrap();
        (store, migration, directory)
    }

    #[test]
    fn cached_resources_are_reused_and_cleanup_is_idempotent() {
        let _lock = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", scratch.path());
        let (store, migration, directory) = cached_fixture(scratch.path());
        let partial = directory.join(".download-interrupted");
        fs::create_dir(&partial).unwrap();
        fs::write(partial.join("incomplete"), b"partial").unwrap();
        // The origin is intentionally unreachable: this must use only local Git.
        let prepared = prepare(&store, &[migration]).unwrap();
        assert!(!partial.exists());
        verify(&prepared).unwrap();
        assert!(load_ui_status().unwrap().is_none());
        cleanup(&prepared).unwrap();
        cleanup(&prepared).unwrap();
        assert!(!directory.exists());
        assert!(verify(&prepared).is_err());
    }

    #[test]
    fn omarchy_four_prepares_resources_and_retry_keeps_the_recorded_selection() {
        let _lock = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", scratch.path());
        let (store, migration, old_directory) = cached_fixture(scratch.path());
        let mut manifest = read_manifest(&store, &migration).unwrap();
        let condition = ResourceCondition {
            omarchy: true,
            version: Some(VersionReq::parse(">=4.0.0, <5.0.0").unwrap()),
        };
        manifest.when = Some(condition.clone());
        fs::write(
            manifest_path(&migration),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let digest = migration_digest(&manifest_path(&migration)).unwrap();
        let directory = root().unwrap().join(format!("{}-{digest}", migration.id));
        fs::rename(old_directory, &directory).unwrap();
        let prepared =
            prepare_downloads_with_detection(&store, &[migration], || Ok(Some("4.0.3-1".into())))
                .unwrap();
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].manifest.when, Some(condition));
        // Verification consumes the recorded selection, not today's desktop.
        verify(&prepared).unwrap();
        assert_eq!(prepared[0].directory, directory);
    }

    #[test]
    fn conditional_resources_do_not_skip_unrelated_resources() {
        let _lock = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", scratch.path());
        let (store, unconditional, _) = cached_fixture(scratch.path());
        let (_, conditional) = conditional_fixture(scratch.path());
        let prepared =
            prepare_downloads_with_detection(&store, &[unconditional, conditional], || Ok(None))
                .unwrap();
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].migration_id, "100-v0.31-plugin.sh");
    }

    #[test]
    fn dirty_resource_fails_before_session_and_is_preserved_for_recovery() {
        let _lock = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", scratch.path());
        let (store, migration, directory) = cached_fixture(scratch.path());
        fs::write(directory.join("omakvn/local-edit"), b"keep me").unwrap();
        assert!(prepare(&store, &[migration]).is_err());
        assert!(load_ui_status().unwrap().is_none());
        assert!(
            preparation_diagnostic()
                .unwrap()
                .unwrap()
                .contains("failed")
        );
        assert!(directory.join("omakvn/local-edit").exists());
    }

    #[test]
    fn active_retry_rejects_changed_metadata_and_missing_resources_without_download() {
        let _lock = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", scratch.path());
        let (store, migration, directory) = cached_fixture(scratch.path());
        let prepared = prepare(&store, &[migration]).unwrap();
        let ready = directory.join("ready.json");
        fs::write(&ready, b"{}").unwrap();
        assert!(verify(&prepared).is_err());
        fs::write(ready, serde_json::to_vec(&prepared[0]).unwrap()).unwrap();
        fs::rename(
            directory.join("omakvn"),
            scratch.path().join("saved-checkout"),
        )
        .unwrap();
        assert!(verify(&prepared).is_err());
        assert!(directory.exists());
    }

    #[test]
    fn cleanup_refuses_unrelated_directories_and_symlinks() {
        let _lock = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", scratch.path());
        let (store, migration, directory) = cached_fixture(scratch.path());
        let mut prepared = prepare(&store, &[migration]).unwrap();
        prepared[0].directory = scratch.path().to_path_buf();
        assert!(cleanup(&prepared).is_err());
        prepared[0].directory = directory.clone();
        let saved = scratch.path().join("saved");
        fs::rename(&directory, &saved).unwrap();
        std::os::unix::fs::symlink(&saved, &directory).unwrap();
        assert!(cleanup(&prepared).is_err());
        assert!(saved.join("omakvn").exists());
    }

    #[test]
    fn changing_resource_pin_changes_transaction_manifest() {
        let scratch = tempfile::tempdir().unwrap();
        let migration = Migration {
            id: "100-example.sh".into(),
            path: scratch.path().join("100-example.sh"),
            summary: "Example".into(),
        };
        fs::write(&migration.path, b"#!/bin/bash\n").unwrap();
        let before = super::super::manifest(std::slice::from_ref(&migration)).unwrap();
        fs::write(manifest_path(&migration), b"{}").unwrap();
        let after = super::super::manifest(std::slice::from_ref(&migration)).unwrap();
        assert_ne!(before, after);
        fs::write(manifest_path(&migration), b"{\"git\":[]}").unwrap();
        assert_ne!(after, super::super::manifest(&[migration]).unwrap());
    }
}

use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};

pub fn detect_omarchy_theme() -> Option<String> {
    let current = omarchy_current_dir()?;
    let raw = std::fs::read_to_string(current.join("theme.name")).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub fn omarchy_current_dir() -> Option<PathBuf> {
    Some(state_home()?.join("omarchy").join("current"))
}

pub fn default_editor() -> Option<String> {
    let path = state_home()?
        .join("omarchy")
        .join("defaults")
        .join("editor");
    let raw = std::fs::read_to_string(path).ok()?;
    let editor = raw.lines().next()?.trim();
    (!editor.is_empty()).then(|| editor.to_string())
}

pub fn theme_colors_path() -> Option<PathBuf> {
    Some(omarchy_current_dir()?.join("theme").join("colors.toml"))
}

pub const OMAKVN_PLUGIN_ID: &str = "yarikov.omakvn";
pub const OMAKVN_MIN_VERSION: Version = Version::new(1, 1, 0);
pub const OMAKVN_UPDATE_COMMAND: &str = "omarchy plugin update yarikov.omakvn";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OmakvnState {
    #[default]
    Missing,
    Installed,
    Outdated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OmakvnStatus {
    pub state: OmakvnState,
    pub required_version: String,
}

impl Default for OmakvnStatus {
    fn default() -> Self {
        Self::with_state(OmakvnState::Missing)
    }
}

impl OmakvnStatus {
    fn with_state(state: OmakvnState) -> Self {
        Self {
            state,
            required_version: OMAKVN_MIN_VERSION.to_string(),
        }
    }

    pub fn is_installed(&self) -> bool {
        self.state != OmakvnState::Missing
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmakvnPlugin {
    pub version: Option<Version>,
}

impl OmakvnPlugin {
    pub fn state(&self) -> OmakvnState {
        match &self.version {
            Some(version) if *version >= OMAKVN_MIN_VERSION => OmakvnState::Installed,
            _ => OmakvnState::Outdated,
        }
    }
}

pub fn omakvn_status() -> OmakvnStatus {
    OmakvnStatus::with_state(
        installed_omakvn().map_or(OmakvnState::Missing, |plugin| plugin.state()),
    )
}

pub fn installed_omakvn() -> Option<OmakvnPlugin> {
    let manifest = dirs::config_dir()?
        .join("omarchy/plugins")
        .join(OMAKVN_PLUGIN_ID)
        .join("manifest.json");
    parse_omakvn_manifest(&std::fs::read_to_string(manifest).ok()?)
}

fn parse_omakvn_manifest(raw: &str) -> Option<OmakvnPlugin> {
    let manifest: serde_json::Value = serde_json::from_str(raw).ok()?;
    if manifest.get("id")?.as_str()? != OMAKVN_PLUGIN_ID {
        return None;
    }
    let version = manifest
        .get("version")
        .and_then(serde_json::Value::as_str)
        .and_then(|version| Version::parse(version.trim()).ok());
    Some(OmakvnPlugin { version })
}

pub fn installed_major_version() -> Option<u64> {
    let version = detect_omarchy_version().ok().flatten()?;
    Some(parse_omarchy_version(&version)?.major)
}

pub fn parse_omarchy_version(version: &str) -> Option<Version> {
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

pub fn detect_omarchy_version() -> Result<Option<String>> {
    let output = match Command::new("omarchy")
        .arg("version")
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).context("failed to detect the installed Omarchy version");
        }
    };
    ensure!(
        output.status.success(),
        "omarchy version failed; cannot determine the installed Omarchy version"
    );
    let version = String::from_utf8(output.stdout).context("invalid Omarchy version output")?;
    Ok(Some(version.trim().to_owned()))
}

fn state_home() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        let path = PathBuf::from(xdg);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    dirs::state_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_helpers::ENV_LOCK;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    struct EnvGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    fn isolate_xdg(config: &std::path::Path, state: &std::path::Path) -> (EnvGuard, EnvGuard) {
        (
            EnvGuard::set("XDG_CONFIG_HOME", config),
            EnvGuard::set("XDG_STATE_HOME", state),
        )
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

    #[test]
    fn installed_major_version_reads_the_omarchy_command() {
        let _lock = ENV_LOCK.lock().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let _path = crate::test_helpers::EnvVarGuard::set("PATH", scratch.path());
        assert_eq!(installed_major_version(), None);
        let command = scratch.path().join("omarchy");
        for (script, expected) in [
            ("#!/bin/sh\nprintf '4.2.0-1\\n'\n", Some(4)),
            ("#!/bin/sh\nprintf 'Omarchy 3.4.0\\n'\n", Some(3)),
            ("#!/bin/sh\nprintf 'unknown\\n'\n", None),
        ] {
            std::fs::write(&command, script).unwrap();
            std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(installed_major_version(), expected);
        }
    }

    #[test]
    fn default_editor_reads_the_first_line_of_the_omarchy_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let _env = isolate_xdg(config.path(), state.path());
        assert_eq!(default_editor(), None);

        let defaults = state.path().join("omarchy").join("defaults");
        fs::create_dir_all(&defaults).unwrap();
        fs::write(defaults.join("editor"), "  hx \nnvim\n").unwrap();
        assert_eq!(default_editor().as_deref(), Some("hx"));

        fs::write(defaults.join("editor"), "\n").unwrap();
        assert_eq!(default_editor(), None);
    }

    #[test]
    fn detect_returns_none_when_file_missing() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let _env = isolate_xdg(config.path(), state.path());
        assert!(detect_omarchy_theme().is_none());
    }

    #[test]
    fn detect_reads_state_theme_name() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let _env = isolate_xdg(config.path(), state.path());
        let current = state.path().join("omarchy").join("current");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("theme.name"), "  lupine  \n").unwrap();
        assert_eq!(detect_omarchy_theme().as_deref(), Some("lupine"));
        assert_eq!(omarchy_current_dir().as_deref(), Some(current.as_path()));
        assert_eq!(
            theme_colors_path(),
            Some(current.join("theme").join("colors.toml"))
        );
    }

    #[test]
    fn detect_ignores_legacy_config_theme_name() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let _env = isolate_xdg(config.path(), state.path());
        let legacy = config.path().join("omarchy").join("current");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("theme.name"), "gruvbox\n").unwrap();
        assert!(detect_omarchy_theme().is_none());
        assert_eq!(
            omarchy_current_dir().as_deref(),
            Some(state.path().join("omarchy").join("current").as_path())
        );
    }

    #[test]
    fn detect_treats_empty_file_as_missing() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let _env = isolate_xdg(config.path(), state.path());
        let current = state.path().join("omarchy").join("current");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("theme.name"), "   \n").unwrap();
        assert!(detect_omarchy_theme().is_none());
    }

    fn manifest(version: Option<&str>) -> String {
        match version {
            Some(version) => format!(r#"{{"id":"{OMAKVN_PLUGIN_ID}","version":"{version}"}}"#),
            None => format!(r#"{{"id":"{OMAKVN_PLUGIN_ID}"}}"#),
        }
    }

    #[test]
    fn the_omakvn_state_compares_the_manifest_version_with_the_required_one() {
        for (version, expected) in [
            (Some("1.1.0"), OmakvnState::Installed),
            (Some("9.0.0"), OmakvnState::Installed),
            (Some("1.0.1"), OmakvnState::Outdated),
            (Some("not a version"), OmakvnState::Outdated),
            (None, OmakvnState::Outdated),
        ] {
            let plugin = parse_omakvn_manifest(&manifest(version)).unwrap();

            assert_eq!(plugin.state(), expected, "{version:?}");
        }
    }

    #[test]
    fn a_manifest_of_another_plugin_or_not_json_is_not_omakvn() {
        for raw in [
            r#"{"id":"someone.else","version":"9.0.0"}"#,
            "{}",
            "not json",
        ] {
            assert_eq!(parse_omakvn_manifest(raw), None, "{raw}");
        }
    }

    #[test]
    fn the_omakvn_status_reads_the_installed_manifest() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let _env = isolate_xdg(config.path(), state.path());
        assert_eq!(omakvn_status(), OmakvnStatus::default());

        let plugin = config.path().join("omarchy/plugins").join(OMAKVN_PLUGIN_ID);
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(plugin.join("manifest.json"), manifest(Some("1.0.1"))).unwrap();

        assert_eq!(
            omakvn_status(),
            OmakvnStatus {
                state: OmakvnState::Outdated,
                required_version: "1.1.0".into(),
            }
        );
    }
}

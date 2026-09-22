//! Detection of an Omarchy installation: its active theme and its version.
//!
//! Kept as a neutral top-level module so the daemon-side config loader (which
//! sets the first-launch default theme), the TUI-side theme watcher (which
//! follows Omarchy's active theme at runtime), `doctor` and the migration
//! resource gate can share it without `config` having to depend on
//! `tui_client`.

use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};
use semver::Version;

/// Read the currently active Omarchy theme slug.
///
/// Omarchy stores runtime state below
/// `$XDG_STATE_HOME/omarchy/current/theme.name`. Returns `None` when that file
/// does not contain a theme name.
pub fn detect_omarchy_theme() -> Option<String> {
    let current = omarchy_current_dir()?;
    let raw = std::fs::read_to_string(current.join("theme.name")).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Path to the active Omarchy `current/` state directory.
pub fn omarchy_current_dir() -> Option<PathBuf> {
    Some(state_home()?.join("omarchy").join("current"))
}

/// Path to the semantic palette of the active Omarchy theme.
pub fn theme_colors_path() -> Option<PathBuf> {
    Some(omarchy_current_dir()?.join("theme").join("colors.toml"))
}

/// Major version of the installed Omarchy, or `None` when Omarchy is absent
/// or its version cannot be determined.
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
}

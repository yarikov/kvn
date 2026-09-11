use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

pub const INITIAL_DELAY_DAYS: i64 = 7;
pub const REMINDER_DELAY_DAYS: i64 = 30;
pub const SUPPORTED_DELAY_DAYS: i64 = 180;
pub const SUPPORT_URL: &str = "https://web.tribute.tg/d/QbU";

/// Small, non-config UX state. Keeping it outside profiles.json avoids making
/// an optional prompt part of the versioned VPN configuration schema.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportPromptState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_show_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub dismissed: bool,
}

impl SupportPromptState {
    pub fn schedule_initial(&mut self, now: DateTime<Utc>) -> bool {
        if self.dismissed || self.next_show_at.is_some() {
            return false;
        }
        self.next_show_at = Some(now + Duration::days(INITIAL_DELAY_DAYS));
        true
    }

    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        !self.dismissed && self.next_show_at.is_some_and(|deadline| now >= deadline)
    }

    pub fn remind_later(&mut self, now: DateTime<Utc>) {
        self.dismissed = false;
        self.next_show_at = Some(now + Duration::days(REMINDER_DELAY_DAYS));
    }

    pub fn supported(&mut self, now: DateTime<Utc>) {
        self.dismissed = false;
        self.next_show_at = Some(now + Duration::days(SUPPORTED_DELAY_DAYS));
    }

    pub fn dismiss(&mut self) {
        self.dismissed = true;
        self.next_show_at = None;
    }
}

pub fn load_at(path: &Path) -> Result<Option<SupportPromptState>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("Failed to read {path:?}")),
    };
    serde_json::from_str(&contents)
        .map(Some)
        .with_context(|| format!("Failed to parse {path:?}"))
}

pub fn save_at(path: &Path, state: &SupportPromptState) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("Support prompt path {path:?} has no parent"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create support prompt directory {parent:?}"))?;
    let json = serde_json::to_string_pretty(state)?;
    crate::atomic_write::write(path, json.as_bytes())
}

pub fn load_for_daemon(geo_configured: bool, now: DateTime<Utc>) -> SupportPromptState {
    let Some(path) = crate::paths::support_prompt_path() else {
        tracing::warn!("Failed to determine support prompt state path");
        return SupportPromptState::default();
    };
    let mut state = match load_at(&path) {
        Ok(Some(state)) => state,
        Ok(None) => SupportPromptState::default(),
        Err(error) => {
            tracing::warn!("Resetting invalid support prompt state: {error:#}");
            SupportPromptState::default()
        }
    };
    if geo_configured
        && state.schedule_initial(now)
        && let Err(error) = save_at(&path, &state)
    {
        tracing::warn!("Failed to persist support prompt schedule: {error:#}");
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 10, 12, 0, 0).unwrap()
    }

    #[test]
    fn initial_schedule_becomes_due_after_seven_days() {
        let now = now();
        let mut state = SupportPromptState::default();
        assert!(state.schedule_initial(now));
        assert!(!state.is_due(now + Duration::days(7) - Duration::seconds(1)));
        assert!(state.is_due(now + Duration::days(7)));
        assert!(!state.schedule_initial(now + Duration::days(1)));
    }

    #[test]
    fn reminder_moves_deadline_thirty_days() {
        let now = now();
        let mut state = SupportPromptState::default();
        state.remind_later(now);
        assert!(!state.is_due(now + Duration::days(30) - Duration::seconds(1)));
        assert!(state.is_due(now + Duration::days(30)));
    }

    #[test]
    fn support_moves_deadline_six_months() {
        let now = now();
        let mut state = SupportPromptState::default();
        state.supported(now);
        assert!(!state.is_due(now + Duration::days(180) - Duration::seconds(1)));
        assert!(state.is_due(now + Duration::days(180)));
    }

    #[test]
    fn dismissal_is_permanent() {
        let now = now();
        let mut state = SupportPromptState::default();
        state.schedule_initial(now);
        state.dismiss();
        assert!(!state.is_due(now + Duration::days(3650)));
        assert!(!state.schedule_initial(now));
    }

    #[test]
    fn state_roundtrips_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/support-prompt.json");
        let mut state = SupportPromptState::default();
        state.remind_later(now());
        save_at(&path, &state).unwrap();
        assert_eq!(load_at(&path).unwrap(), Some(state));
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn state_io_reports_unreadable_paths() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_at(dir.path()).is_err());

        let parent_file = dir.path().join("not-a-directory");
        fs::write(&parent_file, "file").unwrap();
        assert!(
            save_at(
                &parent_file.join("support-prompt.json"),
                &SupportPromptState::default()
            )
            .is_err()
        );
    }

    #[test]
    fn absent_state_is_not_scheduled_before_geo_setup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("support-prompt.json");
        let state = load_at(&path).unwrap().unwrap_or_default();
        assert_eq!(state, SupportPromptState::default());
    }

    #[test]
    fn daemon_initializes_schedule_only_after_geo_setup() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", dir.path());
        let path = crate::paths::support_prompt_path().unwrap();

        assert_eq!(load_for_daemon(false, now()), SupportPromptState::default());
        assert!(!path.exists());

        let state = load_for_daemon(true, now());
        assert_eq!(
            state.next_show_at,
            Some(now() + Duration::days(INITIAL_DELAY_DAYS))
        );
        assert_eq!(load_at(&path).unwrap(), Some(state));
    }

    #[test]
    fn daemon_replaces_corrupt_state_with_fresh_schedule() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", dir.path());
        let path = crate::paths::support_prompt_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "not json").unwrap();

        let state = load_for_daemon(true, now());
        assert_eq!(
            state.next_show_at,
            Some(now() + Duration::days(INITIAL_DELAY_DAYS))
        );
        assert_eq!(load_at(&path).unwrap(), Some(state));
    }

    #[test]
    fn daemon_preserves_existing_dismissal() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", dir.path());
        let path = crate::paths::support_prompt_path().unwrap();
        let mut dismissed = SupportPromptState::default();
        dismissed.dismiss();
        save_at(&path, &dismissed).unwrap();

        assert_eq!(load_for_daemon(true, now()), dismissed);
    }
}

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use jiff::{Timestamp, ToSpan, tz::TimeZone};
use serde::{Deserialize, Serialize};

pub const INITIAL_DELAY_DAYS: i64 = 14;
pub const REMINDER_DELAY_DAYS: i64 = 30;
pub const SUPPORT_URL: &str = "https://web.tribute.tg/d/QUv";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportPromptState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_show_at: Option<Timestamp>,
    #[serde(default)]
    pub dismissed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_at: Option<Timestamp>,
}

impl SupportPromptState {
    pub fn schedule_initial(&mut self, now: Timestamp) -> bool {
        if self.is_closed() || self.next_show_at.is_some() {
            return false;
        }
        self.next_show_at = Some(deadline_after_days(now, INITIAL_DELAY_DAYS));
        true
    }

    pub fn is_due(&self, now: Timestamp) -> bool {
        !self.is_closed() && self.next_show_at.is_some_and(|deadline| now >= deadline)
    }

    pub fn remind_later(&mut self, now: Timestamp) {
        self.dismissed = false;
        self.next_show_at = Some(deadline_after_days(now, REMINDER_DELAY_DAYS));
    }

    pub fn supported(&mut self, now: Timestamp) {
        self.supported_at = Some(now);
        self.next_show_at = None;
    }

    pub fn dismiss(&mut self) {
        self.dismissed = true;
        self.next_show_at = None;
    }

    fn is_closed(&self) -> bool {
        self.dismissed || self.supported_at.is_some()
    }
}

fn deadline_after_days(now: Timestamp, days: i64) -> Timestamp {
    now.to_zoned_datetime(TimeZone::UTC)
        .checked_add(days.days())
        .map(|deadline| deadline.timestamp())
        .unwrap_or(Timestamp::MAX)
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

pub fn dismiss_at(path: &Path) -> Result<()> {
    let mut state = load_at(path)
        .unwrap_or_else(|error| {
            tracing::warn!("Replacing invalid support prompt state: {error:#}");
            None
        })
        .unwrap_or_default();
    state.dismiss();
    save_at(path, &state)
}

// NOTE: this start-up path only recovers a daemon that died between the
// onboarding write and its support-prompt write. The live daemon arms the
// deadline from `daemon::config_io::persist_onboarding`, because a daemon can
// run for weeks without restarting.
pub fn load_for_daemon(onboarding_completed_at: Option<Timestamp>) -> SupportPromptState {
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
    if let Some(completed_at) = onboarding_completed_at
        && state.schedule_initial(completed_at)
        && let Err(error) = save_at(&path, &state)
    {
        tracing::warn!("Failed to persist support prompt schedule: {error:#}");
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::SignedDuration;

    fn now() -> Timestamp {
        "2026-09-10T12:00:00Z".parse().unwrap()
    }

    #[test]
    fn initial_schedule_becomes_due_after_fourteen_days() {
        let now = now();
        let deadline: Timestamp = "2026-09-24T12:00:00Z".parse().unwrap();
        let mut state = SupportPromptState::default();
        assert!(state.schedule_initial(now));
        assert!(!state.is_due(deadline - SignedDuration::from_secs(1)));
        assert!(state.is_due(deadline));
        assert!(!state.schedule_initial(deadline));
    }

    #[test]
    fn reminder_moves_deadline_thirty_days() {
        let now = "2026-03-20T12:00:00Z".parse().unwrap();
        let deadline: Timestamp = "2026-04-19T12:00:00Z".parse().unwrap();
        let mut state = SupportPromptState::default();
        state.remind_later(now);
        assert!(!state.is_due(deadline - SignedDuration::from_secs(1)));
        assert!(state.is_due(deadline));
    }

    #[test]
    fn support_is_recorded_and_permanent() {
        let now = now();
        let mut state = SupportPromptState::default();
        state.schedule_initial(now);
        state.supported(now);
        assert_eq!(state.supported_at, Some(now));
        assert!(!state.dismissed);
        assert!(!state.is_due("2036-09-10T12:00:00Z".parse().unwrap()));
        assert!(!state.schedule_initial(now));
    }

    #[test]
    fn dismissal_is_permanent() {
        let now = now();
        let mut state = SupportPromptState::default();
        state.schedule_initial(now);
        state.dismiss();
        assert!(!state.is_due("2036-09-10T12:00:00Z".parse().unwrap()));
        assert!(!state.schedule_initial(now));
    }

    #[test]
    fn state_roundtrips_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("support-prompt.json");
        fs::write(
            &path,
            r#"{"next_show_at":"2026-10-10T12:00:00.123Z","dismissed":false}"#,
        )
        .unwrap();
        let state = load_at(&path).unwrap().unwrap();
        assert_eq!(state.next_show_at, "2026-10-10T12:00:00.123Z".parse().ok());
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
    fn dismiss_at_keeps_support_record_and_replaces_corrupt_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/support-prompt.json");
        let mut supported = SupportPromptState::default();
        supported.supported(now());
        save_at(&path, &supported).unwrap();

        dismiss_at(&path).unwrap();
        let state = load_at(&path).unwrap().unwrap();
        assert!(state.dismissed);
        assert_eq!(state.supported_at, Some(now()));

        fs::write(&path, "not json").unwrap();
        dismiss_at(&path).unwrap();
        assert!(load_at(&path).unwrap().unwrap().dismissed);
    }

    #[test]
    fn absent_state_is_not_scheduled_before_onboarding_completes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("support-prompt.json");
        let state = load_at(&path).unwrap().unwrap_or_default();
        assert_eq!(state, SupportPromptState::default());
    }

    #[test]
    fn daemon_counts_the_delay_from_onboarding_completion() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", dir.path());
        let path = crate::paths::support_prompt_path().unwrap();

        assert_eq!(load_for_daemon(None), SupportPromptState::default());
        assert!(!path.exists());

        let completed_at = "2026-09-07T12:00:00Z".parse().unwrap();
        let state = load_for_daemon(Some(completed_at));
        assert_eq!(
            state.next_show_at,
            Some("2026-09-21T12:00:00Z".parse().unwrap())
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

        let state = load_for_daemon(Some(now()));
        assert_eq!(
            state.next_show_at,
            Some("2026-09-24T12:00:00Z".parse().unwrap())
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

        assert_eq!(load_for_daemon(Some(now())), dismissed);
    }
}

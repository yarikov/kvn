use std::time::{Duration, Instant};

use crate::app::model::AppStatus;

const TOAST_INFO_DURATION: Duration = Duration::from_secs(3);
const TOAST_ERROR_DURATION: Duration = Duration::from_secs(7);

/// Presentation-only lifetime for daemon status events. Keeping the deadline
/// here avoids leaking wall-clock concerns into the shared TEA model.
pub(super) struct ToastState {
    last_revision: u64,
    status: Option<AppStatus>,
    expires_at: Option<Instant>,
}

impl ToastState {
    pub(super) fn new(last_revision: u64) -> Self {
        Self {
            last_revision,
            status: None,
            expires_at: None,
        }
    }

    pub(super) fn show_initial_error(&mut self, status: Option<AppStatus>, now: Instant) -> bool {
        if matches!(status, Some(AppStatus::Error(_))) {
            self.status = status;
            self.expires_at = Some(now + TOAST_ERROR_DURATION);
            return true;
        }
        false
    }

    pub(super) fn show_info(&mut self, message: impl Into<String>, now: Instant) {
        self.status = Some(AppStatus::Info(message.into()));
        self.expires_at = Some(now + TOAST_INFO_DURATION);
    }

    pub(super) fn observe(
        &mut self,
        revision: u64,
        status: Option<AppStatus>,
        now: Instant,
    ) -> Option<u64> {
        if revision == self.last_revision {
            return None;
        }
        self.last_revision = revision;
        let Some(status) = status else {
            self.status = None;
            self.expires_at = None;
            return None;
        };
        let error_revision = matches!(status, AppStatus::Error(_)).then_some(revision);
        let duration = if matches!(status, AppStatus::Error(_)) {
            TOAST_ERROR_DURATION
        } else {
            TOAST_INFO_DURATION
        };
        self.status = Some(status);
        self.expires_at = Some(now + duration);
        error_revision
    }

    pub(super) fn expire(&mut self, now: Instant) {
        if self.expires_at.is_some_and(|deadline| now >= deadline) {
            self.status = None;
            self.expires_at = None;
        }
    }

    pub(super) fn current(&self) -> Option<&AppStatus> {
        self.status.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::super::key::DEPRECATED_PANE_FOCUS_MESSAGE;
    use super::*;

    #[test]
    fn toast_state_treats_repeated_text_as_a_new_revision() {
        let start = Instant::now();
        let mut toast = ToastState::new(4);
        assert_eq!(
            toast.observe(5, Some(AppStatus::Info("Saved".into())), start),
            None
        );
        let first_deadline = toast.expires_at.unwrap();
        toast.observe(
            6,
            Some(AppStatus::Info("Saved".into())),
            start + Duration::from_secs(1),
        );
        assert_eq!(toast.current().map(AppStatus::text), Some("Saved"));
        assert!(toast.expires_at.unwrap() > first_deadline);
    }

    #[test]
    fn local_info_toast_restarts_its_lifetime_without_changing_daemon_revision() {
        let start = Instant::now();
        let mut toast = ToastState::new(4);

        toast.show_info(DEPRECATED_PANE_FOCUS_MESSAGE, start);
        let first_deadline = toast.expires_at.unwrap();
        toast.show_info(
            DEPRECATED_PANE_FOCUS_MESSAGE,
            start + Duration::from_secs(1),
        );
        assert_eq!(
            toast.observe(
                4,
                Some(AppStatus::Info("unchanged daemon status".into())),
                start + Duration::from_secs(2),
            ),
            None
        );

        assert_eq!(toast.last_revision, 4);
        assert_eq!(
            toast.current().map(AppStatus::text),
            Some(DEPRECATED_PANE_FOCUS_MESSAGE)
        );
        assert!(toast.expires_at.unwrap() > first_deadline);
    }

    #[test]
    fn toast_state_ignores_duplicate_snapshots_and_expires() {
        let start = Instant::now();
        let mut toast = ToastState::new(2);
        assert_eq!(
            toast.observe(3, Some(AppStatus::Error("Failed".into())), start),
            Some(3)
        );
        let deadline = toast.expires_at.unwrap();
        assert_eq!(
            toast.observe(
                3,
                Some(AppStatus::Info("stale".into())),
                start + Duration::from_secs(1),
            ),
            None
        );
        assert_eq!(toast.current().map(AppStatus::text), Some("Failed"));
        toast.expire(deadline);
        assert!(toast.current().is_none());
    }

    #[test]
    fn toast_state_suppresses_empty_messages() {
        let start = Instant::now();
        let mut toast = ToastState::new(0);
        toast.observe(1, None, start);
        assert!(toast.current().is_none());
    }

    #[test]
    fn toast_state_initializes_only_from_errors() {
        let start = Instant::now();
        let mut toast = ToastState::new(2);

        assert!(!toast.show_initial_error(Some(AppStatus::Info("Connected".into())), start));
        assert!(toast.current().is_none());

        assert!(toast.show_initial_error(Some(AppStatus::Error("Startup failed".into())), start));
        assert_eq!(toast.current().map(AppStatus::text), Some("Startup failed"));

        toast.observe(3, Some(AppStatus::Info("Recovered".into())), start);
        assert_eq!(toast.current().map(AppStatus::text), Some("Recovered"));
    }
}

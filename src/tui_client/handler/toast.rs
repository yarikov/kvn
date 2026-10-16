use std::time::{Duration, Instant};

use crate::app::model::{AppStatus, StatusEvent, StatusKey};

const TOAST_INFO_DURATION: Duration = Duration::from_secs(3);
const TOAST_ERROR_DURATION: Duration = Duration::from_secs(7);
const MAX_TOASTS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToastKey {
    Daemon(StatusKey),
    Editor,
    Scroll,
}

struct Toast {
    key: ToastKey,
    status: AppStatus,
    expires_at: Instant,
}

/// Presentation-only lifetime for daemon status events. Keeping the deadline
/// here avoids leaking wall-clock concerns into the shared TEA model.
pub(super) struct ToastState {
    last_revision: u64,
    newest_first: Vec<Toast>,
}

impl ToastState {
    pub(super) fn new(last_revision: u64) -> Self {
        Self {
            last_revision,
            newest_first: Vec::new(),
        }
    }

    pub(super) fn reattach(&mut self, revision: u64) {
        *self = Self::new(revision);
    }

    pub(super) fn show_info(&mut self, key: ToastKey, message: impl Into<String>, now: Instant) {
        self.show(key, AppStatus::Info(message.into()), now);
    }

    pub(super) fn observe(
        &mut self,
        revision: u64,
        events: &[StatusEvent],
        now: Instant,
    ) -> Option<u64> {
        if revision == self.last_revision {
            return None;
        }
        let seen = std::mem::replace(&mut self.last_revision, revision);
        let mut newest = None;
        for event in events.iter().filter(|event| event.revision > seen) {
            self.show(ToastKey::Daemon(event.key), event.status.clone(), now);
            newest = Some(event);
        }
        let newest = newest?;
        matches!(newest.status, AppStatus::Error(_)).then_some(newest.revision)
    }

    fn show(&mut self, key: ToastKey, status: AppStatus, now: Instant) {
        let expires_at = now + lifetime(&status);
        if let Some(toast) = self.newest_first.iter_mut().find(|toast| toast.key == key) {
            toast.status = status;
            toast.expires_at = expires_at;
            return;
        }
        if self.newest_first.len() == MAX_TOASTS {
            self.drop_oldest();
        }
        self.newest_first.insert(
            0,
            Toast {
                key,
                status,
                expires_at,
            },
        );
    }

    fn drop_oldest(&mut self) {
        let oldest_info = self
            .newest_first
            .iter()
            .rposition(|toast| matches!(toast.status, AppStatus::Info(_)));
        let index = oldest_info.unwrap_or(self.newest_first.len() - 1);
        self.newest_first.remove(index);
    }

    pub(super) fn expire(&mut self, now: Instant) {
        self.newest_first.retain(|toast| now < toast.expires_at);
    }

    pub(super) fn statuses(&self) -> Vec<&AppStatus> {
        self.newest_first
            .iter()
            .map(|toast| &toast.status)
            .collect()
    }
}

fn lifetime(status: &AppStatus) -> Duration {
    match status {
        AppStatus::Error(_) => TOAST_ERROR_DURATION,
        AppStatus::Info(_) => TOAST_INFO_DURATION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(revision: u64, key: StatusKey, status: AppStatus) -> StatusEvent {
        StatusEvent {
            revision,
            key,
            status,
        }
    }

    fn info(revision: u64, text: &str) -> StatusEvent {
        event(
            revision,
            StatusKey::Connection,
            AppStatus::Info(text.into()),
        )
    }

    fn error(revision: u64, text: &str) -> StatusEvent {
        event(
            revision,
            StatusKey::Connection,
            AppStatus::Error(text.into()),
        )
    }

    fn texts(toast: &ToastState) -> Vec<&str> {
        toast.statuses().into_iter().map(AppStatus::text).collect()
    }

    #[test]
    fn toast_state_treats_repeated_text_as_a_new_revision() {
        let start = Instant::now();
        let mut toast = ToastState::new(4);
        assert_eq!(toast.observe(5, &[info(5, "Saved")], start), None);
        let first_deadline = toast.newest_first[0].expires_at;
        toast.observe(6, &[info(6, "Saved")], start + Duration::from_secs(1));
        assert_eq!(texts(&toast), ["Saved"]);
        assert!(toast.newest_first[0].expires_at > first_deadline);
    }

    #[test]
    fn local_info_toast_restarts_its_lifetime_without_changing_daemon_revision() {
        let start = Instant::now();
        let mut toast = ToastState::new(4);

        toast.show_info(ToastKey::Editor, "Copied", start);
        let first_deadline = toast.newest_first[0].expires_at;
        toast.show_info(ToastKey::Editor, "Copied", start + Duration::from_secs(1));
        assert_eq!(
            toast.observe(
                4,
                &[info(4, "unchanged daemon status")],
                start + Duration::from_secs(2),
            ),
            None
        );

        assert_eq!(toast.last_revision, 4);
        assert_eq!(texts(&toast), ["Copied"]);
        assert!(toast.newest_first[0].expires_at > first_deadline);
    }

    #[test]
    fn toast_state_ignores_duplicate_snapshots_and_expires() {
        let start = Instant::now();
        let mut toast = ToastState::new(2);
        assert_eq!(toast.observe(3, &[error(3, "Failed")], start), Some(3));
        let deadline = toast.newest_first[0].expires_at;
        assert_eq!(
            toast.observe(3, &[info(3, "stale")], start + Duration::from_secs(1)),
            None
        );
        assert_eq!(texts(&toast), ["Failed"]);
        toast.expire(deadline);
        assert!(toast.statuses().is_empty());
    }

    #[test]
    fn toast_state_stacks_unseen_events_newest_first() {
        let start = Instant::now();
        let mut toast = ToastState::new(2);
        let events = [
            info(1, "seen"),
            event(
                4,
                StatusKey::Geo,
                AppStatus::Error("Geo update failed".into()),
            ),
            info(5, "Connected"),
        ];

        assert_eq!(toast.observe(5, &events, start), None);
        assert_eq!(texts(&toast), ["Connected", "Geo update failed"]);
    }

    #[test]
    fn toast_with_the_same_key_is_replaced_in_place() {
        let start = Instant::now();
        let mut toast = ToastState::new(0);
        toast.observe(1, &[info(1, "Connecting…")], start);
        toast.observe(
            2,
            &[event(
                2,
                StatusKey::Dns,
                AppStatus::Info("DNS settings changed".into()),
            )],
            start,
        );

        toast.observe(3, &[info(3, "Connected")], start);

        assert_eq!(texts(&toast), ["DNS settings changed", "Connected"]);
    }

    #[test]
    fn toast_state_keeps_its_toasts_without_unseen_events() {
        let start = Instant::now();
        let mut toast = ToastState::new(0);
        toast.observe(1, &[info(1, "Connected")], start);

        assert_eq!(toast.observe(2, &[info(1, "Connected")], start), None);
        assert_eq!(texts(&toast), ["Connected"]);
    }

    #[test]
    fn each_toast_expires_on_its_own_deadline() {
        let start = Instant::now();
        let mut toast = ToastState::new(0);
        toast.observe(1, &[error(1, "Connection failed")], start);
        toast.show_info(ToastKey::Scroll, "Scrolling timed out", start);

        toast.expire(start + TOAST_INFO_DURATION);

        assert_eq!(texts(&toast), ["Connection failed"]);
    }

    #[test]
    fn a_full_stack_drops_its_oldest_info_before_any_error() {
        let start = Instant::now();
        let mut toast = ToastState::new(0);
        let events = [
            event(
                1,
                StatusKey::Geo,
                AppStatus::Error("Geo update failed".into()),
            ),
            event(
                2,
                StatusKey::Dns,
                AppStatus::Info("DNS settings changed".into()),
            ),
            event(
                3,
                StatusKey::Routing,
                AppStatus::Info("Routing mode changed".into()),
            ),
            event(4, StatusKey::Config, AppStatus::Error("Save failed".into())),
            info(5, "Connected"),
        ];

        toast.observe(5, &events, start);

        assert_eq!(
            texts(&toast),
            [
                "Connected",
                "Save failed",
                "Routing mode changed",
                "Geo update failed"
            ]
        );
    }

    #[test]
    fn a_full_stack_of_errors_drops_the_oldest() {
        let start = Instant::now();
        let mut toast = ToastState::new(0);
        let events: Vec<_> = (1..=5)
            .map(|revision| {
                event(
                    revision,
                    StatusKey::ConnectionWarning(revision as usize),
                    AppStatus::Error(format!("warning {revision}")),
                )
            })
            .collect();

        toast.observe(5, &events, start);

        assert_eq!(
            texts(&toast),
            ["warning 5", "warning 4", "warning 3", "warning 2"]
        );
    }

    #[test]
    fn only_the_newest_unseen_error_is_cleared() {
        let start = Instant::now();
        let mut toast = ToastState::new(0);
        let events = [
            error(1, "Connection failed"),
            event(
                2,
                StatusKey::Geo,
                AppStatus::Info("Checking geo databases".into()),
            ),
        ];

        assert_eq!(toast.observe(2, &events, start), None);
    }

    #[test]
    fn reattached_daemon_statuses_are_shown_even_when_revisions_repeat() {
        let start = Instant::now();
        let mut toast = ToastState::new(0);
        toast.observe(5, &[info(5, "Old daemon")], start);

        toast.reattach(1);
        assert!(toast.statuses().is_empty());
        assert_eq!(toast.observe(5, &[error(5, "New failure")], start), Some(5));
        assert_eq!(texts(&toast), ["New failure"]);
    }
}

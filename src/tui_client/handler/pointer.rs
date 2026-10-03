use std::io::{self, Write};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::model::{MainPaneFocus, Model};
use crate::tui_client::{OSC_POINTER_DEFAULT, OSC_POINTER_INTERACTIVE, OSC_POINTER_TEXT};

const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(300);

#[derive(Default)]
pub(super) struct ClickTracker(Option<(uuid::Uuid, Instant)>);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PointerShape {
    #[default]
    Default,
    Source,
    Logs,
}

impl ClickTracker {
    pub(super) fn profile_pressed(&mut self, profile_id: uuid::Uuid, now: Instant) -> bool {
        let double = self.0.is_some_and(|(previous, at)| {
            previous == profile_id && now.saturating_duration_since(at) <= DOUBLE_CLICK_INTERVAL
        });
        self.0 = (!double).then_some((profile_id, now));
        double
    }

    pub(super) fn reset(&mut self) {
        self.0 = None;
    }
}

pub(super) fn update_pointer_shape(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    model: &Model,
    position: Option<(u16, u16)>,
    pointer_shape: &mut PointerShape,
) -> Result<Option<usize>> {
    let area: ratatui::layout::Rect = terminal.size()?.into();
    let hit = if let Some((column, row)) = position {
        crate::ui::layout::source_hit_test(model, area, column, row)
    } else {
        None
    };
    let next_shape = if hit.is_some() {
        PointerShape::Source
    } else if position.is_some_and(|(column, row)| {
        crate::ui::layout::log_viewport(model, area)
            .is_some_and(|viewport| viewport.contains(column, row))
    }) {
        PointerShape::Logs
    } else {
        PointerShape::Default
    };
    if next_shape != *pointer_shape {
        let sequence = match next_shape {
            PointerShape::Default => OSC_POINTER_DEFAULT,
            PointerShape::Source => OSC_POINTER_INTERACTIVE,
            PointerShape::Logs => OSC_POINTER_TEXT,
        };
        terminal.backend_mut().write_all(sequence.as_bytes())?;
        terminal.backend_mut().flush()?;
        *pointer_shape = next_shape;
    }
    Ok(hit)
}

pub(super) struct PendingFocus {
    id: uuid::Uuid,
    focus: MainPaneFocus,
    sent_at: Instant,
}

impl PendingFocus {
    pub(super) fn new(id: uuid::Uuid, focus: MainPaneFocus, sent_at: Instant) -> Self {
        Self { id, focus, sent_at }
    }
}

pub(super) fn expire_pane_focus(pending: &mut Option<PendingFocus>, now: Instant) -> bool {
    if pending.as_ref().is_some_and(|pending| {
        now.saturating_duration_since(pending.sent_at) >= super::IPC_INTERACTION_TIMEOUT
    }) {
        *pending = None;
        true
    } else {
        false
    }
}

pub(super) fn reconcile_pane_focus(
    pending: &mut Option<PendingFocus>,
    response_to: Option<uuid::Uuid>,
    snapshot_focus: MainPaneFocus,
    now: Instant,
) -> MainPaneFocus {
    expire_pane_focus(pending, now);
    if pending
        .as_ref()
        .is_some_and(|pending| Some(pending.id) == response_to)
    {
        *pending = None;
    }
    pending
        .as_ref()
        .map_or(snapshot_focus, |pending| pending.focus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_keeps_the_latest_request_until_its_own_acknowledgment() {
        let previous = uuid::Uuid::new_v4();
        let latest = uuid::Uuid::new_v4();
        let now = Instant::now();
        let mut pending = Some(PendingFocus::new(latest, MainPaneFocus::Sources, now));
        assert_eq!(
            reconcile_pane_focus(&mut pending, Some(previous), MainPaneFocus::Logs, now),
            MainPaneFocus::Sources
        );
        assert!(pending.is_some());
        assert_eq!(
            reconcile_pane_focus(&mut pending, Some(latest), MainPaneFocus::Sources, now),
            MainPaneFocus::Sources
        );
        assert!(pending.is_none());
        assert_eq!(
            reconcile_pane_focus(&mut pending, None, MainPaneFocus::Logs, now),
            MainPaneFocus::Logs
        );
    }

    #[test]
    fn missing_focus_reply_stops_overriding_snapshots_after_timeout() {
        let now = Instant::now();
        let id = uuid::Uuid::new_v4();
        let mut pending = Some(PendingFocus::new(id, MainPaneFocus::Logs, now));
        assert!(!expire_pane_focus(
            &mut pending,
            now + super::super::IPC_INTERACTION_TIMEOUT - Duration::from_millis(1)
        ));
        assert_eq!(
            reconcile_pane_focus(
                &mut pending,
                None,
                MainPaneFocus::Sources,
                now + super::super::IPC_INTERACTION_TIMEOUT
            ),
            MainPaneFocus::Sources
        );
        assert!(pending.is_none());
        let next_id = uuid::Uuid::new_v4();
        let later = now + super::super::IPC_INTERACTION_TIMEOUT;
        pending = Some(PendingFocus::new(next_id, MainPaneFocus::Logs, later));
        assert_eq!(
            reconcile_pane_focus(&mut pending, Some(id), MainPaneFocus::Sources, later),
            MainPaneFocus::Logs
        );
        assert!(pending.is_some());
    }

    #[test]
    fn click_tracker_requires_same_profile_within_300ms() {
        let first = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();
        let start = Instant::now();

        let mut tracker = ClickTracker::default();
        assert!(!tracker.profile_pressed(first, start));
        assert!(tracker.profile_pressed(first, start + Duration::from_millis(300)));

        assert!(!tracker.profile_pressed(first, start));
        assert!(!tracker.profile_pressed(other, start + Duration::from_millis(100)));
        assert!(!tracker.profile_pressed(other, start + Duration::from_millis(401)));
    }

    #[test]
    fn pointer_shape_sequences_use_osc_22() {
        assert_eq!(OSC_POINTER_INTERACTIVE, "\x1b]22;pointer\x1b\\");
        assert_eq!(OSC_POINTER_TEXT, "\x1b]22;text\x1b\\");
        assert_eq!(OSC_POINTER_DEFAULT, "\x1b]22;\x1b\\");
    }
}

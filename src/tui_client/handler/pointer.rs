use std::io::{self, Write};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::model::Model;
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

#[cfg(test)]
mod tests {
    use super::*;

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

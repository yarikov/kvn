use ratatui::layout::Rect;
use std::time::{Duration, Instant};

use crate::app::model::Model;
use crate::ui::layout::text::visual_width;

use super::{LogDisplayRow, format_log_for_display, text_between_columns};

const LOG_CURSOR_TIMEOUT: Duration = Duration::from_secs(15);

/// TUI-client-local keyboard state for the log pane. Log contents are local
/// to the client as well, so none of this belongs in the daemon snapshot.
#[derive(Debug, Clone, Default)]
pub(crate) struct LogNavigation {
    pub(super) cursor: Option<usize>,
    anchor: Option<usize>,
    pub(super) scroll_top_log: Option<usize>,
    last_activity: Option<Instant>,
}

impl LogNavigation {
    pub(crate) fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    pub(crate) fn is_visual(&self) -> bool {
        self.anchor.is_some()
    }

    pub(crate) fn select_edge(&mut self, viewport: &LogViewport, from_top: bool, now: Instant) {
        let cursor = if from_top {
            viewport.first_log_index()
        } else {
            viewport.last_log_index()
        };
        if let Some(cursor) = cursor {
            self.cursor = Some(cursor);
            self.scroll_top_log = viewport.first_log_index();
            self.last_activity = Some(now);
        }
    }

    pub(crate) fn select_buffer_edge(&mut self, log_count: usize, from_top: bool, now: Instant) {
        if log_count == 0 {
            return;
        }
        let cursor = if from_top { 0 } else { log_count - 1 };
        self.cursor = Some(cursor);
        self.scroll_top_log = Some(cursor);
        self.last_activity = Some(now);
    }

    pub(crate) fn move_by(&mut self, delta: isize, log_count: usize, now: Instant) {
        let Some(cursor) = self.cursor else {
            return;
        };
        if log_count == 0 {
            self.clear();
            return;
        }
        self.cursor = Some(cursor.saturating_add_signed(delta).min(log_count - 1));
        self.last_activity = Some(now);
    }

    pub(crate) fn enter_visual(&mut self, now: Instant) {
        if let Some(cursor) = self.cursor {
            self.anchor = Some(cursor);
            self.last_activity = Some(now);
        }
    }

    pub(crate) fn cancel_visual(&mut self) {
        self.anchor = None;
    }

    pub(crate) fn selected_range(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let cursor = self.cursor?;
        let anchor = self.anchor.unwrap_or(cursor);
        Some(anchor.min(cursor)..=anchor.max(cursor))
    }

    pub(crate) fn selected_text(&self, model: &Model) -> Option<(String, usize)> {
        let range = self.selected_range()?;
        let lines = range
            .clone()
            .filter_map(|index| model.logs.get(index))
            .map(|line| format_log_for_display(line).text)
            .collect::<Vec<_>>();
        (!lines.is_empty()).then(|| (lines.join("\n"), lines.len()))
    }

    pub(crate) fn copied(&mut self, now: Instant) {
        self.anchor = None;
        self.last_activity = Some(now);
    }

    pub(crate) fn expire_if_idle(&mut self, now: Instant) -> bool {
        if self
            .last_activity
            .is_some_and(|last| now.saturating_duration_since(last) >= LOG_CURSOR_TIMEOUT)
        {
            self.clear();
            true
        } else {
            false
        }
    }

    pub(crate) fn oldest_log_evicted(&mut self) {
        if self.cursor == Some(0) {
            self.clear();
            return;
        }
        self.cursor = self.cursor.map(|index| index - 1);
        self.anchor = self.anchor.and_then(|index| index.checked_sub(1));
        self.scroll_top_log = self
            .scroll_top_log
            .and_then(|index| index.checked_sub(1))
            .or(Some(0));
    }

    pub(crate) fn clear(&mut self) {
        self.cursor = None;
        self.anchor = None;
        self.scroll_top_log = None;
        self.last_activity = None;
    }

    pub(super) fn contains(&self, log_index: usize) -> bool {
        self.selected_range()
            .is_some_and(|range| range.contains(&log_index))
    }

    pub(crate) fn set_scroll_top_from(&mut self, viewport: &LogViewport) {
        if self.cursor.is_some() {
            self.scroll_top_log = viewport.first_log_index();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct LogPoint {
    row: usize,
    column: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LogViewport {
    pub(in crate::ui::layout) area: Rect,
    pub(in crate::ui::layout) rows: Vec<LogDisplayRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LogSelection {
    pub(in crate::ui::layout) viewport: LogViewport,
    anchor: LogPoint,
    focus: LogPoint,
}

impl LogSelection {
    pub(crate) fn start(viewport: LogViewport, column: u16, row: u16) -> Option<Self> {
        let point = viewport.point_at(column, row)?;
        Some(Self {
            viewport,
            anchor: point,
            focus: point,
        })
    }

    pub(crate) fn update(&mut self, column: u16, row: u16) {
        self.focus = self.viewport.clamped_point(column, row);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }

    pub(crate) fn text(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let (start, end) = self.bounds();
        let mut output = String::new();
        for row_index in start.row..=end.row {
            let row = &self.viewport.rows[row_index];
            let from = if row_index == start.row {
                start.column
            } else {
                0
            };
            let to = if row_index == end.row {
                end.column
            } else {
                u16::MAX
            };
            output.push_str(&text_between_columns(&row.text, from, to));
            if row_index < end.row && row.hard_break_after {
                output.push('\n');
            }
        }
        output
    }

    fn bounds(&self) -> (LogPoint, LogPoint) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    pub(super) fn contains(&self, row: usize, column: u16, width: u16) -> bool {
        if self.is_empty() {
            return false;
        }
        let (start, end) = self.bounds();
        let point_end = column.saturating_add(width.saturating_sub(1));
        (row > start.row || (row == start.row && point_end >= start.column))
            && (row < end.row || (row == end.row && column <= end.column))
    }
}

impl LogViewport {
    pub(crate) fn contains(&self, column: u16, row: u16) -> bool {
        column >= self.area.x
            && column < self.area.x.saturating_add(self.area.width)
            && row >= self.area.y
            && row < self.area.y.saturating_add(self.area.height)
    }

    fn point_at(&self, column: u16, row: u16) -> Option<LogPoint> {
        if self.rows.is_empty()
            || !self.contains(column, row)
            || row.saturating_sub(self.area.y) as usize >= self.rows.len()
        {
            return None;
        }
        Some(self.clamped_point(column, row))
    }

    fn clamped_point(&self, column: u16, row: u16) -> LogPoint {
        let max_row = self.rows.len().saturating_sub(1);
        let local_row = row.saturating_sub(self.area.y) as usize;
        let row = local_row.min(max_row);
        let max_column = visual_width(&self.rows[row].text).saturating_sub(1) as u16;
        LogPoint {
            row,
            column: column.saturating_sub(self.area.x).min(max_column),
        }
    }

    fn first_log_index(&self) -> Option<usize> {
        self.rows.first().map(|row| row.log_index)
    }

    fn last_log_index(&self) -> Option<usize> {
        self.rows.last().map(|row| row.log_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::Overlay;
    use crate::test_helpers::{buffer_to_string, model_with_subscription};
    use crate::ui::layout::log::build_log_viewport;
    use crate::ui::layout::panes::log_viewport;
    use crate::ui::layout::{draw, draw_with_log_selection};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn log_navigation_copies_the_display_format() {
        let mut model = model_with_subscription();
        model.push_log(
            "[sb] 09:25:39 INFO [2083199607 81ms] dns: exchanged OPT OPT PSEUDOSECTION: EDNS: version 0 flags: udp: 1232".into(),
        );
        let now = Instant::now();
        let mut navigation = LogNavigation::default();
        navigation.select_buffer_edge(model.logs.len(), true, now);

        assert_eq!(
            navigation.selected_text(&model),
            Some((
                "09:25:39 [sbx] INFO dns: exchanged OPT OPT PSEUDOSECTION: EDNS: version 0 flags: udp: 1232".into(),
                1,
            ))
        );
    }

    #[test]
    fn log_viewport_hit_test_excludes_border_and_overlay() {
        let mut model = model_with_subscription();
        model.push_log("hello".into());
        let area = Rect::new(0, 0, 90, 20);
        let viewport = log_viewport(&model, area).unwrap();
        assert!(viewport.contains(47, 4));
        assert!(!viewport.contains(46, 4));
        assert!(!viewport.contains(47, 3));
        model.overlay = Overlay::Help(crate::app::model::HelpState::default());
        assert!(log_viewport(&model, area).is_none());
    }

    #[test]
    fn log_navigation_starts_j_at_top_and_k_at_bottom_of_visible_logs() {
        let mut model = model_with_subscription();
        for index in 0..12 {
            model.push_log(format!("line {index}"));
        }
        let area = Rect::new(0, 0, 90, 15); // nine log content rows
        let viewport = log_viewport(&model, area).unwrap();
        assert_eq!(viewport.first_log_index(), Some(3));
        assert_eq!(viewport.last_log_index(), Some(11));

        let now = Instant::now();
        let mut down = LogNavigation::default();
        down.select_edge(&viewport, true, now);
        assert_eq!(down.cursor(), Some(3));

        let mut up = LogNavigation::default();
        up.select_edge(&viewport, false, now);
        assert_eq!(up.cursor(), Some(11));
    }

    #[test]
    fn log_navigation_moves_by_whole_wrapped_records() {
        let mut model = model_with_subscription();
        model.push_log("abcdefgh".into());
        model.push_log("next".into());
        let viewport = build_log_viewport(&model, Rect::new(0, 0, 4, 3), None);
        assert_eq!(viewport.rows.len(), 3);
        assert_eq!(viewport.rows[0].log_index, 0);
        assert_eq!(viewport.rows[1].log_index, 0);

        let now = Instant::now();
        let mut navigation = LogNavigation::default();
        navigation.select_edge(&viewport, true, now);
        assert_eq!(navigation.cursor(), Some(0));
        navigation.move_by(1, model.logs.len(), now);
        assert_eq!(navigation.cursor(), Some(1));
        assert_eq!(navigation.selected_text(&model), Some(("next".into(), 1)));
    }

    #[test]
    fn log_visual_selection_copies_complete_records_in_both_directions() {
        let mut model = model_with_subscription();
        for line in ["first", "second", "third"] {
            model.push_log(line.into());
        }
        let viewport = build_log_viewport(&model, Rect::new(0, 0, 20, 3), None);
        let now = Instant::now();
        let mut navigation = LogNavigation::default();
        navigation.select_edge(&viewport, false, now);
        navigation.enter_visual(now);
        navigation.move_by(-2, model.logs.len(), now);
        assert_eq!(
            navigation.selected_text(&model),
            Some(("first\nsecond\nthird".into(), 3))
        );

        navigation.copied(now);
        assert!(!navigation.is_visual());
        assert_eq!(navigation.cursor(), Some(0));
    }

    #[test]
    fn log_navigation_expires_after_fifteen_seconds() {
        let mut model = model_with_subscription();
        model.push_log("line".into());
        let viewport = build_log_viewport(&model, Rect::new(0, 0, 20, 1), None);
        let start = Instant::now();
        let mut navigation = LogNavigation::default();
        navigation.select_edge(&viewport, true, start);
        navigation.enter_visual(start);

        assert!(!navigation.expire_if_idle(start + LOG_CURSOR_TIMEOUT - Duration::from_millis(1)));
        assert!(navigation.is_visual());
        assert!(navigation.expire_if_idle(start + LOG_CURSOR_TIMEOUT));
        assert_eq!(navigation.cursor(), None);
        assert!(!navigation.is_visual());
    }

    #[test]
    fn log_buffer_edge_jumps_scroll_to_full_buffer_bounds() {
        let mut model = model_with_subscription();
        for index in 0..6 {
            model.push_log(format!("line {index}"));
        }
        let now = Instant::now();
        let mut navigation = LogNavigation::default();
        navigation.select_buffer_edge(model.logs.len(), true, now);
        let viewport = build_log_viewport(&model, Rect::new(0, 0, 20, 3), Some(&navigation));
        assert_eq!(navigation.cursor(), Some(0));
        assert_eq!(viewport.first_log_index(), Some(0));

        navigation.select_buffer_edge(model.logs.len(), false, now);
        let viewport = build_log_viewport(&model, Rect::new(0, 0, 20, 3), Some(&navigation));
        assert_eq!(navigation.cursor(), Some(5));
        assert_eq!(viewport.last_log_index(), Some(5));
    }

    #[test]
    fn log_visual_buffer_jumps_extend_beyond_viewport() {
        let mut model = model_with_subscription();
        for index in 0..6 {
            model.push_log(format!("line {index}"));
        }
        let now = Instant::now();
        let mut navigation = LogNavigation::default();
        navigation.select_buffer_edge(model.logs.len(), false, now);
        navigation.move_by(-1, model.logs.len(), now);
        navigation.enter_visual(now);

        navigation.select_buffer_edge(model.logs.len(), true, now);
        assert_eq!(navigation.selected_range(), Some(0..=4));
        assert_eq!(navigation.selected_text(&model).unwrap().1, 5);
        navigation.select_buffer_edge(model.logs.len(), false, now);
        assert_eq!(navigation.selected_range(), Some(4..=5));
    }

    #[test]
    fn log_buffer_edge_jump_is_noop_when_empty() {
        let mut navigation = LogNavigation::default();
        navigation.select_buffer_edge(0, true, Instant::now());
        assert_eq!(navigation.cursor(), None);
    }

    #[test]
    fn log_navigation_tracks_oldest_buffer_eviction() {
        let mut model = model_with_subscription();
        for line in ["old", "selected", "new"] {
            model.push_log(line.into());
        }
        let viewport = build_log_viewport(&model, Rect::new(0, 0, 20, 3), None);
        let now = Instant::now();
        let mut navigation = LogNavigation::default();
        navigation.select_edge(&viewport, true, now);
        navigation.move_by(1, model.logs.len(), now);
        navigation.oldest_log_evicted();
        model.logs.pop_front();
        assert_eq!(navigation.cursor(), Some(0));
        assert_eq!(
            navigation.selected_text(&model),
            Some(("selected".into(), 1))
        );
    }

    #[test]
    fn log_selection_joins_soft_wraps_and_preserves_real_line_breaks() {
        let mut model = model_with_subscription();
        model.push_log("abcdef".into());
        model.push_log("ghi".into());
        let viewport = build_log_viewport(&model, Rect::new(10, 5, 3, 3), None);
        assert_eq!(viewport.rows.len(), 3);
        let mut selection = LogSelection::start(viewport, 10, 5).unwrap();
        selection.update(12, 7);
        assert_eq!(selection.text(), "abcdef\nghi");
    }

    #[test]
    fn log_selection_works_backwards_and_clamps_to_log_bounds() {
        let mut model = model_with_subscription();
        model.push_log("abc".into());
        model.push_log("def".into());
        let viewport = build_log_viewport(&model, Rect::new(10, 5, 3, 2), None);
        let mut selection = LogSelection::start(viewport, 12, 6).unwrap();
        selection.update(0, 0);
        assert_eq!(selection.text(), "abc\ndef");
    }

    #[test]
    fn log_selection_does_not_split_wide_unicode_characters() {
        let mut model = model_with_subscription();
        model.push_log("a界b".into());
        let viewport = build_log_viewport(&model, Rect::new(0, 0, 4, 1), None);
        let mut selection = LogSelection::start(viewport, 1, 0).unwrap();
        selection.update(2, 0);
        assert_eq!(selection.text(), "界");
    }

    #[test]
    fn log_selection_single_click_is_empty() {
        let mut model = model_with_subscription();
        model.push_log("abc".into());
        let viewport = build_log_viewport(&model, Rect::new(0, 0, 3, 1), None);
        let selection = LogSelection::start(viewport, 1, 0).unwrap();
        assert!(selection.is_empty());
        assert!(selection.text().is_empty());
    }

    #[test]
    fn active_log_selection_freezes_viewport_until_it_is_cleared() {
        let mut model = model_with_subscription();
        model.push_log("visible before drag".into());
        let area = Rect::new(0, 0, 90, 20);
        let viewport = log_viewport(&model, area).unwrap();
        let mut selection = LogSelection::start(viewport, 47, 4).unwrap();
        selection.update(48, 4);

        model.push_log("arrived during drag".into());
        let backend = TestBackend::new(90, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_with_log_selection(frame, &model, Some(&selection)))
            .unwrap();
        let frozen = buffer_to_string(terminal.backend().buffer());
        assert!(frozen.contains("visible before drag"));
        assert!(!frozen.contains("arrived during drag"));

        terminal.draw(|frame| draw(frame, &model)).unwrap();
        let current = buffer_to_string(terminal.backend().buffer());
        assert!(current.contains("arrived during drag"));
    }
}

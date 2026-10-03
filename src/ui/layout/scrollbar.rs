use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

pub(super) struct ScrollWindow {
    pub(super) total: usize,
    pub(super) visible: usize,
    pub(super) start: usize,
}

pub(super) fn draw_scrollbar(frame: &mut Frame, track: Rect, window: ScrollWindow, style: Style) {
    if window.total <= window.visible || track.width == 0 || track.height == 0 {
        return;
    }
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .track_style(style)
        .thumb_symbol("┃")
        .thumb_style(style);
    let mut state = ScrollbarState::new(window.total - window.visible + 1)
        .position(window.start)
        .viewport_content_length(window.visible);
    frame.render_stateful_widget(scrollbar, track, &mut state);
}

pub(super) fn right_border_track(area: Rect) -> Rect {
    Rect::new(
        area.right().saturating_sub(1),
        area.y.saturating_add(1),
        area.width.min(1),
        area.height.saturating_sub(2),
    )
}

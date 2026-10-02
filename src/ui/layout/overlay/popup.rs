use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::ui::layout::scrollbar::{ScrollWindow, draw_scrollbar};
use crate::ui::layout::text::{align_in_centered_column, fit_to_visual_width, visual_width};
use crate::ui::styles::Theme;

pub(super) const DIALOG_POPUP_WIDTH: u16 = 48;
pub(super) const SETTINGS_POPUP_MIN_WIDTH: u16 = 33;
pub(super) const SETTINGS_TEXT_MARGIN: usize = 1;
pub(super) const SELECTION_POPUP_MAX_HEIGHT_PERCENT: u16 = 90;

pub(super) fn centered_fixed_width_rect(width: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y,
        width,
        area.height,
    )
}

pub(super) fn content_sized_rect(width: u16, area: Rect, lines: &[Line]) -> Rect {
    let horizontal_area = centered_fixed_width_rect(width, area);
    content_sized_rect_in(horizontal_area, area, lines, 0)
}

pub(super) fn settings_popup_width(
    area: Rect,
    content_widths: impl IntoIterator<Item = usize>,
) -> u16 {
    let chrome = 2 + SETTINGS_TEXT_MARGIN * 2;
    let needed = content_widths.into_iter().max().unwrap_or(0) + chrome;
    u16::try_from(needed)
        .unwrap_or(u16::MAX)
        .max(SETTINGS_POPUP_MIN_WIDTH)
        .min(area.width)
}

pub(super) fn settings_content_sized_rect(area: Rect, popup_width: u16, lines: &[Line]) -> Rect {
    let horizontal_area = centered_fixed_width_rect(popup_width, area);
    content_sized_rect_in(horizontal_area, area, lines, 0)
}

pub(super) fn settings_row_width(popup_width: u16) -> usize {
    popup_width.saturating_sub(2) as usize
}

pub(super) fn settings_content_width(popup_width: u16) -> usize {
    settings_row_width(popup_width).saturating_sub(SETTINGS_TEXT_MARGIN * 2)
}

pub(super) fn content_sized_rect_in(
    horizontal_area: Rect,
    area: Rect,
    lines: &[Line],
    horizontal_padding: u16,
) -> Rect {
    let content_width = horizontal_area
        .width
        .saturating_sub(2 + horizontal_padding.saturating_mul(2))
        .max(1) as usize;
    let content_height = lines.iter().fold(0u16, |height, line| {
        let line_height =
            u16::try_from(line.width().max(1).div_ceil(content_width)).unwrap_or(u16::MAX);
        height.saturating_add(line_height)
    });
    let popup_height = content_height.saturating_add(2).min(area.height);
    Rect::new(
        horizontal_area.x,
        area.y + area.height.saturating_sub(popup_height) / 2,
        horizontal_area.width,
        popup_height,
    )
}

/// Helper to render a centered popup with a border and text.
pub(super) fn draw_modal(frame: &mut Frame, theme: &Theme, area: Rect, lines: Vec<Line>) {
    let popup_area = content_sized_rect(DIALOG_POPUP_WIDTH, area, &lines);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.accent())
        .style(theme.popup_bg());

    let paragraph = Paragraph::new(lines)
        .block(block)
        .style(theme.normal())
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, popup_area);
}

pub(super) fn draw_settings_modal(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    popup_width: u16,
    lines: Vec<Line>,
) -> Rect {
    let popup_area = settings_content_sized_rect(area, popup_width, &lines);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.accent())
        .style(theme.popup_bg());

    let paragraph = Paragraph::new(lines)
        .block(block)
        .style(theme.normal())
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, popup_area);
    popup_area
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_selection_modal(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    heading: &str,
    items: &[&str],
    selected: usize,
    scroll: Option<usize>,
    active: Option<usize>,
    footer: String,
) {
    let popup_width = settings_popup_width(
        area,
        [visual_width(heading), visual_width(&footer)]
            .into_iter()
            .chain(items.iter().map(|item| visual_width(item))),
    );
    let height_area = centered_rect(100, SELECTION_POPUP_MAX_HEIGHT_PERCENT, area);
    let popup_area = Rect::new(
        area.x + area.width.saturating_sub(popup_width) / 2,
        height_area.y,
        popup_width,
        height_area.height,
    );
    let row_width = popup_area.width.saturating_sub(2) as usize;
    let content_width = row_width.saturating_sub(SETTINGS_TEXT_MARGIN * 2);
    let column_width = items
        .iter()
        .map(|label| visual_width(label))
        .max()
        .unwrap_or(0)
        .min(content_width);
    let visible_count = selection_visible(area, items.len());
    let window_start = selection_window_start(items.len(), visible_count, selected, scroll);
    let window_end = window_start + visible_count;

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(heading, theme.accent())),
        Line::from(""),
    ];
    for (i, label) in items.iter().enumerate().take(window_end).skip(window_start) {
        let is_active = active == Some(i);
        let style = if is_active && i == selected {
            theme.selected_connected()
        } else if i == selected {
            theme.selected()
        } else if is_active {
            theme.success()
        } else {
            theme.normal()
        };
        let text = align_in_centered_column(label, content_width, column_width);
        let text = fit_to_visual_width(&format!(" {text}"), row_width);
        lines.push(Line::from(Span::styled(text, style)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(footer));
    let popup_area = draw_settings_modal(frame, theme, area, popup_width, lines);
    let item_rows = Rect::new(
        popup_area.right().saturating_sub(1),
        popup_area.y.saturating_add(3),
        popup_area.width.min(1),
        visible_count as u16,
    )
    .intersection(popup_area);
    draw_scrollbar(
        frame,
        item_rows,
        ScrollWindow {
            total: items.len(),
            visible: visible_count,
            start: window_start,
        },
        theme.accent(),
    );
}

/// Compute a centered rectangle with given percentage sizes.
pub(super) fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

pub(in crate::ui::layout) fn selection_visible(area: Rect, total: usize) -> usize {
    let height = centered_rect(100, SELECTION_POPUP_MAX_HEIGHT_PERCENT, area).height;
    total.min(height.saturating_sub(6) as usize)
}

pub(in crate::ui::layout) fn selection_window_start(
    total: usize,
    visible: usize,
    selected: usize,
    scroll: Option<usize>,
) -> usize {
    let max_start = total.saturating_sub(visible);
    let Some(start) = scroll else {
        return selected.saturating_sub(visible / 2).min(max_start);
    };
    let start = start.min(max_start);
    if selected < start {
        selected
    } else if selected >= start.saturating_add(visible) {
        selected
            .saturating_add(1)
            .saturating_sub(visible)
            .min(max_start)
    } else {
        start
    }
}

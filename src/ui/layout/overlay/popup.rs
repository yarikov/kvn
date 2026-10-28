use ratatui::Frame;
use ratatui::layout::{Alignment, Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::ui::layout::panes::main_area;
use crate::ui::layout::scrollbar::{ScrollWindow, draw_scrollbar};
use crate::ui::layout::text::{align_in_centered_column, fit_to_visual_width, visual_width};
use crate::ui::styles::Theme;

pub(super) const DIALOG_POPUP_WIDTH: u16 = 48;
pub(super) const SETTINGS_POPUP_MIN_WIDTH: u16 = 33;
pub(super) const SETTINGS_TEXT_MARGIN: usize = 1;

const POPUP_SIDE_GUTTER: u16 = 1;
const OVERLAY_MAX_HEIGHT: u16 = 28;

fn above_status_bar(area: Rect) -> Rect {
    Rect::new(
        area.x,
        area.y,
        area.width,
        main_area(area).bottom() - area.y,
    )
}

pub(super) fn overlay_max_height(area: Rect) -> u16 {
    OVERLAY_MAX_HEIGHT.min(above_status_bar(area).height)
}

pub(super) fn vertically_placed_rect(horizontal_area: Rect, area: Rect, height: u16) -> Rect {
    let height = height.min(overlay_max_height(area));
    let panes = main_area(area);
    let column = if height <= panes.height {
        panes
    } else {
        above_status_bar(area)
    };
    Rect::new(
        horizontal_area.x,
        column.y + (column.height - height) / 2,
        horizontal_area.width,
        height,
    )
}

pub(super) fn clear_popup(frame: &mut Frame, theme: &Theme, popup_area: Rect) {
    let gutter_area = popup_area
        .outer(Margin::new(POPUP_SIDE_GUTTER, 0))
        .intersection(frame.area());
    frame.render_widget(Clear, gutter_area);
    frame.render_widget(Block::default().style(theme.background()), gutter_area);
}

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
    vertically_placed_rect(horizontal_area, area, content_height.saturating_add(2))
}

pub(super) fn draw_modal(frame: &mut Frame, theme: &Theme, area: Rect, lines: Vec<Line>) {
    let popup_area = content_sized_rect(DIALOG_POPUP_WIDTH, area, &lines);

    clear_popup(frame, theme, popup_area);

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

    clear_popup(frame, theme, popup_area);

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
    let row_width = popup_width.saturating_sub(2) as usize;
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

pub(in crate::ui::layout) fn selection_visible(area: Rect, total: usize) -> usize {
    total.min(overlay_max_height(area).saturating_sub(6) as usize)
}

pub(in crate::ui::layout) fn row_window_start(
    rows: &[Option<usize>],
    visible: usize,
    selected: usize,
    scroll: Option<usize>,
) -> usize {
    let line = rows
        .iter()
        .position(|row| *row == Some(selected))
        .unwrap_or(0);
    selection_window_start(rows.len(), visible, line, scroll)
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

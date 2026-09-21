use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use crate::ui::layout::text::{fit_to_visual_width, pad_to_visual_width, visual_width};

use super::popup::{
    SETTINGS_TEXT_MARGIN, settings_content_width, settings_popup_width, settings_row_width,
};

pub(super) fn settings_value_line<'a>(
    name: &str,
    value: &str,
    dirty: bool,
    selected: bool,
    layout: SettingsValueLayout,
    theme: &crate::ui::styles::Theme,
) -> Line<'a> {
    let text = settings_value_text(name, value, dirty, layout.label_width);
    let indent = " ".repeat(layout.content_width.saturating_sub(layout.group_width) / 2);
    let row = format!("{indent}{text}");
    settings_full_width_line(&row, layout.row_width, selected, theme)
}

pub(super) fn settings_value_text(
    name: &str,
    value: &str,
    dirty: bool,
    label_width: usize,
) -> String {
    let label = pad_to_visual_width(name, label_width);
    format!("{label} ‹ {value} ›{}", if dirty { " *" } else { "  " })
}

#[derive(Clone, Copy)]
pub(super) struct SettingsValueLayout {
    pub(super) label_width: usize,
    pub(super) group_width: usize,
    content_width: usize,
    row_width: usize,
    pub(super) popup_width: u16,
}

pub(super) fn settings_value_layout(
    settings: &[(&str, String, bool)],
    stable_value_width: usize,
    area: Rect,
    frame_widths: &[usize],
) -> SettingsValueLayout {
    let label_width = settings
        .iter()
        .map(|(name, _, _)| visual_width(name))
        .max()
        .unwrap_or(0);
    let value_width = settings
        .iter()
        .map(|(_, value, _)| visual_width(value))
        .max()
        .unwrap_or(0)
        .max(stable_value_width);
    let group_width = label_width + value_width + 7;
    let popup_width = settings_popup_width(area, frame_widths.iter().copied().chain([group_width]));
    SettingsValueLayout {
        label_width,
        group_width,
        content_width: settings_content_width(popup_width),
        row_width: settings_row_width(popup_width),
        popup_width,
    }
}

pub(super) fn settings_full_width_line<'a>(
    text: &str,
    row_width: usize,
    selected: bool,
    theme: &crate::ui::styles::Theme,
) -> Line<'a> {
    let content_width = row_width.saturating_sub(SETTINGS_TEXT_MARGIN * 2);
    let content = fit_to_visual_width(text, content_width);
    let row = fit_to_visual_width(&format!(" {content}"), row_width);
    Line::from(Span::styled(
        row,
        if selected {
            theme.selected()
        } else {
            theme.normal()
        },
    ))
}

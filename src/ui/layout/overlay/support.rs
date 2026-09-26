use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::model::Model;
use crate::ui::layout::text::{align_in_centered_column, visual_width};

use super::onboarding::{ONBOARDING_POPUP_WIDTH, wrap_plain};
use super::popup::centered_fixed_width_rect;
use super::{CONFIRM_ACTION, overlay_footer};

pub(super) fn draw(frame: &mut Frame, model: &Model, area: Rect) {
    const ITEMS: [&str; 3] = ["Buy me a coffee ☕", "Maybe later", "Don't ask again"];
    let horizontal_area = centered_fixed_width_rect(ONBOARDING_POPUP_WIDTH, area);
    let row_width = horizontal_area.width.saturating_sub(2) as usize;
    let column_width = ITEMS
        .iter()
        .map(|label| visual_width(label))
        .max()
        .unwrap_or(0)
        .min(row_width);
    let build_lines = |compact: bool| {
        let mut lines =
            vec![Line::from(Span::styled("Enjoying kvn?", model.theme.normal())).centered()];
        if !compact {
            lines.push(Line::from(""));
        }
        lines.extend(wrap_plain(
            "If it saves you time, you can say thanks with a coffee.",
            row_width,
        ));
        if !compact {
            lines.push(Line::from(""));
        }
        for (index, label) in ITEMS.iter().enumerate() {
            let style = if index == model.support_selected {
                model.theme.selected()
            } else {
                model.theme.normal()
            };
            let text = align_in_centered_column(label, row_width, column_width);
            lines.push(Line::from(Span::styled(text, style)));
        }
        if !compact {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(overlay_footer(
            model,
            Some(CONFIRM_ACTION),
            true,
            false,
        )));
        lines
    };
    let content_width = row_width.max(1);
    let measure = |lines: &[Line]| -> u16 {
        lines
            .iter()
            .map(|line| line.width().max(1).div_ceil(content_width) as u16)
            .sum()
    };
    let mut lines = build_lines(false);
    let mut content_height = measure(&lines);
    if content_height.saturating_add(2) > area.height {
        lines = build_lines(true);
        content_height = measure(&lines);
    }
    let paragraph = Paragraph::new(lines)
        .style(model.theme.normal())
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });
    let popup_height = content_height.saturating_add(2).min(area.height);
    let popup_area = Rect::new(
        horizontal_area.x,
        area.y + area.height.saturating_sub(popup_height) / 2,
        horizontal_area.width,
        popup_height,
    );
    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(model.theme.accent())
        .style(model.theme.popup_bg());
    frame.render_widget(paragraph.block(block), popup_area);
}

#[cfg(test)]
mod tests {

    use crate::app::model::Overlay;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_profiles, snapshot_terminal,
    };
    use crate::ui::layout::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH};

    #[test]
    fn draw_support_overlay_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::Support;
        model.support_selected = 1;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_support_overlay_at_minimum_terminal_size_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::Support;
        model.support_selected = 1;
        insta::assert_snapshot!(snapshot_terminal(
            &model,
            MIN_TERMINAL_WIDTH,
            MIN_TERMINAL_HEIGHT
        ));
    }
}

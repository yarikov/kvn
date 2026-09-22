use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use crate::app::model::Model;

use super::overlay_footer;
use super::popup::draw_modal;

pub(super) fn draw(frame: &mut Frame, model: &Model, area: Rect) {
    let lines = vec![
        Line::from(Span::styled("Restart required", model.theme.accent())),
        Line::from(""),
        Line::from("kvn was updated."),
        Line::from(""),
        Line::from(overlay_footer(
            model,
            Some("restart the daemon"),
            false,
            false,
        )),
    ];
    draw_modal(frame, &model.theme, area, lines);
}

#[cfg(test)]
mod tests {
    use crate::app::model::Overlay;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_profiles, snapshot_terminal,
    };

    #[test]
    fn draw_restart_required_overlay_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::RestartRequired;
        model.restart_required = true;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }
}

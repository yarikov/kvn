use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use crate::app::model::Model;

use super::popup::draw_modal;
use super::{CONFIRM_ACTION, overlay_footer};

/// Draw the delete confirmation dialog.
pub(super) fn draw(frame: &mut Frame, model: &Model, area: Rect) {
    use crate::app::model::SourceRow;
    let theme = &model.theme;
    let message = match model.selected_row() {
        Some(SourceRow::SubscriptionHeader(_)) => "Delete selected subscription and its profiles?",
        _ => "Delete selected profile?",
    };
    draw_modal(
        frame,
        theme,
        area,
        vec![
            Line::from(Span::styled(message, theme.error())),
            Line::from(""),
            Line::from(overlay_footer(model, Some(CONFIRM_ACTION), true, false)),
        ],
    );
}

#[cfg(test)]
mod tests {

    use crate::app::model::Overlay;
    use crate::config::profile::Profile;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_profiles, snapshot_styles, snapshot_terminal,
    };

    #[test]
    fn draw_confirm_delete_overlay_snapshot() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Alpha".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.overlay = Overlay::ConfirmDelete;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn light_overlay_footer_uses_theme_foreground() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Alpha".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.overlay = Overlay::ConfirmDelete;
        model.theme = crate::ui::styles::Theme::resolve("catppuccin-latte");

        insta::assert_snapshot!(snapshot_styles(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }
}

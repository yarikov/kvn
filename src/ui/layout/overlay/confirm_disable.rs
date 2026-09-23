use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use crate::app::model::{DisableTarget, Model};

use super::popup::draw_modal;
use super::{CONFIRM_ACTION, overlay_footer};

/// Draw the confirmation dialog shown before a protection is turned off.
pub(super) fn draw(frame: &mut Frame, model: &Model, target: DisableTarget, area: Rect) {
    let theme = &model.theme;
    let message = match target {
        DisableTarget::AutoConnect => "Disable auto-connect?",
        DisableTarget::KillSwitch => "Disable the kill switch?",
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
    use crate::app::model::{DisableTarget, Overlay};
    use crate::config::profile::Profile;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_profiles, snapshot_terminal,
    };

    #[test]
    fn draw_confirm_disable_auto_connect_overlay_snapshot() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Alpha".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.config.settings.auto_connect = true;
        model.overlay = Overlay::ConfirmDisable(DisableTarget::AutoConnect);
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_confirm_disable_kill_switch_overlay_snapshot() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Alpha".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.config.settings.kill_switch = true;
        model.overlay = Overlay::ConfirmDisable(DisableTarget::KillSwitch);
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }
}

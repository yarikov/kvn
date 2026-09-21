use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use crate::app::model::Model;

use super::overlay_footer;
use super::popup::draw_modal;

pub(super) fn draw(frame: &mut Frame, model: &Model, area: Rect) {
    use crate::app::model::MigrationPhase;

    let Some(status) = &model.migration else {
        return;
    };
    let (heading, detail) = match status.phase {
        MigrationPhase::Running => ("Updating kvn", status.summary.as_str()),
        MigrationPhase::Finalizing => ("Finishing update", status.summary.as_str()),
        MigrationPhase::Failed => ("Migration failed", status.summary.as_str()),
    };
    let mut lines = vec![
        Line::from(Span::styled(heading, model.theme.accent())),
        Line::from(""),
        Line::from(detail.to_string()),
        Line::from("Migration continues after closing the TUI."),
        Line::from(format!(
            "Step {} of {}",
            status.completed.min(status.total),
            status.total
        )),
    ];
    if let Some(error) = &status.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(error.clone(), model.theme.error())));
        lines.push(Line::from("Run `kvn migrate` in a terminal to retry."));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(overlay_footer(model, None, true, false)));
    draw_modal(frame, &model.theme, area, lines);
}

#[cfg(test)]
mod tests {

    use crate::app::model::Overlay;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_profiles, snapshot_terminal,
    };

    #[test]
    fn draw_migration_overlay_snapshot() {
        use crate::app::model::{MigrationPhase, MigrationStatus};

        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::Migration;
        model.migration = Some(MigrationStatus {
            session_id: "session".into(),
            phase: MigrationPhase::Failed,
            completed: 2,
            total: 4,
            summary: "Updating integration".into(),
            error: Some("sudo command failed".into()),
        });
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }
}

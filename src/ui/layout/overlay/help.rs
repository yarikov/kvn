use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Row, Table, TableState};

use crate::app::model::Model;

use super::overlay_footer;
use super::popup::{centered_fixed_width_rect, centered_rect};

pub(super) const HELP_POPUP_WIDTH: u16 = 51;

/// Draw the help popup overlay.
pub(super) fn draw(
    frame: &mut Frame,
    model: &Model,
    help_state: crate::app::model::HelpState,
    area: Rect,
) {
    let theme = &model.theme;
    let help_rows = crate::ui::help::rows(help_state.context);
    let needed = help_rows.len() as u16 + 2 + 2;
    let percent = needed
        .saturating_mul(100)
        .checked_div(area.height)
        .unwrap_or(90)
        .clamp(50, 90);
    let popup_area = centered_fixed_width_rect(HELP_POPUP_WIDTH, centered_rect(100, percent, area));

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.accent())
        .style(theme.popup_bg());

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);
    let visible_count = chunks[0].height as usize;
    let selected = help_state.selected.min(help_rows.len().saturating_sub(1));
    let window_start = if help_rows.len() > visible_count {
        selected
            .saturating_sub(visible_count / 2)
            .min(help_rows.len() - visible_count)
    } else {
        0
    };
    let window_end = (window_start + visible_count).min(help_rows.len());
    let rows = help_rows[window_start..window_end]
        .iter()
        .map(|line| match line {
            crate::ui::help::HelpLine::Heading(title) => {
                Row::new(vec![*title, ""]).style(theme.accent().add_modifier(Modifier::BOLD))
            }
            crate::ui::help::HelpLine::Separator => Row::new(vec!["", ""]),
            crate::ui::help::HelpLine::Command { key, action } => Row::new(vec![*key, *action]),
        });
    let table = Table::new(rows, [Constraint::Length(10), Constraint::Min(1)])
        .style(theme.normal())
        .row_highlight_style(theme.selected())
        .highlight_symbol(" ");
    let mut table_state = TableState::default().with_selected(Some(selected - window_start));
    frame.render_stateful_widget(table, chunks[0], &mut table_state);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(overlay_footer(model, None, true, false)).centered(),
        ])
        .style(theme.normal()),
        chunks[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::Overlay;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_profiles, render_to_string, snapshot_terminal,
    };
    use crate::ui::layout::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH};

    #[test]
    fn help_renders_commands() {
        let model = model_with_profiles(vec![]);
        let state = crate::app::model::HelpState::default();
        let content = render_to_string(80, 60, |frame| {
            let area = frame.area();
            draw(frame, &model, state, area);
        });
        let lines = crate::ui::help::rows(state.context);
        let expected = [
            ("Ctrl+h/l", "Focus panes"),
            ("j/k, ↑/↓", "Move or scroll"),
            ("gg/G", "Go to first / last"),
            ("Enter", "Connect selected profile"),
            ("e", "Open profiles.json in $EDITOR"),
            ("y", "Yank profile or subscription"),
            ("p", "Paste profile or subscription"),
            ("d", "Delete profile or subscription"),
            ("u", "Update subscription or geo"),
            ("i/I", "Cycle subscription / geo auto-update"),
            ("t/T", "Test selected / all profiles"),
            ("r", "Reconnect"),
            ("s", "Disconnect"),
            ("a", "Toggle auto-connect"),
            ("Shift+K", "Toggle kill switch"),
            ("Space r", "Routing"),
            ("Space i", "Interface"),
            ("Space c", "Connection"),
            ("q/Esc", "Detach TUI from main screen"),
            ("Ctrl+C", "Quit daemon"),
            ("?", "Open or close help"),
        ];
        for (key, action) in expected {
            assert!(
                lines.iter().any(|line| matches!(
                    line,
                    crate::ui::help::HelpLine::Command {
                        key: actual_key,
                        action: actual_action,
                    } if *actual_key == key && *actual_action == action
                )),
                "help catalog should contain {key}: {action}"
            );
        }
        assert!(content.contains("Navigation"));
        assert!(content.contains("Profiles"));
    }

    #[test]
    fn draw_help_overlay_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.overlay = Overlay::Help(crate::app::model::HelpState::default());
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_help_overlay_at_minimum_terminal_size_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.overlay = Overlay::Help(crate::app::model::HelpState::default());
        insta::assert_snapshot!(snapshot_terminal(
            &model,
            MIN_TERMINAL_WIDTH,
            MIN_TERMINAL_HEIGHT
        ));
    }

    #[test]
    fn draw_logs_help_overlay_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::Help(crate::app::model::HelpState {
            context: crate::app::model::HelpContext::Logs,
            selected: 12,
        });
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }
}

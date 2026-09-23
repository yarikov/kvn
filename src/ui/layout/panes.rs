use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

use crate::app::model::{MainPaneFocus, Model, Overlay, SourceRow};
use crate::ui::widgets::{
    StatusBar, format_bps_field, format_bytes_field, format_connections_field,
    format_connections_padding,
};

use super::log::navigation::{LogNavigation, LogSelection, LogViewport};
use super::log::{build_log_viewport, log_display_line};
use super::sources::draw_sources;
use super::{TRAFFIC_PANEL_HEIGHT, terminal_size_supported};

/// Minimum terminal width that leaves enough room for both main panes. At
/// narrower widths the Profiles pane uses the full row and Logs is hidden.
pub(super) const TWO_PANE_MIN_WIDTH: u16 = 90;

pub(super) fn main_panes(terminal_area: Rect) -> (Rect, Rect) {
    let main_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(TRAFFIC_PANEL_HEIGHT),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(terminal_area)[1];
    split_main_panes(main_area)
}

pub(super) fn split_main_panes(area: Rect) -> (Rect, Rect) {
    if !logs_visible(area) {
        return (area, Rect::new(area.right(), area.y, 0, area.height));
    }
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    (panes[0], panes[1])
}

pub(crate) fn logs_visible(area: Rect) -> bool {
    area.width >= TWO_PANE_MIN_WIDTH
}

pub(super) fn panel_inner(area: Rect) -> Rect {
    Block::default()
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
        .inner(area)
}

pub(crate) fn log_viewport(model: &Model, terminal_area: Rect) -> Option<LogViewport> {
    log_viewport_with_navigation(model, terminal_area, None)
}

pub(crate) fn log_viewport_with_navigation(
    model: &Model,
    terminal_area: Rect,
    navigation: Option<&LogNavigation>,
) -> Option<LogViewport> {
    if model.overlay != Overlay::None || !terminal_size_supported(terminal_area) {
        return None;
    }
    let (_, logs) = main_panes(terminal_area);
    let area = panel_inner(logs);
    if area.width == 0 || area.height == 0 {
        return None;
    }
    Some(build_log_viewport(model, area, navigation))
}

pub(crate) fn sync_log_scroll(model: &Model, terminal_area: Rect, navigation: &mut LogNavigation) {
    if let Some(viewport) = log_viewport_with_navigation(model, terminal_area, Some(navigation)) {
        navigation.set_scroll_top_from(&viewport);
    }
}

/// Return the selectable Sources row under a terminal cell. Borders, group
/// labels, separators, Logs, clipped rows, and every overlay are inert.
pub(crate) fn source_hit_test(
    model: &Model,
    terminal_area: Rect,
    column: u16,
    row: u16,
) -> Option<usize> {
    if model.overlay != Overlay::None || !terminal_size_supported(terminal_area) {
        return None;
    }
    let (sources, _) = main_panes(terminal_area);
    let content = panel_inner(sources);
    let inside = column >= content.x
        && column < content.x.saturating_add(content.width)
        && row >= content.y
        && row < content.y.saturating_add(content.height);
    if !inside {
        return None;
    }

    let rows = model.source_rows();
    let mut visual_rows = Vec::new();
    let standalone: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, source)| matches!(source, SourceRow::StandaloneProfile(_)))
        .map(|(index, _)| index)
        .collect();
    if !standalone.is_empty() {
        visual_rows.extend(standalone.into_iter().map(Some));
        visual_rows.push(None);
    }
    for sub_idx in 0..model.config.subscriptions.len() {
        visual_rows.push(rows.iter().position(
            |source| matches!(source, SourceRow::SubscriptionHeader(index) if *index == sub_idx),
        ));
        visual_rows.extend(rows.iter().enumerate().filter_map(|(index, source)| {
            matches!(source, SourceRow::SubscriptionProfile { sub_idx: index_sub, .. } if *index_sub == sub_idx)
                .then_some(Some(index))
        }));
        visual_rows.push(None);
    }
    let line = row.saturating_sub(content.y) as usize;
    visual_rows.get(line).copied().flatten()
}

/// Draw the main content area with the Sources list and logs.
pub(super) fn draw_main(
    frame: &mut Frame,
    model: &Model,
    area: Rect,
    pane_focus: MainPaneFocus,
    log_navigation: Option<&LogNavigation>,
    log_selection: Option<&LogSelection>,
) {
    let theme = &model.theme;
    let main_focus_active = model.overlay == Overlay::None;
    let (sources_area, logs_area) = split_main_panes(area);

    draw_sources(
        frame,
        model,
        sources_area,
        main_focus_active && (pane_focus == MainPaneFocus::Sources || !logs_visible(area)),
    );

    if logs_area.width == 0 {
        return;
    }

    let log_block = Block::default()
        .title(" Logs ")
        .borders(Borders::ALL)
        .border_style(if main_focus_active && pane_focus == MainPaneFocus::Logs {
            theme.accent()
        } else {
            theme.border()
        });

    let inner = panel_inner(logs_area);
    let viewport = log_selection
        .map(|selection| &selection.viewport)
        .filter(|viewport| viewport.area == inner)
        .cloned()
        .unwrap_or_else(|| build_log_viewport(model, inner, log_navigation));
    let log_text: Vec<Line> = viewport
        .rows
        .iter()
        .enumerate()
        .map(|(row_index, row)| {
            log_display_line(
                model,
                row,
                row_index,
                inner.width as usize,
                log_selection,
                log_navigation,
            )
        })
        .collect();

    let logs = Paragraph::new(log_text).block(log_block);
    frame.render_widget(logs, logs_area);
}

/// Render the full-width traffic header: instantaneous ↑/↓ rate, cumulative
/// totals, and active connection count laid out on a single content row
/// inside a bordered block. Driven by `model.traffic`, updated ~1 Hz by the
/// daemon.
pub(super) fn draw_traffic_panel(frame: &mut Frame, model: &Model, area: Rect) {
    let theme = &model.theme;
    let t = &model.traffic;
    let line = Line::from(vec![
        Span::styled("↑ ", theme.success()),
        Span::styled(format_bps_field(t.up_rate_bps), theme.normal()),
        Span::raw("  "),
        Span::styled("↓ ", theme.accent()),
        Span::styled(format_bps_field(t.down_rate_bps), theme.normal()),
        Span::raw("    "),
        Span::styled("Total: ", theme.normal()),
        Span::styled("↑ ", theme.success()),
        Span::styled(format_bytes_field(t.up_total), theme.normal()),
        Span::raw("  "),
        Span::styled("↓ ", theme.accent()),
        Span::styled(format_bytes_field(t.down_total), theme.normal()),
        Span::raw("    "),
        Span::styled(format_connections_field(t.conn_count), theme.accent()),
        Span::styled(" connections", theme.normal()),
        Span::raw(format_connections_padding(t.conn_count)),
    ]);
    let block = Block::default()
        .title(" Traffic ")
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
        .border_style(theme.border());
    let paragraph = Paragraph::new(line).block(block);
    frame.render_widget(paragraph, area);
}

/// Draw the bottom status bar.
pub(super) fn draw_status_bar(frame: &mut Frame, model: &Model, area: Rect) {
    let status = StatusBar::new(model);
    frame.render_widget(status, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::ConnectionState;
    use crate::config::profile::Profile;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, buffer_to_styled_string, model_with_profiles,
        model_with_subscription, render_to_buffer, snapshot_styles, snapshot_terminal,
    };
    use crate::ui::layout::draw_with_interaction;

    #[test]
    fn source_hit_test_maps_rows_and_rejects_unsupported_height() {
        let model = model_with_subscription();
        // Tall: traffic panel occupies rows 0..=2, Sources content starts at 4.
        assert_eq!(
            source_hit_test(&model, Rect::new(0, 0, 80, 20), 2, 4),
            Some(0)
        );
        assert_eq!(
            source_hit_test(&model, Rect::new(0, 0, 80, 20), 2, 6),
            Some(1)
        );
        assert_eq!(
            source_hit_test(&model, Rect::new(0, 0, 80, 20), 2, 7),
            Some(2)
        );
        assert_eq!(source_hit_test(&model, Rect::new(0, 0, 80, 8), 2, 1), None);
    }

    #[test]
    fn source_hit_test_ignores_labels_borders_separators_logs_clipping_and_overlays() {
        let mut model = model_with_subscription();
        let area = Rect::new(0, 0, 90, 20);
        assert_eq!(source_hit_test(&model, area, 2, 5), None); // separator
        assert_eq!(source_hit_test(&model, area, 0, 4), None); // border
        assert_eq!(source_hit_test(&model, area, 1, 4), None); // padding
        assert_eq!(source_hit_test(&model, area, 50, 4), None); // Logs
        assert_eq!(source_hit_test(&model, Rect::new(0, 0, 90, 5), 2, 4), None);
        model.overlay = Overlay::Help(crate::app::model::HelpState::default());
        assert_eq!(source_hit_test(&model, area, 2, 5), None);
    }

    #[test]
    fn draw_traffic_panel_snapshot() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Alpha".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(model.config.profiles[0].id);
        model.traffic = crate::app::model::TrafficStats {
            up_rate_bps: 12_345,
            down_rate_bps: 3_500_000,
            up_total: 142 * 1024 * 1024,
            down_total: 3 * 1024 * 1024 * 1024,
            conn_count: 18,
        };
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_main_shows_zeroed_traffic_panel_when_idle() {
        let model = model_with_profiles(vec![Profile::new_vless(
            "Alpha".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        let output = snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS);
        assert!(
            output.contains("Traffic") && output.contains("0.0 KB/s"),
            "traffic panel must show zeroed stats while disconnected: {}",
            output
        );
    }

    #[test]
    fn narrow_layout_hides_logs_and_expands_sources() {
        let mut model = model_with_subscription();
        model.push_log("must stay hidden".into());

        let output = snapshot_terminal(&model, TWO_PANE_MIN_WIDTH - 1, APP_WINDOW_ROWS);

        assert!(!output.contains("must stay hidden"));
        assert_eq!(
            source_hit_test(
                &model,
                Rect::new(0, 0, TWO_PANE_MIN_WIDTH - 1, APP_WINDOW_ROWS),
                60,
                4
            ),
            Some(0)
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn logs_use_the_ninety_column_breakpoint() {
        let model = model_with_subscription();
        let narrow = Rect::new(0, 0, TWO_PANE_MIN_WIDTH - 1, 20);
        let wide = Rect::new(0, 0, TWO_PANE_MIN_WIDTH, 20);

        assert!(!logs_visible(narrow));
        assert!(logs_visible(wide));
        assert!(log_viewport(&model, narrow).is_none());
        assert!(log_viewport(&model, wide).is_some());
    }

    #[test]
    fn overlay_focus_temporarily_suspends_and_restores_main_pane_focus() {
        for (label, pane_focus) in [
            ("sources", MainPaneFocus::Sources),
            ("logs", MainPaneFocus::Logs),
        ] {
            let mut model = model_with_subscription();
            model.overlay = Overlay::ConfirmDelete;
            let suspended = render_to_buffer(APP_WINDOW_COLS, APP_WINDOW_ROWS, |frame| {
                draw_with_interaction(frame, &model, pane_focus, None, None);
            });
            insta::with_settings!({snapshot_suffix => format!("{label}-overlay")}, {
                insta::assert_snapshot!(buffer_to_styled_string(&suspended));
            });

            model.overlay = Overlay::None;
            let restored = render_to_buffer(APP_WINDOW_COLS, APP_WINDOW_ROWS, |frame| {
                draw_with_interaction(frame, &model, pane_focus, None, None);
            });
            insta::with_settings!({snapshot_suffix => label}, {
                insta::assert_snapshot!(buffer_to_styled_string(&restored));
            });
        }
    }

    /// Pin the `[error]`-prefixed log styling path in `draw_main`.
    #[test]
    fn draw_main_error_log_styling_snapshot() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Alpha".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(model.config.profiles[0].id);
        model.logs.push_back("normal info line".to_string());
        model
            .logs
            .push_back("[error] sing-box exited with code 1".to_string());
        insta::assert_snapshot!(snapshot_styles(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }
}

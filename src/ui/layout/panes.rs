use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

use crate::app::model::{MainPaneFocus, Model, Overlay};
use crate::ui::widgets::{
    StatusBar, format_bps_field, format_bytes_field, format_connections_field,
    format_connections_padding,
};

use super::log::navigation::{LogNavigation, LogSelection, LogViewport};
use super::log::{build_log_viewport, log_display_line, scroll_log_viewport};
use super::scrollbar::{ScrollWindow, draw_scrollbar, right_border_track};
use super::sources::{draw_sources, source_visual_rows, sources_window_start};
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
    let area = log_content_area(model, terminal_area)?;
    Some(build_log_viewport(model, area, navigation))
}

fn log_content_area(model: &Model, terminal_area: Rect) -> Option<Rect> {
    if model.overlay != Overlay::None || !terminal_size_supported(terminal_area) {
        return None;
    }
    let (_, logs) = main_panes(terminal_area);
    let area = panel_inner(logs);
    (area.width > 0 && area.height > 0).then_some(area)
}

pub(crate) fn sync_log_scroll(model: &Model, terminal_area: Rect, navigation: &mut LogNavigation) {
    if let Some(viewport) = log_viewport_with_navigation(model, terminal_area, Some(navigation)) {
        navigation.set_scroll_top_from(&viewport);
    }
}

pub(crate) fn scroll_logs(
    model: &Model,
    terminal_area: Rect,
    navigation: &mut LogNavigation,
    delta_rows: isize,
    now: Instant,
) -> bool {
    log_content_area(model, terminal_area)
        .is_some_and(|area| scroll_log_viewport(model, area, navigation, delta_rows, now))
}

pub(crate) fn sync_sources_scroll(model: &mut Model, terminal_area: Rect) {
    if !terminal_size_supported(terminal_area) {
        return;
    }
    let (sources, _) = main_panes(terminal_area);
    model.sources_scroll = sources_window_start(model, panel_inner(sources).height as usize);
}

pub(crate) fn main_pane_at(
    model: &Model,
    terminal_area: Rect,
    column: u16,
    row: u16,
) -> Option<MainPaneFocus> {
    if model.overlay != Overlay::None || !terminal_size_supported(terminal_area) {
        return None;
    }
    let (sources, logs) = main_panes(terminal_area);
    let position = Position::new(column, row);
    if sources.contains(position) {
        Some(MainPaneFocus::Sources)
    } else if logs.contains(position) {
        Some(MainPaneFocus::Logs)
    } else {
        None
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

    let visual_rows = source_visual_rows(model);
    let line = row.saturating_sub(content.y) as usize
        + sources_window_start(model, content.height as usize);
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

    let log_border_style = if main_focus_active && pane_focus == MainPaneFocus::Logs {
        theme.accent()
    } else {
        theme.border()
    };
    let log_block = Block::default()
        .title(" Logs ")
        .borders(Borders::ALL)
        .border_style(log_border_style);

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
    draw_scrollbar(
        frame,
        right_border_track(logs_area),
        ScrollWindow {
            total: viewport.total_rows,
            visible: inner.height as usize,
            start: viewport.first_row,
        },
        log_border_style,
    );
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
    fn main_pane_at_maps_both_panes_and_rejects_overlays() {
        let mut model = model_with_subscription();
        let area = Rect::new(0, 0, 90, 20);
        assert_eq!(
            main_pane_at(&model, area, 0, 3),
            Some(MainPaneFocus::Sources)
        );
        assert_eq!(
            main_pane_at(&model, area, 45, 18),
            Some(MainPaneFocus::Logs)
        );
        assert_eq!(main_pane_at(&model, area, 45, 1), None);
        assert_eq!(main_pane_at(&model, area, 45, 19), None);
        assert_eq!(
            main_pane_at(&model, Rect::new(0, 0, TWO_PANE_MIN_WIDTH - 1, 20), 88, 5),
            Some(MainPaneFocus::Sources)
        );
        model.overlay = Overlay::Help(crate::app::model::HelpState::default());
        assert_eq!(main_pane_at(&model, area, 0, 3), None);
    }

    fn model_with_overflowing_panes() -> Model {
        let profiles = (0..40)
            .map(|index| {
                Profile::new_vless(
                    format!("Profile {index}"),
                    format!("10.0.0.{index}"),
                    443,
                    format!("u{index}"),
                )
            })
            .collect();
        let mut model = model_with_profiles(profiles);
        for index in 0..80 {
            model.push_log(format!("line {index}"));
        }
        model.selected = 30;
        model
    }

    #[test]
    fn log_record_and_text_selection_styles_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.push_log((0..300).map(|index| format!("word{index:03} ")).collect());
        model.push_log("tail".into());
        let area = Rect::new(0, 0, APP_WINDOW_COLS, APP_WINDOW_ROWS);
        let viewport = log_viewport(&model, area).unwrap();
        let mut navigation = LogNavigation::default();
        let column = viewport.area.x + 3;
        let row = viewport.area.y + 2;
        navigation.select_at(&model, &viewport, column, row, Instant::now());
        let mut selection = LogSelection::start(viewport, column, row).unwrap();
        for name in ["clicked", "dragging"] {
            if name == "dragging" {
                selection.update(column + 8, row);
            }
            let buffer = render_to_buffer(area.width, area.height, |frame| {
                draw_with_interaction(
                    frame,
                    &model,
                    MainPaneFocus::Logs,
                    Some(&navigation),
                    Some(&selection),
                );
            });
            insta::assert_snapshot!(format!("log_{name}"), buffer_to_styled_string(&buffer));
        }
    }

    #[test]
    fn overflowing_panes_scroll_with_scrollbars_snapshot() {
        let model = model_with_overflowing_panes();
        let area = Rect::new(0, 0, APP_WINDOW_COLS, APP_WINDOW_ROWS);
        let mut navigation = LogNavigation::default();
        scroll_logs(&model, area, &mut navigation, -28, Instant::now());

        let buffer = render_to_buffer(APP_WINDOW_COLS, APP_WINDOW_ROWS, |frame| {
            draw_with_interaction(
                frame,
                &model,
                MainPaneFocus::Sources,
                Some(&navigation),
                None,
            )
        });
        insta::assert_snapshot!(buffer_to_styled_string(&buffer));
    }

    #[test]
    fn scrolled_profiles_list_keeps_its_offset_so_a_double_click_hits_one_profile() {
        let mut model = model_with_overflowing_panes();
        let area = Rect::new(0, 0, APP_WINDOW_COLS, APP_WINDOW_ROWS);
        sync_sources_scroll(&mut model, area);
        assert_eq!(model.sources_scroll, 2);

        model.sources_scroll = 11;
        for _ in 0..2 {
            let clicked = source_hit_test(&model, area, 2, 4).unwrap();
            assert_eq!(clicked, 11);
            model.selected = clicked;
            sync_sources_scroll(&mut model, area);
            assert_eq!(model.sources_scroll, 11);
        }

        model.selected = 0;
        sync_sources_scroll(&mut model, area);
        assert_eq!(model.sources_scroll, 0);
    }

    #[test]
    fn long_subscription_name_stays_on_one_line_snapshot() {
        let mut model = model_with_subscription();
        model.config.subscriptions[0].name =
            "A subscription name far too long to fit in the Profiles pane".into();
        insta::assert_snapshot!(snapshot_terminal(&model, TWO_PANE_MIN_WIDTH, 16));
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

mod log;
mod overlay;
mod panes;
mod scrollbar;
mod sources;
mod text;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

pub(crate) fn wheel_window(
    model: &crate::app::model::Model,
    area: ratatui::layout::Rect,
) -> Option<(usize, usize)> {
    use crate::app::model::Overlay;
    if !terminal_size_supported(area) {
        return None;
    }
    let (rows, selected) = crate::app::scroll::list(model)?;
    let visible = match model.overlay {
        Overlay::None => {
            let (sources, _) = panes::main_panes(area);
            let visible = panes::panel_inner(sources).height as usize;
            return Some((sources::sources_window_start(model, visible), visible));
        }
        Overlay::Help(state) => {
            let visible = overlay::help::popup_area(area, rows.len())
                .height
                .saturating_sub(4) as usize;
            return Some((
                overlay::help::window_start(
                    &crate::ui::help::rows(state.context),
                    visible,
                    selected,
                    model.overlay_scroll,
                ),
                visible,
            ));
        }
        Overlay::ThemeSettings
        | Overlay::GeoRegions
        | Overlay::RoutingMode
        | Overlay::DnsSettings => overlay::popup::selection_visible(area, rows.len()),
        _ => rows.len(),
    };
    Some((
        overlay::popup::selection_window_start(rows.len(), visible, selected, model.overlay_scroll),
        visible,
    ))
}

pub(crate) fn sync_overlay_scroll(model: &mut Model, area: Rect) {
    if model.overlay_scroll.is_some()
        && let Some((start, _)) = wheel_window(model, area)
    {
        model.overlay_scroll = Some(start);
    }
}

#[cfg(test)]
use crate::app::model::AppStatus;
use crate::app::model::{MainPaneFocus, Model};
use crate::ui::widgets::Toast;

use panes::{draw_main, draw_status_bar, draw_traffic_panel};

pub(crate) use log::navigation::{LogNavigation, LogSelection};
pub(crate) use panes::{
    log_viewport, log_viewport_with_navigation, logs_visible, main_pane_at, scroll_logs,
    source_hit_test, sync_log_scroll, sync_sources_scroll,
};

/// Height (including borders) of the full-width traffic header rendered at
/// the very top of the UI when the VPN is connected. One content row plus
/// top/bottom borders = 3 lines.
const TRAFFIC_PANEL_HEIGHT: u16 = 3;

pub(crate) const MIN_TERMINAL_WIDTH: u16 = 70;
pub(crate) const MIN_TERMINAL_HEIGHT: u16 = 15;

/// Render the full application UI into the terminal frame.
#[cfg(test)]
pub(crate) fn draw(frame: &mut Frame, model: &Model) {
    draw_impl(frame, model, MainPaneFocus::Sources, None, None, None);
}

#[cfg(test)]
pub(crate) fn draw_with_log_selection(
    frame: &mut Frame,
    model: &Model,
    log_selection: Option<&LogSelection>,
) {
    draw_impl(
        frame,
        model,
        MainPaneFocus::Sources,
        None,
        log_selection,
        None,
    );
}

#[cfg(test)]
pub(crate) fn draw_with_interaction(
    frame: &mut Frame,
    model: &Model,
    pane_focus: MainPaneFocus,
    log_navigation: Option<&LogNavigation>,
    log_selection: Option<&LogSelection>,
) {
    draw_impl(
        frame,
        model,
        pane_focus,
        log_navigation,
        log_selection,
        None,
    );
}

pub(crate) fn draw_with_toast(
    frame: &mut Frame,
    model: &Model,
    pane_focus: MainPaneFocus,
    log_navigation: Option<&LogNavigation>,
    log_selection: Option<&LogSelection>,
    toast: Option<&crate::app::model::AppStatus>,
) {
    draw_impl(
        frame,
        model,
        pane_focus,
        log_navigation,
        log_selection,
        toast,
    );
}

fn draw_impl(
    frame: &mut Frame,
    model: &Model,
    pane_focus: MainPaneFocus,
    log_navigation: Option<&LogNavigation>,
    log_selection: Option<&LogSelection>,
    toast: Option<&crate::app::model::AppStatus>,
) {
    let area = frame.area();

    // Paint the palette's background across the whole frame first so that
    // every widget below — most of which set only `fg` — inherits a
    // theme-consistent background instead of the terminal default. Popups
    // override with their own `popup_bg()` (currently the same color).
    frame.render_widget(Block::default().style(model.theme.background()), area);

    if !terminal_size_supported(area) {
        draw_terminal_too_small(frame, model, area);
        return;
    }

    // Top-level vertical layout: traffic header, main content, status bar. The
    // minimum-size guard above guarantees enough room for every section.
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(TRAFFIC_PANEL_HEIGHT),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);
    let (traffic_area, main_area, status_area) = (split[0], split[1], split[2]);

    draw_traffic_panel(frame, model, traffic_area);
    draw_main(
        frame,
        model,
        main_area,
        pane_focus,
        log_navigation,
        log_selection,
    );
    draw_status_bar(frame, model, status_area);

    overlay::draw(frame, model, area);

    if let Some(status) = toast {
        let toast = Toast::new(status, &model.theme);
        if let Some(area) = toast.area(frame.area()) {
            frame.render_widget(toast, area);
        }
    }
}

pub(crate) fn terminal_size_supported(area: Rect) -> bool {
    area.width >= MIN_TERMINAL_WIDTH && area.height >= MIN_TERMINAL_HEIGHT
}

fn draw_terminal_too_small(frame: &mut Frame, model: &Model, area: Rect) {
    let message_height = area.height.min(3);
    let message_area = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(message_height) / 2,
        area.width,
        message_height,
    );
    let message = Paragraph::new(vec![
        Line::from(Span::styled("Terminal too small", model.theme.error())),
        Line::from(Span::styled(
            format!(
                "Minimum: {MIN_TERMINAL_WIDTH}×{MIN_TERMINAL_HEIGHT}  Current: {}×{}",
                area.width, area.height
            ),
            model.theme.normal(),
        )),
        Line::from(Span::styled(
            "Increase the window size",
            model.theme.normal(),
        )),
    ])
    .alignment(Alignment::Center);
    frame.render_widget(message, message_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::{ConnectionState, Overlay};
    use crate::config::profile::Profile;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_profiles, model_with_subscription,
        render_to_string, snapshot_styles, snapshot_terminal,
    };

    #[test]
    fn help_keyboard_round_trip_restores_the_leading_heading_offset() {
        use crate::app::model::{HelpState, Overlay};
        use crate::app::msg::Msg;
        use crossterm::event::{KeyCode, KeyEvent};

        let area = Rect::new(0, 0, APP_WINDOW_COLS, APP_WINDOW_ROWS);
        let rows = crate::ui::help::rows(HelpState::default().context);
        for offset in [None, Some(0)] {
            let mut model = model_with_profiles(vec![]);
            model.overlay = Overlay::Help(HelpState::default());
            model.overlay_scroll = offset;
            for key in ['j', 'k'] {
                for _ in 0..rows.len() {
                    crate::app::update::update(
                        &mut model,
                        Msg::Key(KeyEvent::from(KeyCode::Char(key))),
                    );
                    sync_overlay_scroll(&mut model, area);
                }
                if key == 'j' {
                    assert!(wheel_window(&model, area).unwrap().0 > 0);
                }
            }
            assert_eq!(wheel_window(&model, area).unwrap().0, 0);
            assert_eq!(model.overlay_scroll, offset);
        }
    }

    #[test]
    fn overlay_keyboard_navigation_retains_and_advances_the_scroll_offset() {
        let _lock = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let mut model = crate::test_helpers::model_with_profiles(vec![]);
        model.overlay = crate::app::model::Overlay::ThemeSettings;
        model.overlay_scroll = Some(8);
        crate::app::scroll::select(&mut model, 8);
        let area = Rect::new(0, 0, MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT);
        let (_, visible) = wheel_window(&model, area).unwrap();
        for step in 1..=visible {
            crate::app::update::update(
                &mut model,
                crate::app::msg::Msg::Key(crossterm::event::KeyEvent::from(
                    crossterm::event::KeyCode::Char('j'),
                )),
            );
            sync_overlay_scroll(&mut model, area);
            assert_eq!(
                model.overlay_scroll,
                Some(if step < visible { 8 } else { 9 })
            );
        }
        crate::app::update::update(
            &mut model,
            crate::app::msg::Msg::Key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Char('k'),
            )),
        );
        sync_overlay_scroll(&mut model, area);
        assert_eq!(model.overlay_scroll, Some(9));
    }

    #[test]
    fn toast_renders_over_the_top_right_corner() {
        let model = model_with_profiles(vec![]);
        let status = crate::app::model::AppStatus::Info("Saved".into());
        let output = render_to_string(80, 20, |frame| {
            draw_with_toast(
                frame,
                &model,
                MainPaneFocus::Sources,
                None,
                None,
                Some(&status),
            )
        });
        assert!(output.lines().nth(2).unwrap().contains("Saved"));
    }

    #[test]
    fn toast_remains_visible_when_logs_are_hidden() {
        let model = model_with_profiles(vec![]);
        let status = crate::app::model::AppStatus::Info("Saved".into());
        let output = render_to_string(71, 20, |frame| {
            draw_with_toast(
                frame,
                &model,
                MainPaneFocus::Sources,
                None,
                None,
                Some(&status),
            )
        });
        assert!(output.contains("Saved"));
        assert!(!output.contains("Logs"));
    }

    #[test]
    fn overlay_shows_toast() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::ConfirmDelete;
        let status = crate::app::model::AppStatus::Error("Must not appear".into());
        let output = render_to_string(80, 20, |frame| {
            draw_with_toast(
                frame,
                &model,
                MainPaneFocus::Sources,
                None,
                None,
                Some(&status),
            )
        });
        assert!(output.contains("Must not appear"));
    }

    #[test]
    fn settings_menu_shows_error_and_info_toasts() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::SettingsMenu(crate::app::model::SettingsMenuPage::Root);
        let error = AppStatus::Error("Failed to save config".into());
        let rendered = render_to_string(80, 20, |frame| {
            draw_with_toast(
                frame,
                &model,
                MainPaneFocus::Sources,
                None,
                None,
                Some(&error),
            )
        });
        assert!(rendered.contains("Failed to save config"));

        let info = AppStatus::Info("Saved".into());
        let rendered = render_to_string(80, 20, |frame| {
            draw_with_toast(
                frame,
                &model,
                MainPaneFocus::Sources,
                None,
                None,
                Some(&info),
            )
        });
        assert!(rendered.contains("Saved"));
    }

    #[test]
    fn draw_main_snapshot() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Alpha".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.logs.push_back("log line 1".to_string());
        model.logs.push_back("log line 2".to_string());
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(model.config.profiles[0].id);
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn minimum_terminal_size_is_inclusive() {
        assert!(terminal_size_supported(Rect::new(
            0,
            0,
            MIN_TERMINAL_WIDTH,
            MIN_TERMINAL_HEIGHT
        )));
        assert!(!terminal_size_supported(Rect::new(
            0,
            0,
            MIN_TERMINAL_WIDTH - 1,
            MIN_TERMINAL_HEIGHT
        )));
        assert!(!terminal_size_supported(Rect::new(
            0,
            0,
            MIN_TERMINAL_WIDTH,
            MIN_TERMINAL_HEIGHT - 1
        )));
    }

    #[test]
    fn undersized_terminal_shows_resize_message_instead_of_ui() {
        let model = model_with_subscription();
        for (width, height) in [
            (MIN_TERMINAL_WIDTH - 1, MIN_TERMINAL_HEIGHT),
            (MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT - 1),
        ] {
            let area = Rect::new(0, 0, width, height);
            let output = snapshot_terminal(&model, width, height);

            assert_eq!(source_hit_test(&model, area, 2, 4), None);
            assert!(log_viewport(&model, area).is_none());
            insta::with_settings!({snapshot_suffix => format!("{width}x{height}")}, {
                insta::assert_snapshot!(output);
            });
        }
    }

    #[test]
    fn exact_minimum_size_draws_the_single_pane_ui() {
        let model = model_with_subscription();
        insta::assert_snapshot!(snapshot_terminal(
            &model,
            MIN_TERMINAL_WIDTH,
            MIN_TERMINAL_HEIGHT
        ));
    }

    #[test]
    fn light_theme_uses_theme_foreground_for_help_rows_and_empty_sources() {
        let mut model = model_with_profiles(vec![]);
        model.theme = crate::ui::styles::Theme::resolve("catppuccin-latte");
        model.overlay = Overlay::Help(crate::app::model::HelpState::default());

        insta::with_settings!({snapshot_suffix => "help"}, {
            insta::assert_snapshot!(snapshot_styles(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
        });

        model.overlay = Overlay::None;
        insta::with_settings!({snapshot_suffix => "empty-sources"}, {
            insta::assert_snapshot!(snapshot_styles(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
        });
    }
}

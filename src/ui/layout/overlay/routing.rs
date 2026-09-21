use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::model::Model;
use crate::ui::layout::text::visual_width;

use super::popup::{draw_selection_modal, settings_content_sized_rect};
use super::settings_row::{settings_value_layout, settings_value_line};
use super::{APPLY_ACTION, overlay_footer, settings_overlay_footer};

/// Draw the routing mode selection modal.
pub(super) fn draw_mode(frame: &mut Frame, model: &Model, area: Rect) {
    let modes = model.config.settings.geo_routing.available_modes();
    let label_strings: Vec<String> = modes.iter().map(|m| m.to_string()).collect();
    let labels: Vec<&str> = label_strings.iter().map(String::as_str).collect();
    let active = modes
        .iter()
        .position(|m| *m == model.config.settings.geo_routing.mode());
    draw_selection_modal(
        frame,
        &model.theme,
        area,
        "Settings › Routing › Mode",
        &labels,
        model.routing_selected,
        active,
        settings_overlay_footer(model),
    );
}

/// Draw the geo region selection modal.
pub(super) fn draw_region(frame: &mut Frame, model: &Model, area: Rect) {
    use crate::config::profile::GeoRegion;
    let labels = GeoRegion::ALL.map(geo_region_label);
    let active = model
        .config
        .settings
        .geo_routing
        .current_region
        .and_then(|r| GeoRegion::ALL.iter().position(|x| *x == r));
    draw_selection_modal(
        frame,
        &model.theme,
        area,
        "Settings › Routing › Region",
        &labels,
        model.geo_region_selected,
        active,
        if model.settings_menu_return.is_some() {
            settings_overlay_footer(model)
        } else if model.config.settings.geo_routing.current_region.is_some() {
            overlay_footer(model, Some(APPLY_ACTION), true, false)
        } else {
            overlay_footer(model, Some(APPLY_ACTION), false, false)
        },
    );
}

pub(super) fn geo_region_label(region: crate::config::profile::GeoRegion) -> &'static str {
    use crate::config::profile::GeoRegion;

    match region {
        GeoRegion::Ru => "🇷🇺 Russia",
        GeoRegion::Cn => "🇨🇳 China",
        GeoRegion::Ir => "🇮🇷 Iran",
        GeoRegion::Global => "🌍 Global",
    }
}

pub(super) fn draw_services(frame: &mut Frame, model: &Model, area: Rect) {
    use crate::config::profile::RoutedService;

    let theme = &model.theme;
    let committed = &model.config.settings.geo_routing.service_routes;

    let heading = "Settings › Routing › Services";
    let footer = settings_overlay_footer(model);
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(heading, theme.accent())).centered(),
        Line::from(""),
    ];

    let settings = RoutedService::ALL.map(|service| {
        let saved = committed.get(&service).copied().unwrap_or_default();
        let shown = match model.service_routing_draft.as_ref() {
            Some(draft) => draft.get(&service).copied().unwrap_or_default(),
            None => saved,
        };
        (service.label(), shown.label().to_string(), shown != saved)
    });
    let stable_value_width = ["Disabled", "Proxy", "Direct"]
        .into_iter()
        .map(visual_width)
        .max()
        .unwrap_or(0);
    let layout = settings_value_layout(
        &settings,
        stable_value_width,
        area,
        &[visual_width(heading), visual_width(&footer)],
    );
    for (index, (name, value, dirty)) in settings.into_iter().enumerate() {
        lines.push(settings_value_line(
            name,
            &value,
            dirty,
            index == model.service_routing_selected,
            layout,
            theme,
        ));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(footer).centered());

    let popup_area = settings_content_sized_rect(area, layout.popup_width, &lines);
    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.accent())
        .style(theme.popup_bg());
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);
    frame.render_widget(Paragraph::new(lines).style(theme.normal()), inner);
}

#[cfg(test)]
mod tests {

    use crate::app::model::Overlay;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_profiles, snapshot_styles, snapshot_terminal,
    };
    use crate::ui::layout::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH};

    #[test]
    fn draw_routing_mode_overlay_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model
            .config
            .settings
            .geo_routing
            .set_region(crate::config::profile::GeoRegion::Ru);
        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 2;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn selection_overlay_styles_active_row_like_connected_profile() {
        let mut model = model_with_profiles(vec![]);
        model
            .config
            .settings
            .geo_routing
            .set_region(crate::config::profile::GeoRegion::Ru);
        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 2;

        insta::with_settings!({snapshot_suffix => "active-row"}, {
            insta::assert_snapshot!(snapshot_styles(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
        });

        model.routing_selected = 0;
        insta::with_settings!({snapshot_suffix => "active-row-selected"}, {
            insta::assert_snapshot!(snapshot_styles(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
        });
    }

    #[test]
    fn draw_geo_region_overlay_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.overlay = Overlay::GeoRegions;
        model.geo_region_selected = 1;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_geo_region_overlay_at_minimum_terminal_size_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::GeoRegions;
        model.geo_region_selected = 1;
        insta::assert_snapshot!(snapshot_terminal(
            &model,
            MIN_TERMINAL_WIDTH,
            MIN_TERMINAL_HEIGHT
        ));
    }

    #[test]
    fn geo_region_footer_only_offers_close_after_initial_selection() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::GeoRegions;

        let required = snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS);
        assert!(required.contains("󰌑 apply"));
        assert!(!required.contains("q/󱊷 close"));
        assert!(!required.contains("j/k navigate"));

        model
            .config
            .settings
            .geo_routing
            .set_region(crate::config::profile::GeoRegion::Ru);
        let optional = snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS);
        assert!(optional.contains("󰌑 apply · q/󱊷 close"));
        assert!(!optional.contains("j/k navigate"));
    }

    /// Service routing overlay with the cursor on the second row and an
    /// uncommitted draft change (`*`). Covers the Direct and Proxy
    /// renderings; Disabled is pinned by the `_no_edits` snapshot below.
    #[test]
    fn draw_service_routing_overlay_snapshot() {
        use crate::config::profile::{RoutedService, ServiceRoute};
        use std::collections::HashMap;

        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.overlay = Overlay::ServiceRouting;
        model.service_routing_selected = 1;
        // Steam is committed as Direct; the Telegram→Proxy edit is
        // draft-only, so its row carries the dirty marker.
        model
            .config
            .settings
            .geo_routing
            .service_routes
            .insert(RoutedService::Steam, ServiceRoute::Direct);
        model.service_routing_draft = Some(HashMap::from([
            (RoutedService::Steam, ServiceRoute::Direct),
            (RoutedService::Telegram, ServiceRoute::Proxy),
        ]));
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    /// Freshly opened overlay with no committed routes: every row renders
    /// Disabled (an absent draft entry), no dirty markers.
    #[test]
    fn draw_service_routing_overlay_no_edits_snapshot() {
        use std::collections::HashMap;

        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.overlay = Overlay::ServiceRouting;
        model.service_routing_selected = 0;
        model.service_routing_draft = Some(HashMap::new());
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    /// Routing-mode overlay in Global region: `available_modes()` returns a
    /// single entry, exercising the small-list rendering path.
    #[test]
    fn draw_routing_mode_overlay_global_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model
            .config
            .settings
            .geo_routing
            .set_region(crate::config::profile::GeoRegion::Global);
        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 0;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }
}

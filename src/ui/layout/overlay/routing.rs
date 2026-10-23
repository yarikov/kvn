use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::model::{Model, SettingsMenuEntry};
use crate::onboarding::OnboardingStep;

use super::popup::draw_selection_modal;
use super::{APPLY_ACTION, overlay_footer, settings_heading};

pub(super) fn draw_mode(frame: &mut Frame, model: &Model, area: Rect) {
    let modes = model.routing.mode_ids();
    let label_strings: Vec<String> = modes.iter().map(|id| model.routing.mode_name(id)).collect();
    let labels: Vec<&str> = label_strings.iter().map(String::as_str).collect();
    let selected = model.config.settings.geo_routing.mode(&model.routing);
    let active = (!awaits(model, OnboardingStep::Routing))
        .then(|| modes.iter().position(|id| *id == selected))
        .flatten();
    draw_selection_modal(
        frame,
        &model.theme,
        area,
        &format!("{} › Mode", settings_heading(SettingsMenuEntry::Routing)),
        &labels,
        model.routing_selected,
        model.overlay_scroll,
        active,
        overlay_footer(model, Some(APPLY_ACTION), false, false),
    );
}

pub(super) fn draw_region(frame: &mut Frame, model: &Model, area: Rect) {
    use crate::config::profile::GeoRegion;
    let labels = GeoRegion::ALL.map(geo_region_label);
    draw_selection_modal(
        frame,
        &model.theme,
        area,
        &format!("{} › Region", settings_heading(SettingsMenuEntry::Routing)),
        &labels,
        model.geo_region_selected,
        model.overlay_scroll,
        None,
        overlay_footer(model, Some(APPLY_ACTION), false, false),
    );
}

fn awaits(model: &Model, step: OnboardingStep) -> bool {
    model.onboarding.awaiting == Some(step)
}

fn geo_region_label(region: crate::config::profile::GeoRegion) -> &'static str {
    use crate::config::profile::GeoRegion;

    match region {
        GeoRegion::Ru => "🇷🇺 Russia",
        GeoRegion::Cn => "🇨🇳 China",
        GeoRegion::Ir => "🇮🇷 Iran",
        GeoRegion::Global => "🌍 Global",
    }
}

#[cfg(test)]
mod tests {

    use crate::app::model::Overlay;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_profiles, snapshot_styles, snapshot_terminal,
    };
    use crate::ui::layout::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH};

    #[test]
    fn the_tour_pickers_drop_the_active_highlight() {
        use crate::onboarding::OnboardingStep;

        for (step, overlay) in [
            (OnboardingStep::Region, Overlay::GeoRegions),
            (OnboardingStep::Routing, Overlay::RoutingMode),
        ] {
            let mut model = model_with_profiles(vec![]);
            model
                .routing
                .add_region_modes(crate::config::profile::GeoRegion::Ru);
            model.overlay = overlay;
            model.onboarding.awaiting = Some(step);

            insta::with_settings!({snapshot_suffix => format!("{step:?}").to_lowercase()}, {
                insta::assert_snapshot!(snapshot_styles(
                    &model,
                    APP_WINDOW_COLS,
                    APP_WINDOW_ROWS
                ));
            });
        }
    }

    #[test]
    fn draw_routing_mode_overlay_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model
            .routing
            .add_region_modes(crate::config::profile::GeoRegion::Ru);
        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 2;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn selection_overlay_styles_active_row_like_connected_profile() {
        let mut model = model_with_profiles(vec![]);
        model
            .routing
            .add_region_modes(crate::config::profile::GeoRegion::Ru);
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
    fn draw_routing_mode_overlay_global_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 0;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }
}

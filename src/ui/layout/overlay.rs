mod confirm_delete;
mod dns;
mod help;
mod migration;
pub(super) mod popup;
mod routing;
mod settings_menu;
mod settings_row;
mod support;
mod theme;

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::model::{Model, Overlay};
use crate::ui::icons::icons;

pub(super) const APPLY_ACTION: &str = "apply";
pub(super) const CONFIRM_ACTION: &str = "confirm";
pub(super) const BACK_ACTION: &str = "⌫ back";

pub(super) fn draw(frame: &mut Frame, model: &Model, area: Rect) {
    match model.overlay {
        Overlay::Help(state) => help::draw(frame, model, state, area),
        Overlay::SettingsMenu(page) => settings_menu::draw(frame, model, page, area),
        Overlay::ConfirmDelete => confirm_delete::draw(frame, model, area),
        Overlay::RoutingMode => routing::draw_mode(frame, model, area),
        Overlay::GeoRegions => routing::draw_region(frame, model, area),
        Overlay::DnsSettings => dns::draw(frame, model, area),
        Overlay::ThemeSettings => theme::draw(frame, model, area),
        Overlay::ServiceRouting => routing::draw_services(frame, model, area),
        Overlay::Support => support::draw(frame, model, area),
        Overlay::Migration => migration::draw(frame, model, area),
        Overlay::None => {}
    }
}

pub(super) fn overlay_footer(
    model: &Model,
    primary: Option<&str>,
    close: bool,
    back: bool,
) -> String {
    let icons = icons(model.icon_set());
    let mut actions = Vec::with_capacity(3);
    if let Some(primary) = primary {
        actions.push(format!("{} {primary}", icons.enter));
    }
    if close {
        actions.push(format!("q/{} close", icons.esc));
    }
    if back {
        actions.push(BACK_ACTION.to_string());
    }
    actions.join(" · ")
}

pub(super) fn settings_overlay_footer(model: &Model) -> String {
    overlay_footer(
        model,
        Some(APPLY_ACTION),
        true,
        model.settings_menu_return.is_some(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::Profile;
    use crate::test_helpers::{model_with_profiles, snapshot_terminal};
    use crate::ui::layout::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH};

    #[test]
    fn overlays_render_at_the_minimum_terminal_size() {
        use crate::app::model::{MigrationPhase, MigrationStatus, SettingsMenuPage};

        type Setup = fn(&mut Model);

        let cases: Vec<(&str, Setup)> = vec![
            ("settings-menu", |model: &mut Model| {
                model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
            }),
            ("confirm-delete", |model: &mut Model| {
                model.overlay = Overlay::ConfirmDelete
            }),
            ("routing-mode", |model: &mut Model| {
                model
                    .config
                    .settings
                    .geo_routing
                    .set_region(crate::config::profile::GeoRegion::Ru);
                model.overlay = Overlay::RoutingMode;
                model.routing_selected = 2;
            }),
            ("dns-settings", |model: &mut Model| {
                model.overlay = Overlay::DnsSettings;
                model.dns_selected = 1;
            }),
            ("service-routing", |model: &mut Model| {
                model.overlay = Overlay::ServiceRouting;
                model.service_routing_selected = 1;
            }),
            ("migration", |model: &mut Model| {
                model.overlay = Overlay::Migration;
                model.migration = Some(MigrationStatus {
                    session_id: "session".into(),
                    phase: MigrationPhase::Failed,
                    completed: 2,
                    total: 4,
                    summary: "Updating integration".into(),
                    error: Some("sudo command failed".into()),
                });
            }),
        ];

        for (label, setup) in cases {
            let mut model = model_with_profiles(vec![Profile::new_vless(
                "Alpha".to_string(),
                "1.1.1.1".to_string(),
                443,
                "u1".to_string(),
            )]);
            model.geo_last_updated = Some("2026-05-31 13:41".to_string());
            setup(&mut model);
            insta::with_settings!({snapshot_suffix => label}, {
                insta::assert_snapshot!(snapshot_terminal(
                    &model,
                    MIN_TERMINAL_WIDTH,
                    MIN_TERMINAL_HEIGHT
                ));
            });
        }
    }
}

mod confirm_delete;
mod confirm_disable;
mod dns;
pub(super) mod help;
mod onboarding;
pub(super) mod popup;
mod routing;
mod routing_settings;
mod settings_menu;
mod settings_row;
mod support;

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::model::{Model, Overlay, SettingsMenuEntry};
use crate::ui::icons::icons;

pub(super) const SETTINGS_HEADING: &str = "Settings";
pub(super) const APPLY_ACTION: &str = "apply";
pub(super) const CONFIRM_ACTION: &str = "confirm";
pub(super) const BACK_ACTION: &str = "⌫ back";
pub(super) const EDIT_ROUTING_ACTION: &str = "e edit";

pub(super) fn draw(frame: &mut Frame, model: &Model, area: Rect) {
    match model.overlay {
        Overlay::Help(state) => help::draw(frame, model, state, area),
        Overlay::SettingsMenu(page) => settings_menu::draw(frame, model, page, area),
        Overlay::ConfirmDelete => confirm_delete::draw(frame, model, area),
        Overlay::ConfirmDisable(target) => confirm_disable::draw(frame, model, target, area),
        Overlay::RoutingMode => routing::draw_mode(frame, model, area),
        Overlay::GeoRegions => routing::draw_region(frame, model, area),
        Overlay::DnsSettings => dns::draw(frame, model, area),
        Overlay::Support => support::draw(frame, model, area),
        Overlay::Onboarding(step) => onboarding::draw(frame, model, step, area),
        Overlay::None => {}
    }
}

pub(super) fn settings_heading(entry: SettingsMenuEntry) -> String {
    format!("{SETTINGS_HEADING} › {}", entry.label())
}

pub(super) fn overlay_footer(
    model: &Model,
    primary: Option<&str>,
    close: bool,
    back: bool,
) -> String {
    overlay_footer_with_key(model, primary, None, close, back)
}

pub(super) fn overlay_footer_with_key(
    model: &Model,
    primary: Option<&str>,
    key_action: Option<&str>,
    close: bool,
    back: bool,
) -> String {
    let icons = icons(model.icon_set());
    let mut actions = Vec::with_capacity(4);
    if let Some(primary) = primary {
        actions.push(format!("{} {primary}", icons.enter));
    }
    if let Some(key_action) = key_action {
        actions.push(key_action.to_string());
    }
    if close {
        actions.push(format!("q/{} close", icons.esc));
    }
    if back {
        actions.push(BACK_ACTION.to_string());
    }
    actions.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::Profile;
    use crate::test_helpers::{model_with_profiles, snapshot_terminal};
    use crate::ui::layout::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH};

    #[test]
    fn overlays_render_at_the_minimum_terminal_size() {
        use crate::app::model::SettingsMenuPage;

        type Setup = fn(&mut Model);

        let cases: Vec<(&str, Setup)> = vec![
            ("settings-menu", |model: &mut Model| {
                model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
            }),
            ("confirm-delete", |model: &mut Model| {
                model.overlay = Overlay::ConfirmDelete
            }),
            ("confirm-disable", |model: &mut Model| {
                model.config.settings.kill_switch = true;
                model.overlay =
                    Overlay::ConfirmDisable(crate::app::model::DisableTarget::KillSwitch)
            }),
            ("routing-mode", |model: &mut Model| {
                model
                    .routing
                    .add_region_modes(crate::config::profile::GeoRegion::Ru);
                model.overlay = Overlay::RoutingMode;
                model.routing_selected = 2;
            }),
            ("dns-settings", |model: &mut Model| {
                model.overlay = Overlay::DnsSettings;
                model.dns_selected = 1;
            }),
            ("onboarding", |model: &mut Model| {
                model.overlay = Overlay::Onboarding(crate::onboarding::OnboardingStep::AutoConnect);
            }),
        ];

        for (label, setup) in cases {
            let mut model = model_with_profiles(vec![Profile::new_vless(
                "Alpha".to_string(),
                "1.1.1.1".to_string(),
                443,
                "u1".to_string(),
            )]);
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

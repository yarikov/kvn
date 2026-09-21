//! Keyboard input dispatchers for every overlay state.
//!
//! Entry point is `handle_key`, which routes by `Model.overlay`. Each
//! overlay has its own handler that returns the same `Vec<Effect>`
//! contract as the top-level `update` function. All handlers stay pure —
//! they mutate the `Model` and return effects; actual I/O happens in
//! the daemon.

pub(super) mod confirm_delete;
pub(super) mod dns;
pub(super) mod ipc;
pub(super) mod regions;
pub(super) mod service_routing;
pub(super) mod settings_menu;
pub(super) mod sources;
pub(super) mod theme;

use crate::app::effect::Effect;
use crate::app::model::{HelpContext, HelpState, MainPaneFocus, Model, Overlay, SettingsMenuPage};
use confirm_delete::handle_confirm_delete;
use crossterm::event::{KeyCode, KeyEvent};
use dns::handle_dns_settings;
use regions::{handle_geo_region, handle_routing_mode};
use service_routing::handle_service_routing;
use settings_menu::handle_settings_menu;
use sources::handle_sources;
use theme::handle_theme_picker;

pub(in crate::app::update) fn handle_key(model: &mut Model, key: KeyEvent) -> Vec<Effect> {
    if model.overlay == Overlay::Migration {
        return vec![];
    }
    if key.code == KeyCode::Char('?') && !matches!(model.overlay, Overlay::Help(_)) {
        open_help(model);
        return vec![];
    }
    if key.code == KeyCode::Char(' ') && model.overlay == Overlay::None {
        model.settings_menu_return = None;
        model.settings_menu_selected = 0;
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
        return vec![];
    }
    match model.overlay {
        Overlay::None if model.main_pane_focus == MainPaneFocus::Sources => {
            handle_sources(model, key)
        }
        Overlay::None => handle_logs(model, key),
        Overlay::Help(state) => handle_help(model, state, key),
        Overlay::SettingsMenu(page) => handle_settings_menu(model, page, key),
        Overlay::ConfirmDelete => handle_confirm_delete(model, key),
        Overlay::RoutingMode => handle_routing_mode(model, key),
        Overlay::GeoRegions => handle_geo_region(model, key),
        Overlay::DnsSettings => handle_dns_settings(model, key),
        Overlay::ThemeSettings => handle_theme_picker(model, key),
        Overlay::ServiceRouting => handle_service_routing(model, key),
        // Navigation and activation are client-local because the selected
        // action may need to launch a browser in that client's GUI session.
        Overlay::Support => vec![],
        Overlay::Migration => vec![],
    }
}

fn handle_logs(model: &mut Model, key: KeyEvent) -> Vec<Effect> {
    match key.code {
        KeyCode::Char('m')
        | KeyCode::Char('o')
        | KeyCode::Char('D')
        | KeyCode::Char('S')
        | KeyCode::Char('C')
        | KeyCode::Char('a')
        | KeyCode::Char('K')
        | KeyCode::Char('r')
        | KeyCode::Char('s')
        | KeyCode::Char('I') => handle_sources(model, key),
        _ => vec![],
    }
}

fn open_help(model: &mut Model) {
    let context = match model.overlay {
        Overlay::None => match model.main_pane_focus {
            MainPaneFocus::Sources => HelpContext::Sources,
            MainPaneFocus::Logs => HelpContext::Logs,
        },
        Overlay::SettingsMenu(page) => HelpContext::SettingsMenu(page),
        Overlay::ConfirmDelete => HelpContext::ConfirmDelete,
        Overlay::RoutingMode => HelpContext::RoutingMode,
        Overlay::GeoRegions => HelpContext::GeoRegions,
        Overlay::DnsSettings => HelpContext::DnsSettings,
        Overlay::ThemeSettings => HelpContext::ThemeSettings,
        Overlay::ServiceRouting => HelpContext::ServiceRouting,
        Overlay::Support => HelpContext::Support,
        Overlay::Migration => return,
        Overlay::Help(_) => return,
    };
    model.overlay = Overlay::Help(HelpState {
        context,
        selected: crate::ui::help::first_command(&crate::ui::help::rows(context)),
    });
}

fn handle_help(model: &mut Model, mut state: HelpState, key: KeyEvent) -> Vec<Effect> {
    let rows = crate::ui::help::rows(state.context);
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            state.selected = crate::ui::help::next_command(&rows, state.selected);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.selected = crate::ui::help::previous_command(&rows, state.selected);
        }
        KeyCode::Char('G') => state.selected = crate::ui::help::last_command(&rows),
        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('?') => {
            model.overlay = state.context.restore_overlay();
            return vec![];
        }
        _ => {}
    }
    model.overlay = Overlay::Help(state);
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::ConnectionState;
    use crate::config::profile::{DnsPreset, DnsStrategy, Profile};
    use crate::test_helpers::*;

    #[test]
    fn help_only_closes_on_exit_keys() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::Help(crate::app::model::HelpState::default());
        assert!(handle_key(&mut model, key('x')).is_empty());
        assert!(matches!(model.overlay, Overlay::Help(_)));

        let effects = handle_key(&mut model, key('q'));
        assert_eq!(model.overlay, Overlay::None);
        assert!(effects.is_empty());
    }

    #[test]
    fn space_opens_settings_menu_from_both_main_panes() {
        for focus in [MainPaneFocus::Sources, MainPaneFocus::Logs] {
            let mut model = model_with_profiles(vec![]);
            model.main_pane_focus = focus;

            assert!(handle_key(&mut model, key(' ')).is_empty());
            assert_eq!(model.overlay, Overlay::SettingsMenu(SettingsMenuPage::Root));
        }
    }

    #[test]
    fn help_returns_to_the_same_settings_menu_page() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Routing);

        handle_key(&mut model, key('?'));
        assert!(matches!(
            model.overlay,
            Overlay::Help(HelpState {
                context: HelpContext::SettingsMenu(SettingsMenuPage::Routing),
                ..
            })
        ));

        handle_key(&mut model, key('?'));
        assert_eq!(
            model.overlay,
            Overlay::SettingsMenu(SettingsMenuPage::Routing)
        );
    }

    #[test]
    fn help_uses_pane_context_and_blocks_source_actions_in_logs() {
        let profile = Profile::new_vless("A".into(), "a".into(), 1, "u".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.main_pane_focus = MainPaneFocus::Logs;

        assert!(handle_key(&mut model, enter()).is_empty());
        assert_eq!(model.connection, ConnectionState::Idle);

        handle_key(&mut model, key('D'));
        assert_eq!(model.overlay, Overlay::DnsSettings);
        model.overlay = Overlay::None;

        let effects = handle_key(&mut model, key('a'));
        assert!(model.auto_connect_pending);
        assert_eq!(effects, vec![Effect::CheckAutoConnectPolkit]);

        let effects = handle_key(&mut model, key('K'));
        assert_eq!(model.kill_switch_pending, Some(true));
        assert!(effects.contains(&Effect::ApplyKillSwitch { enabled: true }));

        let previous_schedule = model.config.settings.geo_routing.auto_update;
        let effects = handle_key(&mut model, key('I'));
        assert_ne!(
            model.config.settings.geo_routing.auto_update,
            previous_schedule
        );
        assert!(effects.contains(&Effect::SaveConfig));

        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile_id);
        handle_key(&mut model, key('r'));
        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(profile_id));

        model.connection = ConnectionState::Connected;
        let effects = handle_key(&mut model, key('s'));
        assert_eq!(effects, vec![Effect::Disconnect]);

        handle_key(&mut model, key('?'));
        assert!(matches!(
            model.overlay,
            Overlay::Help(HelpState {
                context: HelpContext::Logs,
                selected: 1,
            })
        ));
    }

    #[test]
    fn help_scrolls_and_restores_overlay_draft() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::DnsSettings;
        model.dns_selected = 1;
        model.dns_preset_draft = Some(DnsPreset::GoogleDot);
        model.dns_strategy_draft = Some(DnsStrategy::PreferIpv6);

        handle_key(&mut model, key('?'));
        handle_key(&mut model, KeyEvent::from(KeyCode::Tab));
        assert!(matches!(
            model.overlay,
            Overlay::Help(HelpState {
                context: HelpContext::DnsSettings,
                selected: 1,
            })
        ));
        handle_key(&mut model, key('j'));
        assert!(matches!(
            model.overlay,
            Overlay::Help(HelpState {
                context: HelpContext::DnsSettings,
                selected: 2,
            })
        ));

        handle_key(&mut model, key('?'));
        assert_eq!(model.overlay, Overlay::DnsSettings);
        assert_eq!(model.dns_selected, 1);
        assert_eq!(model.dns_preset_draft, Some(DnsPreset::GoogleDot));
        assert_eq!(model.dns_strategy_draft, Some(DnsStrategy::PreferIpv6));
    }
}

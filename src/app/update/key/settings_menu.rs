use crate::app::effect::Effect;
use crate::app::model::{
    AppStatus, ConnectionSettingsDraft, Model, Overlay, RoutingSettingsDraft, RoutingSettingsItem,
    SettingsMenuPage,
};
use crate::config::profile::GeoRegion;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::update::key::theme::theme_picker_slugs;
use crate::app::update::routing::commit_routing_settings;
use crate::app::update::status::push_status;

pub(in crate::app::update) fn handle_settings_menu(
    model: &mut Model,
    page: SettingsMenuPage,
    key: KeyEvent,
) -> Vec<Effect> {
    let len = match page {
        SettingsMenuPage::Root => 4,
        SettingsMenuPage::Routing => model
            .routing_settings_draft
            .as_ref()
            .map(|draft| RoutingSettingsItem::available(draft.region).len())
            .unwrap_or(0),
        SettingsMenuPage::Connection => 2,
        SettingsMenuPage::Interface => 2,
    };
    match (page, key.code) {
        (_, KeyCode::Char('j') | KeyCode::Down) => {
            crate::ui::nav::select_next(&mut model.settings_menu_selected, len);
        }
        (_, KeyCode::Char('k') | KeyCode::Up) => {
            crate::ui::nav::select_prev(&mut model.settings_menu_selected);
        }
        (_, KeyCode::Char('G')) => {
            crate::ui::nav::select_last(&mut model.settings_menu_selected, len);
        }
        (SettingsMenuPage::Root, KeyCode::Char('r')) => {
            model.settings_menu_selected = 3;
            open_routing_settings(model);
        }
        (SettingsMenuPage::Root, KeyCode::Char('d')) => {
            model.settings_menu_selected = 1;
            open_dns_settings(model, Some(SettingsMenuPage::Root));
        }
        (SettingsMenuPage::Root, KeyCode::Char('c')) => {
            model.settings_menu_selected = 0;
            open_connection_settings(model);
        }
        (SettingsMenuPage::Root, KeyCode::Char('i')) => {
            open_interface_settings(model);
        }
        (SettingsMenuPage::Root, KeyCode::Enter) => match model.settings_menu_selected {
            0 => open_connection_settings(model),
            1 => open_dns_settings(model, Some(SettingsMenuPage::Root)),
            2 => open_interface_settings(model),
            3 => open_routing_settings(model),
            _ => {}
        },
        (SettingsMenuPage::Routing, KeyCode::Char('l') | KeyCode::Right) => {
            cycle_routing_draft(model, true);
        }
        (SettingsMenuPage::Routing, KeyCode::Char('h') | KeyCode::Left) => {
            cycle_routing_draft(model, false);
        }
        (SettingsMenuPage::Routing, KeyCode::Enter) => {
            let Some(draft) = model.routing_settings_draft.take() else {
                return vec![];
            };
            model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
            model.settings_menu_selected = 3;
            return commit_routing_settings(model, draft);
        }
        (SettingsMenuPage::Connection, KeyCode::Char('l') | KeyCode::Right) => {
            cycle_connection_draft(model);
        }
        (SettingsMenuPage::Connection, KeyCode::Char('h') | KeyCode::Left) => {
            cycle_connection_draft(model);
        }
        (SettingsMenuPage::Connection, KeyCode::Enter) => {
            let Some(draft) = model.connection_settings_draft.take() else {
                return vec![];
            };
            model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
            model.settings_menu_selected = 0;
            let mut effects = set_auto_connect(model, draft.auto_connect);
            effects.extend(set_kill_switch(model, draft.kill_switch));
            return effects;
        }
        (SettingsMenuPage::Interface, KeyCode::Char('l') | KeyCode::Right) => {
            cycle_interface_draft(model, true);
        }
        (SettingsMenuPage::Interface, KeyCode::Char('h') | KeyCode::Left) => {
            cycle_interface_draft(model, false);
        }
        (SettingsMenuPage::Interface, KeyCode::Enter) => {
            model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
            model.settings_menu_selected = 2;
            return commit_interface_settings(model);
        }
        (
            SettingsMenuPage::Routing | SettingsMenuPage::Connection | SettingsMenuPage::Interface,
            KeyCode::Backspace,
        ) => {
            model.settings_menu_selected = match page {
                SettingsMenuPage::Interface => 2,
                SettingsMenuPage::Routing => 3,
                _ => 0,
            };
            clear_settings_drafts(model);
            model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
        }
        (_, KeyCode::Char('q') | KeyCode::Esc) => {
            model.settings_menu_return = None;
            clear_settings_drafts(model);
            model.overlay = Overlay::None;
        }
        _ => {}
    }
    vec![]
}

fn open_routing_settings(model: &mut Model) {
    let geo_routing = &model.config.settings.geo_routing;
    let region = geo_routing.current_region.unwrap_or(GeoRegion::Global);
    model.routing_settings_draft = Some(RoutingSettingsDraft {
        region,
        mode: geo_routing.mode(),
        service_routes: geo_routing.service_routes.clone(),
    });
    model.settings_menu_selected = 0;
    model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Routing);
}

fn open_interface_settings(model: &mut Model) {
    model.theme_draft = None;
    model.interface_settings_draft = None;
    model.settings_menu_selected = 0;
    model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Interface);
}

fn cycle_interface_draft(model: &mut Model, forward: bool) {
    match model.settings_menu_selected {
        0 => {
            let slugs = theme_picker_slugs();
            if slugs.is_empty() {
                return;
            }
            let shown = model
                .theme_draft
                .as_deref()
                .unwrap_or(&model.config.settings.theme);
            let next = match slugs.iter().position(|slug| slug == shown) {
                Some(current) if forward => (current + 1) % slugs.len(),
                Some(current) => (current + slugs.len() - 1) % slugs.len(),
                None => 0,
            };
            model.theme_draft = Some(slugs[next].clone());
        }
        1 => model.interface_settings_draft = Some(model.icon_set().next()),
        _ => {}
    }
}

fn commit_interface_settings(model: &mut Model) -> Vec<Effect> {
    let mut effects = vec![];
    if let Some(icons) = model.interface_settings_draft.take()
        && icons != model.config.settings.icons
    {
        model.config.settings.icons = icons;
        effects.push(Effect::SaveConfig);
    }
    if let Some(slug) = model.theme_draft.take()
        && slug != model.config.settings.theme
    {
        model.config.settings.theme = slug.clone();
        if effects.is_empty() {
            effects.push(Effect::SaveConfig);
        }
        push_status(
            &mut effects,
            model,
            AppStatus::Info(format!("Theme: {slug}")),
        );
    }
    effects
}

fn open_connection_settings(model: &mut Model) {
    model.connection_settings_draft = Some(ConnectionSettingsDraft {
        auto_connect: model.config.settings.auto_connect,
        kill_switch: model
            .kill_switch_pending
            .unwrap_or(model.config.settings.kill_switch),
    });
    model.settings_menu_selected = 0;
    model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Connection);
}

fn clear_settings_drafts(model: &mut Model) {
    model.routing_settings_draft = None;
    model.connection_settings_draft = None;
    model.interface_settings_draft = None;
    model.theme_draft = None;
}

fn cycle_routing_draft(model: &mut Model, forward: bool) {
    use crate::config::profile::ServiceRoute;

    let Some(draft) = model.routing_settings_draft.as_mut() else {
        return;
    };
    let item = RoutingSettingsItem::available(draft.region)
        .get(model.settings_menu_selected)
        .copied();
    match item {
        Some(RoutingSettingsItem::Region) => {
            let current = GeoRegion::ALL
                .iter()
                .position(|region| *region == draft.region)
                .unwrap_or(0);
            let next = if forward {
                (current + 1) % GeoRegion::ALL.len()
            } else {
                (current + GeoRegion::ALL.len() - 1) % GeoRegion::ALL.len()
            };
            draft.region = GeoRegion::ALL[next];
            draft.mode = model
                .config
                .settings
                .geo_routing
                .selected_region_modes
                .get(&draft.region)
                .copied()
                .unwrap_or_default();
        }
        Some(RoutingSettingsItem::Mode) => {
            let modes = crate::config::profile::RoutingMode::available(Some(draft.region));
            let current = modes
                .iter()
                .position(|mode| *mode == draft.mode)
                .unwrap_or(0);
            let next = if forward {
                (current + 1) % modes.len()
            } else {
                (current + modes.len() - 1) % modes.len()
            };
            draft.mode = modes[next];
        }
        Some(RoutingSettingsItem::Service(service)) => {
            let current = draft
                .service_routes
                .get(&service)
                .copied()
                .unwrap_or_default();
            let next = if forward {
                current.next()
            } else {
                current.prev()
            };
            if next == ServiceRoute::Disabled {
                draft.service_routes.remove(&service);
            } else {
                draft.service_routes.insert(service, next);
            }
        }
        None => {}
    }
}

fn cycle_connection_draft(model: &mut Model) {
    let Some(draft) = model.connection_settings_draft.as_mut() else {
        return;
    };
    match model.settings_menu_selected {
        0 => draft.auto_connect = !draft.auto_connect,
        1 if model.kill_switch_pending.is_none() => draft.kill_switch = !draft.kill_switch,
        _ => {}
    }
}

pub(in crate::app::update) fn set_auto_connect(model: &mut Model, enabled: bool) -> Vec<Effect> {
    if enabled {
        if model.config.settings.auto_connect || model.auto_connect_pending {
            return vec![];
        }
        model.auto_connect_pending = true;
        return vec![Effect::CheckAutoConnectPolkit];
    }
    model.auto_connect_pending = false;
    if !model.config.settings.auto_connect {
        return vec![];
    }
    model.config.settings.auto_connect = false;
    let mut effects = vec![];
    push_status(
        &mut effects,
        model,
        AppStatus::Info("Auto-connect disabled".into()),
    );
    effects.push(Effect::SaveConfig);
    effects
}

pub(in crate::app::update) fn set_kill_switch(model: &mut Model, enabled: bool) -> Vec<Effect> {
    if model.kill_switch_pending.is_some() || model.config.settings.kill_switch == enabled {
        return vec![];
    }
    model.kill_switch_pending = Some(enabled);
    let mut effects = vec![];
    push_status(
        &mut effects,
        model,
        AppStatus::Info(format!(
            "Kill switch {}…",
            if enabled { "enabling" } else { "disabling" }
        )),
    );
    effects.push(Effect::ApplyKillSwitch { enabled });
    effects
}

pub(in crate::app::update) fn open_routing_mode(
    model: &mut Model,
    settings_menu_return: Option<SettingsMenuPage>,
) {
    model.settings_menu_return = settings_menu_return;
    model.overlay = Overlay::RoutingMode;
    let available = model.config.settings.geo_routing.available_modes();
    model.routing_selected = available
        .iter()
        .position(|m| *m == model.config.settings.geo_routing.mode())
        .unwrap_or(0);
}

pub(in crate::app::update) fn open_geo_region(
    model: &mut Model,
    settings_menu_return: Option<SettingsMenuPage>,
) {
    model.settings_menu_return = settings_menu_return;
    model.overlay = Overlay::GeoRegions;
    model.geo_region_selected = model
        .config
        .settings
        .geo_routing
        .current_region
        .and_then(|r| GeoRegion::ALL.iter().position(|x| *x == r))
        .unwrap_or(0);
}

pub(in crate::app::update) fn open_dns_settings(
    model: &mut Model,
    settings_menu_return: Option<SettingsMenuPage>,
) {
    model.settings_menu_return = settings_menu_return;
    model.overlay = Overlay::DnsSettings;
    model.dns_selected = 0;
    model.dns_preset_draft = None;
    model.dns_strategy_draft = None;
    model.dns_fakeip_draft = None;
}

pub(in crate::app::update) fn open_service_routing(
    model: &mut Model,
    settings_menu_return: Option<SettingsMenuPage>,
) {
    model.settings_menu_return = settings_menu_return;
    model.overlay = Overlay::ServiceRouting;
    model.service_routing_selected = 0;
    model.service_routing_draft = Some(model.config.settings.geo_routing.service_routes.clone());
}

pub(in crate::app::update) fn open_theme_settings(
    model: &mut Model,
    settings_menu_return: Option<SettingsMenuPage>,
) {
    model.settings_menu_return = settings_menu_return;
    let slugs = theme_picker_slugs();
    model.theme_selected = slugs
        .iter()
        .position(|s| s == &model.config.settings.theme)
        .unwrap_or(0);
    model.theme_draft = None;
    model.overlay = Overlay::ThemeSettings;
}

pub(in crate::app::update) fn return_to_settings_menu(model: &mut Model) -> bool {
    if model.settings_menu_return.is_none() {
        return false;
    }
    finish_settings_overlay(model);
    true
}

pub(in crate::app::update) fn finish_settings_overlay(model: &mut Model) {
    model.overlay = model
        .settings_menu_return
        .take()
        .map(Overlay::SettingsMenu)
        .unwrap_or(Overlay::None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::ConnectionState;
    use crate::app::msg::Msg;
    use crate::app::update::key::handle_key;
    use crate::app::update::key::sources::handle_sources;
    use crate::app::update::update;
    use crate::config::profile::{
        DnsPreset, DnsStrategy, IconSet, Profile, RoutedService, RoutingMode, ServiceRoute,
    };
    use crate::test_helpers::*;

    #[test]
    fn toggle_auto_connect() {
        let mut model = model_with_profiles(vec![]);
        assert!(!model.config.settings.auto_connect);
        let effects = handle_sources(&mut model, key('A'));
        assert!(!model.config.settings.auto_connect);
        assert!(model.auto_connect_pending);
        assert_eq!(effects, vec![Effect::CheckAutoConnectPolkit]);

        assert!(handle_sources(&mut model, key('A')).is_empty());

        update(&mut model, Msg::AutoConnectPolkitChecked { error: None });
        assert!(model.config.settings.auto_connect);
        assert!(model.status_text().contains("enabled"));

        handle_sources(&mut model, key('A'));
        let effects = handle_key(&mut model, key('y'));
        assert!(!model.config.settings.auto_connect);
        assert!(model.status_text().contains("disabled"));
        assert_eq!(
            effects,
            vec![app_log_info("Auto-connect disabled"), Effect::SaveConfig]
        );
    }

    #[test]
    fn toggle_kill_switch_emits_apply_effect_and_does_not_flip_bool() {
        let mut model = model_with_profiles(vec![]);
        assert!(!model.config.settings.kill_switch);

        let effects = handle_sources(&mut model, key('K'));
        // Bool is NOT flipped synchronously — it waits for KillSwitchApplied.
        assert!(!model.config.settings.kill_switch);
        assert_eq!(model.kill_switch_pending, Some(true));
        assert!(model.status_text().contains("enabling"));
        assert_eq!(
            effects,
            vec![
                app_log_info("Kill switch enabling…"),
                Effect::ApplyKillSwitch { enabled: true },
            ]
        );
    }

    #[test]
    fn repeated_kill_switch_toggle_is_ignored_while_pending() {
        let mut model = model_with_profiles(vec![]);
        let first = handle_sources(&mut model, key('K'));
        let status = model.status.clone();

        let second = handle_sources(&mut model, key('K'));

        assert!(first.contains(&Effect::ApplyKillSwitch { enabled: true }));
        assert!(second.is_empty());
        assert_eq!(model.kill_switch_pending, Some(true));
        assert_eq!(model.status, status);
    }

    #[test]
    fn settings_menu_opens_routing_group_and_returns_to_root() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);

        handle_key(&mut model, key('r'));
        assert_eq!(
            model.overlay,
            Overlay::SettingsMenu(SettingsMenuPage::Routing)
        );

        handle_key(&mut model, KeyEvent::from(KeyCode::Backspace));
        assert_eq!(model.overlay, Overlay::SettingsMenu(SettingsMenuPage::Root));
        assert_eq!(model.settings_menu_selected, 3);
    }

    #[test]
    fn settings_menu_connection_commits_all_drafts_and_returns_to_root() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);

        assert!(handle_key(&mut model, key('c')).is_empty());
        assert_eq!(
            model.overlay,
            Overlay::SettingsMenu(SettingsMenuPage::Connection)
        );

        assert!(handle_key(&mut model, key('l')).is_empty());
        assert!(handle_key(&mut model, key('j')).is_empty());
        assert!(handle_key(&mut model, key('l')).is_empty());
        assert!(!model.config.settings.auto_connect);
        assert_eq!(model.kill_switch_pending, None);

        let effects = handle_key(&mut model, KeyEvent::from(KeyCode::Enter));
        assert!(effects.contains(&Effect::CheckAutoConnectPolkit));
        assert!(effects.contains(&Effect::ApplyKillSwitch { enabled: true }));
        assert!(!model.config.settings.auto_connect);
        assert!(model.auto_connect_pending);
        assert_eq!(model.kill_switch_pending, Some(true));
        assert_eq!(model.overlay, Overlay::SettingsMenu(SettingsMenuPage::Root));
        assert_eq!(model.settings_menu_selected, 0);

        handle_key(&mut model, key('c'));
        assert!(handle_key(&mut model, KeyEvent::from(KeyCode::Backspace)).is_empty());
        assert_eq!(model.overlay, Overlay::SettingsMenu(SettingsMenuPage::Root));
        assert_eq!(model.connection_settings_draft, None);
        assert_eq!(model.settings_menu_selected, 0);
    }

    #[test]
    fn settings_menu_interface_previews_and_saves_theme_and_icons() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let slugs = theme_picker_slugs();
        let mut model = model_with_profiles(vec![]);
        model.config.settings.theme = slugs[0].clone();
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);

        assert!(handle_key(&mut model, key('i')).is_empty());
        assert_eq!(
            model.overlay,
            Overlay::SettingsMenu(SettingsMenuPage::Interface)
        );
        assert_eq!(model.theme_draft, None);
        assert_eq!(model.interface_settings_draft, None);

        assert!(handle_key(&mut model, key('h')).is_empty());
        assert_eq!(model.theme_draft.as_ref(), slugs.last());
        assert!(handle_key(&mut model, key('l')).is_empty());
        assert!(handle_key(&mut model, key('l')).is_empty());
        assert_eq!(model.theme_draft.as_ref(), slugs.get(1));

        assert!(handle_key(&mut model, key('j')).is_empty());
        assert!(handle_key(&mut model, key('l')).is_empty());
        assert_eq!(model.icon_set(), IconSet::Unicode);
        assert_eq!(model.config.settings.icons, IconSet::Nerd);
        assert_eq!(model.config.settings.theme, slugs[0]);

        let effects = handle_key(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            effects
                .iter()
                .filter(|effect| matches!(effect, Effect::SaveConfig))
                .count(),
            1
        );
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::AppendAppLog { message, .. } if *message == format!("Theme: {}", slugs[1])
        )));
        assert_eq!(model.config.settings.theme, slugs[1]);
        assert_eq!(model.config.settings.icons, IconSet::Unicode);
        assert_eq!(model.theme_draft, None);
        assert_eq!(model.interface_settings_draft, None);
        assert_eq!(model.overlay, Overlay::SettingsMenu(SettingsMenuPage::Root));
        assert_eq!(model.settings_menu_selected, 2);

        handle_key(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            model.overlay,
            Overlay::SettingsMenu(SettingsMenuPage::Interface)
        );
        assert!(handle_key(&mut model, KeyEvent::from(KeyCode::Enter)).is_empty());
    }

    #[test]
    fn settings_menu_interface_saves_icons_without_theme_status() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);

        handle_key(&mut model, key('i'));
        handle_key(&mut model, key('j'));
        handle_key(&mut model, key('h'));
        let effects = handle_key(&mut model, KeyEvent::from(KeyCode::Enter));

        assert_eq!(effects, vec![Effect::SaveConfig]);
        assert_eq!(model.config.settings.icons, IconSet::Unicode);
    }

    #[test]
    fn settings_menu_interface_discards_drafts_on_close() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let mut model = model_with_profiles(vec![]);
        let saved_theme = model.config.settings.theme.clone();
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);

        handle_key(&mut model, key('i'));
        handle_key(&mut model, key('l'));
        handle_key(&mut model, key('j'));
        handle_key(&mut model, key('l'));
        assert!(handle_key(&mut model, KeyEvent::from(KeyCode::Esc)).is_empty());

        assert_eq!(model.overlay, Overlay::None);
        assert_eq!(model.theme_draft, None);
        assert_eq!(model.interface_settings_draft, None);
        assert_eq!(model.config.settings.theme, saved_theme);
        assert_eq!(model.icon_set(), IconSet::Nerd);
    }

    #[test]
    fn settings_menu_root_navigates_and_opens_selected_page() {
        let mut model = model_with_profiles(vec![]);
        handle_key(&mut model, key(' '));
        handle_key(&mut model, key('j'));
        handle_key(&mut model, key('j'));
        handle_key(&mut model, key('j'));
        handle_key(&mut model, KeyEvent::from(KeyCode::Enter));

        assert_eq!(
            model.overlay,
            Overlay::SettingsMenu(SettingsMenuPage::Routing)
        );
        assert_eq!(model.settings_menu_selected, 0);
        assert!(model.routing_settings_draft.is_some());
    }

    #[test]
    fn settings_routing_service_change_defers_one_connected_reconnect() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u".into());
        let active_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(active_id);
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);

        handle_key(&mut model, key('r'));
        handle_key(&mut model, key('j'));
        handle_key(&mut model, key('j'));
        handle_key(&mut model, key('l'));
        let effects = handle_key(&mut model, KeyEvent::from(KeyCode::Enter));

        assert_eq!(model.connection, ConnectionState::Connected);
        assert!(model.pending_service_reconnect);
        assert!(effects.contains(&Effect::DownloadServiceRuleSetsIfMissing));
        assert_eq!(
            effects
                .iter()
                .filter(|effect| matches!(effect, Effect::SaveConfig))
                .count(),
            1
        );
    }

    #[test]
    fn connection_menu_does_not_replace_pending_kill_switch_target() {
        let mut model = model_with_profiles(vec![]);
        model.kill_switch_pending = Some(true);
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);

        handle_key(&mut model, key('c'));
        handle_key(&mut model, key('j'));
        handle_key(&mut model, key('l'));
        let effects = handle_key(&mut model, KeyEvent::from(KeyCode::Enter));

        assert_eq!(model.kill_switch_pending, Some(true));
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::ApplyKillSwitch { .. }))
        );
    }

    #[test]
    fn settings_menu_root_opens_dns_with_clean_drafts() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
        model.dns_selected = 2;
        model.dns_preset_draft = Some(DnsPreset::GoogleDot);
        model.dns_strategy_draft = Some(DnsStrategy::OnlyIpv4);
        model.dns_fakeip_draft = Some(true);

        handle_key(&mut model, key('d'));

        assert_eq!(model.overlay, Overlay::DnsSettings);
        assert_eq!(model.dns_selected, 0);
        assert_eq!(model.dns_preset_draft, None);
        assert_eq!(model.dns_strategy_draft, None);
        assert_eq!(model.dns_fakeip_draft, None);
        assert_eq!(model.settings_menu_return, Some(SettingsMenuPage::Root));
    }

    #[test]
    fn settings_routing_menu_ignores_letter_shortcuts() {
        for shortcut in ['m', 'r', 's'] {
            let mut model = model_with_profiles(vec![]);
            model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
            handle_key(&mut model, key('r'));
            handle_key(&mut model, key(shortcut));
            assert_eq!(
                model.overlay,
                Overlay::SettingsMenu(SettingsMenuPage::Routing)
            );
            assert_eq!(model.settings_menu_selected, 0);
        }
    }

    #[test]
    fn settings_routing_global_skips_mode_during_navigation() {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
        handle_key(&mut model, key('r'));

        assert_eq!(
            model.routing_settings_draft.as_ref().unwrap().region,
            GeoRegion::Global
        );
        handle_key(&mut model, key('j'));
        assert_eq!(model.settings_menu_selected, 1);
        handle_key(&mut model, key('l'));
        assert_eq!(
            model
                .routing_settings_draft
                .as_ref()
                .unwrap()
                .service_routes
                .get(&RoutedService::Steam),
            Some(&ServiceRoute::Proxy)
        );
        handle_key(&mut model, key('G'));
        assert_eq!(model.settings_menu_selected, 2);
        handle_key(&mut model, key('j'));
        assert_eq!(model.settings_menu_selected, 2);
    }

    #[test]
    fn settings_routing_restores_region_mode_after_global() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model
            .config
            .settings
            .geo_routing
            .set_mode(RoutingMode::Bypass(GeoRegion::Ru));
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
        handle_key(&mut model, key('r'));

        handle_key(&mut model, key('h'));
        let draft = model.routing_settings_draft.as_ref().unwrap();
        assert_eq!(draft.region, GeoRegion::Global);
        assert_eq!(draft.mode, RoutingMode::Global);
        assert_eq!(RoutingSettingsItem::available(draft.region).len(), 3);

        handle_key(&mut model, key('l'));
        let draft = model.routing_settings_draft.as_ref().unwrap();
        assert_eq!(draft.region, GeoRegion::Ru);
        assert_eq!(draft.mode, RoutingMode::Bypass(GeoRegion::Ru));
        assert_eq!(RoutingSettingsItem::available(draft.region).len(), 4);
    }

    #[test]
    fn settings_menu_closes_or_ignores_keys_as_expected() {
        for code in [KeyCode::Char('q'), KeyCode::Esc] {
            let mut model = model_with_profiles(vec![]);
            model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
            handle_key(&mut model, KeyEvent::from(code));
            assert_eq!(model.overlay, Overlay::None);
        }

        for code in [KeyCode::Char(' '), KeyCode::Backspace, KeyCode::Char('x')] {
            let mut model = model_with_profiles(vec![]);
            model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
            handle_key(&mut model, KeyEvent::from(code));
            assert_eq!(model.overlay, Overlay::SettingsMenu(SettingsMenuPage::Root));
        }

        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Routing);
        handle_key(&mut model, key(' '));
        assert_eq!(
            model.overlay,
            Overlay::SettingsMenu(SettingsMenuPage::Routing)
        );
    }

    #[test]
    fn backspace_returns_from_settings_overlays_and_discards_drafts() {
        let mut routing = model_with_profiles(vec![]);
        routing.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
        handle_key(&mut routing, key('r'));
        handle_key(&mut routing, key('l'));
        handle_key(&mut routing, KeyEvent::from(KeyCode::Backspace));
        assert_eq!(
            routing.overlay,
            Overlay::SettingsMenu(SettingsMenuPage::Root)
        );
        assert_eq!(routing.routing_settings_draft, None);
        assert_eq!(routing.settings_menu_selected, 3);

        let mut dns = model_with_profiles(vec![]);
        dns.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
        handle_key(&mut dns, key('d'));
        dns.dns_preset_draft = Some(DnsPreset::GoogleDot);
        dns.dns_strategy_draft = Some(DnsStrategy::OnlyIpv4);
        dns.dns_fakeip_draft = Some(true);
        handle_key(&mut dns, KeyEvent::from(KeyCode::Backspace));
        assert_eq!(dns.overlay, Overlay::SettingsMenu(SettingsMenuPage::Root));
        assert_eq!(dns.dns_preset_draft, None);
        assert_eq!(dns.dns_strategy_draft, None);
        assert_eq!(dns.dns_fakeip_draft, None);
        assert_eq!(dns.settings_menu_return, None);
        assert_eq!(dns.settings_menu_selected, 1);

        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let mut interface = model_with_profiles(vec![]);
        interface.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
        handle_key(&mut interface, key('i'));
        handle_key(&mut interface, key('l'));
        handle_key(&mut interface, key('j'));
        handle_key(&mut interface, key('l'));
        handle_key(&mut interface, KeyEvent::from(KeyCode::Backspace));
        assert_eq!(
            interface.overlay,
            Overlay::SettingsMenu(SettingsMenuPage::Root)
        );
        assert_eq!(interface.theme_draft, None);
        assert_eq!(interface.interface_settings_draft, None);
        assert_eq!(interface.settings_menu_return, None);
        assert_eq!(interface.settings_menu_selected, 2);
    }

    #[test]
    fn enter_commits_and_returns_to_the_originating_settings_menu() {
        let mut routing = model_with_profiles(vec![]);
        routing.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
        handle_key(&mut routing, key('r'));
        handle_key(&mut routing, key('l'));
        handle_key(&mut routing, key('j'));
        handle_key(&mut routing, key('l'));
        handle_key(&mut routing, key('j'));
        handle_key(&mut routing, key('l'));
        handle_key(&mut routing, key('j'));
        handle_key(&mut routing, key('l'));
        let effects = handle_key(&mut routing, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            routing.overlay,
            Overlay::SettingsMenu(SettingsMenuPage::Root)
        );
        assert_eq!(
            routing.config.settings.geo_routing.current_region,
            Some(GeoRegion::Ru)
        );
        assert_eq!(
            routing.config.settings.geo_routing.mode(),
            crate::config::profile::RoutingMode::Bypass(GeoRegion::Ru)
        );
        assert_eq!(
            routing
                .config
                .settings
                .geo_routing
                .service_route(RoutedService::Steam),
            ServiceRoute::Proxy
        );
        assert_eq!(
            routing
                .config
                .settings
                .geo_routing
                .service_route(RoutedService::Telegram),
            ServiceRoute::Proxy
        );
        assert_eq!(
            effects
                .iter()
                .filter(|effect| matches!(effect, Effect::SaveConfig))
                .count(),
            1
        );

        let mut dns = model_with_profiles(vec![]);
        dns.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
        handle_key(&mut dns, key('d'));
        dns.dns_preset_draft = Some(DnsPreset::GoogleDot);
        let effects = handle_key(&mut dns, KeyEvent::from(KeyCode::Enter));
        assert_eq!(dns.overlay, Overlay::SettingsMenu(SettingsMenuPage::Root));
        assert_eq!(
            DnsPreset::detect(&dns.config.settings.dns),
            Some(DnsPreset::GoogleDot)
        );
        assert!(effects.contains(&Effect::SaveConfig));
        assert_eq!(dns.settings_menu_return, None);

        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let mut theme = model_with_profiles(vec![]);
        theme.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
        handle_key(&mut theme, key('i'));
        handle_key(&mut theme, key('l'));
        let selected_theme = theme.theme_draft.clone().unwrap();
        handle_key(&mut theme, KeyEvent::from(KeyCode::Enter));
        assert_eq!(theme.overlay, Overlay::SettingsMenu(SettingsMenuPage::Root));
        assert_eq!(theme.config.settings.theme, selected_theme);
        assert_eq!(theme.theme_draft, None);
        assert_eq!(theme.settings_menu_return, None);
    }

    #[test]
    fn direct_settings_shortcuts_do_not_enable_back_navigation() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        for shortcut in ['m', 'o', 'D', 'S', 'C'] {
            let mut model = model_with_profiles(vec![]);
            model.settings_menu_return = Some(SettingsMenuPage::Root);
            handle_key(&mut model, key(shortcut));
            assert_eq!(model.settings_menu_return, None);
        }

        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::RoutingMode;
        handle_key(&mut model, KeyEvent::from(KeyCode::Backspace));
        assert_eq!(model.overlay, Overlay::RoutingMode);
    }

    #[test]
    fn enter_after_direct_settings_shortcuts_closes_the_overlay() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        for shortcut in ['m', 'o', 'D', 'S', 'C'] {
            let mut model = model_with_profiles(vec![]);
            handle_key(&mut model, key(shortcut));
            handle_key(&mut model, KeyEvent::from(KeyCode::Enter));
            assert_eq!(model.overlay, Overlay::None);
            assert_eq!(model.settings_menu_return, None);
        }
    }
}

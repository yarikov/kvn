use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model, Overlay};
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::update::connection::queue_connect;
use crate::app::update::key::settings_menu::{finish_settings_overlay, return_to_settings_menu};
use crate::app::update::status::push_status;

/// Items in the DNS settings overlay, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app::update) enum DnsSettingsItem {
    Preset,
    CycleStrategy,
    ToggleFakeIp,
}

impl DnsSettingsItem {
    pub const ALL: [DnsSettingsItem; 3] = [
        DnsSettingsItem::Preset,
        DnsSettingsItem::CycleStrategy,
        DnsSettingsItem::ToggleFakeIp,
    ];

    pub fn from_index(idx: usize) -> Option<Self> {
        Self::ALL.get(idx).copied()
    }
}

pub(in crate::app::update) fn handle_dns_settings(model: &mut Model, key: KeyEvent) -> Vec<Effect> {
    use crate::config::profile::DnsPreset;

    let len = DnsSettingsItem::ALL.len();
    let on_preset =
        DnsSettingsItem::from_index(model.dns_selected) == Some(DnsSettingsItem::Preset);
    let on_strategy =
        DnsSettingsItem::from_index(model.dns_selected) == Some(DnsSettingsItem::CycleStrategy);
    let on_fakeip =
        DnsSettingsItem::from_index(model.dns_selected) == Some(DnsSettingsItem::ToggleFakeIp);
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            crate::ui::nav::select_next(&mut model.dns_selected, len);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            crate::ui::nav::select_prev(&mut model.dns_selected);
        }
        KeyCode::Char('G') => crate::ui::nav::select_last(&mut model.dns_selected, len),
        KeyCode::Char('l') | KeyCode::Right if on_preset => {
            let next = model
                .dns_preset_draft
                .or_else(|| DnsPreset::detect(&model.config.settings.dns))
                .map(DnsPreset::next)
                .unwrap_or(DnsPreset::CloudflareDoh);
            model.dns_preset_draft = Some(next);
        }
        KeyCode::Char('h') | KeyCode::Left if on_preset => {
            let previous = model
                .dns_preset_draft
                .or_else(|| DnsPreset::detect(&model.config.settings.dns))
                .map(DnsPreset::prev)
                .unwrap_or(DnsPreset::SystemLocal);
            model.dns_preset_draft = Some(previous);
        }
        KeyCode::Char('l') | KeyCode::Right if on_strategy => {
            let base = model
                .dns_strategy_draft
                .clone()
                .unwrap_or_else(|| model.config.settings.dns.strategy.clone());
            model.dns_strategy_draft = Some(base.next());
        }
        KeyCode::Char('h') | KeyCode::Left if on_strategy => {
            let base = model
                .dns_strategy_draft
                .clone()
                .unwrap_or_else(|| model.config.settings.dns.strategy.clone());
            model.dns_strategy_draft = Some(base.prev());
        }
        KeyCode::Char('l') | KeyCode::Right if on_fakeip => {
            let current = model
                .dns_fakeip_draft
                .unwrap_or(model.config.settings.dns.fakeip_enabled);
            model.dns_fakeip_draft = Some(!current);
        }
        KeyCode::Char('h') | KeyCode::Left if on_fakeip => {
            let current = model
                .dns_fakeip_draft
                .unwrap_or(model.config.settings.dns.fakeip_enabled);
            model.dns_fakeip_draft = Some(!current);
        }
        KeyCode::Enter => {
            let effects = apply_dns_drafts(model);
            finish_settings_overlay(model);
            model.dns_preset_draft = None;
            model.dns_strategy_draft = None;
            model.dns_fakeip_draft = None;
            return effects;
        }
        KeyCode::Char('q') | KeyCode::Esc => {
            model.settings_menu_return = None;
            model.overlay = Overlay::None;
            model.dns_preset_draft = None;
            model.dns_strategy_draft = None;
            model.dns_fakeip_draft = None;
        }
        KeyCode::Backspace if model.settings_menu_return.is_some() => {
            model.dns_preset_draft = None;
            model.dns_strategy_draft = None;
            model.dns_fakeip_draft = None;
            return_to_settings_menu(model);
        }
        _ => {}
    }
    vec![]
}

fn apply_dns_drafts(model: &mut Model) -> Vec<Effect> {
    use crate::config::profile::{DnsPreset, DnsServer, DnsStrategy};

    let previous = model.config.settings.dns.clone();
    if let Some(draft) = model.dns_preset_draft.take()
        && DnsPreset::detect(&model.config.settings.dns) != Some(draft)
    {
        draft.apply(&mut model.config.settings.dns);
    }
    if let Some(draft) = model.dns_strategy_draft.take() {
        model.config.settings.dns.strategy = draft;
    }
    if let Some(enabled) = model.dns_fakeip_draft.take() {
        model.config.settings.dns.fakeip_enabled = enabled;
        if enabled
            && !model
                .config
                .settings
                .dns
                .servers
                .iter()
                .any(|server| matches!(server, DnsServer::FakeIp { .. }))
        {
            model.config.settings.dns.servers.push(DnsServer::FakeIp {
                tag: "fakeip".to_string(),
                inet4_range: "198.18.0.0/15".to_string(),
                inet6_range: "fc00::/18".to_string(),
            });
        }
    }
    if model.config.settings.dns.fakeip_enabled
        && matches!(model.config.settings.dns.strategy, DnsStrategy::OnlyIpv6)
    {
        model.config.settings.dns.strategy = DnsStrategy::PreferIpv4;
    }
    model.config.settings.dns_strategy = model.config.settings.dns.strategy.clone();

    if model.config.settings.dns == previous {
        return vec![];
    }

    let mut effects = vec![Effect::SaveConfig, Effect::BroadcastState];
    push_status(
        &mut effects,
        model,
        AppStatus::Info("DNS settings updated".into()),
    );
    if model.connection == ConnectionState::Connected
        && let Some(active_id) = model.active_profile_id
        && queue_connect(model, active_id)
    {
        push_status(
            &mut effects,
            model,
            AppStatus::Info("DNS changed — reconnecting…".into()),
        );
    }
    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::update::key::ipc::handle_go_first;
    use crate::config::profile::{DnsPreset, DnsServer, DnsStrategy, Profile};
    use crate::test_helpers::*;

    fn with_dns_overlay() -> Model {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::DnsSettings;
        model
    }

    fn final_server_is(model: &Model, expected_kind: fn(&DnsServer) -> bool) -> bool {
        model
            .config
            .settings
            .dns
            .final_server_entry()
            .map(expected_kind)
            .unwrap_or(false)
    }

    // ---- handle_dns_settings ----

    #[test]
    fn dns_settings_navigation_wraps() {
        let mut model = with_dns_overlay();
        assert_eq!(model.dns_selected, 0);
        handle_dns_settings(&mut model, key('j'));
        assert_eq!(model.dns_selected, 1);
        handle_dns_settings(&mut model, key('k'));
        assert_eq!(model.dns_selected, 0);
        handle_dns_settings(&mut model, key('G'));
        assert_eq!(model.dns_selected, DnsSettingsItem::ALL.len() - 1);
        handle_dns_settings(&mut model, key('g'));
        assert_eq!(model.dns_selected, DnsSettingsItem::ALL.len() - 1);
        handle_go_first(&mut model);
        assert_eq!(model.dns_selected, 0);
    }

    #[test]
    fn dns_settings_navigation_arrows() {
        let mut model = with_dns_overlay();
        handle_dns_settings(&mut model, KeyEvent::from(KeyCode::Down));
        assert_eq!(model.dns_selected, 1);
        handle_dns_settings(&mut model, KeyEvent::from(KeyCode::Up));
        assert_eq!(model.dns_selected, 0);
    }

    #[test]
    fn dns_settings_h_l_cycles_preset_draft() {
        let mut model = with_dns_overlay();
        // Default is Cloudflare; l moves forward and h moves back.
        handle_dns_settings(&mut model, key('l'));
        assert_eq!(model.dns_preset_draft, Some(DnsPreset::GoogleDot));
        handle_dns_settings(&mut model, key('h'));
        assert_eq!(model.dns_preset_draft, Some(DnsPreset::CloudflareDoh));
        assert_eq!(
            DnsPreset::detect(&model.config.settings.dns),
            Some(DnsPreset::CloudflareDoh)
        );
    }

    #[test]
    fn dns_settings_custom_preset_starts_at_directional_edge() {
        let mut model = with_dns_overlay();
        model.config.settings.dns.servers = vec![DnsServer::Udp {
            tag: "custom".into(),
            server: "10.0.0.1".into(),
            server_port: None,
        }];
        model.config.settings.dns.final_server = "custom".into();

        handle_dns_settings(&mut model, key('l'));
        assert_eq!(model.dns_preset_draft, Some(DnsPreset::CloudflareDoh));

        model.dns_preset_draft = None;
        handle_dns_settings(&mut model, key('h'));
        assert_eq!(model.dns_preset_draft, Some(DnsPreset::SystemLocal));
    }

    #[test]
    fn dns_settings_l_cycles_strategy_draft_forward() {
        let mut model = with_dns_overlay();
        model.dns_selected = DnsSettingsItem::ALL
            .iter()
            .position(|i| *i == DnsSettingsItem::CycleStrategy)
            .unwrap();
        // Default is PreferIpv4; l → PreferIpv6.
        handle_dns_settings(&mut model, key('l'));
        assert_eq!(model.dns_strategy_draft, Some(DnsStrategy::PreferIpv6));
        // l again → OnlyIpv4.
        handle_dns_settings(&mut model, KeyEvent::from(KeyCode::Right));
        assert_eq!(model.dns_strategy_draft, Some(DnsStrategy::OnlyIpv4));
    }

    #[test]
    fn dns_settings_h_cycles_strategy_draft_backward() {
        let mut model = with_dns_overlay();
        model.dns_selected = DnsSettingsItem::ALL
            .iter()
            .position(|i| *i == DnsSettingsItem::CycleStrategy)
            .unwrap();
        handle_dns_settings(&mut model, key('h'));
        assert_eq!(model.dns_strategy_draft, Some(DnsStrategy::OnlyIpv6));
        handle_dns_settings(&mut model, KeyEvent::from(KeyCode::Left));
        assert_eq!(model.dns_strategy_draft, Some(DnsStrategy::OnlyIpv4));
    }

    #[test]
    fn dns_settings_enter_strategy_no_draft_is_noop() {
        let mut model = with_dns_overlay();
        model.dns_selected = DnsSettingsItem::ALL
            .iter()
            .position(|i| *i == DnsSettingsItem::CycleStrategy)
            .unwrap();
        let before = model.config.settings.dns.strategy.clone();
        let effects = handle_dns_settings(&mut model, enter());
        assert!(effects.is_empty());
        assert_eq!(model.config.settings.dns.strategy, before);
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn dns_settings_enter_strategy_commits_draft() {
        let mut model = with_dns_overlay();
        model.dns_selected = DnsSettingsItem::ALL
            .iter()
            .position(|i| *i == DnsSettingsItem::CycleStrategy)
            .unwrap();
        model.dns_strategy_draft = Some(DnsStrategy::OnlyIpv4);
        model.dns_preset_draft = Some(DnsPreset::GoogleDot);
        model.dns_fakeip_draft = Some(true);
        let effects = handle_dns_settings(&mut model, enter());
        assert_eq!(model.config.settings.dns.strategy, DnsStrategy::OnlyIpv4);
        assert_eq!(model.config.settings.dns_strategy, DnsStrategy::OnlyIpv4);
        assert_eq!(
            DnsPreset::detect(&model.config.settings.dns),
            Some(DnsPreset::GoogleDot)
        );
        assert!(model.config.settings.dns.fakeip_enabled);
        assert!(model.config.settings.dns.fakeip_server().is_some());
        assert!(effects.contains(&Effect::SaveConfig));
        assert!(effects.contains(&Effect::BroadcastState));
        assert!(model.dns_preset_draft.is_none());
        assert!(model.dns_strategy_draft.is_none());
        assert!(model.dns_fakeip_draft.is_none());
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn dns_settings_enter_strategy_same_value_is_noop() {
        let mut model = with_dns_overlay();
        model.dns_selected = DnsSettingsItem::ALL
            .iter()
            .position(|i| *i == DnsSettingsItem::CycleStrategy)
            .unwrap();
        // Draft equals current — Enter discards draft, emits nothing.
        let same = model.config.settings.dns.strategy.clone();
        model.dns_strategy_draft = Some(same.clone());
        let effects = handle_dns_settings(&mut model, enter());
        assert!(effects.is_empty());
        assert!(model.dns_strategy_draft.is_none());
        assert_eq!(model.config.settings.dns.strategy, same);
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn dns_settings_enter_cloudflare_preset() {
        let mut model = with_dns_overlay();
        DnsPreset::SystemLocal.apply(&mut model.config.settings.dns);
        model.dns_selected = 0;
        model.dns_preset_draft = Some(DnsPreset::CloudflareDoh);
        let effects = handle_dns_settings(&mut model, enter());
        assert!(effects.contains(&Effect::SaveConfig));
        assert!(effects.contains(&Effect::BroadcastState));
        assert_eq!(model.overlay, Overlay::None);
        assert!(final_server_is(&model, |s| matches!(
            s,
            DnsServer::Https { server, .. } if server == "1.1.1.1"
        )));
    }

    #[test]
    fn dns_settings_enter_google_preset() {
        let mut model = with_dns_overlay();
        model.dns_selected = 0;
        model.dns_preset_draft = Some(DnsPreset::GoogleDot);
        handle_dns_settings(&mut model, enter());
        assert!(final_server_is(&model, |s| matches!(
            s,
            DnsServer::Tls { server, .. } if server == "8.8.8.8"
        )));
    }

    #[test]
    fn dns_settings_enter_quad9_preset() {
        let mut model = with_dns_overlay();
        model.dns_selected = 0;
        model.dns_preset_draft = Some(DnsPreset::Quad9Doh);
        handle_dns_settings(&mut model, enter());
        assert!(final_server_is(&model, |s| matches!(
            s,
            DnsServer::Https { server, .. } if server == "9.9.9.9"
        )));
    }

    #[test]
    fn dns_settings_enter_system_preset() {
        let mut model = with_dns_overlay();
        model.dns_selected = 0;
        model.dns_preset_draft = Some(DnsPreset::SystemLocal);
        handle_dns_settings(&mut model, enter());
        assert!(final_server_is(&model, |s| matches!(
            s,
            DnsServer::Local { .. }
        )));
        assert_eq!(model.config.settings.dns.servers.len(), 1);
    }

    #[test]
    fn dns_settings_toggle_fakeip_enables_and_adds_server() {
        let mut model = with_dns_overlay();
        model.dns_selected = DnsSettingsItem::ALL
            .iter()
            .position(|i| *i == DnsSettingsItem::ToggleFakeIp)
            .unwrap();
        assert!(!model.config.settings.dns.fakeip_enabled);
        handle_dns_settings(&mut model, key('l'));
        assert_eq!(model.dns_fakeip_draft, Some(true));
        handle_dns_settings(&mut model, enter());
        assert!(model.config.settings.dns.fakeip_enabled);
        assert!(model.config.settings.dns.fakeip_server().is_some());
        assert!(model.dns_fakeip_draft.is_none());
    }

    #[test]
    fn dns_settings_toggle_fakeip_disables_back() {
        let mut model = with_dns_overlay();
        model.dns_selected = DnsSettingsItem::ALL
            .iter()
            .position(|i| *i == DnsSettingsItem::ToggleFakeIp)
            .unwrap();
        // Select and commit on.
        handle_dns_settings(&mut model, key('l'));
        handle_dns_settings(&mut model, enter());
        assert!(model.config.settings.dns.fakeip_enabled);
        // Select and commit off.
        handle_dns_settings(&mut model, key('h'));
        handle_dns_settings(&mut model, enter());
        assert!(!model.config.settings.dns.fakeip_enabled);
    }

    #[test]
    fn dns_settings_fakeip_cycles_in_both_directions() {
        let mut model = with_dns_overlay();
        model.dns_selected = DnsSettingsItem::ALL
            .iter()
            .position(|i| *i == DnsSettingsItem::ToggleFakeIp)
            .unwrap();

        handle_dns_settings(&mut model, key('l'));
        assert_eq!(model.dns_fakeip_draft, Some(true));
        handle_dns_settings(&mut model, KeyEvent::from(KeyCode::Right));
        assert_eq!(model.dns_fakeip_draft, Some(false));

        handle_dns_settings(&mut model, key('h'));
        assert_eq!(model.dns_fakeip_draft, Some(true));
        handle_dns_settings(&mut model, KeyEvent::from(KeyCode::Left));
        assert_eq!(model.dns_fakeip_draft, Some(false));
    }

    #[test]
    fn dns_settings_toggle_fakeip_forces_ipv4_strategy() {
        let mut model = with_dns_overlay();
        model.config.settings.dns.strategy = DnsStrategy::OnlyIpv6;
        model.config.settings.dns_strategy = DnsStrategy::OnlyIpv6;
        model.dns_selected = DnsSettingsItem::ALL
            .iter()
            .position(|i| *i == DnsSettingsItem::ToggleFakeIp)
            .unwrap();
        handle_dns_settings(&mut model, key('l'));
        handle_dns_settings(&mut model, enter());
        assert_eq!(model.config.settings.dns.strategy, DnsStrategy::PreferIpv4);
        assert_eq!(model.config.settings.dns_strategy, DnsStrategy::PreferIpv4);
    }

    #[test]
    fn dns_settings_esc_clears_draft_and_closes_overlay() {
        let mut model = with_dns_overlay();
        model.dns_selected = DnsSettingsItem::ALL
            .iter()
            .position(|i| *i == DnsSettingsItem::CycleStrategy)
            .unwrap();
        model.dns_strategy_draft = Some(DnsStrategy::OnlyIpv6);
        model.dns_preset_draft = Some(DnsPreset::GoogleDot);
        model.dns_fakeip_draft = Some(true);
        let effects = handle_dns_settings(&mut model, esc());
        assert!(effects.is_empty());
        assert_eq!(model.overlay, Overlay::None);
        assert!(model.dns_preset_draft.is_none());
        assert!(model.dns_strategy_draft.is_none());
        assert!(model.dns_fakeip_draft.is_none());
    }

    #[test]
    fn dns_settings_q_closes_overlay() {
        let mut model = with_dns_overlay();
        handle_dns_settings(&mut model, key('q'));
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn dns_settings_changing_preset_while_connected_triggers_reconnect() {
        let a = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let b = Profile::new_vless("B".into(), "2.2.2.2".into(), 443, "u2".into());
        let active_id = a.id;
        let mut model = model_with_profiles(vec![a, b]);
        model.select_next();
        model.overlay = Overlay::DnsSettings;
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(active_id);
        model.dns_selected = 0;
        model.dns_preset_draft = Some(DnsPreset::GoogleDot);
        let effects = handle_dns_settings(&mut model, enter());
        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(active_id));
        assert!(!effects.iter().any(|e| matches!(e, Effect::Connect { .. })));
    }

    #[test]
    fn dns_settings_unknown_key_is_noop() {
        let mut model = with_dns_overlay();
        let before = model.dns_selected;
        let effects = handle_dns_settings(&mut model, key('z'));
        assert!(effects.is_empty());
        assert_eq!(model.dns_selected, before);
    }
}

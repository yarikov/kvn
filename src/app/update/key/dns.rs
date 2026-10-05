use crate::app::effect::Effect;
use crate::app::model::{AppStatus, Model, Overlay};
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::update::connection::reconnect_with_new_settings;
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
            model.dns_preset_draft = Some(adjacent_preset(model, 1));
        }
        KeyCode::Char('h') | KeyCode::Left if on_preset => {
            model.dns_preset_draft = Some(adjacent_preset(model, -1));
        }
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Char('h') | KeyCode::Left if on_strategy => {
            let base = model
                .dns_strategy_draft
                .clone()
                .unwrap_or_else(|| model.config.settings.dns.strategy.clone());
            model.dns_strategy_draft = Some(base.toggled());
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
        KeyCode::Enter => return commit_dns_drafts(model),
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

fn adjacent_preset(model: &Model, step: isize) -> String {
    let dns = &model.config.settings.dns;
    let names = dns.preset_names();
    let current = model
        .dns_preset_draft
        .as_deref()
        .unwrap_or(&dns.current_preset);
    let next = match names.iter().position(|name| *name == current) {
        Some(idx) => (idx as isize + step).rem_euclid(names.len() as isize) as usize,
        None if step > 0 => 0,
        None => names.len() - 1,
    };
    names[next].to_string()
}

fn commit_dns_drafts(model: &mut Model) -> Vec<Effect> {
    if let Some(missing) = model
        .dns_preset_draft
        .take_if(|draft| !model.config.settings.dns.has_preset(draft))
    {
        let mut effects = vec![];
        push_status(
            &mut effects,
            model,
            AppStatus::Error(format!("DNS preset {missing:?} no longer exists")),
        );
        return effects;
    }
    let previous = model.config.settings.dns.clone();
    apply_dns_drafts(model);
    finish_settings_overlay(model);
    if model.config.settings.dns == previous {
        return vec![];
    }
    let mut effects = vec![Effect::SaveConfig, Effect::BroadcastState];
    push_status(
        &mut effects,
        model,
        AppStatus::Info("DNS settings changed".into()),
    );
    if reconnect_with_new_settings(model) {
        push_status(
            &mut effects,
            model,
            AppStatus::Info("DNS changed — reconnecting…".into()),
        );
    }
    if !previous.fakeip_enabled
        && let Some(tag) = fakeip_conflicting_rule_set(&model.config.settings.dns)
    {
        push_status(
            &mut effects,
            model,
            AppStatus::Info(format!(
                "Fake-IP is on: connecting fails while the {tag:?} DNS rule is active, unless the preset has a rule with \"server\": \"fakeip\""
            )),
        );
    }
    effects
}

fn fakeip_conflicting_rule_set(dns: &crate::config::profile::DnsConfig) -> Option<String> {
    let active = dns.active()?;
    let fakeip_tag = active.fakeip.as_ref()?.tag.as_str();
    if active.rules.iter().any(|rule| rule.server == fakeip_tag) {
        return None;
    }
    active
        .rules
        .iter()
        .flat_map(|rule| &rule.rule_set)
        .find(|tag| crate::geo::is_ip_rule_set_tag(tag))
        .cloned()
}

fn apply_dns_drafts(model: &mut Model) {
    if let Some(draft) = model.dns_preset_draft.take() {
        model.config.settings.dns.current_preset = draft;
    }
    if let Some(draft) = model.dns_strategy_draft.take() {
        model.config.settings.dns.strategy = draft;
    }
    if let Some(enabled) = model.dns_fakeip_draft.take() {
        model.config.settings.dns.fakeip_enabled = enabled;
    }
    model.config.settings.dns_strategy = model.config.settings.dns.strategy.clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::ConnectionState;
    use crate::app::update::key::ipc::handle_go_first;
    use crate::config::profile::{CustomDnsPreset, DnsRule, DnsServer, DnsStrategy, Profile};
    use crate::test_helpers::*;

    fn with_dns_overlay() -> Model {
        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::DnsSettings;
        model
    }

    fn home_preset(rules: Vec<DnsRule>) -> CustomDnsPreset {
        CustomDnsPreset {
            name: "home".into(),
            servers: vec![DnsServer::Local {
                tag: "local".into(),
            }],
            rules,
            final_server: "local".into(),
        }
    }

    fn select(model: &mut Model, item: DnsSettingsItem) {
        model.dns_selected = DnsSettingsItem::ALL
            .iter()
            .position(|candidate| *candidate == item)
            .unwrap();
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
    fn dns_settings_h_l_cycle_built_in_then_custom_presets() {
        let mut model = with_dns_overlay();
        model
            .config
            .settings
            .dns
            .custom_presets
            .push(home_preset(Vec::new()));

        handle_dns_settings(&mut model, key('l'));
        assert_eq!(model.dns_preset_draft.as_deref(), Some("google_dot"));
        handle_dns_settings(&mut model, key('h'));
        handle_dns_settings(&mut model, key('h'));
        assert_eq!(model.dns_preset_draft.as_deref(), Some("home"));
        handle_dns_settings(&mut model, key('l'));
        assert_eq!(model.dns_preset_draft.as_deref(), Some("cloudflare_doh"));
        assert_eq!(model.config.settings.dns.current_preset, "cloudflare_doh");
    }

    #[test]
    fn dns_settings_unknown_preset_starts_at_directional_edge() {
        let mut model = with_dns_overlay();
        model.config.settings.dns.current_preset = "removed".into();

        handle_dns_settings(&mut model, key('l'));
        assert_eq!(model.dns_preset_draft.as_deref(), Some("cloudflare_doh"));

        model.dns_preset_draft = None;
        handle_dns_settings(&mut model, key('h'));
        assert_eq!(model.dns_preset_draft.as_deref(), Some("system_local"));
    }

    #[test]
    fn dns_settings_strategy_keys_toggle_the_ipv4_strategies() {
        for (forward, backward) in [
            (key('l'), key('h')),
            (
                KeyEvent::from(KeyCode::Right),
                KeyEvent::from(KeyCode::Left),
            ),
        ] {
            let mut model = with_dns_overlay();
            select(&mut model, DnsSettingsItem::CycleStrategy);
            handle_dns_settings(&mut model, forward);
            assert_eq!(model.dns_strategy_draft, Some(DnsStrategy::OnlyIpv4));
            handle_dns_settings(&mut model, backward);
            assert_eq!(model.dns_strategy_draft, Some(DnsStrategy::PreferIpv4));
        }
    }

    #[test]
    fn dns_settings_enter_strategy_no_draft_is_noop() {
        let mut model = with_dns_overlay();
        select(&mut model, DnsSettingsItem::CycleStrategy);
        let before = model.config.settings.dns.strategy.clone();
        let effects = handle_dns_settings(&mut model, enter());
        assert!(effects.is_empty());
        assert_eq!(model.config.settings.dns.strategy, before);
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn dns_settings_enter_commits_every_draft() {
        let mut model = with_dns_overlay();
        select(&mut model, DnsSettingsItem::CycleStrategy);
        model.dns_strategy_draft = Some(DnsStrategy::OnlyIpv4);
        model.dns_preset_draft = Some("google_dot".into());
        model.dns_fakeip_draft = Some(true);
        let effects = handle_dns_settings(&mut model, enter());
        assert_eq!(model.config.settings.dns.strategy, DnsStrategy::OnlyIpv4);
        assert_eq!(model.config.settings.dns_strategy, DnsStrategy::OnlyIpv4);
        assert_eq!(model.config.settings.dns.current_preset, "google_dot");
        assert!(model.config.settings.dns.fakeip_enabled);
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
        select(&mut model, DnsSettingsItem::CycleStrategy);
        let same = model.config.settings.dns.strategy.clone();
        model.dns_strategy_draft = Some(same.clone());
        let effects = handle_dns_settings(&mut model, enter());
        assert!(effects.is_empty());
        assert!(model.dns_strategy_draft.is_none());
        assert_eq!(model.config.settings.dns.strategy, same);
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn dns_settings_enter_selects_a_custom_preset_without_touching_it() {
        let mut model = with_dns_overlay();
        let home = home_preset(Vec::new());
        model.config.settings.dns.custom_presets.push(home.clone());
        model.dns_preset_draft = Some("home".into());

        let effects = handle_dns_settings(&mut model, enter());

        assert!(effects.contains(&Effect::SaveConfig));
        assert_eq!(model.config.settings.dns.current_preset, "home");
        assert_eq!(model.config.settings.dns.custom_presets, [home]);
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn dns_settings_toggle_fakeip_enables_and_disables() {
        let mut model = with_dns_overlay();
        select(&mut model, DnsSettingsItem::ToggleFakeIp);
        assert!(!model.config.settings.dns.fakeip_enabled);
        handle_dns_settings(&mut model, key('l'));
        assert_eq!(model.dns_fakeip_draft, Some(true));
        handle_dns_settings(&mut model, enter());
        assert!(model.config.settings.dns.fakeip_enabled);
        assert!(model.dns_fakeip_draft.is_none());

        model.overlay = Overlay::DnsSettings;
        handle_dns_settings(&mut model, key('h'));
        handle_dns_settings(&mut model, enter());
        assert!(!model.config.settings.dns.fakeip_enabled);
    }

    #[test]
    fn dns_settings_fakeip_cycles_in_both_directions() {
        let mut model = with_dns_overlay();
        select(&mut model, DnsSettingsItem::ToggleFakeIp);

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
    fn dns_settings_esc_clears_draft_and_closes_overlay() {
        let mut model = with_dns_overlay();
        select(&mut model, DnsSettingsItem::CycleStrategy);
        model.dns_strategy_draft = Some(DnsStrategy::OnlyIpv4);
        model.dns_preset_draft = Some("google_dot".into());
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
        model.dns_preset_draft = Some("google_dot".into());
        let effects = handle_dns_settings(&mut model, enter());
        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(active_id));
        assert!(!effects.iter().any(|e| matches!(e, Effect::Connect { .. })));
    }

    #[test]
    fn dns_settings_change_while_connect_pending_restarts_the_attempt() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.overlay = Overlay::DnsSettings;
        model.connection = ConnectionState::ConnectPending;
        model.connecting_profile_id = Some(profile_id);
        let attempt = model.connect_attempt_id;
        model.dns_preset_draft = Some("google_dot".into());

        handle_dns_settings(&mut model, enter());

        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connect_attempt_id, attempt.wrapping_add(1));
    }

    #[test]
    fn dns_settings_refuses_a_preset_that_no_longer_exists() {
        let profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let active_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(active_id);
        model.overlay = Overlay::DnsSettings;
        let before = model.config.settings.dns.clone();
        model.dns_preset_draft = Some("removed".into());

        let effects = handle_dns_settings(&mut model, enter());

        assert!(!effects.contains(&Effect::SaveConfig));
        assert_eq!(model.config.settings.dns, before);
        assert_eq!(model.connection, ConnectionState::Connected);
        assert_eq!(model.overlay, Overlay::DnsSettings);
        assert!(model.dns_preset_draft.is_none());
        assert!(
            matches!(&model.status, Some(AppStatus::Error(message)) if message.contains("removed"))
        );
    }

    #[test]
    fn dns_settings_warns_when_fakeip_meets_an_ip_rule_set_rule() {
        for (rule_set, warns) in [("geoip-ru", true), ("geosite-category-ru", false)] {
            let mut model = with_dns_overlay();
            model
                .config
                .settings
                .dns
                .custom_presets
                .push(home_preset(vec![DnsRule {
                    rule_set: vec![rule_set.into()],
                    server: "local".into(),
                    ..Default::default()
                }]));
            model.config.settings.dns.current_preset = "home".into();
            model.dns_fakeip_draft = Some(true);

            handle_dns_settings(&mut model, enter());

            assert!(model.config.settings.dns.fakeip_enabled);
            assert_eq!(
                matches!(&model.status, Some(AppStatus::Info(message)) if message.contains(rule_set)),
                warns,
                "{rule_set}"
            );
        }
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

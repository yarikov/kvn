use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::model::Model;
use crate::config::profile::DnsStrategy;
use crate::ui::layout::text::{fit_to_visual_width, visual_width};

use super::popup::draw_selection_modal;
use super::settings_overlay_footer;
use super::settings_row::{settings_value_layout, settings_value_text};

fn preset_label(name: &str) -> &str {
    use crate::config::profile::DnsPreset;
    match DnsPreset::from_name(name) {
        Some(DnsPreset::CloudflareDoh) => "Cloudflare DoH",
        Some(DnsPreset::GoogleDot) => "Google DoT",
        Some(DnsPreset::Quad9Doh) => "Quad9 DoH",
        Some(DnsPreset::SystemLocal) => "System local",
        None => name,
    }
}

/// Draw the DNS settings overlay: preset, strategy, and fake-IP selectors.
/// Custom presets are written in profiles.json and selected here.
pub(super) fn draw(frame: &mut Frame, model: &Model, area: Rect) {
    let dns = &model.config.settings.dns;
    let displayed_preset = model
        .dns_preset_draft
        .as_deref()
        .unwrap_or(&dns.current_preset);
    let preset_dirty = displayed_preset != dns.current_preset;
    let strategy = model.dns_strategy_draft.as_ref().unwrap_or(&dns.strategy);
    let strategy_dirty = model
        .dns_strategy_draft
        .as_ref()
        .is_some_and(|draft| *draft != dns.strategy);
    let fakeip = model.dns_fakeip_draft.unwrap_or(dns.fakeip_enabled);
    let fakeip_dirty = model.dns_fakeip_draft.is_some() && fakeip != dns.fakeip_enabled;
    let preset_value = preset_label(displayed_preset);
    let strategy_label = match strategy {
        DnsStrategy::PreferIpv4 => "Prefer IPv4",
        DnsStrategy::PreferIpv6 => "Prefer IPv6",
        DnsStrategy::OnlyIpv4 => "IPv4 only",
        DnsStrategy::OnlyIpv6 => "IPv6 only",
    };
    let settings = [
        ("Preset", preset_value.to_string(), preset_dirty),
        ("Strategy", strategy_label.to_string(), strategy_dirty),
        (
            "Fake-IP",
            if fakeip { "on" } else { "off" }.to_string(),
            fakeip_dirty,
        ),
    ];
    let stable_value_width = dns
        .preset_names()
        .into_iter()
        .map(preset_label)
        .chain(["Prefer IPv4", "IPv4 only", "on", "off"])
        .map(visual_width)
        .max()
        .unwrap_or(0);
    let layout = settings_value_layout(&settings, stable_value_width, area, &[]);
    let labels = settings.map(|(name, value, dirty)| {
        fit_to_visual_width(
            &settings_value_text(name, &value, dirty, layout.label_width),
            layout.group_width,
        )
    });
    let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    draw_selection_modal(
        frame,
        &model.theme,
        area,
        "Settings › DNS",
        &label_refs,
        model.dns_selected,
        model.overlay_scroll,
        None,
        settings_overlay_footer(model),
    );
}

#[cfg(test)]
mod tests {

    use crate::app::model::Overlay;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_profiles, snapshot_terminal,
    };

    #[test]
    fn draw_dns_settings_overlay_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.overlay = Overlay::DnsSettings;
        model.settings_menu_return = Some(crate::app::model::SettingsMenuPage::Root);
        model.dns_selected = 1;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_dns_settings_overlay_custom_preset_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model
            .config
            .settings
            .dns
            .custom_presets
            .push(crate::config::profile::CustomDnsPreset {
                name: "home-adguard".to_string(),
                servers: vec![crate::config::profile::DnsServer::Local {
                    tag: "local".to_string(),
                }],
                rules: Vec::new(),
                final_server: "local".to_string(),
            });
        model.config.settings.dns.current_preset = "home-adguard".to_string();
        model.overlay = Overlay::DnsSettings;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_dns_settings_overlay_fakeip_on_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.config.settings.dns.fakeip_enabled = true;
        model.dns_preset_draft = Some("system_local".to_string());
        model.overlay = Overlay::DnsSettings;
        model.dns_selected = 2;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }
}

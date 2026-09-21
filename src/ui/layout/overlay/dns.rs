use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::model::Model;
use crate::ui::layout::text::{fit_to_visual_width, visual_width};

use super::popup::draw_selection_modal;
use super::settings_overlay_footer;
use super::settings_row::{settings_value_layout, settings_value_text};

/// Draw the DNS settings overlay: preset, strategy, and fake-IP selectors.
/// Custom servers and per-domain rules are edited via profiles.json.
pub(super) fn draw(frame: &mut Frame, model: &Model, area: Rect) {
    use crate::config::profile::DnsPreset;

    let dns = &model.config.settings.dns;
    let current_preset = DnsPreset::detect(dns);
    let displayed_preset = model.dns_preset_draft.or(current_preset);
    let preset_dirty = model.dns_preset_draft.is_some() && displayed_preset != current_preset;
    let strategy = model.dns_strategy_draft.as_ref().unwrap_or(&dns.strategy);
    let strategy_dirty = model
        .dns_strategy_draft
        .as_ref()
        .is_some_and(|draft| *draft != dns.strategy);
    let fakeip = model.dns_fakeip_draft.unwrap_or(dns.fakeip_enabled);
    let fakeip_dirty = model.dns_fakeip_draft.is_some() && fakeip != dns.fakeip_enabled;
    let preset_label = displayed_preset.map_or("Custom", |preset| match preset {
        DnsPreset::CloudflareDoh => "Cloudflare DoH",
        DnsPreset::GoogleDot => "Google DoT",
        DnsPreset::Quad9Doh => "Quad9 DoH",
        DnsPreset::SystemLocal => "System local",
    });
    let strategy_label = match strategy {
        crate::config::profile::DnsStrategy::PreferIpv4 => "Prefer IPv4",
        crate::config::profile::DnsStrategy::PreferIpv6 => "Prefer IPv6",
        crate::config::profile::DnsStrategy::OnlyIpv4 => "IPv4 only",
        crate::config::profile::DnsStrategy::OnlyIpv6 => "IPv6 only",
    };
    let settings = [
        ("Preset", preset_label.to_string(), preset_dirty),
        ("Strategy", strategy_label.to_string(), strategy_dirty),
        (
            "Fake-IP",
            if fakeip { "on" } else { "off" }.to_string(),
            fakeip_dirty,
        ),
    ];
    let stable_value_width = [
        "Custom",
        "Cloudflare DoH",
        "Google DoT",
        "Quad9 DoH",
        "System local",
        "Prefer IPv4",
        "Prefer IPv6",
        "IPv4 only",
        "IPv6 only",
        "on",
        "off",
    ]
    .into_iter()
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
    fn draw_dns_settings_overlay_fakeip_on_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.config.settings.dns.fakeip_enabled = true;
        model.dns_preset_draft = Some(crate::config::profile::DnsPreset::SystemLocal);
        model
            .config
            .settings
            .dns
            .servers
            .push(crate::config::profile::DnsServer::FakeIp {
                tag: "fakeip".to_string(),
                inet4_range: "198.18.0.0/15".to_string(),
                inet6_range: "fc00::/18".to_string(),
            });
        model.overlay = Overlay::DnsSettings;
        model.dns_selected = 2;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }
}

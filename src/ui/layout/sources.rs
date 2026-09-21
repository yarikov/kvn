use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::model::{Model, SourceRow};
use crate::ui::icons::icons;

use super::panes::panel_inner;
use super::text::{
    fit_to_visual_width, pad_to_visual_width, truncate_to_visual_width, visual_width,
};

/// Draw the unified Sources list: standalone profiles and subscription trees.
pub(super) fn draw_sources(frame: &mut Frame, model: &Model, area: Rect, focused: bool) {
    let theme = &model.theme;
    let block = Block::default()
        .title(" Profiles ")
        .borders(Borders::ALL)
        .border_style(if focused {
            theme.accent()
        } else {
            theme.border()
        });

    let inner_width = panel_inner(area).width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let rows = model.source_rows();

    if rows.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(" ", theme.normal()),
            Span::styled(
                "No sources. Press p to paste a profile or subscription URL from clipboard.",
                theme.normal(),
            ),
        ]));
    } else {
        // Global address-column width: max across all visible profiles so every
        // row shares the same column layout and aligns vertically.
        let show_latency =
            !model.profile_latencies.is_empty() || !model.testing_profiles.is_empty();
        let overhead = FIXED_OVERHEAD_BASE + if show_latency { LATENCY_WIDTH } else { 0 };
        let remaining = inner_width.saturating_sub(overhead);
        let addr_width = model
            .config
            .profiles
            .iter()
            .map(|p| visual_width(&format!("{}:{}", p.address, p.port)).max(MIN_ADDR_WIDTH))
            .max()
            .unwrap_or(MIN_ADDR_WIDTH)
            .min(MAX_ADDR_WIDTH)
            .min(remaining.saturating_sub(MIN_NAME_WIDTH));

        // Single pass over rows to collect indices — avoids O(n²) position() searches.
        let sub_count = model.config.subscriptions.len();
        let mut standalone: Vec<(usize, usize)> = Vec::new(); // (row_idx, profile_idx)
        let mut sub_header_idx = vec![0usize; sub_count];
        let mut sub_profile_rows: Vec<Vec<(usize, usize)>> = vec![Vec::new(); sub_count];
        for (i, row) in rows.iter().enumerate() {
            match row {
                SourceRow::StandaloneProfile(idx) => standalone.push((i, *idx)),
                SourceRow::SubscriptionHeader(idx) => sub_header_idx[*idx] = i,
                SourceRow::SubscriptionProfile {
                    sub_idx,
                    profile_idx,
                } => {
                    sub_profile_rows[*sub_idx].push((i, *profile_idx));
                }
            }
        }

        // Standalone profiles group.
        if !standalone.is_empty() {
            let last = standalone.len() - 1;
            for (pos, (row_idx, profile_idx)) in standalone.iter().enumerate() {
                lines.push(profile_line(
                    model,
                    *profile_idx,
                    *row_idx,
                    pos == last,
                    inner_width,
                    addr_width,
                    show_latency,
                ));
            }
            lines.push(Line::from(""));
        }

        // Subscription groups.
        for (sub_idx, sub) in model.config.subscriptions.iter().enumerate() {
            let header_idx = sub_header_idx[sub_idx];
            let is_selected = model.selected == header_idx;
            let header_style = if is_selected {
                theme.selected()
            } else {
                theme.normal()
            };
            let profiles = &sub_profile_rows[sub_idx];
            let header_text = format!(
                "Subscription: {} {} {}",
                sub.name,
                icons(model.icon_set()).refresh,
                sub.auto_update.label()
            );
            let header_text = if is_selected {
                pad_to_visual_width(&header_text, inner_width)
            } else {
                header_text
            };
            lines.push(Line::from(vec![
                Span::styled(" ", header_style),
                Span::styled(header_text, header_style),
                Span::styled(" ", header_style),
            ]));

            let last = profiles.len().saturating_sub(1);
            for (pos, (row_idx, profile_idx)) in profiles.iter().enumerate() {
                lines.push(profile_line(
                    model,
                    *profile_idx,
                    *row_idx,
                    pos == last,
                    inner_width,
                    addr_width,
                    show_latency,
                ));
            }
            lines.push(Line::from(""));
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// Width reserved for the tree prefix at the start of a profile row.
pub(super) const PREFIX_WIDTH: usize = 2;
/// Minimum width for the profile name column.
const MIN_NAME_WIDTH: usize = 8;
/// Width reserved for the protocol column.
const PROTOCOL_WIDTH: usize = 6;
/// Minimum width for the address:port column.
const MIN_ADDR_WIDTH: usize = 11;
/// Maximum width for the address:port column (covers full IPv4 `255.255.255.255:65535`).
/// Caps the address column so the name is never squeezed below MIN_NAME_WIDTH
/// even when a profile has a very long hostname.
const MAX_ADDR_WIDTH: usize = 21;
/// Width of the latency column including its leading space: " 9999ms" = 7 chars.
const LATENCY_WIDTH: usize = 7;
/// Fixed overhead without latency column: prefix + protocol + two spaces between columns.
const FIXED_OVERHEAD_BASE: usize = PREFIX_WIDTH + PROTOCOL_WIDTH + 2;

/// Build one profile row for the Sources list.
/// `addr_width` is pre-computed globally so all rows share the same column layout.
pub(super) fn profile_line(
    model: &Model,
    profile_idx: usize,
    row_idx: usize,
    is_last: bool,
    inner_width: usize,
    addr_width: usize,
    show_latency: bool,
) -> Line<'static> {
    use ratatui::text::Span;

    let theme = &model.theme;
    let profile = &model.config.profiles[profile_idx];
    let is_selected = model.selected == row_idx;

    let is_connected = model.active_profile_id == Some(profile.id);

    let prefix = if is_last { "└ " } else { "├ " };

    let addr_port = format!("{}:{}", profile.address, profile.port);
    let latency_w = if show_latency { LATENCY_WIDTH } else { 0 };
    let remaining = inner_width.saturating_sub(FIXED_OVERHEAD_BASE + latency_w);
    let name_width = remaining.saturating_sub(addr_width).max(MIN_NAME_WIDTH);

    let name_col = fit_to_visual_width(&profile.name, name_width);
    let protocol_col = fit_to_visual_width(profile.protocol_label(), PROTOCOL_WIDTH);
    let addr_col = truncate_to_visual_width(&addr_port, addr_width);

    let style = if is_selected && is_connected {
        theme.selected_connected()
    } else if is_selected {
        theme.selected()
    } else if is_connected {
        theme.success()
    } else {
        theme.normal()
    };

    let addr_col_padded = if is_selected || show_latency {
        pad_to_visual_width(&addr_col, addr_width)
    } else {
        addr_col
    };

    let used = PREFIX_WIDTH
        + name_width
        + 1
        + PROTOCOL_WIDTH
        + 1
        + visual_width(&addr_col_padded)
        + latency_w;
    let trailing = inner_width.saturating_sub(used);

    let mut spans = vec![
        Span::styled(" ", style),
        Span::styled(prefix, style),
        Span::styled(name_col, style),
        Span::styled(" ", style),
        Span::styled(protocol_col, style),
        Span::styled(" ", style),
        Span::styled(addr_col_padded, style),
    ];
    if show_latency {
        let latency_text = if model.testing_profiles.contains(&profile.id) {
            format!(" {:<6}", "…")
        } else {
            match model.profile_latencies.get(&profile.id) {
                Some(&Some(ms)) => format!(" {:<6}", format!("{}ms", ms.min(9999))),
                Some(None) => format!(" {:<6}", "err"),
                None => " ".repeat(LATENCY_WIDTH),
            }
        };
        spans.push(Span::styled(latency_text, style));
    }
    if is_selected && trailing > 0 {
        spans.push(Span::styled(" ".repeat(trailing), style));
    }
    spans.push(Span::styled(" ", style));
    Line::from(spans)
}

#[cfg(test)]
mod tests {

    use crate::app::model::ConnectionState;
    use crate::config::profile::Profile;
    use crate::test_helpers::{
        APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_profiles, snapshot_styles, snapshot_terminal,
    };

    #[test]
    fn draw_sources_snapshot() {
        use crate::config::profile::{Subscription, SubscriptionAutoUpdate};
        use uuid::Uuid;

        let sub_id = Uuid::new_v4();
        let profiles = vec![
            Profile::new_vless(
                "Alpha".to_string(),
                "1.1.1.1".to_string(),
                443,
                "u1".to_string(),
            ),
            Profile::new_vless(
                "Beta".to_string(),
                "2.2.2.2".to_string(),
                443,
                "u2".to_string(),
            ),
            {
                let mut p = Profile::new_vless(
                    "Gamma".to_string(),
                    "3.3.3.3".to_string(),
                    443,
                    "u3".to_string(),
                );
                p.subscription_id = Some(sub_id);
                p
            },
            {
                let mut p = Profile::new_vless(
                    "Delta".to_string(),
                    "4.4.4.4".to_string(),
                    443,
                    "u4".to_string(),
                );
                p.subscription_id = Some(sub_id);
                p
            },
        ];
        let mut model = model_with_profiles(profiles);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Example".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        model.selected = 0;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    /// A very long hostname must not push the name column below MIN_NAME_WIDTH.
    #[test]
    fn draw_sources_long_address_does_not_hide_name() {
        let profiles = vec![
            Profile::new_vless(
                "MyProfile".to_string(),
                "very-long-hostname.infrastructure.example.com".to_string(),
                65535,
                "u1".to_string(),
            ),
            Profile::new_vless(
                "Short".to_string(),
                "1.1.1.1".to_string(),
                443,
                "u2".to_string(),
            ),
        ];
        let mut model = model_with_profiles(profiles);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.selected = 0;
        let output = snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS);
        // Name "MyProfile" must still be visible (truncated to MIN_NAME_WIDTH).
        assert!(
            output.contains("MyPro"),
            "name column vanished with long address"
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn draw_sources_long_name_truncated() {
        let profiles = vec![
            Profile::new_vless(
                "VeryLongProfileNameThatMustBeTruncated".to_string(),
                "1.1.1.1".to_string(),
                443,
                "u1".to_string(),
            ),
            Profile::new_vless(
                "Second".to_string(),
                "2.2.2.2".to_string(),
                443,
                "u2".to_string(),
            ),
        ];
        let mut model = model_with_profiles(profiles);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.selected = 0;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    /// Verify every protocol's UI badge renders without truncation.
    #[test]
    fn draw_sources_multi_protocol_badges_snapshot() {
        use crate::config::profile::{
            AnytlsConfig, HttpConfig, Hysteria2Config, ProtocolConfig, ShadowsocksCipher,
            ShadowsocksConfig, ShadowtlsConfig, SocksConfig, SshConfig, TrojanConfig, TuicConfig,
            VmessConfig,
        };
        use uuid::Uuid;

        let make = |name: &str, address: &str, port: u16, config: ProtocolConfig| Profile {
            id: Uuid::new_v4(),
            name: name.to_string(),
            address: address.to_string(),
            port,
            config,
            tags: vec![],
            subscription_id: None,
        };

        let profiles = vec![
            Profile::new_vless(
                "A-vless".to_string(),
                "1.1.1.1".to_string(),
                443,
                "u1".to_string(),
            ),
            make(
                "B-vmess",
                "2.2.2.2",
                443,
                ProtocolConfig::Vmess(VmessConfig {
                    uuid: "u2".to_string(),
                    ..Default::default()
                }),
            ),
            make(
                "C-trojan",
                "3.3.3.3",
                443,
                ProtocolConfig::Trojan(TrojanConfig {
                    password: "pw".to_string(),
                    ..Default::default()
                }),
            ),
            make(
                "D-ss",
                "4.4.4.4",
                8388,
                ProtocolConfig::Shadowsocks(ShadowsocksConfig {
                    method: ShadowsocksCipher::Chacha20IetfPoly1305,
                    password: "pw".to_string(),
                }),
            ),
            make(
                "E-hy2",
                "5.5.5.5",
                443,
                ProtocolConfig::Hysteria2(Hysteria2Config {
                    password: "pw".to_string(),
                    ..Default::default()
                }),
            ),
            make(
                "F-tuic",
                "6.6.6.6",
                443,
                ProtocolConfig::Tuic(TuicConfig {
                    uuid: "u6".to_string(),
                    password: "pw".to_string(),
                    ..Default::default()
                }),
            ),
            make(
                "G-stls",
                "7.7.7.7",
                443,
                ProtocolConfig::Shadowtls(ShadowtlsConfig {
                    password: "pw".to_string(),
                    ss_password: "sp".to_string(),
                    ..Default::default()
                }),
            ),
            make(
                "H-anytls",
                "8.8.8.8",
                443,
                ProtocolConfig::Anytls(AnytlsConfig {
                    password: "pw".to_string(),
                    ..Default::default()
                }),
            ),
            make(
                "I-socks",
                "9.9.9.9",
                1080,
                ProtocolConfig::Socks(SocksConfig::default()),
            ),
            make(
                "J-http",
                "10.0.0.1",
                8080,
                ProtocolConfig::Http(HttpConfig::default()),
            ),
            make(
                "K-ssh",
                "10.0.0.2",
                22,
                ProtocolConfig::Ssh(SshConfig {
                    user: "root".to_string(),
                    ..Default::default()
                }),
            ),
        ];
        let mut model = model_with_profiles(profiles);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.selected = 0;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    #[test]
    fn draw_sources_connected_profile_colored() {
        let mut model = model_with_profiles(vec![
            Profile::new_vless(
                "Alpha".to_string(),
                "1.1.1.1".to_string(),
                443,
                "u1".to_string(),
            ),
            Profile::new_vless(
                "Beta".to_string(),
                "2.2.2.2".to_string(),
                443,
                "u2".to_string(),
            ),
        ]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(model.config.profiles[1].id);
        model.selected = 1;
        insta::assert_snapshot!(snapshot_styles(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    /// Empty Sources pane: pins the "No sources." placeholder at
    /// `draw_sources` L402–405, which currently has no visual regression guard.
    #[test]
    fn draw_sources_empty_state_snapshot() {
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }

    /// Subscription rendered with a populated `last_updated` and a non-default
    /// `auto_update` interval — covers the subscription-header formatting path
    /// when `Every1d` is active.
    #[test]
    fn draw_sources_subscription_with_last_updated_snapshot() {
        use crate::config::profile::{Subscription, SubscriptionAutoUpdate};
        use chrono::TimeZone;
        use uuid::Uuid;

        let sub_id = Uuid::new_v4();
        let mut profile = Profile::new_vless(
            "Alpha".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        );
        profile.subscription_id = Some(sub_id);
        let mut model = model_with_profiles(vec![profile]);
        model.geo_last_updated = Some("2026-05-31 13:41".to_string());
        let last_updated = chrono::Local
            .with_ymd_and_hms(2026, 6, 14, 9, 30, 0)
            .unwrap();
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Example".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: Some(last_updated),
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        model.selected = 0;
        insta::assert_snapshot!(snapshot_terminal(&model, APP_WINDOW_COLS, APP_WINDOW_ROWS));
    }
}

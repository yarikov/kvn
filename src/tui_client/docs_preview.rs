use std::io;
use std::time::Duration;

use anyhow::{Context, Result};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::model::{AppStatus, ConnectionState, Model, TrafficStats};

use super::{TerminalSession, apply_terminal_colors};

/// Render a fixed, side-effect-free application state for documentation captures.
pub fn run_docs_preview(theme_slug: &str) -> Result<()> {
    use crossterm::event::{self, Event, KeyCode, KeyModifiers};

    let DocsPreviewState { model, toast } = build_docs_preview_state(theme_slug)?;

    let _terminal_session = TerminalSession::enter()?;
    apply_terminal_colors(
        model.theme.palette_foreground(),
        model.theme.palette_background(),
    );
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    loop {
        terminal.draw(|frame| {
            crate::ui::layout::draw_with_toast(
                frame,
                &model,
                crate::app::model::MainPaneFocus::Sources,
                None,
                None,
                Some(&toast),
            )
        })?;
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && (matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                || (key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)))
        {
            return Ok(());
        }
    }
}

const DOCS_PREVIEW_TOAST: &str = "Kill switch enabled";

struct DocsPreviewState {
    model: Model,
    toast: AppStatus,
}

fn build_docs_preview_state(theme_slug: &str) -> Result<DocsPreviewState> {
    build_docs_preview_state_at(theme_slug, chrono::Local::now())
}

fn build_docs_preview_state_at(
    theme_slug: &str,
    preview_now: chrono::DateTime<chrono::Local>,
) -> Result<DocsPreviewState> {
    use crate::config::profile::{
        Config, GeoAutoUpdate, GeoRegion, Hysteria2Config, ProtocolConfig, RoutingMode,
        Subscription, SubscriptionAutoUpdate, TrojanConfig, TuicConfig, VlessConfig, VmessConfig,
    };
    use chrono::Timelike;
    use uuid::Uuid;

    let Some(palette) = crate::ui::palette::Palette::lookup(theme_slug) else {
        anyhow::bail!("unknown bundled theme {theme_slug:?}");
    };

    let finland_subscription_id = Uuid::from_u128(0x22222222222222222222222222222222);
    let france_subscription_id = Uuid::from_u128(0x33333333333333333333333333333333);
    let profiles = vec![
        preview_profile(
            1,
            "🇳🇱 Netherlands",
            "nl-1.demo.example",
            ProtocolConfig::Vless(VlessConfig {
                uuid: preview_uuid(),
                ..Default::default()
            }),
            None,
        ),
        preview_profile(
            2,
            "🇨🇳 China",
            "cn-1.demo.example",
            ProtocolConfig::Vmess(VmessConfig {
                uuid: preview_uuid(),
                ..Default::default()
            }),
            None,
        ),
        preview_profile(
            3,
            "🇺🇸 United States",
            "us-1.demo.example",
            ProtocolConfig::Trojan(TrojanConfig {
                password: "demo".into(),
                ..Default::default()
            }),
            None,
        ),
        preview_profile(
            4,
            "🇨🇦 Canada",
            "ca-1.demo.example",
            ProtocolConfig::Hysteria2(Hysteria2Config {
                password: "demo".into(),
                ..Default::default()
            }),
            None,
        ),
        preview_profile(
            5,
            "🇩🇪 Germany",
            "de-1.demo.example",
            ProtocolConfig::Tuic(TuicConfig {
                uuid: preview_uuid(),
                password: "demo".into(),
                ..Default::default()
            }),
            None,
        ),
        preview_profile(
            6,
            "🇫🇮 Helsinki",
            "fi-1.demo.example",
            ProtocolConfig::Vless(VlessConfig {
                uuid: preview_uuid(),
                ..Default::default()
            }),
            Some(finland_subscription_id),
        ),
        preview_profile(
            7,
            "🇫🇮 Tampere",
            "fi-2.demo.example",
            ProtocolConfig::Vless(VlessConfig {
                uuid: preview_uuid(),
                ..Default::default()
            }),
            Some(finland_subscription_id),
        ),
        preview_profile(
            8,
            "🇫🇷 Paris",
            "fr-1.demo.example",
            ProtocolConfig::Hysteria2(Hysteria2Config {
                password: "demo".into(),
                ..Default::default()
            }),
            Some(france_subscription_id),
        ),
        preview_profile(
            9,
            "🇫🇷 Lyon",
            "fr-2.demo.example",
            ProtocolConfig::Hysteria2(Hysteria2Config {
                password: "demo".into(),
                ..Default::default()
            }),
            Some(france_subscription_id),
        ),
        preview_profile(
            10,
            "🇫🇷 Marseille",
            "fr-3.demo.example",
            ProtocolConfig::Hysteria2(Hysteria2Config {
                password: "demo".into(),
                ..Default::default()
            }),
            Some(france_subscription_id),
        ),
    ];
    let active_id = profiles[2].id;
    let mut config = Config {
        profiles,
        ..Default::default()
    };
    config.subscriptions.push(Subscription {
        id: finland_subscription_id,
        name: "🇫🇮 Finland VLESS".into(),
        url: "http://finland-subscription.demo.example/list".into(),
        auto_update: SubscriptionAutoUpdate::Every1d,
        last_updated: None,
        next_auto_update: None,
        retry_state: None,
        send_hwid: false,
        hwid: None,
    });
    config.subscriptions.push(Subscription {
        id: france_subscription_id,
        name: "🇫🇷 France Hysteria2".into(),
        url: "https://france-subscription.demo.example/list".into(),
        auto_update: SubscriptionAutoUpdate::Every7d,
        last_updated: None,
        next_auto_update: None,
        retry_state: None,
        send_hwid: false,
        hwid: None,
    });
    config.settings.theme = theme_slug.into();
    config.settings.auto_connect = true;
    config.settings.kill_switch = true;
    config.settings.allow_insecure_http_subscriptions = true;
    config.settings.geo_routing.set_region(GeoRegion::Ru);
    config.settings.geo_routing.set_mode(RoutingMode::Global);
    config.settings.geo_routing.auto_update = GeoAutoUpdate::Every3d;

    let deprecated_http_warning = crate::config::subscription::deprecated_http_warning(
        &config.subscriptions[0],
        &config.settings,
    )
    .context("first documentation-preview subscription should use deprecated HTTP")?;

    let mut model = Model::in_memory(config);
    model.theme = crate::ui::styles::Theme::from_palette(palette);
    model.connection = ConnectionState::Connected;
    model.active_profile_id = Some(active_id);
    model.selected = 2;
    model.main_pane_focus = crate::app::model::MainPaneFocus::Sources;
    model.status = AppStatus::Info("Connected to 🇺🇸 United States".into());
    model.geo_last_checked_at = Some(
        preview_now
            .with_hour(8)
            .and_then(|time| time.with_minute(0))
            .and_then(|time| time.with_second(0))
            .and_then(|time| time.with_nanosecond(0))
            .context("could not set documentation-preview rule-set time to 08:00")?,
    );
    model.traffic = TrafficStats {
        up_rate_bps: 731 * 1024,
        down_rate_bps: 5_033_165,
        up_total: 105 * 1024 * 1024,
        down_total: 822 * 1024 * 1024,
        conn_count: 25,
    };
    let log_time = |second: u8| format!("15:30:{second:02}");
    for line in [
        format!("[app] {} INFO Connected to 🇺🇸 United States", log_time(1)),
        format!("[app] {} WARN {deprecated_http_warning}", log_time(1)),
        format!(
            "[app] {} INFO Imported 2 profile(s) from subscription",
            log_time(1)
        ),
        format!(
            "[sb] {} INFO inbound/tun[tun-in]: inbound connection from 10.222.0.1:57460",
            log_time(2)
        ),
        format!(
            "[sb] {} INFO inbound/tun[tun-in]: inbound connection to 198.51.100.20:443",
            log_time(2)
        ),
        format!(
            "[sb] {} INFO outbound/trojan[proxy]: outbound connection to 198.51.100.20:443",
            log_time(2)
        ),
        format!(
            "[sb] {} INFO inbound/tun[tun-in]: inbound packet connection from 10.222.0.1:46808",
            log_time(3)
        ),
        format!(
            "[sb] {} INFO inbound/tun[tun-in]: inbound packet connection to 10.222.0.2:53",
            log_time(3)
        ),
        format!(
            "[sb] {} INFO dns: exchanged A docs.demo.example. 291 IN A 192.0.2.10",
            log_time(3)
        ),
        format!(
            "[sb] {} INFO outbound/trojan[proxy]: outbound connection to 192.0.2.10:443",
            log_time(4)
        ),
        format!(
            "[sb] {} INFO inbound/tun[tun-in]: inbound connection from 10.222.0.1:42130",
            log_time(4)
        ),
        format!(
            "[sb] {} INFO inbound/tun[tun-in]: inbound connection to 203.0.113.40:443",
            log_time(4)
        ),
        format!(
            "[sb] {} INFO outbound/trojan[proxy]: outbound connection to 203.0.113.40:443",
            log_time(4)
        ),
        format!(
            "[sb] {} INFO dns: exchanged AAAA api.demo.example. 300 IN AAAA 2001:db8::20",
            log_time(5)
        ),
        format!("[app] {} INFO {DOCS_PREVIEW_TOAST}", log_time(7)),
    ] {
        model.push_log(line);
    }

    Ok(DocsPreviewState {
        model,
        toast: AppStatus::Info(DOCS_PREVIEW_TOAST.into()),
    })
}

fn preview_profile(
    id: u128,
    name: &str,
    address: &str,
    config: crate::config::profile::ProtocolConfig,
    subscription_id: Option<uuid::Uuid>,
) -> crate::config::profile::Profile {
    crate::config::profile::Profile {
        id: uuid::Uuid::from_u128(id),
        name: name.into(),
        address: address.into(),
        port: 443,
        config,
        tags: Vec::new(),
        subscription_id,
    }
}

fn preview_uuid() -> String {
    "11111111-1111-1111-1111-111111111111".to_string()
}
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn docs_preview_shows_realistic_logs_and_kill_switch_toast() {
        let preview_now = chrono::Local
            .with_ymd_and_hms(2026, 9, 8, 11, 19, 42)
            .single()
            .unwrap();
        let state = build_docs_preview_state_at("tokyo-night", preview_now).unwrap();
        let logs: Vec<_> = state.model.logs.iter().map(String::as_str).collect();
        let endpoints: Vec<_> = state
            .model
            .config
            .profiles
            .iter()
            .map(|profile| format!("{}:{}", profile.address, profile.port))
            .collect();

        assert!(
            endpoints
                .windows(2)
                .all(|pair| pair[0].len() == pair[1].len())
        );
        assert_eq!(endpoints[0].len(), 21);
        assert_eq!(
            logs.first().copied(),
            Some("[app] 15:30:01 INFO Connected to 🇺🇸 United States")
        );
        assert!(logs[1].starts_with(
            "[app] 15:30:01 WARN HTTP subscription '🇫🇮 Finland VLESS' uses deprecated"
        ));
        assert!(!logs[1].contains(&state.model.config.subscriptions[0].url));
        assert_eq!(
            logs.last().copied(),
            Some("[app] 15:30:07 INFO Kill switch enabled")
        );
        assert_eq!(
            state.model.geo_last_checked_at.unwrap(),
            chrono::Local
                .with_ymd_and_hms(2026, 9, 8, 8, 0, 0)
                .single()
                .unwrap()
        );
        assert!(logs.iter().any(|line| line.starts_with("[sb] ")));
        assert!(logs.iter().all(|line| !line.contains(" ERROR ")));
        assert_eq!(state.toast.text(), DOCS_PREVIEW_TOAST);

        let buffer = crate::test_helpers::render_to_buffer(
            crate::test_helpers::APP_WINDOW_COLS,
            crate::test_helpers::APP_WINDOW_ROWS,
            |frame| {
                crate::ui::layout::draw_with_toast(
                    frame,
                    &state.model,
                    crate::app::model::MainPaneFocus::Sources,
                    None,
                    None,
                    Some(&state.toast),
                )
            },
        );
        let output = crate::test_helpers::buffer_to_string(&buffer);
        let lines: Vec<_> = output.lines().collect();
        let connected_row = lines
            .iter()
            .position(|line| line.contains("15:30:01 [app] INFO Connected to"))
            .unwrap();
        let warning_row = lines
            .iter()
            .position(|line| line.contains("15:30:01 [app] WARN HTTP subscription"))
            .unwrap();

        assert_eq!(warning_row, connected_row + 1);
        assert!(output.contains("[sbx] INFO"));
        assert!(output.contains(" (3d) 08 Sep 08:00"));
        assert!(!output.contains("ERROR"));
        assert!(output.matches(DOCS_PREVIEW_TOAST).count() >= 2);

        for row in 4..(crate::test_helpers::APP_WINDOW_ROWS - 2) {
            assert!(
                (58..112).any(|column| {
                    buffer
                        .cell((column, row))
                        .is_some_and(|cell| !cell.symbol().trim().is_empty())
                }),
                "log viewport row {row} should not be blank"
            );
        }
    }
}

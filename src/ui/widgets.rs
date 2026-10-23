use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Widget, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::model::{AppStatus, ConnectionState, Model};
use crate::config::profile::RoutingMode;

const RULE_SET_STALE_GRACE_MINUTES: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleSetFreshness {
    Disabled,
    Updating,
    Never,
    Fresh(chrono::DateTime<chrono::Local>),
    Stale(chrono::DateTime<chrono::Local>),
    Manual(chrono::DateTime<chrono::Local>),
}

fn rule_set_freshness(model: &Model, now: chrono::DateTime<chrono::Local>) -> RuleSetFreshness {
    let routing = &model.config.settings.geo_routing;
    let selectable = model.selectable_rule_sets();

    if selectable.is_empty() {
        return RuleSetFreshness::Disabled;
    }
    if selectable
        .iter()
        .any(|rule_set| model.rule_set_updates.contains_key(&rule_set.key()))
    {
        return RuleSetFreshness::Updating;
    }

    let Some(last_checked_at) = model.selectable_rule_sets_checked_at() else {
        return RuleSetFreshness::Never;
    };

    let interval = routing.auto_update.interval_minutes();
    if interval == 0 {
        return RuleSetFreshness::Manual(last_checked_at);
    }
    let stale_after = interval.saturating_add(RULE_SET_STALE_GRACE_MINUTES);
    if now.signed_duration_since(last_checked_at).num_minutes() >= stale_after as i64 {
        RuleSetFreshness::Stale(last_checked_at)
    } else {
        RuleSetFreshness::Fresh(last_checked_at)
    }
}

fn rule_set_badge(
    model: &Model,
    now: chrono::DateTime<chrono::Local>,
) -> Option<(String, ratatui::style::Style)> {
    let (text, style) = match rule_set_freshness(model, now) {
        RuleSetFreshness::Disabled => return None,
        RuleSetFreshness::Updating => (
            "updating…".to_string(),
            model.theme.ruleset_updating_badge(),
        ),
        RuleSetFreshness::Never => ("never".to_string(), model.theme.ruleset_stale_badge()),
        RuleSetFreshness::Fresh(checked_at) => (
            checked_at.format("%d %b %H:%M").to_string(),
            model.theme.ruleset_fresh_badge(),
        ),
        RuleSetFreshness::Stale(checked_at) => (
            checked_at.format("%d %b %H:%M").to_string(),
            model.theme.ruleset_stale_badge(),
        ),
        RuleSetFreshness::Manual(checked_at) => (
            checked_at.format("%d %b %H:%M").to_string(),
            model.theme.ruleset_manual_badge(),
        ),
    };
    let refresh = crate::ui::icons::icons(model.icon_set()).refresh;
    let schedule = model.config.settings.geo_routing.auto_update.label();
    Some((format!(" {refresh} {schedule} {text} "), style))
}

pub struct StatusBar<'a> {
    model: &'a Model,
}

impl<'a> StatusBar<'a> {
    pub fn new(model: &'a Model) -> Self {
        Self { model }
    }
}

pub fn format_bps(bps: u64) -> String {
    format_quantity(bps, true)
}

fn format_quantity(bytes: u64, per_second: bool) -> String {
    const UNITS: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];

    let (mut value, mut unit_index) = (bytes as f64 / 1024.0, 1);
    while value >= 999.5 && unit_index < UNITS.len() - 1 {
        value /= 1024.0;
        unit_index += 1;
    }

    let number = if value < 9.95 {
        format!("{value:.1}")
    } else {
        format!("{value:.0}")
    };
    let suffix = if per_second { "/s" } else { "" };
    format!("{number} {}{suffix}", UNITS[unit_index])
}

pub fn format_bytes(bytes: u64) -> String {
    format_quantity(bytes, false)
}

const TRAFFIC_RATE_WIDTH: usize = 8;
const TRAFFIC_TOTAL_WIDTH: usize = 6;
const TRAFFIC_CONNECTIONS_COUNT_WIDTH: usize = 6;

fn fixed_width_field(value: String, width: usize, overflow: &str) -> String {
    let value = if value.len() > width {
        overflow
    } else {
        &value
    };
    format!("{value:>width$}")
}

pub fn format_bps_field(bps: u64) -> String {
    fixed_width_field(format_bps(bps), TRAFFIC_RATE_WIDTH, ">9 EB/s")
}

pub fn format_bytes_field(bytes: u64) -> String {
    fixed_width_field(format_bytes(bytes), TRAFFIC_TOTAL_WIDTH, ">9 EB")
}

pub fn format_connections_field(connections: usize) -> String {
    if connections > 99_999 {
        "99999+".to_string()
    } else {
        connections.to_string()
    }
}

pub fn format_connections_padding(connections: usize) -> String {
    " ".repeat(TRAFFIC_CONNECTIONS_COUNT_WIDTH - format_connections_field(connections).len())
}

fn truncate_to_width(s: &str, max_width: usize) -> String {
    if max_width <= 3 {
        return ".".repeat(max_width);
    }
    let mut result = String::with_capacity(max_width);
    let mut width = 0;
    for ch in s.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width > max_width.saturating_sub(3) {
            result.push_str("...");
            return result;
        }
        result.push(ch);
        width += ch_width;
    }
    result
}

impl<'a> Widget for StatusBar<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        Paragraph::new("")
            .style(self.model.theme.status_bar())
            .render(area, buf);

        let (status, style) = match self.model.connection {
            ConnectionState::Connected => (" CONNECTED ", self.model.theme.connected_badge()),
            ConnectionState::Connecting | ConnectionState::ConnectPending => {
                (" CONNECTING ", self.model.theme.connecting_badge())
            }
            ConnectionState::Idle => (" DISCONNECTED ", self.model.theme.offline_badge()),
        };

        let dns = &self.model.config.settings.dns;
        let dns_label = if dns.fakeip_enabled {
            "fakeip".to_string()
        } else {
            dns.active()
                .and_then(|active| {
                    active
                        .final_server_entry()
                        .map(|s| s.kind_label().to_string())
                })
                .unwrap_or_else(|| "?".to_string())
        };

        let mut right = Vec::new();
        if self.model.config.settings.kill_switch {
            right.push(("KS".to_string(), self.model.theme.accent(), 4));
        }
        if self.model.config.settings.auto_connect {
            right.push(("Auto".to_string(), self.model.theme.accent(), 0));
        }
        right.push((dns_label, self.model.theme.accent(), 1_u8));
        let routing_mode = self
            .model
            .config
            .settings
            .geo_routing
            .mode(&self.model.routing);
        right.push(match self.model.routing_fallback {
            Some(fallback) if routing_mode != RoutingMode::Global => (
                format!(
                    "{} → {fallback}",
                    self.model.routing.mode_name(routing_mode)
                ),
                self.model.theme.error(),
                3,
            ),
            _ => (
                self.model.routing.mode_name(routing_mode),
                self.model.theme.accent(),
                3,
            ),
        });
        if let Some((label, style)) = rule_set_badge(self.model, chrono::Local::now()) {
            right.push((label, style, 2));
        }

        let right_width = |parts: &[(String, ratatui::style::Style, u8)]| {
            parts.iter().map(|(text, _, _)| text.width()).sum::<usize>()
                + parts.len().saturating_sub(1) * 2
        };
        let available = usize::from(area.width);
        let status_width = status.width();
        while status_width + 1 + right_width(&right) > available {
            let Some((index, _)) = right
                .iter()
                .enumerate()
                .min_by_key(|(_, (_, _, priority))| *priority)
            else {
                break;
            };
            right.remove(index);
        }

        let right_size = right_width(&right);
        let profile = (self.model.connection == ConnectionState::Connected)
            .then(|| {
                self.model.active_profile_id.and_then(|id| {
                    self.model
                        .config
                        .profiles
                        .iter()
                        .find(|profile| profile.id == id)
                        .map(|profile| profile.name.as_str())
                })
            })
            .flatten();
        let left_limit = available.saturating_sub(right_size + usize::from(!right.is_empty()));
        let profile_limit = left_limit.saturating_sub(status_width + 1);
        let profile = profile
            .filter(|_| profile_limit >= 4)
            .map(|name| truncate_to_width(name, profile_limit));

        let mut left = vec![Span::styled(status, style)];
        if let Some(profile) = profile {
            left.push(Span::raw(" "));
            left.push(Span::styled(profile, self.model.theme.normal()));
        }
        Paragraph::new(Line::from(left))
            .style(self.model.theme.status_bar())
            .alignment(Alignment::Left)
            .render(area, buf);

        if !right.is_empty() {
            let mut spans = Vec::new();
            for (index, (text, style, _)) in right.into_iter().enumerate() {
                if index > 0 {
                    spans.push(Span::raw("  "));
                }
                spans.push(Span::styled(text, style));
            }
            let right_area = Rect::new(
                area.right().saturating_sub(right_size as u16),
                area.y,
                right_size as u16,
                1,
            );
            Paragraph::new(Line::from(spans))
                .style(self.model.theme.status_bar())
                .alignment(Alignment::Right)
                .render(right_area, buf);
        }
    }
}

struct Toast<'a> {
    status: &'a AppStatus,
    theme: &'a crate::ui::styles::Theme,
}

impl<'a> Toast<'a> {
    fn new(status: &'a AppStatus, theme: &'a crate::ui::styles::Theme) -> Self {
        Self { status, theme }
    }

    fn width(&self, container: Rect) -> u16 {
        let max_width = container.width.saturating_sub(2).min(TOAST_MAX_WIDTH);
        let text_width = self.status.text().width().min(usize::from(max_width)) as u16;
        text_width.saturating_add(4).min(max_width)
    }

    fn height(&self, width: u16) -> u16 {
        let inner_width = usize::from(width.saturating_sub(4)).max(1);
        if self.status.text().width() > inner_width {
            4
        } else {
            3
        }
    }
}

const TOAST_MAX_WIDTH: u16 = 40;

pub struct ToastStack<'a> {
    toasts: Vec<Toast<'a>>,
}

impl<'a> ToastStack<'a> {
    pub fn new(newest_first: &[&'a AppStatus], theme: &'a crate::ui::styles::Theme) -> Self {
        let toasts = newest_first
            .iter()
            .filter(|status| !status.text().is_empty())
            .map(|status| Toast::new(status, theme))
            .collect();
        Self { toasts }
    }

    fn areas(&self, container: Rect) -> Vec<Rect> {
        if container.width < 12 || container.height < 5 {
            return Vec::new();
        }
        let bottom = container.bottom().saturating_sub(1);
        let mut y = container.y + 1;
        let mut areas = Vec::new();
        for toast in &self.toasts {
            let width = toast.width(container);
            let height = toast.height(width);
            if y + height > bottom {
                break;
            }
            let x = container.right().saturating_sub(width + 1);
            areas.push(Rect::new(x, y, width, height));
            y += height;
        }
        areas
    }
}

impl Widget for ToastStack<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let areas = self.areas(area);
        for (toast, toast_area) in self.toasts.into_iter().zip(areas) {
            toast.render(toast_area, buf);
        }
    }
}

impl Widget for Toast<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 3 || area.height < 3 {
            return;
        }
        Clear.render(area, buf);
        let border_style = if matches!(self.status, AppStatus::Error(_)) {
            self.theme.toast_error()
        } else {
            self.theme.toast_info()
        };
        let inner_width = usize::from(area.width.saturating_sub(4));
        let text = truncate_to_width(self.status.text(), inner_width * 2);
        Paragraph::new(text)
            .style(self.theme.popup_bg())
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .padding(Padding::horizontal(1))
                    .border_style(border_style)
                    .style(self.theme.popup_bg()),
            )
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::ConnectionState;
    use crate::config::profile::{GeoAutoUpdate, GeoRegion, Profile};
    use crate::test_helpers::{
        APP_WINDOW_COLS, buffer_to_string, buffer_to_styled_string, model_with_profiles,
    };
    use chrono::{Duration, TimeZone};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn status_bar_styles(model: &crate::app::model::Model) -> String {
        let area = Rect::new(0, 0, APP_WINDOW_COLS, 1);
        let mut buf = Buffer::empty(area);
        StatusBar::new(model).render(area, &mut buf);
        buffer_to_styled_string(&buf)
    }

    fn local_time(day: u32, hour: u32) -> chrono::DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(2026, 8, day, hour, 20, 0)
            .single()
            .unwrap()
    }

    fn model_with_region_rule_sets() -> Model {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model
    }

    #[test]
    fn rule_set_freshness_is_disabled_without_active_sets() {
        let model = model_with_profiles(vec![]);
        assert_eq!(
            rule_set_freshness(&model, local_time(14, 12)),
            RuleSetFreshness::Disabled
        );
    }

    fn mark_ru_rule_sets_updating(model: &mut Model) {
        for rule_set in crate::test_helpers::region_rule_sets(GeoRegion::Ru) {
            model
                .rule_set_updates
                .insert(rule_set.key(), crate::app::model::UpdateTrigger::Manual);
        }
    }

    #[test]
    fn rule_set_freshness_reports_updating_and_never() {
        let mut model = model_with_region_rule_sets();
        assert_eq!(
            rule_set_freshness(&model, local_time(14, 12)),
            RuleSetFreshness::Never
        );

        mark_ru_rule_sets_updating(&mut model);
        assert_eq!(
            rule_set_freshness(&model, local_time(14, 12)),
            RuleSetFreshness::Updating
        );
    }

    #[test]
    fn rule_set_freshness_becomes_stale_one_hour_after_configured_interval() {
        let mut model = model_with_region_rule_sets();
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Every3d;
        let checked_at = local_time(11, 12);
        model.mark_selectable_rule_sets_checked(checked_at);

        assert_eq!(
            rule_set_freshness(&model, checked_at + Duration::days(3)),
            RuleSetFreshness::Fresh(checked_at)
        );
        assert_eq!(
            rule_set_freshness(
                &model,
                checked_at + Duration::days(3) + Duration::minutes(59)
            ),
            RuleSetFreshness::Fresh(checked_at)
        );
        assert_eq!(
            rule_set_freshness(&model, checked_at + Duration::days(3) + Duration::hours(1)),
            RuleSetFreshness::Stale(checked_at)
        );
    }

    #[test]
    fn rule_set_freshness_is_neutral_when_auto_update_is_off() {
        let mut model = model_with_region_rule_sets();
        let checked_at = local_time(1, 8);
        model.mark_selectable_rule_sets_checked(checked_at);

        assert_eq!(
            rule_set_freshness(&model, local_time(14, 12)),
            RuleSetFreshness::Manual(checked_at)
        );
    }

    #[test]
    fn rule_set_badge_has_omarchy_icon_padding_and_state_style() {
        let mut model = model_with_region_rule_sets();
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Every1d;
        model.mark_selectable_rule_sets_checked(local_time(14, 10));

        let (label, style) = rule_set_badge(&model, local_time(14, 12)).unwrap();
        assert_eq!(label, "  (1d) 14 Aug 10:20 ");
        assert_eq!(style, model.theme.ruleset_fresh_badge());

        mark_ru_rule_sets_updating(&mut model);
        let (label, style) = rule_set_badge(&model, local_time(14, 12)).unwrap();
        assert_eq!(label, "  (1d) updating… ");
        assert_eq!(style, model.theme.ruleset_updating_badge());

        model.rule_set_updates.clear();
        model.rule_set_states.clear();
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Every3d;
        let (label, style) = rule_set_badge(&model, local_time(14, 12)).unwrap();
        assert_eq!(label, "  (3d) never ");
        assert_eq!(style, model.theme.ruleset_stale_badge());

        model.mark_selectable_rule_sets_checked(local_time(14, 10));
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Off;
        let (label, style) = rule_set_badge(&model, local_time(14, 12)).unwrap();
        assert_eq!(label, "  (off) 14 Aug 10:20 ");
        assert_eq!(style, model.theme.ruleset_manual_badge());
    }

    #[test]
    fn status_bar_shows_disconnected() {
        let model = model_with_profiles(vec![]);
        insta::assert_snapshot!(status_bar_styles(&model));
    }

    #[test]
    fn status_bar_connected_snapshot() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Alpha".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(model.config.profiles[0].id);
        let mut buf = Buffer::empty(Rect::new(0, 0, APP_WINDOW_COLS, 1));
        StatusBar::new(&model).render(Rect::new(0, 0, APP_WINDOW_COLS, 1), &mut buf);
        insta::assert_snapshot!(buffer_to_string(&buf));
    }

    #[test]
    fn status_bar_marks_the_global_fallback_of_a_regional_mode() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Alpha".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model
            .config
            .settings
            .geo_routing
            .set_mode(crate::config::profile::RoutingMode::Bypass(GeoRegion::Ru));
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(model.config.profiles[0].id);
        model.routing_fallback = Some(crate::config::profile::RoutingFallback::Global);
        insta::assert_snapshot!(status_bar_styles(&model));
    }

    #[test]
    fn status_bar_connected_auto_snapshot() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Alpha".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(model.config.profiles[0].id);
        model.config.settings.auto_connect = true;
        let mut buf = Buffer::empty(Rect::new(0, 0, APP_WINDOW_COLS, 1));
        StatusBar::new(&model).render(Rect::new(0, 0, APP_WINDOW_COLS, 1), &mut buf);
        insta::assert_snapshot!(buffer_to_string(&buf));
    }

    #[test]
    fn status_bar_does_not_include_transient_geo_state() {
        let mut model = model_with_profiles(vec![]);
        mark_ru_rule_sets_updating(&mut model);
        let mut buf = Buffer::empty(Rect::new(0, 0, APP_WINDOW_COLS, 1));
        StatusBar::new(&model).render(Rect::new(0, 0, APP_WINDOW_COLS, 1), &mut buf);
        assert!(!buffer_to_string(&buf).contains("Geo"));
    }

    #[test]
    fn status_bar_does_not_include_geo_timestamp() {
        let model = model_with_profiles(vec![]);
        let mut buf = Buffer::empty(Rect::new(0, 0, APP_WINDOW_COLS, 1));
        StatusBar::new(&model).render(Rect::new(0, 0, APP_WINDOW_COLS, 1), &mut buf);
        let content: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(content.contains("DISCONNECTED"));
        assert!(content.contains("Global"));
        assert!(!content.contains("Geo"));
    }

    #[test]
    fn format_bps_boundaries() {
        assert_eq!(format_bps(0), "0.0 KB/s");
        assert_eq!(format_bps(850), "0.8 KB/s");
        assert_eq!(format_bps(999), "1.0 KB/s");
        assert_eq!(format_bps(1000), "1.0 KB/s");
        assert_eq!(format_bps(1023), "1.0 KB/s");
        assert_eq!(format_bps(1024), "1.0 KB/s");
        assert_eq!(format_bps(1500), "1.5 KB/s");
        assert_eq!(format_bps(1_500_000), "1.4 MB/s");
        assert_eq!(format_bps(10 * 1024), "10 KB/s");
        assert_eq!(format_bps(100 * 1024), "100 KB/s");
        assert_eq!(format_bps(999 * 1024), "999 KB/s");
        assert_eq!(format_bps(1000 * 1024), "1.0 MB/s");
        assert_eq!(format_bps(2 * 1024 * 1024 * 1024), "2.0 GB/s");
        assert_eq!(format_bps(1000 * 1024 * 1024 * 1024), "1.0 TB/s");
    }

    #[test]
    fn format_bytes_boundaries() {
        assert_eq!(format_bytes(0), "0.0 KB");
        assert_eq!(format_bytes(850), "0.8 KB");
        assert_eq!(format_bytes(999), "1.0 KB");
        assert_eq!(format_bytes(1000), "1.0 KB");
        assert_eq!(format_bytes(1023), "1.0 KB");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1_500_000), "1.4 MB");
        assert_eq!(format_bytes(10 * 1024), "10 KB");
        assert_eq!(format_bytes(100 * 1024), "100 KB");
        assert_eq!(format_bytes(999 * 1024), "999 KB");
        assert_eq!(format_bytes(1000 * 1024), "1.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert_eq!(format_bytes(1000 * 1024 * 1024 * 1024), "1.0 TB");
    }

    #[test]
    fn traffic_fields_have_fixed_widths() {
        for bps in [0, 1023, 1024, 1_500_000, 2 * 1024 * 1024 * 1024] {
            assert_eq!(format_bps_field(bps).len(), TRAFFIC_RATE_WIDTH);
        }
        for bytes in [0, 1023, 1024, 1_500_000, 3 * 1024 * 1024 * 1024] {
            assert_eq!(format_bytes_field(bytes).len(), TRAFFIC_TOTAL_WIDTH);
        }
        assert_eq!(format_bps_field(0), "0.0 KB/s");
        assert_eq!(format_bps_field(12 * 1024), " 12 KB/s");
        assert_eq!(format_bytes_field(0), "0.0 KB");
    }

    #[test]
    fn traffic_fields_handle_overflow_without_growing() {
        assert_eq!(format_bps_field(u64::MAX), " 16 EB/s");
        assert_eq!(format_bytes_field(u64::MAX), " 16 EB");
        assert_eq!(format_connections_field(18), "18");
        assert_eq!(format_connections_field(99_999), "99999");
        assert_eq!(format_connections_field(100_000), "99999+");
        assert_eq!(
            format_connections_field(18).len() + format_connections_padding(18).len(),
            TRAFFIC_CONNECTIONS_COUNT_WIDTH
        );
    }

    #[test]
    fn status_bar_does_not_include_status_message() {
        let mut model = model_with_profiles(vec![]);
        model.status = Some(crate::app::model::AppStatus::Error(
            "Connection failed: sing-box exited immediately (code: Some(1)). stderr: FATAL[0000] create service: parse outbound[0].server_settings.address: lookup example.com: no such host".to_string(),
        ));
        let mut buf = Buffer::empty(Rect::new(0, 0, APP_WINDOW_COLS, 1));
        StatusBar::new(&model).render(Rect::new(0, 0, APP_WINDOW_COLS, 1), &mut buf);
        let content = buffer_to_string(&buf);
        assert!(!content.contains("Connection failed"));
        assert!(content.contains("DISCONNECTED"));
    }

    #[test]
    fn status_bar_connecting_snapshot() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Alpha".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.connection = ConnectionState::Connecting;
        let mut buf = Buffer::empty(Rect::new(0, 0, APP_WINDOW_COLS, 1));
        StatusBar::new(&model).render(Rect::new(0, 0, APP_WINDOW_COLS, 1), &mut buf);
        insta::assert_snapshot!(buffer_to_string(&buf));
    }

    #[test]
    fn status_bar_connect_pending_snapshot() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Alpha".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.connection = ConnectionState::ConnectPending;
        insta::assert_snapshot!(status_bar_styles(&model));
    }

    #[test]
    fn narrow_status_bar_keeps_kill_switch_before_lower_priority_segments() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.kill_switch = true;
        model.config.settings.auto_connect = true;
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.mark_selectable_rule_sets_checked(chrono::Local::now());
        let area = Rect::new(0, 0, 18, 1);
        let mut buf = Buffer::empty(area);
        StatusBar::new(&model).render(area, &mut buf);
        let content = buffer_to_string(&buf);
        assert!(content.contains("DISCONNECTED"));
        assert!(content.contains("KS"));
        assert!(!content.contains("DNS"));
        assert!(!content.contains("Global"));
        assert!(!content.contains("Auto"));
        assert!(!content.contains(''));
    }

    #[test]
    fn status_bar_uses_full_panel_width_and_uniform_setting_styles() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.kill_switch = true;
        model.config.settings.auto_connect = true;
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.mark_selectable_rule_sets_checked(
            chrono::Local
                .with_ymd_and_hms(2026, 5, 31, 13, 41, 0)
                .single()
                .unwrap(),
        );
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Every1d;
        insta::assert_snapshot!(status_bar_styles(&model));
    }

    fn render_toasts(statuses: &[&AppStatus], container: Rect) -> String {
        let theme = crate::ui::styles::Theme::legacy();
        let mut buf = Buffer::empty(container);
        ToastStack::new(statuses, &theme).render(container, &mut buf);
        buffer_to_string(&buf)
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn info_toast_snapshot() {
        let status = AppStatus::Info("Connected to Alpha".into());
        insta::assert_snapshot!(render_toasts(&[&status], Rect::new(0, 0, 60, 10)));
    }

    #[test]
    fn error_toast_wraps_to_two_lines_snapshot() {
        let status = AppStatus::Error(
            "Connection failed: handshake timed out while contacting the selected server".into(),
        );
        insta::assert_snapshot!(render_toasts(&[&status], Rect::new(0, 0, 42, 10)));
    }

    #[test]
    fn toast_stack_snapshot() {
        let connected = AppStatus::Info("Connected to Alpha".into());
        let geo = AppStatus::Error(
            "Geo update failed: rule-set download timed out after 30 seconds".into(),
        );
        let dns = AppStatus::Info("DNS settings changed".into());
        let hidden = AppStatus::Info("Does not fit".into());
        insta::assert_snapshot!(render_toasts(
            &[&connected, &geo, &dns, &hidden],
            Rect::new(0, 0, 60, 12)
        ));
    }

    #[test]
    fn toast_skips_terminals_too_small_for_a_popup() {
        let status = AppStatus::Info("Saved".into());
        let theme = crate::ui::styles::Theme::legacy();
        assert!(
            ToastStack::new(&[&status], &theme)
                .areas(Rect::new(0, 0, 10, 4))
                .is_empty()
        );
    }
}

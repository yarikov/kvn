use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{DnsConfig, DnsStrategy, GeoRouting};

/// Per-file on-disk log line limits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LineRetention {
    #[serde(default = "default_app_line_retention")]
    pub app: u32,
    #[serde(default = "default_singbox_line_retention")]
    pub singbox: u32,
}

impl Default for LineRetention {
    fn default() -> Self {
        Self {
            app: default_app_line_retention(),
            singbox: default_singbox_line_retention(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LogsConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub line_retention: LineRetention,
}

impl Default for LogsConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            line_retention: LineRetention::default(),
        }
    }
}

/// Endpoint used only by manual profile latency tests (`t` / `T`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConnectivityProbeConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub url: Option<String>,
}

impl Default for ConnectivityProbeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            url: Some("https://connectivitycheck.gstatic.com/generate_204".to_string()),
        }
    }
}

/// Application settings stored alongside profiles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<Uuid>,
    #[serde(default = "default_tun_interface")]
    pub tun_interface: String,
    /// Legacy field, superseded by `dns.strategy`. Kept for one release so
    /// existing config files still load; on save we re-emit it from `dns.strategy`
    /// to avoid splitting the source of truth.
    #[serde(default = "default_dns_strategy")]
    pub dns_strategy: DnsStrategy,
    #[serde(default)]
    pub dns: DnsConfig,
    #[serde(default)]
    pub geo_routing: GeoRouting,
    #[serde(default)]
    pub auto_connect: bool,
    #[serde(default)]
    pub kill_switch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_connected_profile: Option<Uuid>,
    /// Active UI theme slug. The literal `"omarchy"` is a sentinel that
    /// means "follow Omarchy's active XDG state theme.name"; any other value
    /// names a bundled palette (see `src/ui/palette.rs`).
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub logs: LogsConfig,
    /// Pre-v4 compatibility field promoted into `logs.level` by the explicit
    /// profile-schema migration.
    #[serde(default, rename = "log_level", skip_serializing)]
    pub(crate) legacy_log_level: Option<String>,
    /// Stable installation identifier (`lnx-` + UUID v4), generated once on
    /// first launch and persisted via the atomic config write path. Sent as
    /// `X-Hwid` only by subscriptions with `send_hwid: true` — never derived
    /// from the subscription URL so rotating a token or changing a domain
    /// does not make the provider see a new device.
    #[serde(default)]
    pub hwid: String,
    #[serde(default)]
    pub allow_insecure_http_subscriptions: bool,
    #[serde(default)]
    pub connectivity_probe: ConnectivityProbeConfig,
    #[serde(default)]
    pub icons: IconSet,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconSet {
    #[default]
    Nerd,
    Unicode,
}

impl IconSet {
    pub fn next(self) -> Self {
        match self {
            Self::Nerd => Self::Unicode,
            Self::Unicode => Self::Nerd,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Nerd => "nerd",
            Self::Unicode => "unicode",
        }
    }
}

pub(super) fn default_tun_interface() -> String {
    "kvn0".to_string()
}

fn default_dns_strategy() -> DnsStrategy {
    DnsStrategy::PreferIpv4
}

/// Default theme slug for fresh installs. Works on every distro because
/// `tokyo-night` is one of the bundled palettes; on Omarchy users can
/// switch to `"omarchy"` via the in-TUI picker to auto-follow the system.
pub fn default_theme() -> String {
    "tokyo-night".to_string()
}

/// Parse and validate the endpoint used by manual latency tests.
pub fn parse_connectivity_probe_url(input: &str) -> anyhow::Result<url::Url> {
    const MAX_PROBE_URL_LEN: usize = 2_048;

    anyhow::ensure!(
        input.len() <= MAX_PROBE_URL_LEN,
        "settings.connectivity_probe.url exceeds {MAX_PROBE_URL_LEN} bytes"
    );
    let url = url::Url::parse(input)
        .map_err(|_| anyhow::anyhow!("settings.connectivity_probe.url is not a valid URL"))?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https"),
        "settings.connectivity_probe.url must use HTTP or HTTPS"
    );
    anyhow::ensure!(
        url.host_str().is_some(),
        "settings.connectivity_probe.url must include a host"
    );
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "settings.connectivity_probe.url must not contain credentials"
    );
    anyhow::ensure!(
        url.fragment().is_none(),
        "settings.connectivity_probe.url must not contain a fragment"
    );
    Ok(url)
}

pub fn default_log_level() -> String {
    "info".to_string()
}

pub fn default_app_line_retention() -> u32 {
    1_000
}

pub fn default_singbox_line_retention() -> u32 {
    100_000
}

/// Canonical log-level values accepted by both `tracing_subscriber::EnvFilter`
/// and sing-box's `log.level`. Also used as the allow-list by
/// [`Settings::validate`].
pub const LOG_LEVELS: &[&str] = &["trace", "debug", "info", "warn", "error"];

const MIN_LOG_LINES: u32 = 1_000;

/// Linux IFNAMSIZ − 1 (the kernel reserves one byte for the terminator).
const MAX_TUN_INTERFACE_LEN: usize = 15;

/// Sentinel value in `settings.theme` that means "follow the active Omarchy
/// theme". Any other value must match one of the bundled palette slugs.
pub const OMARCHY_THEME_SENTINEL: &str = "omarchy";

// Names-only table generated by `build.rs` from `themes/*.toml`. Kept as a
// separate include (rather than referencing `ui::palette::BUNDLED`) because
// `ui` depends on `config`, so a dep in the other direction would be circular.
include!(concat!(env!("OUT_DIR"), "/bundled_theme_names.rs"));

/// Map a `settings.logs.level` value to one of the five canonical levels
/// (`trace`/`debug`/`info`/`warn`/`error`). Anything else returns `"info"`.
/// Used both by the tracing filter in `main.rs` and by the sing-box config
/// generator, so the level the user sets in the JSON applies to both.
///
/// This fallback remains a runtime safety net for env-injected values; values
/// coming from `profiles.json` are additionally rejected by
/// [`Settings::validate`] before they reach this function.
pub fn normalized_log_level(level: &str) -> &'static str {
    match level {
        "trace" => "trace",
        "debug" => "debug",
        "info" => "info",
        "warn" => "warn",
        "error" => "error",
        _ => "info",
    }
}

fn is_safe_slug_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

impl Settings {
    /// Reject settings values that would either fail immediately at sing-box
    /// startup or fall back silently at runtime. Called from
    /// [`Config::validate`](crate::config::profile::Config::validate).
    pub fn validate(&self) -> anyhow::Result<()> {
        let tun = self.tun_interface.trim();
        if tun.is_empty() {
            anyhow::bail!("settings.tun_interface must not be empty");
        }
        if tun.len() > MAX_TUN_INTERFACE_LEN {
            anyhow::bail!(
                "settings.tun_interface {:?} exceeds Linux IFNAMSIZ limit ({} chars)",
                self.tun_interface,
                MAX_TUN_INTERFACE_LEN,
            );
        }
        if !tun.chars().all(is_safe_slug_char) {
            anyhow::bail!(
                "settings.tun_interface {:?} contains disallowed characters (allowed: a-z, A-Z, 0-9, `-`, `_`)",
                self.tun_interface,
            );
        }
        if !tun.starts_with("kvn") {
            anyhow::bail!("settings.tun_interface must start with \"kvn\"");
        }

        if self.theme != OMARCHY_THEME_SENTINEL
            && !BUNDLED_THEME_NAMES.contains(&self.theme.as_str())
        {
            anyhow::bail!(
                "settings.theme {:?} is not a bundled palette slug (expected {:?} or one of {} bundled themes)",
                self.theme,
                OMARCHY_THEME_SENTINEL,
                BUNDLED_THEME_NAMES.len(),
            );
        }

        if !LOG_LEVELS.contains(&self.logs.level.as_str()) {
            anyhow::bail!(
                "settings.logs.level {:?} is not one of {:?}",
                self.logs.level,
                LOG_LEVELS,
            );
        }

        if self.logs.line_retention.app < MIN_LOG_LINES {
            anyhow::bail!("settings.logs.line_retention.app must be at least {MIN_LOG_LINES}");
        }
        if self.logs.line_retention.singbox < MIN_LOG_LINES {
            anyhow::bail!("settings.logs.line_retention.singbox must be at least {MIN_LOG_LINES}");
        }

        if self.connectivity_probe.enabled {
            let url = self.connectivity_probe.url.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "settings.connectivity_probe.url is required when connectivity probing is enabled"
                )
            })?;
            parse_connectivity_probe_url(url)?;
        }

        Ok(())
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_profile: None,
            tun_interface: default_tun_interface(),
            dns_strategy: default_dns_strategy(),
            dns: DnsConfig::default(),
            geo_routing: GeoRouting::default(),
            auto_connect: false,
            kill_switch: false,
            last_connected_profile: None,
            theme: default_theme(),
            logs: LogsConfig::default(),
            legacy_log_level: None,
            hwid: String::new(),
            allow_insecure_http_subscriptions: false,
            connectivity_probe: ConnectivityProbeConfig::default(),
            icons: IconSet::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::*;

    #[test]
    fn settings_default() {
        let s = Settings::default();
        assert_eq!(s.tun_interface, "kvn0");
        assert_eq!(s.dns_strategy, DnsStrategy::PreferIpv4);
        assert!(s.default_profile.is_none());
        assert!(!s.auto_connect);
        assert!(!s.kill_switch);
        assert!(s.last_connected_profile.is_none());
        assert!(s.geo_routing.current_region.is_none());
        assert!(s.geo_routing.selected_region_modes.is_empty());
        assert_eq!(s.geo_routing.auto_update, GeoAutoUpdate::Off);
        assert_eq!(s.geo_routing.mode(), RoutingMode::Global);
        assert_eq!(s.logs.level, "info");
        assert_eq!(s.logs.line_retention.app, 1_000);
        assert_eq!(s.logs.line_retention.singbox, 100_000);
    }

    #[test]
    fn settings_log_level_round_trips_through_json() {
        let mut s = Settings::default();
        s.logs.level = "debug".to_string();
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"logs\":{\"level\":\"debug\""));
        assert!(!json.contains("\"log_level\""));
        let restored: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.logs.level, "debug");
        assert_eq!(restored.logs.line_retention.app, 1_000);
        assert_eq!(restored.logs.line_retention.singbox, 100_000);
    }

    #[test]
    fn normalized_log_level_passes_canonical_levels_through() {
        for level in ["trace", "debug", "info", "warn", "error"] {
            assert_eq!(normalized_log_level(level), level);
        }
    }

    #[test]
    fn normalized_log_level_falls_back_to_info_on_garbage() {
        for bad in ["verbose", "", "INFO", "fatal", "panic", "kvn_tui=debug"] {
            assert_eq!(normalized_log_level(bad), "info");
        }
    }

    #[test]
    fn settings_log_level_defaults_when_absent() {
        let json = r#"{
            "tun_interface": "kvn0",
            "dns_strategy": "prefer_ipv4",
            "geo_routing": {},
            "auto_connect": false
        }"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.logs.level, "info");
        assert_eq!(s.logs.line_retention.app, 1_000);
        assert_eq!(s.logs.line_retention.singbox, 100_000);
        assert_eq!(s.geo_routing.auto_update, GeoAutoUpdate::Off);
        assert_eq!(s.icons, IconSet::Nerd);
    }

    #[test]
    fn icon_set_serde_values_round_trip() {
        for (icons, wire) in [
            (IconSet::Nerd, "\"nerd\""),
            (IconSet::Unicode, "\"unicode\""),
        ] {
            assert_eq!(serde_json::to_string(&icons).unwrap(), wire);
            assert_eq!(serde_json::from_str::<IconSet>(wire).unwrap(), icons);
        }
        assert!(serde_json::from_str::<IconSet>("\"emoji\"").is_err());
        assert_eq!(IconSet::Nerd.next(), IconSet::Unicode);
        assert_eq!(IconSet::Unicode.next(), IconSet::Nerd);
        assert_eq!(IconSet::Nerd.label(), "nerd");
        assert_eq!(IconSet::Unicode.label(), "unicode");
    }

    #[test]
    fn settings_serde_roundtrip_with_kill_switch() {
        let s = Settings {
            kill_switch: true,
            ..Settings::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"kill_switch\":true"));
        let restored: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, restored);
        assert!(restored.kill_switch);
    }

    #[test]
    fn settings_serde_kill_switch_defaults_when_absent() {
        // Older configs without the field should deserialize with kill_switch=false.
        let json = r#"{
            "tun_interface": "kvn0",
            "dns_strategy": "prefer_ipv4",
            "geo_routing": {},
            "auto_connect": false
        }"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert!(!s.kill_switch);
    }

    // ---- Settings::validate ----

    #[test]
    fn settings_validate_default_ok() {
        Settings::default().validate().unwrap();
    }

    #[test]
    fn connectivity_probe_defaults_to_https_and_missing_field_is_backward_compatible() {
        let default = Settings::default();
        assert!(default.connectivity_probe.enabled);
        assert_eq!(
            default.connectivity_probe.url.as_deref(),
            Some("https://connectivitycheck.gstatic.com/generate_204")
        );

        let restored: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(restored.connectivity_probe, default.connectivity_probe);
    }

    #[test]
    fn new_settings_disable_http_subscriptions_by_default() {
        let default = Settings::default();
        let restored: Settings = serde_json::from_str("{}").unwrap();

        assert!(!default.allow_insecure_http_subscriptions);
        assert!(!restored.allow_insecure_http_subscriptions);
    }

    #[test]
    fn connectivity_probe_disabled_preserves_url_without_validating_it() {
        let settings: Settings =
            serde_json::from_str(r#"{"connectivity_probe":{"enabled":false,"url":"not a URL"}}"#)
                .unwrap();
        assert!(!settings.connectivity_probe.enabled);
        assert_eq!(
            settings.connectivity_probe.url.as_deref(),
            Some("not a URL")
        );
        settings.validate().unwrap();
        assert!(
            serde_json::to_string(&settings)
                .unwrap()
                .contains("\"connectivity_probe\":{\"enabled\":false,\"url\":\"not a URL\"}")
        );
    }

    #[test]
    fn connectivity_probe_enabled_requires_url() {
        let settings: Settings =
            serde_json::from_str(r#"{"connectivity_probe":{"enabled":true}}"#).unwrap();
        let error = settings.validate().unwrap_err().to_string();
        assert!(error.contains("url is required"), "got: {error}");
    }

    #[test]
    fn connectivity_probe_validation_accepts_http_and_https() {
        for endpoint in [
            "https://example.com/generate_204?source=kvn",
            "http://127.0.0.1:8080/health",
            "https://[2001:db8::1]/",
        ] {
            parse_connectivity_probe_url(endpoint).unwrap();
        }
    }

    #[test]
    fn connectivity_probe_validation_rejects_unsafe_or_unsupported_urls() {
        for endpoint in [
            "ftp://example.com/file",
            "https://user:password@example.com/",
            "https://example.com/#fragment",
            "not a URL",
        ] {
            assert!(
                parse_connectivity_probe_url(endpoint).is_err(),
                "accepted {endpoint}"
            );
        }
    }

    #[test]
    fn settings_validate_rejects_empty_tun_interface() {
        let s = Settings {
            tun_interface: "   ".into(),
            ..Settings::default()
        };
        let err = s.validate().unwrap_err().to_string();
        assert!(err.contains("tun_interface"), "Error was: {}", err);
    }

    #[test]
    fn settings_validate_rejects_overlong_tun_interface() {
        // IFNAMSIZ − 1 = 15; 16 chars must be rejected.
        let s = Settings {
            tun_interface: "a".repeat(16),
            ..Settings::default()
        };
        let err = s.validate().unwrap_err().to_string();
        assert!(err.contains("IFNAMSIZ"), "Error was: {}", err);
    }

    #[test]
    fn settings_validate_rejects_tun_interface_with_bad_chars() {
        let s = Settings {
            tun_interface: "tun 0".into(),
            ..Settings::default()
        };
        let err = s.validate().unwrap_err().to_string();
        assert!(err.contains("disallowed"), "Error was: {}", err);
    }

    #[test]
    fn settings_validate_accepts_tun_interfaces_with_kvn_prefix() {
        for tun_interface in ["kvn", "kvn0", "kvn-work"] {
            let s = Settings {
                tun_interface: tun_interface.into(),
                ..Settings::default()
            };
            s.validate().unwrap();
        }
    }

    #[test]
    fn settings_validate_rejects_tun_interface_without_kvn_prefix() {
        let s = Settings {
            tun_interface: "tun0".into(),
            ..Settings::default()
        };
        let err = s.validate().unwrap_err().to_string();
        assert!(err.contains("must start with \"kvn\""), "Error was: {err}");
    }

    #[test]
    fn settings_validate_accepts_omarchy_sentinel() {
        let s = Settings {
            theme: OMARCHY_THEME_SENTINEL.into(),
            ..Settings::default()
        };
        s.validate().unwrap();
    }

    #[test]
    fn settings_validate_accepts_bundled_theme() {
        // "tokyo-night" ships in themes/, so it must be in BUNDLED_THEME_NAMES.
        let s = Settings {
            theme: "tokyo-night".into(),
            ..Settings::default()
        };
        s.validate().unwrap();
    }

    #[test]
    fn settings_validate_rejects_unknown_theme() {
        let s = Settings {
            theme: "not-a-bundled-slug".into(),
            ..Settings::default()
        };
        let err = s.validate().unwrap_err().to_string();
        assert!(err.contains("theme"), "Error was: {}", err);
    }

    #[test]
    fn settings_validate_rejects_empty_theme() {
        let s = Settings {
            theme: String::new(),
            ..Settings::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn settings_validate_accepts_every_canonical_log_level() {
        for level in LOG_LEVELS {
            let mut s = Settings::default();
            s.logs.level = (*level).to_string();
            s.validate().unwrap();
        }
    }

    #[test]
    fn settings_validate_rejects_unknown_log_level() {
        let mut s = Settings::default();
        s.logs.level = "verbose".into();
        let err = s.validate().unwrap_err().to_string();
        assert!(err.contains("logs.level"), "Error was: {}", err);
    }

    #[test]
    fn settings_validate_rejects_log_line_limits_below_minimum() {
        for value in [0, 1, 999] {
            let mut app = Settings::default();
            app.logs.line_retention.app = value;
            assert!(app.validate().unwrap_err().to_string().contains(".app"));

            let mut singbox = Settings::default();
            singbox.logs.line_retention.singbox = value;
            assert!(
                singbox
                    .validate()
                    .unwrap_err()
                    .to_string()
                    .contains(".singbox")
            );
        }
    }

    #[test]
    fn settings_validate_accepts_minimum_log_line_limits() {
        let mut settings = Settings::default();
        settings.logs.line_retention.app = MIN_LOG_LINES;
        settings.logs.line_retention.singbox = MIN_LOG_LINES;
        settings.validate().unwrap();
    }

    #[test]
    fn settings_validate_rejects_uppercased_log_level() {
        // normalized_log_level lowercases at runtime, but on-disk config is
        // validated case-sensitively so the JSON does not silently drift.
        let mut s = Settings::default();
        s.logs.level = "INFO".into();
        assert!(s.validate().is_err());
    }
}

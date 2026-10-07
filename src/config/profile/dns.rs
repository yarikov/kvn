//! DNS configuration: strategy, servers, rules, and validation.
//!
//! `DnsConfig` is the source of truth used both by sing-box config generation
//! (`singbox::config`) and by the TUI's settings overlay.

use std::collections::HashSet;
use std::net::IpAddr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ConfigDiagnostic;

pub const FAKEIP_SERVER_TAG: &str = "fakeip";

// (body appended by sed)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema)]
pub enum DnsStrategy {
    #[default]
    #[serde(rename = "prefer_ipv4")]
    PreferIpv4,
    #[serde(rename = "prefer_ipv6")]
    PreferIpv6,
    #[serde(rename = "ipv4_only")]
    OnlyIpv4,
    #[serde(rename = "ipv6_only")]
    OnlyIpv6,
}

impl DnsStrategy {
    pub fn as_str(&self) -> &'static str {
        match self {
            DnsStrategy::PreferIpv4 => "prefer_ipv4",
            DnsStrategy::PreferIpv6 => "prefer_ipv6",
            DnsStrategy::OnlyIpv4 => "ipv4_only",
            DnsStrategy::OnlyIpv6 => "ipv6_only",
        }
    }

    pub fn toggled(&self) -> Self {
        match self.ipv4_counterpart() {
            DnsStrategy::PreferIpv4 => DnsStrategy::OnlyIpv4,
            _ => DnsStrategy::PreferIpv4,
        }
    }

    pub fn ipv4_counterpart(&self) -> Self {
        match self {
            DnsStrategy::PreferIpv4 | DnsStrategy::PreferIpv6 => DnsStrategy::PreferIpv4,
            DnsStrategy::OnlyIpv4 | DnsStrategy::OnlyIpv6 => DnsStrategy::OnlyIpv4,
        }
    }
}

/// Built-in DNS server presets exposed by the TUI settings overlay.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DnsPreset {
    CloudflareDoh,
    GoogleDot,
    Quad9Doh,
    SystemLocal,
}

impl DnsPreset {
    pub const ALL: [Self; 4] = [
        Self::CloudflareDoh,
        Self::GoogleDot,
        Self::Quad9Doh,
        Self::SystemLocal,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::CloudflareDoh => "cloudflare_doh",
            Self::GoogleDot => "google_dot",
            Self::Quad9Doh => "quad9_doh",
            Self::SystemLocal => "system_local",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|preset| preset.name() == name)
    }

    fn matches(self, servers: &[DnsServer], final_server: &str) -> bool {
        let (canonical_servers, canonical_final) = self.canonical();
        let configured: Vec<DnsServer> =
            servers.iter().map(DnsServer::with_explicit_port).collect();
        final_server == canonical_final
            && configured.len() == canonical_servers.len()
            && canonical_servers
                .iter()
                .all(|server| configured.contains(&server.with_explicit_port()))
    }

    fn canonical(self) -> (Vec<DnsServer>, &'static str) {
        let local = DnsServer::Local {
            tag: "local".to_string(),
        };
        match self {
            Self::CloudflareDoh => (
                vec![
                    local,
                    DnsServer::Https {
                        tag: "remote".to_string(),
                        server: "1.1.1.1".to_string(),
                        server_port: None,
                        path: "/dns-query".to_string(),
                    },
                ],
                "remote",
            ),
            Self::GoogleDot => (
                vec![
                    local,
                    DnsServer::Tls {
                        tag: "remote".to_string(),
                        server: "8.8.8.8".to_string(),
                        server_port: Some(853),
                    },
                ],
                "remote",
            ),
            Self::Quad9Doh => (
                vec![
                    local,
                    DnsServer::Https {
                        tag: "remote".to_string(),
                        server: "9.9.9.9".to_string(),
                        server_port: None,
                        path: "/dns-query".to_string(),
                    },
                ],
                "remote",
            ),
            Self::SystemLocal => (vec![local], "local"),
        }
    }
}

/// A single sing-box DNS server. Variants map 1:1 onto sing-box 1.14 server types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DnsServer {
    Local {
        tag: String,
    },
    Udp {
        tag: String,
        server: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server_port: Option<u16>,
    },
    Tcp {
        tag: String,
        server: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server_port: Option<u16>,
    },
    Tls {
        tag: String,
        server: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server_port: Option<u16>,
    },
    Https {
        tag: String,
        server: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server_port: Option<u16>,
        #[serde(default = "default_doh_path")]
        path: String,
    },
    Quic {
        tag: String,
        server: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server_port: Option<u16>,
    },
}

impl DnsServer {
    pub fn tag(&self) -> &str {
        match self {
            DnsServer::Local { tag }
            | DnsServer::Udp { tag, .. }
            | DnsServer::Tcp { tag, .. }
            | DnsServer::Tls { tag, .. }
            | DnsServer::Https { tag, .. }
            | DnsServer::Quic { tag, .. } => tag,
        }
    }

    fn tag_mut(&mut self) -> &mut String {
        match self {
            DnsServer::Local { tag }
            | DnsServer::Udp { tag, .. }
            | DnsServer::Tcp { tag, .. }
            | DnsServer::Tls { tag, .. }
            | DnsServer::Https { tag, .. }
            | DnsServer::Quic { tag, .. } => tag,
        }
    }

    pub fn address(&self) -> Option<&str> {
        match self {
            DnsServer::Udp { server, .. }
            | DnsServer::Tcp { server, .. }
            | DnsServer::Tls { server, .. }
            | DnsServer::Https { server, .. }
            | DnsServer::Quic { server, .. } => Some(server),
            DnsServer::Local { .. } => None,
        }
    }

    pub fn hostname(&self) -> Option<&str> {
        self.address()
            .filter(|address| address.parse::<IpAddr>().is_err())
    }

    fn with_explicit_port(&self) -> Self {
        let mut server = self.clone();
        let (port, default_port) = match &mut server {
            DnsServer::Udp { server_port, .. } | DnsServer::Tcp { server_port, .. } => {
                (server_port, 53)
            }
            DnsServer::Tls { server_port, .. } | DnsServer::Quic { server_port, .. } => {
                (server_port, 853)
            }
            DnsServer::Https { server_port, .. } => (server_port, 443),
            DnsServer::Local { .. } => return server,
        };
        port.get_or_insert(default_port);
        server
    }

    /// Short label used in the status bar, e.g. "DoH", "DoT".
    pub fn kind_label(&self) -> &'static str {
        match self {
            DnsServer::Local { .. } => "local",
            DnsServer::Udp { .. } => "UDP",
            DnsServer::Tcp { .. } => "TCP",
            DnsServer::Tls { .. } => "DoT",
            DnsServer::Https { .. } => "DoH",
            DnsServer::Quic { .. } => "DoQ",
        }
    }
}

/// A per-domain DNS routing rule. Maps onto sing-box `dns.rules[*]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DnsRule {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domain: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domain_suffix: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domain_keyword: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domain_regex: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_set: Vec<String>,
    /// Must match a server tag of the same preset.
    pub server: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disable_cache: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomDnsPreset {
    #[schemars(length(min = 1))]
    pub name: String,
    pub servers: Vec<DnsServer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<DnsRule>,
    pub final_server: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FakeIpRanges {
    #[serde(default = "default_fakeip_v4")]
    pub inet4_range: String,
    #[serde(default = "default_fakeip_v6")]
    pub inet6_range: String,
}

impl Default for FakeIpRanges {
    fn default() -> Self {
        Self {
            inet4_range: default_fakeip_v4(),
            inet6_range: default_fakeip_v6(),
        }
    }
}

impl FakeIpRanges {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// User-controlled DNS configuration. Replaces the hard-coded DNS section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DnsConfig {
    #[serde(default = "default_current_preset")]
    pub current_preset: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_presets: Vec<CustomDnsPreset>,
    #[serde(default)]
    #[schemars(extend("enum" = ["prefer_ipv4", "ipv4_only"]))]
    pub strategy: DnsStrategy,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fakeip_enabled: bool,
    #[serde(default, skip_serializing_if = "FakeIpRanges::is_default")]
    pub fakeip_ranges: FakeIpRanges,
    #[serde(default, rename = "servers", skip_serializing)]
    #[schemars(skip)]
    pub(super) legacy_servers: Option<Vec<LegacyDnsServer>>,
    #[serde(default, rename = "rules", skip_serializing)]
    #[schemars(skip)]
    pub(super) legacy_rules: Option<Vec<DnsRule>>,
    #[serde(default, rename = "final_server", skip_serializing)]
    #[schemars(skip)]
    pub(super) legacy_final_server: Option<String>,
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            current_preset: default_current_preset(),
            custom_presets: Vec::new(),
            strategy: DnsStrategy::default(),
            fakeip_enabled: false,
            fakeip_ranges: FakeIpRanges::default(),
            legacy_servers: None,
            legacy_rules: None,
            legacy_final_server: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub(super) enum LegacyDnsServer {
    Server(DnsServer),
    FakeIp(LegacyFakeIpServer),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum LegacyFakeIpServer {
    FakeIp {
        tag: String,
        #[serde(default = "default_fakeip_v4")]
        inet4_range: String,
        #[serde(default = "default_fakeip_v6")]
        inet6_range: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveDns {
    pub servers: Vec<DnsServer>,
    pub rules: Vec<DnsRule>,
    pub final_server: String,
    pub fakeip: Option<FakeIpServer>,
    pub fakeip_catch_all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeIpServer {
    pub tag: String,
    pub ranges: FakeIpRanges,
}

impl ActiveDns {
    pub fn local_server_tag(&self) -> Option<&str> {
        local_server_tag(&self.servers)
    }

    pub fn final_server_entry(&self) -> Option<&DnsServer> {
        self.servers
            .iter()
            .find(|server| server.tag() == self.final_server)
    }

    pub fn unused_tag(&self, base: &str) -> String {
        unused_name(base, |tag| {
            self.servers.iter().any(|server| server.tag() == tag)
                || self.fakeip.as_ref().is_some_and(|fakeip| fakeip.tag == tag)
        })
    }
}

fn default_doh_path() -> String {
    "/dns-query".to_string()
}

fn default_fakeip_v4() -> String {
    "198.18.0.0/15".to_string()
}

fn default_fakeip_v6() -> String {
    "fc00::/18".to_string()
}

fn default_current_preset() -> String {
    DnsPreset::CloudflareDoh.name().to_string()
}

impl DnsConfig {
    pub fn active(&self) -> Option<ActiveDns> {
        let (servers, rules, final_server) = match DnsPreset::from_name(&self.current_preset) {
            Some(preset) => {
                let (servers, final_server) = preset.canonical();
                (servers, Vec::new(), final_server.to_string())
            }
            None => {
                let preset = self.custom_preset(&self.current_preset)?;
                (
                    preset.servers.clone(),
                    preset.rules.clone(),
                    preset.final_server.clone(),
                )
            }
        };
        let rules_use_fakeip = rules.iter().any(|rule| rule.server == FAKEIP_SERVER_TAG);
        let fakeip = (self.fakeip_enabled || rules_use_fakeip).then(|| FakeIpServer {
            tag: FAKEIP_SERVER_TAG.to_string(),
            ranges: self.fakeip_ranges.clone(),
        });
        Some(ActiveDns {
            servers,
            rules,
            final_server,
            fakeip,
            fakeip_catch_all: self.fakeip_enabled,
        })
    }

    pub fn preset_names(&self) -> Vec<&str> {
        let custom = self
            .custom_presets
            .iter()
            .map(|preset| preset.name.as_str());
        DnsPreset::ALL
            .into_iter()
            .map(|preset| -> &str { preset.name() })
            .chain(custom)
            .collect()
    }

    pub fn custom_preset(&self, name: &str) -> Option<&CustomDnsPreset> {
        self.custom_presets
            .iter()
            .find(|preset| preset.name == name)
    }

    pub fn has_preset(&self, name: &str) -> bool {
        DnsPreset::from_name(name).is_some() || self.custom_preset(name).is_some()
    }

    pub fn diagnostics(&self) -> Vec<ConfigDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.strategy != self.strategy.ipv4_counterpart() {
            diagnostics.push(ConfigDiagnostic::new(
                "/strategy",
                format!(
                    "dns.strategy {:?} is not supported: the tunnel carries IPv4 only; use \"prefer_ipv4\" or \"ipv4_only\"",
                    self.strategy.as_str()
                ),
            ));
        }
        for (field, present) in [
            ("servers", self.legacy_servers.is_some()),
            ("rules", self.legacy_rules.is_some()),
            ("final_server", self.legacy_final_server.is_some()),
        ] {
            if present {
                diagnostics.push(ConfigDiagnostic::new(
                    format!("/{field}"),
                    format!(
                        "dns.{field} is no longer read; move it into a preset under dns.custom_presets and select it with dns.current_preset"
                    ),
                ));
            }
        }
        if !self.has_preset(&self.current_preset) {
            diagnostics.push(ConfigDiagnostic::new(
                "/current_preset",
                format!(
                    "dns.current_preset {:?} does not name a built-in or custom preset",
                    self.current_preset
                ),
            ));
        }
        let mut names = HashSet::new();
        for (idx, preset) in self.custom_presets.iter().enumerate() {
            let pointer = format!("/custom_presets/{idx}");
            let name = preset.name.as_str();
            if name.trim().is_empty() {
                diagnostics.push(ConfigDiagnostic::new(
                    format!("{pointer}/name"),
                    "dns.custom_presets: preset name must not be empty",
                ));
            } else if DnsPreset::from_name(name).is_some() {
                diagnostics.push(ConfigDiagnostic::new(
                    format!("{pointer}/name"),
                    format!("dns.custom_presets: {name:?} is the name of a built-in preset"),
                ));
            } else if !names.insert(name) {
                diagnostics.push(ConfigDiagnostic::new(
                    format!("{pointer}/name"),
                    format!("dns.custom_presets: duplicate preset name {name:?}"),
                ));
            }
            let label = format!("dns.custom_presets[{idx}]");
            diagnostics.extend(
                server_set_diagnostics(&preset.servers, &preset.rules, &preset.final_server)
                    .into_iter()
                    .map(|diagnostic| diagnostic.within(&pointer).labelled(&label)),
            );
        }
        for (field, range) in [
            ("inet4_range", &self.fakeip_ranges.inet4_range),
            ("inet6_range", &self.fakeip_ranges.inet6_range),
        ] {
            if !is_ip_prefix(range) {
                diagnostics.push(ConfigDiagnostic::new(
                    format!("/fakeip_ranges/{field}"),
                    format!("dns.fakeip_ranges.{field} {range:?} is not an IP prefix"),
                ));
            }
        }
        diagnostics
    }

    pub(super) fn migrate_legacy_servers(&mut self) {
        let legacy_servers = self.legacy_servers.take();
        let legacy_rules = self.legacy_rules.take();
        let final_server = self.legacy_final_server.take();
        if legacy_servers.is_none() && legacy_rules.is_none() && final_server.is_none() {
            return;
        }
        let mut rules = legacy_rules.unwrap_or_default();
        let mut servers = Vec::new();
        let mut fakeip_ranges = None;
        let mut fakeip_tags = Vec::new();
        let legacy_servers = legacy_servers.unwrap_or_else(|| {
            DnsPreset::CloudflareDoh
                .canonical()
                .0
                .into_iter()
                .map(LegacyDnsServer::Server)
                .collect()
        });
        for server in legacy_servers {
            match server {
                LegacyDnsServer::Server(server) => servers.push(server),
                LegacyDnsServer::FakeIp(LegacyFakeIpServer::FakeIp {
                    tag,
                    inet4_range,
                    inet6_range,
                }) => {
                    fakeip_tags.push(tag);
                    fakeip_ranges.get_or_insert(FakeIpRanges {
                        inet4_range,
                        inet6_range,
                    });
                }
            }
        }
        if let Some(ranges) = fakeip_ranges {
            self.fakeip_ranges = ranges;
        }
        let mut final_server = final_server.unwrap_or_else(|| "remote".to_string());
        let taken_tags: Vec<String> = servers
            .iter()
            .map(|server| server.tag().to_string())
            .chain(fakeip_tags.iter().cloned())
            .collect();
        if let Some(server) = servers
            .iter_mut()
            .find(|server| server.tag() == FAKEIP_SERVER_TAG)
        {
            let tag = unused_name(FAKEIP_SERVER_TAG, |tag| {
                taken_tags.iter().any(|taken| taken == tag)
            });
            *server.tag_mut() = tag.clone();
            for reference in rules
                .iter_mut()
                .map(|rule| &mut rule.server)
                .chain([&mut final_server])
                .filter(|reference| *reference == FAKEIP_SERVER_TAG)
            {
                *reference = tag.clone();
            }
        }
        for rule in &mut rules {
            if fakeip_tags.contains(&rule.server) {
                rule.server = FAKEIP_SERVER_TAG.to_string();
            }
        }
        let built_in = DnsPreset::ALL
            .into_iter()
            .find(|preset| rules.is_empty() && preset.matches(&servers, &final_server));
        self.current_preset = match built_in {
            Some(preset) => preset.name().to_string(),
            None => {
                let name = unused_name("custom", |name| self.custom_preset(name).is_some());
                self.custom_presets.push(CustomDnsPreset {
                    name: name.clone(),
                    servers,
                    rules,
                    final_server,
                });
                name
            }
        };
    }
}

fn server_set_diagnostics(
    servers: &[DnsServer],
    rules: &[DnsRule],
    final_server: &str,
) -> Vec<ConfigDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut tags = HashSet::new();
    for (idx, server) in servers.iter().enumerate() {
        let tag = server.tag();
        let pointer = format!("/servers/{idx}/tag");
        if tag.trim().is_empty() {
            diagnostics.push(ConfigDiagnostic::new(
                pointer,
                "servers: server tag must not be empty",
            ));
        } else if tag == FAKEIP_SERVER_TAG {
            diagnostics.push(ConfigDiagnostic::new(
                pointer,
                format!("servers: tag {tag:?} is reserved for the fake-IP server"),
            ));
        } else if !tags.insert(tag) {
            diagnostics.push(ConfigDiagnostic::new(
                pointer,
                format!("servers: duplicate server tag {tag:?}"),
            ));
        }
    }
    if local_server_tag(servers).is_none() {
        for (idx, server) in servers.iter().enumerate() {
            if let Some(hostname) = server.hostname() {
                diagnostics.push(ConfigDiagnostic::new(
                    format!("/servers/{idx}/server"),
                    format!(
                        "servers[{idx}]: hostname {hostname:?} needs a \"local\" server to resolve it"
                    ),
                ));
            }
        }
    }
    if !tags.contains(final_server) {
        diagnostics.push(ConfigDiagnostic::new(
            "/final_server",
            format!("final_server {final_server:?} does not match any server tag"),
        ));
    }
    for (idx, rule) in rules.iter().enumerate() {
        if rule.server != FAKEIP_SERVER_TAG && !tags.contains(rule.server.as_str()) {
            diagnostics.push(ConfigDiagnostic::new(
                format!("/rules/{idx}/server"),
                format!(
                    "rules[{idx}].server {:?} does not match any server tag",
                    rule.server
                ),
            ));
        }
    }
    diagnostics
}

fn local_server_tag(servers: &[DnsServer]) -> Option<&str> {
    servers
        .iter()
        .find(|server| matches!(server, DnsServer::Local { .. }))
        .map(DnsServer::tag)
}

fn unused_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut name = base.to_string();
    let mut suffix = 1;
    while taken(&name) {
        suffix += 1;
        name = format!("{base}-{suffix}");
    }
    name
}

fn is_ip_prefix(value: &str) -> bool {
    let Some((address, prefix_len)) = value.split_once('/') else {
        return false;
    };
    let (Ok(address), Ok(prefix_len)) = (address.parse::<IpAddr>(), prefix_len.parse::<u8>())
    else {
        return false;
    };
    let max_prefix_len = match address {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    prefix_len <= max_prefix_len
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> DnsServer {
        DnsServer::Local {
            tag: "local".into(),
        }
    }

    fn doh(tag: &str, server: &str) -> DnsServer {
        DnsServer::Https {
            tag: tag.into(),
            server: server.into(),
            server_port: None,
            path: "/dns-query".into(),
        }
    }

    fn preset(name: &str, servers: Vec<DnsServer>, final_server: &str) -> CustomDnsPreset {
        CustomDnsPreset {
            name: name.into(),
            servers,
            rules: Vec::new(),
            final_server: final_server.into(),
        }
    }

    fn with_preset(preset: CustomDnsPreset) -> DnsConfig {
        DnsConfig {
            current_preset: preset.name.clone(),
            custom_presets: vec![preset],
            ..DnsConfig::default()
        }
    }

    fn pointers(dns: &DnsConfig) -> Vec<String> {
        dns.diagnostics()
            .into_iter()
            .map(|diagnostic| diagnostic.pointer)
            .collect()
    }

    #[test]
    fn active_resolves_built_in_presets_without_rules() {
        for (name, final_kind) in [
            ("cloudflare_doh", "DoH"),
            ("google_dot", "DoT"),
            ("quad9_doh", "DoH"),
            ("system_local", "local"),
        ] {
            let dns = DnsConfig {
                current_preset: name.into(),
                ..DnsConfig::default()
            };
            let active = dns.active().unwrap();
            assert_eq!(
                active.final_server_entry().unwrap().kind_label(),
                final_kind
            );
            assert!(active.rules.is_empty());
            assert!(active.fakeip.is_none());
        }
    }

    #[test]
    fn active_resolves_a_custom_preset_by_name() {
        let mut home = preset(
            "home",
            vec![local(), doh("adguard", "94.140.14.14")],
            "adguard",
        );
        home.rules.push(DnsRule {
            domain_suffix: vec!["lan".into()],
            server: "local".into(),
            ..DnsRule::default()
        });
        let active = with_preset(home.clone()).active().unwrap();
        assert_eq!(active.servers, home.servers);
        assert_eq!(active.rules, home.rules);
        assert_eq!(active.final_server, "adguard");

        let unknown = DnsConfig {
            current_preset: "missing".into(),
            ..DnsConfig::default()
        };
        assert!(unknown.active().is_none());
    }

    #[test]
    fn active_adds_the_fakeip_server_when_enabled_or_targeted_by_a_rule() {
        let mut dns = with_preset(preset("home", vec![local()], "local"));
        assert!(dns.active().unwrap().fakeip.is_none());

        dns.custom_presets[0].rules.push(DnsRule {
            domain_suffix: vec!["example.com".into()],
            server: FAKEIP_SERVER_TAG.into(),
            ..DnsRule::default()
        });
        let targeted = dns.active().unwrap();
        assert_eq!(targeted.fakeip.unwrap().tag, "fakeip");
        assert_eq!(targeted.rules.len(), 1);
        assert!(!targeted.fakeip_catch_all);

        dns.fakeip_enabled = true;
        let enabled = dns.active().unwrap();
        assert_eq!(enabled.fakeip.unwrap().ranges, FakeIpRanges::default());
        assert!(enabled.fakeip_catch_all);
    }

    #[test]
    fn preset_names_list_built_ins_before_custom_presets() {
        let dns = with_preset(preset("home", vec![local()], "local"));
        assert_eq!(
            dns.preset_names(),
            [
                "cloudflare_doh",
                "google_dot",
                "quad9_doh",
                "system_local",
                "home"
            ]
        );
    }

    #[test]
    fn strategy_toggles_between_the_ipv4_strategies() {
        assert_eq!(DnsStrategy::PreferIpv4.toggled(), DnsStrategy::OnlyIpv4);
        assert_eq!(DnsStrategy::OnlyIpv4.toggled(), DnsStrategy::PreferIpv4);
    }

    #[test]
    fn strategy_ipv4_counterpart_maps_ipv6_strategies() {
        assert_eq!(
            DnsStrategy::PreferIpv6.ipv4_counterpart(),
            DnsStrategy::PreferIpv4
        );
        assert_eq!(
            DnsStrategy::OnlyIpv6.ipv4_counterpart(),
            DnsStrategy::OnlyIpv4
        );
        assert_eq!(
            DnsStrategy::PreferIpv4.ipv4_counterpart(),
            DnsStrategy::PreferIpv4
        );
        assert_eq!(
            DnsStrategy::OnlyIpv4.ipv4_counterpart(),
            DnsStrategy::OnlyIpv4
        );
    }

    #[test]
    fn diagnostics_rejects_ipv6_strategies() {
        for strategy in [DnsStrategy::PreferIpv6, DnsStrategy::OnlyIpv6] {
            let dns = DnsConfig {
                strategy,
                ..DnsConfig::default()
            };
            let diagnostic = crate::test_helpers::single_diagnostic(dns.diagnostics());
            assert_eq!(diagnostic.pointer, "/strategy");
            assert!(diagnostic.message.contains("ipv4_only"));
        }
    }

    #[test]
    fn strategy_as_str_uses_singbox_wire_format() {
        assert_eq!(DnsStrategy::PreferIpv4.as_str(), "prefer_ipv4");
        assert_eq!(DnsStrategy::PreferIpv6.as_str(), "prefer_ipv6");
        assert_eq!(DnsStrategy::OnlyIpv4.as_str(), "ipv4_only");
        assert_eq!(DnsStrategy::OnlyIpv6.as_str(), "ipv6_only");
    }

    #[test]
    fn strategy_serde_roundtrip() {
        for s in [
            DnsStrategy::PreferIpv4,
            DnsStrategy::PreferIpv6,
            DnsStrategy::OnlyIpv4,
            DnsStrategy::OnlyIpv6,
        ] {
            let json = serde_json::to_string(&s).unwrap();
            let back: DnsStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(s, back);
        }
    }

    #[test]
    fn server_tag_and_kind_label_for_each_variant() {
        let servers = [
            DnsServer::Local { tag: "l".into() },
            DnsServer::Udp {
                tag: "u".into(),
                server: "1.1.1.1".into(),
                server_port: None,
            },
            DnsServer::Tcp {
                tag: "t".into(),
                server: "1.1.1.1".into(),
                server_port: Some(53),
            },
            DnsServer::Tls {
                tag: "dot".into(),
                server: "1.1.1.1".into(),
                server_port: Some(853),
            },
            doh("doh", "1.1.1.1"),
            DnsServer::Quic {
                tag: "doq".into(),
                server: "1.1.1.1".into(),
                server_port: None,
            },
        ];
        let labels: Vec<&'static str> = servers.iter().map(|s| s.kind_label()).collect();
        assert_eq!(labels, vec!["local", "UDP", "TCP", "DoT", "DoH", "DoQ"]);
        let tags: Vec<&str> = servers.iter().map(|s| s.tag()).collect();
        assert_eq!(tags, vec!["l", "u", "t", "dot", "doh", "doq"]);
    }

    #[test]
    fn server_serde_roundtrip_each_variant() {
        let servers = vec![
            DnsServer::Local { tag: "l".into() },
            DnsServer::Tls {
                tag: "dot".into(),
                server: "8.8.8.8".into(),
                server_port: Some(853),
            },
            doh("doh", "1.1.1.1"),
        ];
        for s in servers {
            let json = serde_json::to_string(&s).unwrap();
            let back: DnsServer = serde_json::from_str(&json).unwrap();
            assert_eq!(s, back);
        }
    }

    #[test]
    fn dns_config_default_is_cloudflare_doh() {
        let cfg = DnsConfig::default();
        assert_eq!(cfg.current_preset, "cloudflare_doh");
        assert!(matches!(cfg.strategy, DnsStrategy::PreferIpv4));
        assert!(!cfg.fakeip_enabled);
        assert_eq!(cfg.diagnostics(), []);
    }

    #[test]
    fn diagnostics_rejects_legacy_fields_left_in_a_current_config() {
        let dns: DnsConfig = serde_json::from_value(serde_json::json!({
            "servers": [{ "type": "udp", "tag": "router", "server": "192.168.1.1" }],
            "rules": [],
            "final_server": "router"
        }))
        .unwrap();
        assert_eq!(pointers(&dns), ["/servers", "/rules", "/final_server"]);
        assert!(dns.diagnostics()[0].message.contains("dns.custom_presets"));
        assert_eq!(
            migrated(serde_json::json!({ "rules": [] })).diagnostics(),
            []
        );
    }

    #[test]
    fn diagnostics_rejects_an_unknown_current_preset() {
        let dns = DnsConfig {
            current_preset: "missing".into(),
            ..DnsConfig::default()
        };
        assert_eq!(pointers(&dns), ["/current_preset"]);
    }

    #[test]
    fn diagnostics_rejects_empty_duplicate_and_built_in_preset_names() {
        let dns = DnsConfig {
            custom_presets: vec![
                preset(" ", vec![local()], "local"),
                preset("home", vec![local()], "local"),
                preset("home", vec![local()], "local"),
                preset("google_dot", vec![local()], "local"),
            ],
            ..DnsConfig::default()
        };
        assert_eq!(
            pointers(&dns),
            [
                "/custom_presets/0/name",
                "/custom_presets/2/name",
                "/custom_presets/3/name",
            ]
        );
    }

    #[test]
    fn diagnostics_checks_each_preset_against_its_own_servers() {
        let mut home = preset(
            "home",
            vec![local(), doh("remote", "1.1.1.1"), doh("remote", "9.9.9.9")],
            "missing",
        );
        home.rules.push(DnsRule {
            server: "router".into(),
            ..DnsRule::default()
        });
        let dns = with_preset(home);
        assert_eq!(
            pointers(&dns),
            [
                "/custom_presets/0/servers/2/tag",
                "/custom_presets/0/final_server",
                "/custom_presets/0/rules/0/server",
            ]
        );
        let messages: Vec<String> = dns.diagnostics().into_iter().map(|d| d.message).collect();
        assert!(messages[2].starts_with("dns.custom_presets[0]: rules[0].server"));
    }

    #[test]
    fn diagnostics_reserve_the_fakeip_tag_for_rules() {
        let mut home = preset("home", vec![local(), doh("fakeip", "1.1.1.1")], "local");
        home.rules.push(DnsRule {
            server: FAKEIP_SERVER_TAG.into(),
            ..DnsRule::default()
        });
        assert_eq!(
            pointers(&with_preset(home)),
            ["/custom_presets/0/servers/1/tag"]
        );
    }

    #[test]
    fn diagnostics_requires_local_only_for_hostname_servers() {
        let hostname_only =
            with_preset(preset("home", vec![doh("remote", "dns.google")], "remote"));
        let diagnostic = crate::test_helpers::single_diagnostic(hostname_only.diagnostics());
        assert_eq!(diagnostic.pointer, "/custom_presets/0/servers/0/server");
        assert!(diagnostic.message.contains("dns.google"));

        let ip_only = with_preset(preset("home", vec![doh("remote", "1.1.1.1")], "remote"));
        assert_eq!(ip_only.diagnostics(), []);
        let with_local = with_preset(preset(
            "home",
            vec![local(), doh("remote", "dns.google")],
            "remote",
        ));
        assert_eq!(with_local.diagnostics(), []);
    }

    #[test]
    fn diagnostics_rejects_malformed_fakeip_ranges() {
        let dns = DnsConfig {
            fakeip_ranges: FakeIpRanges {
                inet4_range: "garbage".into(),
                inet6_range: "fc00::/129".into(),
            },
            ..DnsConfig::default()
        };
        assert_eq!(
            pointers(&dns),
            ["/fakeip_ranges/inet4_range", "/fakeip_ranges/inet6_range"]
        );
    }

    #[test]
    fn diagnostics_accepts_fakeip_ranges_that_sing_box_accepts() {
        for (inet4_range, inet6_range) in [
            ("198.18.0.1/15", "fc00::/18"),
            ("fc00::/18", "198.18.0.0/15"),
        ] {
            let dns = DnsConfig {
                fakeip_ranges: FakeIpRanges {
                    inet4_range: inet4_range.into(),
                    inet6_range: inet6_range.into(),
                },
                ..DnsConfig::default()
            };
            assert_eq!(dns.diagnostics(), [], "{inet4_range} / {inet6_range}");
        }
    }

    #[test]
    fn doh_path_default_applied_on_deserialize() {
        let json = r#"{"type":"https","tag":"doh","server":"1.1.1.1"}"#;
        let s: DnsServer = serde_json::from_str(json).unwrap();
        match s {
            DnsServer::Https { path, .. } => assert_eq!(path, "/dns-query"),
            _ => panic!("expected Https"),
        }
    }

    fn migrated(legacy_dns: serde_json::Value) -> DnsConfig {
        let mut dns: DnsConfig = serde_json::from_value(legacy_dns).unwrap();
        dns.migrate_legacy_servers();
        dns
    }

    #[test]
    fn migration_selects_the_built_in_preset_a_legacy_block_equals() {
        assert_eq!(
            migrated(serde_json::json!({})).current_preset,
            "cloudflare_doh"
        );
        let google = migrated(serde_json::json!({
            "servers": [
                { "type": "tls", "tag": "remote", "server": "8.8.8.8" },
                { "type": "local", "tag": "local" }
            ],
            "final_server": "remote"
        }));
        assert_eq!(google.current_preset, "google_dot");
        assert!(google.custom_presets.is_empty());
    }

    #[test]
    fn migration_keeps_a_hand_written_block_as_the_selected_custom_preset() {
        let rule = serde_json::json!({ "domain_suffix": ["lan"], "server": "local" });
        let cloudflare_with_rule = migrated(serde_json::json!({ "rules": [rule] }));
        assert_eq!(cloudflare_with_rule.current_preset, "custom");
        assert_eq!(cloudflare_with_rule.custom_presets[0].rules.len(), 1);

        let adguard = migrated(serde_json::json!({
            "servers": [
                { "type": "local", "tag": "local" },
                { "type": "https", "tag": "adguard", "server": "94.140.14.14" }
            ],
            "rules": [rule],
            "final_server": "adguard",
            "custom_presets": [{ "name": "custom", "servers": [{ "type": "local", "tag": "local" }], "final_server": "local" }]
        }));
        assert_eq!(adguard.current_preset, "custom-2");
        let preset = adguard.custom_preset("custom-2").unwrap();
        assert_eq!(preset.final_server, "adguard");
        assert_eq!(preset.servers.len(), 2);
        assert_eq!(preset.rules.len(), 1);
    }

    #[test]
    fn migration_moves_the_legacy_fakeip_ranges_to_the_global_setting() {
        let dns = migrated(serde_json::json!({
            "servers": [
                { "type": "local", "tag": "local" },
                { "type": "https", "tag": "remote", "server": "1.1.1.1" },
                { "type": "fake_ip", "tag": "fake", "inet4_range": "198.19.0.0/16" }
            ],
            "final_server": "remote",
            "fakeip_enabled": true
        }));
        assert_eq!(dns.current_preset, "cloudflare_doh");
        assert_eq!(dns.fakeip_ranges.inet4_range, "198.19.0.0/16");
        assert_eq!(dns.fakeip_ranges.inet6_range, "fc00::/18");
        assert!(dns.fakeip_enabled);
    }

    #[test]
    fn migration_points_rules_at_the_legacy_fakeip_server_to_the_reserved_tag() {
        let dns = migrated(serde_json::json!({
            "servers": [
                { "type": "local", "tag": "local" },
                { "type": "fake_ip", "tag": "fake" }
            ],
            "rules": [{ "domain_suffix": ["example.com"], "server": "fake" }],
            "final_server": "local",
            "fakeip_enabled": true
        }));
        assert_eq!(dns.current_preset, "custom");
        assert_eq!(dns.custom_presets[0].rules[0].server, FAKEIP_SERVER_TAG);
        assert_eq!(dns.diagnostics(), []);
    }

    #[test]
    fn migration_keeps_explicit_fakeip_rules_working_with_fakeip_off() {
        let dns = migrated(serde_json::json!({
            "servers": [
                { "type": "local", "tag": "local" },
                { "type": "fake_ip", "tag": "fake" }
            ],
            "rules": [{ "domain_suffix": ["example.com"], "server": "fake" }],
            "final_server": "local"
        }));
        let active = dns.active().unwrap();
        assert!(active.fakeip.is_some());
        assert_eq!(active.rules[0].server, FAKEIP_SERVER_TAG);
        assert!(!active.fakeip_catch_all);
    }

    #[test]
    fn migration_renames_a_regular_server_tagged_fakeip() {
        let dns = migrated(serde_json::json!({
            "servers": [
                { "type": "local", "tag": "local" },
                { "type": "https", "tag": "fakeip", "server": "1.1.1.1" },
                { "type": "https", "tag": "fakeip-2", "server": "9.9.9.9" }
            ],
            "rules": [{ "domain_suffix": ["lan"], "server": "fakeip" }],
            "final_server": "fakeip"
        }));
        let preset = dns.custom_preset("custom").unwrap();
        assert_eq!(preset.servers[1].tag(), "fakeip-3");
        assert_eq!(preset.rules[0].server, "fakeip-3");
        assert_eq!(preset.final_server, "fakeip-3");
        assert_eq!(dns.diagnostics(), []);
    }

    #[test]
    fn migration_is_idempotent() {
        let mut dns = migrated(serde_json::json!({
            "servers": [{ "type": "local", "tag": "local" }],
            "final_server": "local",
            "rules": [{ "domain": ["a"], "server": "local" }]
        }));
        let once = dns.clone();
        dns.migrate_legacy_servers();
        assert_eq!(dns, once);
    }
}

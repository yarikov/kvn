//! DNS configuration: strategy, servers, rules, and validation.
//!
//! `DnsConfig` is the source of truth used both by sing-box config generation
//! (`singbox::config`) and by the TUI's settings overlay. The legacy
//! top-level `settings.dns_strategy` field is mirrored from `dns.strategy`
//! by the v0 → v1 schema migration (see `Config::migrate`) for backward
//! compatibility.

use serde::{Deserialize, Serialize};

// (body appended by sed)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
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

    pub fn next(&self) -> Self {
        match self {
            DnsStrategy::PreferIpv4 => DnsStrategy::PreferIpv6,
            DnsStrategy::PreferIpv6 => DnsStrategy::OnlyIpv4,
            DnsStrategy::OnlyIpv4 => DnsStrategy::OnlyIpv6,
            DnsStrategy::OnlyIpv6 => DnsStrategy::PreferIpv4,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            DnsStrategy::PreferIpv4 => DnsStrategy::OnlyIpv6,
            DnsStrategy::PreferIpv6 => DnsStrategy::PreferIpv4,
            DnsStrategy::OnlyIpv4 => DnsStrategy::PreferIpv6,
            DnsStrategy::OnlyIpv6 => DnsStrategy::OnlyIpv4,
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
    pub fn label(self) -> &'static str {
        match self {
            Self::CloudflareDoh => "Cloudflare DoH (1.1.1.1)",
            Self::GoogleDot => "Google DoT (8.8.8.8)",
            Self::Quad9Doh => "Quad9 DoH (9.9.9.9)",
            Self::SystemLocal => "System resolver (local)",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::CloudflareDoh => Self::GoogleDot,
            Self::GoogleDot => Self::Quad9Doh,
            Self::Quad9Doh => Self::SystemLocal,
            Self::SystemLocal => Self::CloudflareDoh,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::CloudflareDoh => Self::SystemLocal,
            Self::GoogleDot => Self::CloudflareDoh,
            Self::Quad9Doh => Self::GoogleDot,
            Self::SystemLocal => Self::Quad9Doh,
        }
    }

    pub fn detect(dns: &DnsConfig) -> Option<Self> {
        let non_fakeip: Vec<&DnsServer> = dns
            .servers
            .iter()
            .filter(|server| !matches!(server, DnsServer::FakeIp { .. }))
            .collect();
        let final_entry = non_fakeip
            .iter()
            .find(|server| server.tag() == dns.final_server)?;

        if non_fakeip.len() == 1 {
            return matches!(final_entry, DnsServer::Local { .. }).then_some(Self::SystemLocal);
        }
        if non_fakeip.len() != 2
            || !non_fakeip
                .iter()
                .any(|server| matches!(server, DnsServer::Local { .. }))
        {
            return None;
        }

        match final_entry {
            DnsServer::Https { server, path, .. }
                if server == "1.1.1.1" && path == "/dns-query" =>
            {
                Some(Self::CloudflareDoh)
            }
            DnsServer::Tls { server, .. } if server == "8.8.8.8" => Some(Self::GoogleDot),
            DnsServer::Https { server, path, .. }
                if server == "9.9.9.9" && path == "/dns-query" =>
            {
                Some(Self::Quad9Doh)
            }
            _ => None,
        }
    }

    pub fn apply(self, dns: &mut DnsConfig) {
        let fakeip_servers: Vec<DnsServer> = dns
            .servers
            .iter()
            .filter(|server| matches!(server, DnsServer::FakeIp { .. }))
            .cloned()
            .collect();
        let (mut servers, final_server) = match self {
            Self::CloudflareDoh => (
                vec![
                    DnsServer::Local {
                        tag: "local".to_string(),
                    },
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
                    DnsServer::Local {
                        tag: "local".to_string(),
                    },
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
                    DnsServer::Local {
                        tag: "local".to_string(),
                    },
                    DnsServer::Https {
                        tag: "remote".to_string(),
                        server: "9.9.9.9".to_string(),
                        server_port: None,
                        path: "/dns-query".to_string(),
                    },
                ],
                "remote",
            ),
            Self::SystemLocal => (
                vec![DnsServer::Local {
                    tag: "local".to_string(),
                }],
                "local",
            ),
        };
        servers.extend(fakeip_servers);
        dns.servers = servers;
        dns.final_server = final_server.to_string();
    }
}

/// A single sing-box DNS server. Variants map 1:1 onto sing-box 1.12 server types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    FakeIp {
        tag: String,
        #[serde(default = "default_fakeip_v4")]
        inet4_range: String,
        #[serde(default = "default_fakeip_v6")]
        inet6_range: String,
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
            | DnsServer::Quic { tag, .. }
            | DnsServer::FakeIp { tag, .. } => tag,
        }
    }

    /// Short label used in the status bar, e.g. "DoH", "DoT", "fakeip".
    pub fn kind_label(&self) -> &'static str {
        match self {
            DnsServer::Local { .. } => "local",
            DnsServer::Udp { .. } => "UDP",
            DnsServer::Tcp { .. } => "TCP",
            DnsServer::Tls { .. } => "DoT",
            DnsServer::Https { .. } => "DoH",
            DnsServer::Quic { .. } => "DoQ",
            DnsServer::FakeIp { .. } => "fakeip",
        }
    }
}

/// A per-domain DNS routing rule. Maps onto sing-box `dns.rules[*]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
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
    /// Must match a server tag in [`DnsConfig::servers`].
    pub server: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disable_cache: bool,
}

/// User-controlled DNS configuration. Replaces the hard-coded DNS section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DnsConfig {
    #[serde(default = "default_dns_servers")]
    pub servers: Vec<DnsServer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<DnsRule>,
    /// Tag of the fallback DNS server used when no rule matches.
    #[serde(default = "default_final_server")]
    pub final_server: String,
    #[serde(default)]
    pub strategy: DnsStrategy,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fakeip_enabled: bool,
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            servers: default_dns_servers(),
            rules: Vec::new(),
            final_server: default_final_server(),
            strategy: DnsStrategy::default(),
            fakeip_enabled: false,
        }
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

fn default_final_server() -> String {
    "remote".to_string()
}

fn default_dns_servers() -> Vec<DnsServer> {
    vec![
        DnsServer::Local {
            tag: "local".to_string(),
        },
        DnsServer::Https {
            tag: "remote".to_string(),
            server: "1.1.1.1".to_string(),
            server_port: None,
            path: default_doh_path(),
        },
    ]
}
impl DnsConfig {
    /// Validate that server tags are unique and rule/final references point at
    /// existing tags. Called from [`Config::validate`].
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut tags: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for server in &self.servers {
            let tag = server.tag();
            if tag.trim().is_empty() {
                anyhow::bail!("dns.servers: server tag must not be empty");
            }
            if !tags.insert(tag) {
                anyhow::bail!("dns.servers: duplicate server tag {:?}", tag);
            }
        }
        if !tags.contains(self.final_server.as_str()) {
            anyhow::bail!(
                "dns.final_server {:?} does not match any server tag",
                self.final_server
            );
        }
        for (idx, rule) in self.rules.iter().enumerate() {
            if !tags.contains(rule.server.as_str()) {
                anyhow::bail!(
                    "dns.rules[{idx}].server {:?} does not match any server tag",
                    rule.server
                );
            }
        }
        if self.fakeip_enabled
            && !self
                .servers
                .iter()
                .any(|s| matches!(s, DnsServer::FakeIp { .. }))
        {
            anyhow::bail!("dns.fakeip_enabled = true but no fakeip server is defined");
        }
        Ok(())
    }

    /// Return the first `fakeip` server, if any.
    pub fn fakeip_server(&self) -> Option<&DnsServer> {
        self.servers
            .iter()
            .find(|s| matches!(s, DnsServer::FakeIp { .. }))
    }

    /// Return the final server entry (used by the status bar to label the
    /// current upstream by its kind).
    pub fn final_server_entry(&self) -> Option<&DnsServer> {
        self.servers
            .iter()
            .find(|s| s.tag() == self.final_server.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_cycles_in_both_directions() {
        assert_eq!(DnsPreset::CloudflareDoh.next(), DnsPreset::GoogleDot);
        assert_eq!(DnsPreset::CloudflareDoh.prev(), DnsPreset::SystemLocal);
        assert_eq!(DnsPreset::SystemLocal.next(), DnsPreset::CloudflareDoh);
    }

    #[test]
    fn preset_detection_ignores_fakeip_and_rejects_custom_servers() {
        let mut dns = DnsConfig::default();
        dns.servers.push(DnsServer::FakeIp {
            tag: "fakeip".into(),
            inet4_range: "198.18.0.0/15".into(),
            inet6_range: "fc00::/18".into(),
        });
        assert_eq!(DnsPreset::detect(&dns), Some(DnsPreset::CloudflareDoh));

        dns.servers[1] = DnsServer::Https {
            tag: "remote".into(),
            server: "94.140.14.14".into(),
            server_port: None,
            path: "/dns-query".into(),
        };
        assert_eq!(DnsPreset::detect(&dns), None);
    }

    #[test]
    fn applying_preset_preserves_fakeip_server() {
        let mut dns = DnsConfig::default();
        dns.servers.push(DnsServer::FakeIp {
            tag: "fakeip".into(),
            inet4_range: "198.18.0.0/15".into(),
            inet6_range: "fc00::/18".into(),
        });

        DnsPreset::SystemLocal.apply(&mut dns);

        assert_eq!(DnsPreset::detect(&dns), Some(DnsPreset::SystemLocal));
        assert!(dns.fakeip_server().is_some());
    }

    #[test]
    fn strategy_next_cycles_through_all_variants() {
        assert_eq!(DnsStrategy::PreferIpv4.next(), DnsStrategy::PreferIpv6);
        assert_eq!(DnsStrategy::PreferIpv6.next(), DnsStrategy::OnlyIpv4);
        assert_eq!(DnsStrategy::OnlyIpv4.next(), DnsStrategy::OnlyIpv6);
        assert_eq!(DnsStrategy::OnlyIpv6.next(), DnsStrategy::PreferIpv4);
    }

    #[test]
    fn strategy_prev_cycles_through_all_variants() {
        assert_eq!(DnsStrategy::PreferIpv4.prev(), DnsStrategy::OnlyIpv6);
        assert_eq!(DnsStrategy::OnlyIpv6.prev(), DnsStrategy::OnlyIpv4);
        assert_eq!(DnsStrategy::OnlyIpv4.prev(), DnsStrategy::PreferIpv6);
        assert_eq!(DnsStrategy::PreferIpv6.prev(), DnsStrategy::PreferIpv4);
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
            DnsServer::Https {
                tag: "doh".into(),
                server: "1.1.1.1".into(),
                server_port: None,
                path: "/dns-query".into(),
            },
            DnsServer::Quic {
                tag: "doq".into(),
                server: "1.1.1.1".into(),
                server_port: None,
            },
            DnsServer::FakeIp {
                tag: "fakeip".into(),
                inet4_range: "198.18.0.0/15".into(),
                inet6_range: "fc00::/18".into(),
            },
        ];
        let labels: Vec<&'static str> = servers.iter().map(|s| s.kind_label()).collect();
        assert_eq!(
            labels,
            vec!["local", "UDP", "TCP", "DoT", "DoH", "DoQ", "fakeip"]
        );
        let tags: Vec<&str> = servers.iter().map(|s| s.tag()).collect();
        assert_eq!(tags, vec!["l", "u", "t", "dot", "doh", "doq", "fakeip"]);
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
            DnsServer::Https {
                tag: "doh".into(),
                server: "1.1.1.1".into(),
                server_port: None,
                path: "/dns-query".into(),
            },
            DnsServer::FakeIp {
                tag: "fakeip".into(),
                inet4_range: "198.18.0.0/15".into(),
                inet6_range: "fc00::/18".into(),
            },
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
        assert_eq!(cfg.final_server, "remote");
        assert!(matches!(cfg.strategy, DnsStrategy::PreferIpv4));
        assert!(!cfg.fakeip_enabled);
        assert_eq!(cfg.servers.len(), 2);
        let final_entry = cfg.final_server_entry().unwrap();
        assert!(matches!(
            final_entry,
            DnsServer::Https { server, .. } if server == "1.1.1.1"
        ));
    }

    #[test]
    fn fakeip_server_returns_first_match() {
        let mut cfg = DnsConfig::default();
        assert!(cfg.fakeip_server().is_none());
        cfg.servers.push(DnsServer::FakeIp {
            tag: "fakeip".into(),
            inet4_range: "198.18.0.0/15".into(),
            inet6_range: "fc00::/18".into(),
        });
        assert!(cfg.fakeip_server().is_some());
    }

    #[test]
    fn validate_rejects_empty_tag() {
        let mut cfg = DnsConfig::default();
        cfg.servers.push(DnsServer::Local { tag: "  ".into() });
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("must not be empty"));
    }

    #[test]
    fn validate_rejects_duplicate_tags() {
        let mut cfg = DnsConfig::default();
        cfg.servers.push(DnsServer::Local {
            tag: "remote".into(),
        });
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("duplicate"));
    }

    #[test]
    fn validate_rejects_unknown_final_server() {
        let cfg = DnsConfig {
            final_server: "nope".into(),
            ..DnsConfig::default()
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("final_server"));
    }

    #[test]
    fn validate_rejects_unknown_rule_server() {
        let mut cfg = DnsConfig::default();
        cfg.rules.push(DnsRule {
            server: "missing".into(),
            ..Default::default()
        });
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("rules"));
    }

    #[test]
    fn validate_rejects_fakeip_enabled_without_server() {
        let cfg = DnsConfig {
            fakeip_enabled: true,
            ..DnsConfig::default()
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("fakeip"));
    }

    #[test]
    fn validate_accepts_default() {
        DnsConfig::default().validate().unwrap();
    }

    #[test]
    fn validate_accepts_fakeip_when_server_present() {
        let mut cfg = DnsConfig {
            fakeip_enabled: true,
            ..DnsConfig::default()
        };
        cfg.servers.push(DnsServer::FakeIp {
            tag: "fakeip".into(),
            inet4_range: "198.18.0.0/15".into(),
            inet6_range: "fc00::/18".into(),
        });
        cfg.validate().unwrap();
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

    #[test]
    fn fakeip_ranges_default_applied_on_deserialize() {
        let json = r#"{"type":"fake_ip","tag":"fakeip"}"#;
        let s: DnsServer = serde_json::from_str(json).unwrap();
        match s {
            DnsServer::FakeIp {
                inet4_range,
                inet6_range,
                ..
            } => {
                assert_eq!(inet4_range, "198.18.0.0/15");
                assert_eq!(inet6_range, "fc00::/18");
            }
            _ => panic!("expected FakeIp"),
        }
    }
}

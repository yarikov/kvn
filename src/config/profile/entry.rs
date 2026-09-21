use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Protocol, ProtocolConfig, Security, VlessConfig};

/// Single VPN profile. The `protocol` discriminant and protocol-specific
/// fields are flattened into [`ProtocolConfig`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    pub address: String,
    pub port: u16,
    #[serde(flatten)]
    pub config: ProtocolConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<Uuid>,
}

impl Profile {
    /// Create a new VLESS profile with a generated UUID. Other protocols
    /// gain dedicated constructors as their share-link parsers land.
    pub fn new_vless(name: String, address: String, port: u16, uuid: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            address,
            port,
            config: ProtocolConfig::Vless(VlessConfig {
                uuid,
                ..VlessConfig::default()
            }),
            tags: Vec::new(),
            subscription_id: None,
        }
    }

    /// Protocol discriminant.
    pub fn protocol(&self) -> Protocol {
        self.config.protocol()
    }

    /// Short label for the UI protocol column (≤6 chars).
    pub fn protocol_label(&self) -> &'static str {
        self.protocol().ui_label()
    }

    /// Deeper semantic validation on top of the per-field non-empty checks
    /// enforced by `Config::validate`. Verifies:
    /// - `port != 0`
    /// - `address` parses as an IPv4/IPv6 literal or a valid hostname
    /// - protocol UUIDs (VLESS/VMess/TUIC) parse as [`Uuid`]
    /// - `security=Reality` requires a populated `reality` block
    pub fn validate_semantic(&self) -> anyhow::Result<()> {
        if self.port == 0 {
            anyhow::bail!("port must not be 0");
        }
        validate_host(&self.address)?;
        match &self.config {
            ProtocolConfig::Vless(cfg) => {
                Uuid::parse_str(cfg.uuid.trim()).map_err(|e| {
                    anyhow::anyhow!("vless.uuid {:?} is not a valid UUID: {e}", cfg.uuid)
                })?;
                if cfg.security == Some(Security::Reality) && cfg.tls.reality.is_none() {
                    anyhow::bail!("vless.security=reality requires a `reality` block");
                }
            }
            ProtocolConfig::Vmess(cfg) => {
                Uuid::parse_str(cfg.uuid.trim()).map_err(|e| {
                    anyhow::anyhow!("vmess.uuid {:?} is not a valid UUID: {e}", cfg.uuid)
                })?;
            }
            ProtocolConfig::Tuic(cfg) => {
                Uuid::parse_str(cfg.uuid.trim()).map_err(|e| {
                    anyhow::anyhow!("tuic.uuid {:?} is not a valid UUID: {e}", cfg.uuid)
                })?;
            }
            ProtocolConfig::Trojan(_)
            | ProtocolConfig::Shadowsocks(_)
            | ProtocolConfig::Hysteria2(_)
            | ProtocolConfig::Shadowtls(_)
            | ProtocolConfig::Anytls(_)
            | ProtocolConfig::Socks(_)
            | ProtocolConfig::Http(_)
            | ProtocolConfig::Ssh(_) => {}
        }
        Ok(())
    }

    /// Stable key identifying the credentials behind this profile,
    /// used by the subscription importer to detect duplicates.
    pub fn dedup_key(&self) -> String {
        match &self.config {
            ProtocolConfig::Vless(c) => format!("vless:{}", c.uuid),
            ProtocolConfig::Vmess(c) => format!("vmess:{}", c.uuid),
            ProtocolConfig::Trojan(c) => {
                format!("trojan:{}@{}:{}", c.password, self.address, self.port)
            }
            ProtocolConfig::Shadowsocks(c) => {
                format!("ss:{}@{}:{}", c.password, self.address, self.port)
            }
            ProtocolConfig::Hysteria2(c) => {
                format!("hy2:{}@{}:{}", c.password, self.address, self.port)
            }
            ProtocolConfig::Tuic(c) => format!("tuic:{}", c.uuid),
            ProtocolConfig::Shadowtls(c) => {
                format!("shadowtls:{}@{}:{}", c.password, self.address, self.port)
            }
            ProtocolConfig::Anytls(c) => {
                format!("anytls:{}@{}:{}", c.password, self.address, self.port)
            }
            ProtocolConfig::Socks(c) => format!(
                "socks:{}@{}:{}",
                c.username.as_deref().unwrap_or(""),
                self.address,
                self.port
            ),
            ProtocolConfig::Http(c) => format!(
                "http:{}@{}:{}",
                c.username.as_deref().unwrap_or(""),
                self.address,
                self.port
            ),
            ProtocolConfig::Ssh(c) => format!("ssh:{}@{}:{}", c.user, self.address, self.port),
        }
    }
}

/// Accept `address` if it parses as a bare IPv4/IPv6 literal or as a hostname.
/// sing-box wants the on-wire form (unbracketed for IPv6), so we try
/// [`IpAddr`](std::net::IpAddr) first and fall back to [`url::Host::parse`]
/// for domain names. Bracketed IPv6 (`[::1]`) is accepted via `Host::parse`.
fn validate_host(address: &str) -> anyhow::Result<()> {
    use std::net::IpAddr;
    if address.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    url::Host::parse(address)
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("address {:?} is not a valid IP or hostname: {e}", address))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::*;
    use crate::test_helpers::vless_cfg;

    #[test]
    fn profile_new_defaults() {
        let p = Profile::new_vless(
            "test".to_string(),
            "1.2.3.4".to_string(),
            443,
            "uuid-here".to_string(),
        );
        assert_eq!(p.name, "test");
        assert_eq!(p.protocol(), Protocol::Vless);
        assert_eq!(p.address, "1.2.3.4");
        assert_eq!(p.port, 443);
        let cfg = vless_cfg(&p);
        assert_eq!(cfg.uuid, "uuid-here");
        assert!(cfg.flow.is_none());
        assert!(cfg.security.is_none());
        assert!(cfg.tls.reality.is_none());
        assert!(cfg.transport_type.is_none());
        assert!(cfg.transport_service_name.is_none());
        assert!(cfg.tls.utls_fingerprint.is_none());
        assert!(cfg.tls.ech.is_none());
        assert!(cfg.legacy_fingerprint.is_none());
        assert!(p.tags.is_empty());
        assert_ne!(p.id, Uuid::nil());
    }

    #[test]
    fn profile_deserialize_missing_optionals() {
        let json = r#"{
            "id": "550e8400-e29b-41d4-a716-446655440000",
            "name": "Minimal",
            "protocol": "vless",
            "address": "1.1.1.1",
            "port": 443,
            "uuid": "uuid"
        }"#;
        let p: Profile = serde_json::from_str(json).unwrap();
        assert_eq!(p.name, "Minimal");
        let cfg = vless_cfg(&p);
        assert!(cfg.flow.is_none());
        assert!(cfg.tls.reality.is_none());
        assert!(p.tags.is_empty());
    }

    #[test]
    fn validate_semantic_rejects_port_zero() {
        let mut p = Profile::new_vless(
            "P".to_string(),
            "1.2.3.4".to_string(),
            0,
            crate::test_helpers::TEST_UUID.to_string(),
        );
        p.port = 0;
        let err = p.validate_semantic().unwrap_err().to_string();
        assert!(err.contains("port"), "Error was: {}", err);
    }

    #[test]
    fn validate_semantic_rejects_garbage_uuid() {
        let p = Profile::new_vless(
            "P".to_string(),
            "1.2.3.4".to_string(),
            443,
            "not-a-uuid".to_string(),
        );
        let err = p.validate_semantic().unwrap_err().to_string();
        assert!(err.contains("vless.uuid"), "Error was: {}", err);
    }

    #[test]
    fn validate_semantic_accepts_ipv6_literal() {
        let p = Profile::new_vless(
            "P".to_string(),
            "2001:db8::1".to_string(),
            443,
            crate::test_helpers::TEST_UUID.to_string(),
        );
        p.validate_semantic().unwrap();
    }

    #[test]
    fn validate_semantic_accepts_hostname() {
        let p = Profile::new_vless(
            "P".to_string(),
            "vpn.example.com".to_string(),
            443,
            crate::test_helpers::TEST_UUID.to_string(),
        );
        p.validate_semantic().unwrap();
    }

    #[test]
    fn validate_semantic_rejects_address_with_spaces() {
        let p = Profile::new_vless(
            "P".to_string(),
            "bad host name".to_string(),
            443,
            crate::test_helpers::TEST_UUID.to_string(),
        );
        assert!(p.validate_semantic().is_err());
    }

    #[test]
    fn validate_semantic_rejects_reality_without_block() {
        let mut p = Profile::new_vless(
            "P".to_string(),
            "1.2.3.4".to_string(),
            443,
            crate::test_helpers::TEST_UUID.to_string(),
        );
        if let ProtocolConfig::Vless(ref mut cfg) = p.config {
            cfg.security = Some(Security::Reality);
            cfg.tls.reality = None;
        }
        let err = p.validate_semantic().unwrap_err().to_string();
        assert!(
            err.contains("reality") && err.contains("block"),
            "Error was: {}",
            err
        );
    }

    #[test]
    fn validate_semantic_accepts_reality_with_block() {
        let mut p = Profile::new_vless(
            "P".to_string(),
            "1.2.3.4".to_string(),
            443,
            crate::test_helpers::TEST_UUID.to_string(),
        );
        if let ProtocolConfig::Vless(ref mut cfg) = p.config {
            cfg.security = Some(Security::Reality);
            cfg.tls.reality = Some(RealitySettings::default());
        }
        p.validate_semantic().unwrap();
    }

    #[test]
    fn dedup_key_distinguishes_protocols() {
        let v = Profile::new_vless("V".into(), "1.1.1.1".into(), 443, "shared-uuid".to_string());
        let m = Profile {
            id: Uuid::nil(),
            name: "M".into(),
            address: "1.1.1.1".into(),
            port: 443,
            config: ProtocolConfig::Vmess(VmessConfig {
                uuid: "shared-uuid".to_string(),
                ..VmessConfig::default()
            }),
            tags: Vec::new(),
            subscription_id: None,
        };
        assert_ne!(
            v.dedup_key(),
            m.dedup_key(),
            "same UUID on different protocols must dedup separately"
        );
    }
}

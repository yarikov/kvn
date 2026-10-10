use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ConfigDiagnostic, Protocol, ProtocolConfig, Security, VlessConfig};

/// Single VPN profile. The `protocol` discriminant and protocol-specific
/// fields are flattened into [`ProtocolConfig`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[schemars(transform = super::json_schema::share_profile_fields_with_protocol_branches)]
#[schemars(transform = super::json_schema::check_branches_only_for_known_protocols)]
pub struct Profile {
    #[serde(default = "Uuid::new_v4")]
    #[schemars(transform = super::json_schema::without_default)]
    pub id: Uuid,
    #[schemars(length(min = 1))]
    pub name: String,
    #[schemars(length(min = 1))]
    pub address: String,
    #[schemars(range(min = 1))]
    pub port: u16,
    #[serde(flatten)]
    pub config: ProtocolConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub share_link_params: BTreeMap<String, serde_json::Value>,
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
            share_link_params: Default::default(),
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

    pub fn diagnostics(&self) -> Vec<ConfigDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.name.trim().is_empty() {
            diagnostics.push(ConfigDiagnostic::new("/name", "name must not be empty"));
        }
        if self.address.trim().is_empty() {
            diagnostics.push(ConfigDiagnostic::new(
                "/address",
                "address must not be empty",
            ));
        } else if let Err(error) = validate_host(&self.address) {
            diagnostics.push(ConfigDiagnostic::new("/address", error.to_string()));
        }
        if self.port == 0 {
            diagnostics.push(ConfigDiagnostic::new("/port", "port must not be 0"));
        }
        diagnostics.extend(self.config.diagnostics());
        diagnostics.extend(self.protocol_uuid_diagnostic());
        if let ProtocolConfig::Vless(cfg) = &self.config
            && cfg.security == Some(Security::Reality)
            && cfg.tls.reality.is_none()
        {
            diagnostics.push(ConfigDiagnostic::new(
                "/security",
                "vless.security=reality requires a `reality` block",
            ));
        }
        diagnostics
    }

    fn protocol_uuid_diagnostic(&self) -> Option<ConfigDiagnostic> {
        let (protocol, uuid) = match &self.config {
            ProtocolConfig::Vless(cfg) => ("vless", &cfg.uuid),
            ProtocolConfig::Vmess(cfg) => ("vmess", &cfg.uuid),
            ProtocolConfig::Tuic(cfg) => ("tuic", &cfg.uuid),
            ProtocolConfig::Trojan(_)
            | ProtocolConfig::Shadowsocks(_)
            | ProtocolConfig::Hysteria2(_)
            | ProtocolConfig::Shadowtls(_)
            | ProtocolConfig::Anytls(_)
            | ProtocolConfig::Naive(_)
            | ProtocolConfig::Socks(_)
            | ProtocolConfig::Http(_)
            | ProtocolConfig::Ssh(_) => return None,
        };
        let uuid = uuid.trim();
        if uuid.is_empty() {
            return None;
        }
        Uuid::parse_str(uuid).err().map(|error| {
            ConfigDiagnostic::new(
                "/uuid",
                format!("{protocol}.uuid {uuid:?} is not a valid UUID: {error}"),
            )
        })
    }

    /// Stable key identifying the credentials and endpoint behind this profile,
    /// used by the subscription importer to detect duplicates.
    pub fn dedup_key(&self) -> String {
        format!("{}@{}", self.credential_key(), self.endpoint())
    }

    pub fn endpoint_key(&self) -> String {
        format!("{}@{}", self.config.protocol(), self.endpoint())
    }

    pub fn reality_rotation_key(&self) -> Option<String> {
        let reality = self.config.tls()?.reality.as_ref()?;
        let flow = match &self.config {
            ProtocolConfig::Vless(vless) => vless.flow.as_ref(),
            _ => None,
        };
        Some(format!(
            "{}@{}:{}|reality-key:{}|flow:{flow:?}{}",
            self.credential_key(),
            self.address,
            self.port,
            reality.public_key,
            self.config.transport_identity()
        ))
    }

    fn endpoint(&self) -> String {
        format!(
            "{}:{}{}",
            self.address,
            self.port,
            self.config.endpoint_identity()
        )
    }

    fn credential_key(&self) -> String {
        match &self.config {
            ProtocolConfig::Vless(c) => format!("vless:{}", c.uuid),
            ProtocolConfig::Vmess(c) => format!("vmess:{}", c.uuid),
            ProtocolConfig::Trojan(c) => format!("trojan:{}", c.password),
            ProtocolConfig::Shadowsocks(c) => format!("ss:{}", c.password),
            ProtocolConfig::Hysteria2(c) => format!("hy2:{}", c.password),
            ProtocolConfig::Tuic(c) => format!("tuic:{}", c.uuid),
            ProtocolConfig::Shadowtls(c) => format!("shadowtls:{}", c.password),
            ProtocolConfig::Anytls(c) => format!("anytls:{}", c.password),
            ProtocolConfig::Naive(c) => format!(
                "naive:{}:{}",
                c.username.as_deref().unwrap_or(""),
                c.password.as_deref().unwrap_or("")
            ),
            ProtocolConfig::Socks(c) => {
                format!("socks:{}", c.username.as_deref().unwrap_or(""))
            }
            ProtocolConfig::Http(c) => format!("http:{}", c.username.as_deref().unwrap_or("")),
            ProtocolConfig::Ssh(c) => format!("ssh:{}", c.user),
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
        assert!(cfg.transport.is_none());
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
    fn diagnostics_rejects_port_zero() {
        let mut p = Profile::new_vless(
            "P".to_string(),
            "1.2.3.4".to_string(),
            0,
            crate::test_helpers::TEST_UUID.to_string(),
        );
        p.port = 0;
        let diagnostic = crate::test_helpers::single_diagnostic(p.diagnostics());
        assert_eq!(diagnostic.pointer, "/port");
    }

    #[test]
    fn diagnostics_rejects_garbage_uuid() {
        let p = Profile::new_vless(
            "P".to_string(),
            "1.2.3.4".to_string(),
            443,
            "not-a-uuid".to_string(),
        );
        let diagnostic = crate::test_helpers::single_diagnostic(p.diagnostics());
        assert_eq!(diagnostic.pointer, "/uuid");
        assert!(diagnostic.message.contains("vless.uuid"));
    }

    #[test]
    fn diagnostics_accepts_ipv6_literal() {
        let p = Profile::new_vless(
            "P".to_string(),
            "2001:db8::1".to_string(),
            443,
            crate::test_helpers::TEST_UUID.to_string(),
        );
        assert_eq!(p.diagnostics(), []);
    }

    #[test]
    fn diagnostics_accepts_hostname() {
        let p = Profile::new_vless(
            "P".to_string(),
            "vpn.example.com".to_string(),
            443,
            crate::test_helpers::TEST_UUID.to_string(),
        );
        assert_eq!(p.diagnostics(), []);
    }

    #[test]
    fn diagnostics_rejects_address_with_spaces() {
        let p = Profile::new_vless(
            "P".to_string(),
            "bad host name".to_string(),
            443,
            crate::test_helpers::TEST_UUID.to_string(),
        );
        let diagnostic = crate::test_helpers::single_diagnostic(p.diagnostics());
        assert_eq!(diagnostic.pointer, "/address");
    }

    #[test]
    fn diagnostics_rejects_reality_without_block() {
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
        let diagnostic = crate::test_helpers::single_diagnostic(p.diagnostics());
        assert_eq!(diagnostic.pointer, "/security");
        assert!(diagnostic.message.contains("requires a `reality` block"));
    }

    #[test]
    fn diagnostics_accepts_reality_with_block() {
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
        assert_eq!(p.diagnostics(), []);
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
            share_link_params: Default::default(),
        };
        assert_ne!(
            v.dedup_key(),
            m.dedup_key(),
            "same UUID on different protocols must dedup separately"
        );
    }

    #[test]
    fn dedup_key_distinguishes_endpoints_sharing_a_uuid() {
        let profile = |address: &str, port: u16, config: ProtocolConfig| Profile {
            id: Uuid::nil(),
            name: "P".into(),
            address: address.into(),
            port,
            config,
            tags: Vec::new(),
            subscription_id: None,
            share_link_params: Default::default(),
        };
        let configs = [
            ProtocolConfig::Vless(VlessConfig {
                uuid: "shared-uuid".to_string(),
                ..VlessConfig::default()
            }),
            ProtocolConfig::Vmess(VmessConfig {
                uuid: "shared-uuid".to_string(),
                ..VmessConfig::default()
            }),
            ProtocolConfig::Tuic(TuicConfig {
                uuid: "shared-uuid".to_string(),
                ..TuicConfig::default()
            }),
        ];
        for config in configs {
            let nl = profile("nl.example.com", 443, config.clone());
            let de = profile("de.example.com", 443, config.clone());
            let other_port = profile("nl.example.com", 8443, config);
            assert_ne!(nl.dedup_key(), de.dedup_key());
            assert_ne!(nl.dedup_key(), other_port.dedup_key());
        }
    }

    fn vless_at(address: &str, uuid: &str, tls: TlsCommon) -> Profile {
        let mut profile = Profile::new_vless("P".into(), address.into(), 443, uuid.into());
        if let ProtocolConfig::Vless(cfg) = &mut profile.config {
            cfg.tls = tls;
        }
        profile
    }

    fn sni(server_name: &str) -> TlsCommon {
        TlsCommon {
            server_name: Some(server_name.into()),
            ..TlsCommon::default()
        }
    }

    fn reality(server_name: &str) -> TlsCommon {
        TlsCommon {
            reality: Some(RealitySettings {
                server_name: server_name.into(),
                ..RealitySettings::default()
            }),
            ..TlsCommon::default()
        }
    }

    #[test]
    fn naive_dedup_merges_one_server_in_two_link_forms_and_keeps_quic_apart() {
        let key = |link: &str| {
            crate::config::profile::parse_share_link(link)
                .unwrap()
                .dedup_key()
        };
        let https = key("naive+https://alice:pw@n.example:443?peer=sni.example#A");
        assert_eq!(
            https,
            key("http2://YWxpY2U6cHdAbi5leGFtcGxlOjQ0Mw==?peer=sni.example#A")
        );
        assert_ne!(
            https,
            key("naive+quic://alice:pw@n.example:443?peer=sni.example#A")
        );
    }

    #[test]
    fn endpoint_key_ignores_credentials() {
        let original = vless_at("cdn.example.com", "old-uuid", sni("nl.example.com"));
        let rotated = vless_at("cdn.example.com", "new-uuid", sni("nl.example.com"));

        assert_eq!(original.endpoint_key(), rotated.endpoint_key());
        assert_ne!(original.dedup_key(), rotated.dedup_key());
    }

    #[test]
    fn endpoint_key_distinguishes_servers_behind_one_address() {
        let nl = vless_at("cdn.example.com", "shared", sni("nl.example.com"));
        let de = vless_at("cdn.example.com", "shared", sni("de.example.com"));
        let reality_nl = vless_at("cdn.example.com", "shared", reality("nl.example.com"));

        assert_ne!(nl.endpoint_key(), de.endpoint_key());
        assert_ne!(nl.endpoint_key(), reality_nl.endpoint_key());
        assert_ne!(nl.dedup_key(), de.dedup_key());
    }

    #[test]
    fn endpoint_key_distinguishes_transport_paths() {
        let trojan = |path: &str| Profile {
            id: Uuid::nil(),
            name: "T".into(),
            address: "cdn.example.com".into(),
            port: 443,
            config: ProtocolConfig::Trojan(TrojanConfig {
                password: "shared".into(),
                tls: TlsCommon::default(),
                transport: Some(TransportConfig {
                    kind: TransportType::Ws,
                    path: Some(path.into()),
                    host: None,
                    service_name: None,
                    headers: Default::default(),
                    early_data: None,
                }),
            }),
            tags: Vec::new(),
            subscription_id: None,
            share_link_params: Default::default(),
        };

        assert_ne!(trojan("/nl").endpoint_key(), trojan("/de").endpoint_key());
    }
}

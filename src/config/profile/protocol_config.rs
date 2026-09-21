use serde::{Deserialize, Serialize};

use super::{
    Flow, Hysteria2Obfs, Protocol, Security, ShadowsocksCipher, ShadowtlsVersion, SocksVersion,
    TlsCommon, TransportConfig, TransportType, TuicCongestion, TuicUdpRelayMode, VmessSecurity,
};

/// VLESS-specific profile configuration.
///
/// TLS parameters live on the shared [`TlsCommon`] via `#[serde(flatten)]`,
/// so legacy `profiles.json` entries with top-level `reality`/`ech` keys
/// deserialize directly into `tls.reality` / `tls.ech` (same wire shape).
/// The pre-v2 top-level `fingerprint` key needs explicit migration into
/// `tls.utls_fingerprint`; see [`Config::migrate`](crate::config::profile::Config::migrate) (`migrate_v1_to_v2`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VlessConfig {
    pub uuid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<Flow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<Security>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_type: Option<TransportType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_service_name: Option<String>,
    /// Pre-v2 alias of `tls.utls_fingerprint`. Read from the legacy JSON
    /// key `"fingerprint"`, never written. `migrate_v1_to_v2` moves it
    /// into `tls.utls_fingerprint` and clears this field.
    #[serde(default, rename = "fingerprint", skip_serializing)]
    pub legacy_fingerprint: Option<String>,
    #[serde(default, flatten)]
    pub tls: TlsCommon,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VmessConfig {
    pub uuid: String,
    #[serde(default)]
    pub alter_id: u32,
    #[serde(default)]
    pub security: VmessSecurity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_padding: Option<bool>,
    #[serde(default, flatten)]
    pub tls: TlsCommon,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TrojanConfig {
    pub password: String,
    #[serde(default, flatten)]
    pub tls: TlsCommon,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct ShadowsocksConfig {
    pub method: ShadowsocksCipher,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Hysteria2Config {
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up_mbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down_mbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub obfs: Option<Hysteria2Obfs>,
    #[serde(default, flatten)]
    pub tls: TlsCommon,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TuicConfig {
    pub uuid: String,
    pub password: String,
    #[serde(default)]
    pub congestion_control: TuicCongestion,
    #[serde(default)]
    pub udp_relay_mode: TuicUdpRelayMode,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub zero_rtt_handshake: bool,
    #[serde(default, flatten)]
    pub tls: TlsCommon,
}

/// ShadowTLS-wrapped Shadowsocks. The sing-box `shadowtls` outbound is
/// a TLS-camouflage wrapper that does not perform any traffic ciphering on
/// its own; an inner Shadowsocks outbound chained via `detour` carries the
/// actual data. We model both halves in one profile so the user supplies
/// the ShadowTLS password (v3) plus the inner SS method/password once.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ShadowtlsConfig {
    #[serde(default)]
    pub version: ShadowtlsVersion,
    /// ShadowTLS v3 client password. Unused for v1/v2.
    pub password: String,
    /// Inner Shadowsocks cipher used by the detour outbound.
    #[serde(default)]
    pub method: ShadowsocksCipher,
    /// Inner Shadowsocks password used by the detour outbound.
    #[serde(default)]
    pub ss_password: String,
    #[serde(default, flatten)]
    pub tls: TlsCommon,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AnytlsConfig {
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_session_check_interval: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_session_timeout: Option<String>,
    #[serde(default, flatten)]
    pub tls: TlsCommon,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct SocksConfig {
    #[serde(default)]
    pub version: SocksVersion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HttpConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, flatten)]
    pub tls: TlsCommon,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct SshConfig {
    pub user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_key_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_key_passphrase: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_key: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_key_algorithms: Vec<String>,
}

/// Protocol-specific profile configuration.
///
/// The `protocol` discriminant is serialized at the same JSON level as the
/// other [`Profile`](crate::config::profile::Profile) fields via `#[serde(flatten)]` (internally-tagged enum).
/// For VLESS this preserves the historic `profiles.json` shape exactly.
///
/// Structs that carry `#[serde(flatten)] tls: TlsCommon` (Vless/Vmess/Trojan/
/// Hysteria2/Tuic/Shadowtls/Anytls/Http) cannot use `#[serde(deny_unknown_fields)]`
/// — serde silently disables the check whenever `flatten` is present, since it
/// can no longer tell which fields "belong" to the parent versus the flattened
/// child. Typos inside those variants therefore still deserialize as `None`.
/// Structs without a flattened tls block do enforce `deny_unknown_fields`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "protocol", rename_all = "lowercase")]
pub enum ProtocolConfig {
    Vless(VlessConfig),
    Vmess(VmessConfig),
    Trojan(TrojanConfig),
    Shadowsocks(ShadowsocksConfig),
    Hysteria2(Hysteria2Config),
    Tuic(TuicConfig),
    Shadowtls(ShadowtlsConfig),
    Anytls(AnytlsConfig),
    Socks(SocksConfig),
    Http(HttpConfig),
    Ssh(SshConfig),
}

impl ProtocolConfig {
    pub fn protocol(&self) -> Protocol {
        match self {
            ProtocolConfig::Vless(_) => Protocol::Vless,
            ProtocolConfig::Vmess(_) => Protocol::Vmess,
            ProtocolConfig::Trojan(_) => Protocol::Trojan,
            ProtocolConfig::Shadowsocks(_) => Protocol::Shadowsocks,
            ProtocolConfig::Hysteria2(_) => Protocol::Hysteria2,
            ProtocolConfig::Tuic(_) => Protocol::Tuic,
            ProtocolConfig::Shadowtls(_) => Protocol::Shadowtls,
            ProtocolConfig::Anytls(_) => Protocol::Anytls,
            ProtocolConfig::Socks(_) => Protocol::Socks,
            ProtocolConfig::Http(_) => Protocol::Http,
            ProtocolConfig::Ssh(_) => Protocol::Ssh,
        }
    }

    fn tls_common(&self) -> Option<&TlsCommon> {
        match self {
            ProtocolConfig::Vmess(c) => Some(&c.tls),
            ProtocolConfig::Trojan(c) => Some(&c.tls),
            ProtocolConfig::Hysteria2(c) => Some(&c.tls),
            ProtocolConfig::Tuic(c) => Some(&c.tls),
            ProtocolConfig::Shadowtls(c) => Some(&c.tls),
            ProtocolConfig::Anytls(c) => Some(&c.tls),
            ProtocolConfig::Http(c) => Some(&c.tls),
            // VLESS keeps reality/ech flat on VlessConfig; no shared block.
            ProtocolConfig::Vless(_)
            | ProtocolConfig::Shadowsocks(_)
            | ProtocolConfig::Socks(_)
            | ProtocolConfig::Ssh(_) => None,
        }
    }

    pub(super) fn validate(&self) -> anyhow::Result<()> {
        match self {
            ProtocolConfig::Vless(c) => {
                if c.uuid.trim().is_empty() {
                    anyhow::bail!("vless.uuid must not be empty");
                }
                c.tls
                    .validate()
                    .map_err(|e| anyhow::anyhow!("vless: {e}"))?;
            }
            ProtocolConfig::Vmess(c) => {
                if c.uuid.trim().is_empty() {
                    anyhow::bail!("vmess.uuid must not be empty");
                }
            }
            ProtocolConfig::Trojan(c) => {
                if c.password.is_empty() {
                    anyhow::bail!("trojan.password must not be empty");
                }
            }
            ProtocolConfig::Shadowsocks(c) => {
                if c.password.is_empty() {
                    anyhow::bail!("shadowsocks.password must not be empty");
                }
            }
            ProtocolConfig::Hysteria2(c) => {
                if c.password.is_empty() {
                    anyhow::bail!("hysteria2.password must not be empty");
                }
            }
            ProtocolConfig::Tuic(c) => {
                if c.uuid.trim().is_empty() {
                    anyhow::bail!("tuic.uuid must not be empty");
                }
                if c.password.is_empty() {
                    anyhow::bail!("tuic.password must not be empty");
                }
            }
            ProtocolConfig::Shadowtls(c) => {
                if c.version == ShadowtlsVersion::V3 && c.password.is_empty() {
                    anyhow::bail!("shadowtls.password must not be empty for v3");
                }
                if c.ss_password.is_empty() {
                    anyhow::bail!(
                        "shadowtls.ss_password must not be empty (inner Shadowsocks detour)"
                    );
                }
            }
            ProtocolConfig::Anytls(c) => {
                if c.password.is_empty() {
                    anyhow::bail!("anytls.password must not be empty");
                }
            }
            ProtocolConfig::Socks(_) | ProtocolConfig::Http(_) => {}
            ProtocolConfig::Ssh(c) => {
                if c.user.trim().is_empty() {
                    anyhow::bail!("ssh.user must not be empty");
                }
            }
        }
        if let Some(tls) = self.tls_common() {
            tls.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::*;
    use crate::test_helpers::vless_cfg;
    use uuid::Uuid;

    #[test]
    fn config_rejects_unknown_field_in_shadowsocks_config() {
        // deny_unknown_fields on ShadowsocksConfig catches typos in the
        // per-protocol block. Verifies point 2 of the validation hardening.
        let json = r#"{
            "profiles": [{
                "name": "SS",
                "protocol": "shadowsocks",
                "address": "1.2.3.4",
                "port": 8388,
                "method": "aes-256-gcm",
                "password": "pw",
                "bogus": "typo"
            }]
        }"#;
        let result: Result<Config, _> = serde_json::from_str(json);
        assert!(result.is_err(), "Expected deny_unknown_fields to reject");
    }

    #[test]
    fn legacy_vless_json_deserializes_into_new_shape() {
        // A pre-v2 profiles.json: top-level `reality` / `ech` / `fingerprint`
        // alongside the new flat layout. `reality` and `ech` are picked up
        // by `#[serde(flatten)] tls: TlsCommon` automatically; `fingerprint`
        // is captured into `legacy_fingerprint` and promoted by migrate().
        let json = r#"{
            "schema_version": 1,
            "profiles": [{
                "id": "550e8400-e29b-41d4-a716-446655440000",
                "name": "Legacy",
                "protocol": "vless",
                "address": "1.1.1.1",
                "port": 443,
                "uuid": "legacy-uuid",
                "flow": "xtls-rprx-vision",
                "security": "reality",
                "reality": {
                    "public_key": "pk",
                    "short_id": "sid",
                    "server_name": "sni",
                    "spider_x": "/"
                },
                "ech": { "enabled": false },
                "transport_type": "grpc",
                "transport_service_name": "svc",
                "fingerprint": "chrome",
                "tags": ["legacy"]
            }]
        }"#;
        let mut config: Config = serde_json::from_str(json).unwrap();
        // Before migrate(): legacy_fingerprint holds the raw value, the new
        // slot is still empty.
        {
            let cfg = vless_cfg(&config.profiles[0]);
            assert_eq!(cfg.legacy_fingerprint.as_deref(), Some("chrome"));
            assert!(cfg.tls.utls_fingerprint.is_none());
            assert!(cfg.tls.reality.is_some(), "reality flattened through");
            assert!(cfg.tls.ech.is_some(), "ech flattened through");
        }
        config.migrate().unwrap();
        assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
        let p = &config.profiles[0];
        let cfg = vless_cfg(p);
        assert_eq!(cfg.uuid, "legacy-uuid");
        assert_eq!(cfg.flow, Some(Flow::XtlsRprxVision));
        assert_eq!(cfg.security, Some(Security::Reality));
        assert!(cfg.tls.reality.is_some());
        assert_eq!(cfg.transport_type, Some(TransportType::Grpc));
        assert_eq!(cfg.transport_service_name.as_deref(), Some("svc"));
        assert_eq!(cfg.tls.utls_fingerprint.as_deref(), Some("chrome"));
        assert!(
            cfg.legacy_fingerprint.is_none(),
            "migration must clear the legacy slot"
        );
        assert_eq!(p.tags, vec!["legacy".to_string()]);
    }

    #[test]
    fn vmess_profile_roundtrip() {
        let profile = Profile {
            id: Uuid::nil(),
            name: "VMess".to_string(),
            address: "1.1.1.1".to_string(),
            port: 443,
            config: ProtocolConfig::Vmess(VmessConfig {
                uuid: "vm-uuid".to_string(),
                alter_id: 0,
                security: VmessSecurity::Aes128Gcm,
                global_padding: None,
                tls: TlsCommon {
                    server_name: Some("sni".to_string()),
                    ech: Some(EchSettings {
                        enabled: true,
                        config: Vec::new(),
                    }),
                    ..TlsCommon::default()
                },
                transport: None,
            }),
            tags: Vec::new(),
            subscription_id: None,
        };
        let json = serde_json::to_string(&profile).unwrap();
        assert!(json.contains("\"protocol\":\"vmess\""));
        assert!(json.contains("\"ech\""));
        let restored: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(profile, restored);
    }
}

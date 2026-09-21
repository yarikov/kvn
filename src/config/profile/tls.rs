use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// REALITY security settings for XTLS Vision.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RealitySettings {
    #[serde(rename = "public_key")]
    pub public_key: String,
    #[serde(rename = "short_id")]
    pub short_id: String,
    #[serde(rename = "server_name")]
    pub server_name: String,
    #[serde(rename = "spider_x")]
    pub spider_x: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Security {
    #[default]
    None,
    Reality,
    Tls,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransportType {
    Grpc,
    Ws,
    Http,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Flow {
    #[default]
    None,
    #[serde(rename = "xtls-rprx-vision")]
    XtlsRprxVision,
}

/// TLS Encrypted Client Hello (ECH) configuration.
///
/// Maps onto sing-box's `tls.ech` block. When `config` is empty, sing-box
/// fetches the `ECHConfigList` from DNS HTTPS RR for the target server.
/// Mutually exclusive with REALITY (validated by [`Config::validate`](crate::config::profile::Config::validate)).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct EchSettings {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config: Vec<String>,
}

/// Shared TLS configuration for protocols that carry a TLS layer
/// (VMess, Trojan, ShadowTLS, AnyTLS, Hysteria2, TUIC).
///
/// VLESS keeps its TLS-related fields flat on [`VlessConfig`](crate::config::profile::VlessConfig) for
/// backward compatibility with existing `profiles.json` files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TlsCommon {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub insecure: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alpn: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utls_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reality: Option<RealitySettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ech: Option<EchSettings>,
}

impl TlsCommon {
    /// REALITY and ECH cannot be enabled simultaneously — REALITY uses its
    /// own SNI-cloaking mechanism that conflicts with ECH's `ECHConfigList`.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.reality.is_some() && self.ech.as_ref().is_some_and(|e| e.enabled) {
            anyhow::bail!("tls.reality and tls.ech are mutually exclusive");
        }
        Ok(())
    }
}

/// Transport layer configuration (ws / grpc / http / httpupgrade).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TransportConfig {
    #[serde(rename = "type")]
    pub kind: TransportType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub headers: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- TlsCommon::validate ----

    #[test]
    fn tls_common_validate_accepts_plain() {
        TlsCommon::default().validate().unwrap();
    }

    #[test]
    fn tls_common_validate_accepts_reality_only() {
        let tls = TlsCommon {
            reality: Some(RealitySettings {
                public_key: "pk".into(),
                short_id: "sid".into(),
                server_name: "sn".into(),
                spider_x: "/".into(),
            }),
            ..TlsCommon::default()
        };
        tls.validate().unwrap();
    }

    #[test]
    fn tls_common_validate_accepts_ech_only() {
        let tls = TlsCommon {
            ech: Some(EchSettings {
                enabled: true,
                ..EchSettings::default()
            }),
            ..TlsCommon::default()
        };
        tls.validate().unwrap();
    }

    #[test]
    fn tls_common_validate_accepts_reality_with_disabled_ech() {
        // The mutual-exclusion check fires only when ECH is *enabled*.
        let tls = TlsCommon {
            reality: Some(RealitySettings {
                public_key: "pk".into(),
                short_id: "sid".into(),
                server_name: "sn".into(),
                spider_x: "/".into(),
            }),
            ech: Some(EchSettings {
                enabled: false,
                ..EchSettings::default()
            }),
            ..TlsCommon::default()
        };
        tls.validate().unwrap();
    }

    #[test]
    fn tls_common_validate_rejects_reality_with_enabled_ech() {
        let tls = TlsCommon {
            reality: Some(RealitySettings {
                public_key: "pk".into(),
                short_id: "sid".into(),
                server_name: "sn".into(),
                spider_x: "/".into(),
            }),
            ech: Some(EchSettings {
                enabled: true,
                ..EchSettings::default()
            }),
            ..TlsCommon::default()
        };
        let err = tls.validate().unwrap_err().to_string();
        assert!(err.contains("mutually exclusive"));
    }
}

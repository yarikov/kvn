use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ConfigDiagnostic;

/// REALITY security settings for XTLS Vision.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq, JsonSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Security {
    #[default]
    None,
    Reality,
    Tls,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransportType {
    Grpc,
    Ws,
    Http,
    #[serde(rename = "httpupgrade")]
    HttpUpgrade,
    Quic,
    #[serde(untagged)]
    Other(String),
}

impl TransportType {
    pub fn as_str(&self) -> &str {
        match self {
            TransportType::Grpc => "grpc",
            TransportType::Ws => "ws",
            TransportType::Http => "http",
            TransportType::HttpUpgrade => "httpupgrade",
            TransportType::Quic => "quic",
            TransportType::Other(kind) => kind,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EchSettings {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config: Vec<String>,
}

const ECH_CONFIGS_PEM_BEGIN: &str = "-----BEGIN ECH CONFIGS-----";
const ECH_CONFIGS_PEM_END: &str = "-----END ECH CONFIGS-----";

impl EchSettings {
    pub fn is_dns_query_link_value(value: &str) -> bool {
        value.contains("://")
    }

    pub fn from_link_value(value: &str, enabled: bool) -> Self {
        let config = if Self::is_dns_query_link_value(value) {
            Vec::new()
        } else {
            vec![
                ECH_CONFIGS_PEM_BEGIN.to_string(),
                value.to_string(),
                ECH_CONFIGS_PEM_END.to_string(),
            ]
        };
        Self { enabled, config }
    }

    pub fn link_value(&self) -> Option<String> {
        let body: String = self
            .config
            .iter()
            .flat_map(|entry| entry.lines())
            .map(str::trim)
            .filter(|line| *line != ECH_CONFIGS_PEM_BEGIN && *line != ECH_CONFIGS_PEM_END)
            .collect();
        (!body.is_empty()).then_some(body)
    }
}

/// Shared TLS configuration for protocols that carry a TLS layer
/// (VMess, Trojan, ShadowTLS, AnyTLS, Hysteria2, TUIC).
///
/// VLESS keeps its TLS-related fields flat on [`VlessConfig`](crate::config::profile::VlessConfig) for
/// backward compatibility with existing `profiles.json` files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema)]
pub struct TlsCommon {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub insecure: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disable_sni: bool,
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
    /// `TlsCommon::default()` represents "no TLS" — we treat a TLS block as
    /// enabled when the user supplied at least one TLS-related field.
    pub fn is_configured(&self) -> bool {
        self.server_name.is_some()
            || self.insecure
            || self.disable_sni
            || !self.alpn.is_empty()
            || self.utls_fingerprint.is_some()
            || self.reality.is_some()
            || self.ech.as_ref().is_some_and(|e| e.enabled)
    }

    /// REALITY and ECH cannot be enabled simultaneously — REALITY uses its
    /// own SNI-cloaking mechanism that conflicts with ECH's `ECHConfigList`.
    pub fn diagnostics(&self) -> Vec<ConfigDiagnostic> {
        if self.reality.is_some() && self.ech.as_ref().is_some_and(|e| e.enabled) {
            return vec![ConfigDiagnostic::new(
                "/ech",
                "tls.reality and tls.ech are mutually exclusive",
            )];
        }
        Vec::new()
    }
}

/// Transport layer configuration (ws / grpc / http / httpupgrade / quic).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub early_data: Option<WebSocketEarlyData>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebSocketEarlyData {
    pub max_bytes: u32,
    pub header_name: String,
}

const EARLY_DATA_PATH_KEY: &str = "ed";
const EARLY_DATA_PROTOCOL_HEADER: &str = "Sec-WebSocket-Protocol";

fn split_path_early_data(path: &str) -> (String, Option<u32>) {
    let Some((base, query)) = path.split_once('?') else {
        return (path.to_string(), None);
    };
    let mut max_bytes = None;
    let rest: Vec<&str> = query
        .split('&')
        .filter(|pair| match pair.split_once('=') {
            Some((EARLY_DATA_PATH_KEY, value)) if max_bytes.is_none() => {
                max_bytes = value.parse::<u32>().ok();
                max_bytes.is_none()
            }
            _ => true,
        })
        .collect();
    if max_bytes.is_none() {
        return (path.to_string(), None);
    }
    let path = if rest.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", rest.join("&"))
    };
    (path, max_bytes)
}

impl TransportConfig {
    pub(crate) fn take_path_early_data(&mut self) {
        if self.kind != TransportType::Ws || self.early_data.is_some() {
            return;
        }
        let Some(path) = self.path.as_deref() else {
            return;
        };
        let (path, Some(max_bytes)) = split_path_early_data(path) else {
            return;
        };
        self.path = Some(path);
        self.early_data = Some(WebSocketEarlyData {
            max_bytes,
            header_name: EARLY_DATA_PROTOCOL_HEADER.to_string(),
        });
    }

    pub(crate) fn identity_path(&self) -> String {
        let path = self.path.as_deref().unwrap_or("");
        if self.kind != TransportType::Ws {
            return path.to_string();
        }
        let (path, early_in_path) = split_path_early_data(path);
        let early = self
            .early_data
            .as_ref()
            .filter(|early| early.header_name == EARLY_DATA_PROTOCOL_HEADER)
            .map(|early| early.max_bytes)
            .or(early_in_path);
        match early {
            Some(max_bytes) => format!("{path}|{EARLY_DATA_PATH_KEY}={max_bytes}"),
            None => path,
        }
    }

    pub fn link_path(&self) -> Option<String> {
        let early_data = self
            .early_data
            .as_ref()
            .filter(|early| early.header_name == EARLY_DATA_PROTOCOL_HEADER);
        match (self.path.as_deref(), early_data) {
            (path, None) => path.map(str::to_string),
            (path, Some(early)) => {
                let path = path.unwrap_or("/");
                let separator = if path.contains('?') { '&' } else { '?' };
                Some(format!(
                    "{path}{separator}{EARLY_DATA_PATH_KEY}={}",
                    early.max_bytes
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- TlsCommon::diagnostics ----

    #[test]
    fn tls_common_validate_accepts_plain() {
        assert_eq!(TlsCommon::default().diagnostics(), []);
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
        assert_eq!(tls.diagnostics(), []);
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
        assert_eq!(tls.diagnostics(), []);
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
        assert_eq!(tls.diagnostics(), []);
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
        let diagnostic = crate::test_helpers::single_diagnostic(tls.diagnostics());
        assert_eq!(diagnostic.pointer, "/ech");
        assert!(diagnostic.message.contains("mutually exclusive"));
    }

    #[test]
    fn ech_link_value_reads_a_multiline_pem_entry() {
        let ech = EchSettings {
            enabled: true,
            config: vec![
                "-----BEGIN ECH CONFIGS-----\nAEb+DQBC\nAAAgACB+\n-----END ECH CONFIGS-----".into(),
            ],
        };
        assert_eq!(ech.link_value().as_deref(), Some("AEb+DQBCAAAgACB+"));
    }
}

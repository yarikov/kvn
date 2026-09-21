use serde::{Deserialize, Serialize};

/// VMess encryption cipher. Sing-box 1.12 still accepts `auto`; we forbid
/// the legacy stream cipher `aes-128-cfb`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum VmessSecurity {
    #[default]
    Auto,
    None,
    Zero,
    #[serde(rename = "aes-128-gcm")]
    Aes128Gcm,
    #[serde(rename = "chacha20-poly1305")]
    Chacha20Poly1305,
}

impl VmessSecurity {
    #[allow(dead_code)] // consumed by per-protocol outbound builders landing in PR2
    pub fn as_str(self) -> &'static str {
        match self {
            VmessSecurity::Auto => "auto",
            VmessSecurity::None => "none",
            VmessSecurity::Zero => "zero",
            VmessSecurity::Aes128Gcm => "aes-128-gcm",
            VmessSecurity::Chacha20Poly1305 => "chacha20-poly1305",
        }
    }
}

/// Shadowsocks AEAD-2022 + AEAD ciphers supported by sing-box 1.12.
/// Legacy stream ciphers (e.g. `aes-128-cfb`) are intentionally excluded.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ShadowsocksCipher {
    #[default]
    #[serde(rename = "chacha20-ietf-poly1305")]
    Chacha20IetfPoly1305,
    #[serde(rename = "aes-128-gcm")]
    Aes128Gcm,
    #[serde(rename = "aes-256-gcm")]
    Aes256Gcm,
    #[serde(rename = "2022-blake3-aes-128-gcm")]
    Blake3Aes128Gcm,
    #[serde(rename = "2022-blake3-aes-256-gcm")]
    Blake3Aes256Gcm,
    #[serde(rename = "2022-blake3-chacha20-poly1305")]
    Blake3Chacha20Poly1305,
    None,
}

impl ShadowsocksCipher {
    #[allow(dead_code)] // consumed by per-protocol outbound builders landing in PR2
    pub fn as_str(self) -> &'static str {
        match self {
            ShadowsocksCipher::Chacha20IetfPoly1305 => "chacha20-ietf-poly1305",
            ShadowsocksCipher::Aes128Gcm => "aes-128-gcm",
            ShadowsocksCipher::Aes256Gcm => "aes-256-gcm",
            ShadowsocksCipher::Blake3Aes128Gcm => "2022-blake3-aes-128-gcm",
            ShadowsocksCipher::Blake3Aes256Gcm => "2022-blake3-aes-256-gcm",
            ShadowsocksCipher::Blake3Chacha20Poly1305 => "2022-blake3-chacha20-poly1305",
            ShadowsocksCipher::None => "none",
        }
    }
}

/// Hysteria2 obfuscation. Sing-box 1.12+ supports the `salamander` type
/// (legacy top-level `obfs_password` is rejected).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Hysteria2Obfs {
    #[serde(rename = "type")]
    pub kind: Hysteria2ObfsType,
    pub password: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Hysteria2ObfsType {
    #[default]
    Salamander,
}

/// TUIC v5 congestion control algorithm.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TuicCongestion {
    #[default]
    Bbr,
    Cubic,
    NewReno,
}

impl TuicCongestion {
    #[allow(dead_code)] // consumed by per-protocol outbound builders landing in PR2
    pub fn as_str(self) -> &'static str {
        match self {
            TuicCongestion::Bbr => "bbr",
            TuicCongestion::Cubic => "cubic",
            TuicCongestion::NewReno => "new_reno",
        }
    }
}

/// TUIC v5 UDP relay mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TuicUdpRelayMode {
    #[default]
    Native,
    Quic,
}

impl TuicUdpRelayMode {
    #[allow(dead_code)] // consumed by per-protocol outbound builders landing in PR2
    pub fn as_str(self) -> &'static str {
        match self {
            TuicUdpRelayMode::Native => "native",
            TuicUdpRelayMode::Quic => "quic",
        }
    }
}

/// ShadowTLS protocol version. v1/v2 are deprecated; v3 is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShadowtlsVersion {
    V1,
    V2,
    #[default]
    V3,
}

impl ShadowtlsVersion {
    pub fn as_u8(self) -> u8 {
        match self {
            ShadowtlsVersion::V1 => 1,
            ShadowtlsVersion::V2 => 2,
            ShadowtlsVersion::V3 => 3,
        }
    }
}

impl Serialize for ShadowtlsVersion {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_u8(self.as_u8())
    }
}

impl<'de> Deserialize<'de> for ShadowtlsVersion {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let v = u8::deserialize(de)?;
        match v {
            1 => Ok(Self::V1),
            2 => Ok(Self::V2),
            3 => Ok(Self::V3),
            other => Err(serde::de::Error::custom(format!(
                "unknown ShadowTLS version {}",
                other
            ))),
        }
    }
}

/// SOCKS proxy version.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SocksVersion {
    #[serde(rename = "4")]
    V4,
    #[serde(rename = "4a")]
    V4a,
    #[default]
    #[serde(rename = "5")]
    V5,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ShadowtlsVersion ----

    #[test]
    fn shadowtls_version_as_u8_maps_each_variant() {
        assert_eq!(ShadowtlsVersion::V1.as_u8(), 1);
        assert_eq!(ShadowtlsVersion::V2.as_u8(), 2);
        assert_eq!(ShadowtlsVersion::V3.as_u8(), 3);
    }

    #[test]
    fn shadowtls_version_serializes_as_numeric() {
        assert_eq!(serde_json::to_string(&ShadowtlsVersion::V1).unwrap(), "1");
        assert_eq!(serde_json::to_string(&ShadowtlsVersion::V2).unwrap(), "2");
        assert_eq!(serde_json::to_string(&ShadowtlsVersion::V3).unwrap(), "3");
    }

    #[test]
    fn shadowtls_version_deserializes_each_known_value() {
        let v: ShadowtlsVersion = serde_json::from_str("1").unwrap();
        assert_eq!(v, ShadowtlsVersion::V1);
        let v: ShadowtlsVersion = serde_json::from_str("2").unwrap();
        assert_eq!(v, ShadowtlsVersion::V2);
        let v: ShadowtlsVersion = serde_json::from_str("3").unwrap();
        assert_eq!(v, ShadowtlsVersion::V3);
    }

    #[test]
    fn shadowtls_version_rejects_unknown_value() {
        let err = serde_json::from_str::<ShadowtlsVersion>("4").unwrap_err();
        assert!(err.to_string().contains("unknown ShadowTLS version"));
    }

    #[test]
    fn shadowtls_version_default_is_v3() {
        assert_eq!(ShadowtlsVersion::default(), ShadowtlsVersion::V3);
    }
}

use serde::{Deserialize, Serialize};

/// Supported VPN protocols.
///
/// This enum is the discriminant for [`ProtocolConfig`](crate::config::profile::ProtocolConfig) and is also used as
/// a lightweight label for UI rendering. Per-protocol fields live on the
/// corresponding [`ProtocolConfig`](crate::config::profile::ProtocolConfig) variant, not on this enum.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Vless,
    Vmess,
    Trojan,
    Shadowsocks,
    Hysteria2,
    Tuic,
    Shadowtls,
    Anytls,
    Socks,
    Http,
    Ssh,
}

impl Protocol {
    /// Lowercase identifier used in JSON serialization and internal dispatch.
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Vless => "vless",
            Protocol::Vmess => "vmess",
            Protocol::Trojan => "trojan",
            Protocol::Shadowsocks => "shadowsocks",
            Protocol::Hysteria2 => "hysteria2",
            Protocol::Tuic => "tuic",
            Protocol::Shadowtls => "shadowtls",
            Protocol::Anytls => "anytls",
            Protocol::Socks => "socks",
            Protocol::Http => "http",
            Protocol::Ssh => "ssh",
        }
    }

    /// Short label for the UI protocol column (fits within 6 characters).
    pub fn ui_label(self) -> &'static str {
        match self {
            Protocol::Shadowsocks => "ss",
            Protocol::Hysteria2 => "hy2",
            Protocol::Shadowtls => "stls",
            other => other.as_str(),
        }
    }
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_display() {
        assert_eq!(format!("{}", Protocol::Vless), "vless");
    }
}

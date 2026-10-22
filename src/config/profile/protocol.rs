use serde::{Deserialize, Serialize};

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
    Naive,
    Socks,
    Http,
    Ssh,
}

impl Protocol {
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
            Protocol::Naive => "naive",
            Protocol::Socks => "socks",
            Protocol::Http => "http",
            Protocol::Ssh => "ssh",
        }
    }

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

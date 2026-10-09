//! Share-link URI parsing and encoding.
//!
//! Each supported VPN protocol has a canonical URI form (`vless://...`,
//! `vmess://...`, etc.) that users paste into the TUI or that subscription
//! servers return. This module owns both directions of the translation —
//! `parse_share_link` builds a [`Profile`](crate::config::profile::Profile) from a URI; `encode_share_link`
//! produces a URI from a [`Profile`](crate::config::profile::Profile).

mod encode;
mod parse;

pub use encode::encode_share_link;
pub(crate) use parse::decode_b64_lenient;
pub use parse::parse_share_link;

/// All share-link URI schemes recognised by [`parse_share_link`].
/// Used both for prefix dispatch and by `config::subscription` to detect
/// subscription bodies after base64 decoding.
pub const SUPPORTED_SHARE_SCHEMES: &[&str] = &[
    "vless://",
    "vmess://",
    "trojan://",
    "ss://",
    "hysteria2://",
    "hy2://",
    "tuic://",
    "socks://",
    "socks5://",
    "socks5h://",
    "socks4://",
    "socks4a://",
    "http://",
    "https://",
    "ssh://",
    "anytls://",
    "shadowtls://",
    "naive+https://",
    "naive+quic://",
    "http2://",
];

pub fn is_http_proxy_link(link: &str) -> bool {
    let Ok(url) = url::Url::parse(link) else {
        return false;
    };
    let is_http = matches!(url.scheme(), "http" | "https");
    let addresses_host_only = url.path() == "/" && url.query().is_none();
    let has_proxy_marker = !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some_and(|name| !name.is_empty())
        || (url.scheme() == "http" && url.port().is_some());
    is_http && addresses_host_only && has_proxy_marker
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_proxy_links_are_recognized() {
        for link in [
            "http://user:pass@1.2.3.4:8080#Office",
            "http://1.2.3.4:3128",
            "https://proxy.example#HTTPS",
            "https://user@proxy.example/",
            "https://user:pass@proxy.example:8443",
            "https://proxy.example:8443#Proxy",
        ] {
            assert!(is_http_proxy_link(link), "{link}");
        }
    }

    #[test]
    fn subscription_urls_are_not_proxy_links() {
        for link in [
            "https://192.0.2.10:2096/sub/test-token",
            "https://sub.example.com:2096",
            "https://sub.example.com:2096/",
            "https://sub.example.com:443",
            "https://[2001:db8::1]:2096",
            "https://sub.example.com:2096#",
            "https://example.com/?token=x",
            "https://example.com",
            "socks5://1.2.3.4:1080#Socks",
            "not a url",
        ] {
            assert!(!is_http_proxy_link(link), "{link}");
        }
    }
}

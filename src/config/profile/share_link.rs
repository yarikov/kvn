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
    "http://",
    "https://",
    "ssh://",
    "anytls://",
    "shadowtls://",
];

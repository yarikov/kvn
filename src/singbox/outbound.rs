//! Per-protocol outbound builders for sing-box 1.12 JSON config.
//!
//! `build_outbound` in the parent module dispatches on `ProtocolConfig` and
//! calls into one of these `build_*_outbound` helpers. They each take the
//! protocol-specific config plus the parent `Profile` (for shared fields
//! like address/port) and return a single outbound JSON object — except
//! ShadowTLS, which emits a wrapper + detour pair.
//!
//! `build_tls_block` and `build_transport_block` are the shared helpers
//! used by every TLS-capable protocol.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Value, json};

use crate::config::profile::{
    AnytlsConfig, HttpConfig, Hysteria2Config, Profile, ProtocolConfig, Security,
    ShadowsocksConfig, ShadowtlsConfig, ShadowtlsVersion, SocksConfig, SocksVersion, SshConfig,
    TlsCommon, TransportConfig, TransportType, TrojanConfig, TuicConfig, VlessConfig, VmessConfig,
};

/// Render the sing-box 1.12 `tls` block from [`TlsCommon`].
/// `default_sni` is used when `tls.server_name` is unset (typically the
/// profile address). `default_alpn` is used when `tls.alpn` is empty
/// (e.g. `["h3"]` for QUIC-based protocols).
fn build_tls_block(default_sni: &str, default_alpn: &[&str], tls: &TlsCommon) -> Value {
    let mut block = Map::new();
    block.insert("enabled".to_string(), json!(true));
    let sni = tls.server_name.as_deref().unwrap_or(default_sni);
    block.insert("server_name".to_string(), json!(sni));
    if tls.insecure {
        block.insert("insecure".to_string(), json!(true));
    }
    let alpn: Vec<&str> = if tls.alpn.is_empty() {
        default_alpn.to_vec()
    } else {
        tls.alpn.iter().map(String::as_str).collect()
    };
    if !alpn.is_empty() {
        block.insert("alpn".to_string(), json!(alpn));
    }
    if let Some(fp) = tls.utls_fingerprint.as_deref() {
        block.insert(
            "utls".to_string(),
            json!({ "enabled": true, "fingerprint": fp }),
        );
    }
    if let Some(reality) = &tls.reality {
        block.insert("server_name".to_string(), json!(reality.server_name));
        block.insert(
            "reality".to_string(),
            json!({
                "enabled": true,
                "public_key": reality.public_key,
                "short_id": reality.short_id,
            }),
        );
    }
    if let Some(ech) = tls.ech.as_ref().filter(|e| e.enabled) {
        let mut ech_block = json!({ "enabled": true });
        if !ech.config.is_empty() {
            ech_block["config"] = json!(ech.config);
        }
        block.insert("ech".to_string(), ech_block);
    }
    Value::Object(block)
}

/// Render the sing-box 1.12 `transport` block from [`TransportConfig`].
fn build_transport_block(
    t: &TransportConfig,
    tls_enabled: bool,
    params: &BTreeMap<String, Value>,
) -> anyhow::Result<Value> {
    if tls_enabled && t.kind == TransportType::Http {
        reject_tcp_http_header_over_tls(params)?;
    }
    let mut obj = Map::new();
    obj.insert("type".to_string(), json!(t.kind.as_str()));
    match t.kind {
        TransportType::Ws => {
            insert_path(&mut obj, t);
            insert_headers(&mut obj, headers_with_host(t));
        }
        TransportType::Http => {
            let hosts = t.host.as_deref().map(split_hosts).unwrap_or_default();
            if !hosts.is_empty() {
                obj.insert("host".to_string(), json!(hosts));
            }
            insert_path(&mut obj, t);
            insert_headers(&mut obj, t.headers.clone());
        }
        TransportType::Grpc => {
            if let Some(service_name) = t.service_name.as_deref() {
                obj.insert("service_name".to_string(), json!(service_name));
            }
            obj.insert("idle_timeout".to_string(), json!("15s"));
            obj.insert("ping_timeout".to_string(), json!("15s"));
        }
        TransportType::HttpUpgrade => {
            if let Some(host) = t.host.as_deref() {
                obj.insert("host".to_string(), json!(host));
            }
            insert_path(&mut obj, t);
            insert_headers(&mut obj, t.headers.clone());
        }
        TransportType::Quic => reject_quic_options_sing_box_lacks(params)?,
        TransportType::Other(_) => {}
    }
    Ok(Value::Object(obj))
}

fn drop_tls_when_disabled(outbound: &mut Value, security: Option<&Security>) {
    if security == Some(&Security::None)
        && let Some(object) = outbound.as_object_mut()
    {
        object.remove("tls");
    }
}

fn tls_enabled(outbound: &Value) -> bool {
    outbound["tls"]["enabled"] == json!(true)
}

fn reject_quic_options_sing_box_lacks(params: &BTreeMap<String, Value>) -> anyhow::Result<()> {
    for option in ["quicSecurity", "headerType"] {
        if let Some(value) = enabled_option(params, option) {
            anyhow::bail!("sing-box QUIC transport does not support {option}={value}");
        }
    }
    Ok(())
}

fn reject_tcp_http_header_over_tls(params: &BTreeMap<String, Value>) -> anyhow::Result<()> {
    if enabled_option(params, "headerType").is_some() {
        anyhow::bail!(
            "sing-box cannot send TCP HTTP header obfuscation over TLS: its HTTP transport uses HTTP/2 with TLS"
        );
    }
    Ok(())
}

fn enabled_option<'a>(params: &'a BTreeMap<String, Value>, option: &str) -> Option<&'a str> {
    params
        .get(option)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && *value != "none")
}

fn insert_path(obj: &mut Map<String, Value>, t: &TransportConfig) {
    if let Some(path) = t.path.as_deref() {
        obj.insert("path".to_string(), json!(path));
    }
}

fn insert_headers(obj: &mut Map<String, Value>, headers: HashMap<String, String>) {
    if !headers.is_empty() {
        obj.insert("headers".to_string(), json!(headers));
    }
}

fn headers_with_host(t: &TransportConfig) -> HashMap<String, String> {
    let mut headers = t.headers.clone();
    let has_host_header = headers.keys().any(|name| name.eq_ignore_ascii_case("host"));
    if let Some(host) = t.host.as_deref()
        && !has_host_header
    {
        headers.insert("Host".to_string(), host.to_string());
    }
    headers
}

fn split_hosts(hosts: &str) -> Vec<&str> {
    hosts
        .split(',')
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .collect()
}

/// Build VLESS outbound with optional REALITY / XTLS Vision / ECH.
pub(super) fn build_vless_outbound(profile: &Profile, cfg: &VlessConfig) -> anyhow::Result<Value> {
    if let Some(encryption) = enabled_option(&profile.share_link_params, "encryption") {
        anyhow::bail!("sing-box does not support VLESS Encryption (encryption={encryption})");
    }
    // REALITY requires a utls fingerprint to be useful; preserve the
    // pre-v2 default of "chrome" when the profile didn't specify one.
    let tls_input = if cfg.tls.reality.is_some() && cfg.tls.utls_fingerprint.is_none() {
        let mut t = cfg.tls.clone();
        t.utls_fingerprint = Some("chrome".to_string());
        std::borrow::Cow::Owned(t)
    } else {
        std::borrow::Cow::Borrowed(&cfg.tls)
    };
    let mut outbound = json!({
        "type": "vless",
        "tag": "proxy",
        "server": profile.address,
        "server_port": profile.port,
        "uuid": cfg.uuid,
        "packet_encoding": "xudp",
        "tls": build_tls_block(&profile.address, &[], tls_input.as_ref()),
    });
    drop_tls_when_disabled(&mut outbound, cfg.security.as_ref());

    if let Some(ref flow) = cfg.flow {
        outbound["flow"] = json!(flow);
    }

    if let Some(transport) = cfg.transport.as_ref() {
        outbound["transport"] = build_transport_block(
            transport,
            tls_enabled(&outbound),
            &profile.share_link_params,
        )?;
    }

    Ok(outbound)
}

/// Build VMess outbound (sing-box 1.12: explicit `security` cipher, no
/// deprecated `aes-128-cfb`; `alter_id: 0` for AEAD-only mode).
pub(super) fn build_vmess_outbound(profile: &Profile, cfg: &VmessConfig) -> anyhow::Result<Value> {
    let mut outbound = json!({
        "type": "vmess",
        "tag": "proxy",
        "server": profile.address,
        "server_port": profile.port,
        "uuid": cfg.uuid,
        "security": cfg.security.as_str(),
        "alter_id": cfg.alter_id,
        "packet_encoding": "xudp",
        "tls": build_tls_block(&profile.address, &[], &cfg.tls),
    });
    drop_tls_when_disabled(&mut outbound, cfg.stream_security.as_ref());
    if let Some(padding) = cfg.global_padding {
        outbound["global_padding"] = json!(padding);
    }
    if let Some(transport) = cfg.transport.as_ref() {
        outbound["transport"] = build_transport_block(
            transport,
            tls_enabled(&outbound),
            &profile.share_link_params,
        )?;
    }
    Ok(outbound)
}

/// Build Trojan outbound.
pub(super) fn build_trojan_outbound(
    profile: &Profile,
    cfg: &TrojanConfig,
) -> anyhow::Result<Value> {
    let mut outbound = json!({
        "type": "trojan",
        "tag": "proxy",
        "server": profile.address,
        "server_port": profile.port,
        "password": cfg.password,
        "tls": build_tls_block(&profile.address, &[], &cfg.tls),
    });
    if let Some(transport) = cfg.transport.as_ref() {
        outbound["transport"] = build_transport_block(
            transport,
            tls_enabled(&outbound),
            &profile.share_link_params,
        )?;
    }
    Ok(outbound)
}

/// Build Shadowsocks outbound (AEAD/AEAD-2022 ciphers only).
pub(super) fn build_shadowsocks_outbound(
    profile: &Profile,
    cfg: &ShadowsocksConfig,
) -> anyhow::Result<Value> {
    let mut outbound = json!({
        "type": "shadowsocks",
        "tag": "proxy",
        "server": profile.address,
        "server_port": profile.port,
        "method": cfg.method.as_str(),
        "password": cfg.password,
    });
    if let Some(plugin) = enabled_option(&profile.share_link_params, "plugin") {
        let (name, options) = plugin.split_once(';').unwrap_or((plugin, ""));
        outbound["plugin"] = json!(name);
        if !options.is_empty() {
            outbound["plugin_opts"] = json!(options);
        }
    }
    Ok(outbound)
}

/// Build Hysteria2 outbound.
///
/// QUIC-based; ALPN defaults to `["h3"]` when not explicitly set.
/// Uses the sing-box 1.12 nested `obfs: { type, password }` form (the
/// legacy top-level `obfs_password` field is not emitted).
pub(super) fn build_hysteria2_outbound(
    profile: &Profile,
    cfg: &Hysteria2Config,
) -> anyhow::Result<Value> {
    let mut outbound = json!({
        "type": "hysteria2",
        "tag": "proxy",
        "server": profile.address,
        "server_port": profile.port,
        "password": cfg.password,
        "tls": build_tls_block(&profile.address, &["h3"], &cfg.tls),
    });
    if let Some(up) = cfg.up_mbps {
        outbound["up_mbps"] = json!(up);
    }
    if let Some(down) = cfg.down_mbps {
        outbound["down_mbps"] = json!(down);
    }
    if let Some(obfs) = cfg.obfs.as_ref() {
        outbound["obfs"] = json!({
            "type": "salamander",
            "password": obfs.password,
        });
        let _ = &obfs.kind; // single supported type today; kept for forward compat
    }
    let port_ranges = enabled_option(&profile.share_link_params, "mport")
        .map(hysteria2_port_ranges)
        .unwrap_or_default();
    if !port_ranges.is_empty() {
        if let Some(object) = outbound.as_object_mut() {
            object.remove("server_port");
        }
        outbound["server_ports"] = json!(port_ranges);
    }
    Ok(outbound)
}

fn hysteria2_port_ranges(ports: &str) -> Vec<String> {
    ports
        .split(',')
        .map(str::trim)
        .filter(|range| !range.is_empty())
        .map(|range| match range.replace('-', ":") {
            range if range.contains(':') => range,
            port => format!("{port}:{port}"),
        })
        .collect()
}

/// Build TUIC v5 outbound.
pub(super) fn build_tuic_outbound(profile: &Profile, cfg: &TuicConfig) -> anyhow::Result<Value> {
    let mut outbound = json!({
        "type": "tuic",
        "tag": "proxy",
        "server": profile.address,
        "server_port": profile.port,
        "uuid": cfg.uuid,
        "password": cfg.password,
        "congestion_control": cfg.congestion_control.as_str(),
        "udp_relay_mode": cfg.udp_relay_mode.as_str(),
        "tls": build_tls_block(&profile.address, &["h3"], &cfg.tls),
    });
    if cfg.zero_rtt_handshake {
        outbound["zero_rtt_handshake"] = json!(true);
    }
    Ok(outbound)
}

/// Build the ShadowTLS wrapper + inner Shadowsocks detour pair.
/// The Shadowsocks outbound is tagged `proxy` (referenced by routing rules);
/// the ShadowTLS outbound is tagged internally and chained via `detour`.
pub(super) fn build_shadowtls_outbounds(
    profile: &Profile,
    cfg: &ShadowtlsConfig,
) -> anyhow::Result<Vec<Value>> {
    const SHADOWTLS_TAG: &str = "shadowtls-wrap";

    let mut shadowtls = json!({
        "type": "shadowtls",
        "tag": SHADOWTLS_TAG,
        "server": profile.address,
        "server_port": profile.port,
        "version": cfg.version.as_u8(),
        "tls": build_tls_block(&profile.address, &[], &cfg.tls),
    });
    if cfg.version == ShadowtlsVersion::V3 {
        shadowtls["password"] = json!(cfg.password);
    }

    let shadowsocks = json!({
        "type": "shadowsocks",
        "tag": "proxy",
        "server": profile.address,
        "server_port": profile.port,
        "method": cfg.method.as_str(),
        "password": cfg.ss_password,
        "detour": SHADOWTLS_TAG,
    });

    Ok(vec![shadowtls, shadowsocks])
}

/// Build AnyTLS outbound.
pub(super) fn build_anytls_outbound(
    profile: &Profile,
    cfg: &AnytlsConfig,
) -> anyhow::Result<Value> {
    let mut outbound = json!({
        "type": "anytls",
        "tag": "proxy",
        "server": profile.address,
        "server_port": profile.port,
        "password": cfg.password,
        "tls": build_tls_block(&profile.address, &[], &cfg.tls),
    });
    if let Some(v) = cfg.idle_session_check_interval.as_deref() {
        outbound["idle_session_check_interval"] = json!(v);
    }
    if let Some(v) = cfg.idle_session_timeout.as_deref() {
        outbound["idle_session_timeout"] = json!(v);
    }
    Ok(outbound)
}

/// Build SOCKS outbound (no TLS layer in sing-box; use ShadowTLS for that).
pub(super) fn build_socks_outbound(profile: &Profile, cfg: &SocksConfig) -> anyhow::Result<Value> {
    let mut outbound = json!({
        "type": "socks",
        "tag": "proxy",
        "server": profile.address,
        "server_port": profile.port,
        "version": socks_version_str(&cfg.version),
    });
    if let Some(user) = cfg.username.as_deref() {
        outbound["username"] = json!(user);
    }
    if let Some(pass) = cfg.password.as_deref() {
        outbound["password"] = json!(pass);
    }
    Ok(outbound)
}

fn socks_version_str(v: &crate::config::profile::SocksVersion) -> &'static str {
    use crate::config::profile::SocksVersion::*;
    match v {
        V4 => "4",
        V4a => "4a",
        V5 => "5",
    }
}

/// Build HTTP CONNECT outbound (TLS optional via [`TlsCommon`]).
pub(super) fn build_http_outbound(profile: &Profile, cfg: &HttpConfig) -> anyhow::Result<Value> {
    let mut outbound = json!({
        "type": "http",
        "tag": "proxy",
        "server": profile.address,
        "server_port": profile.port,
    });
    if let Some(user) = cfg.username.as_deref() {
        outbound["username"] = json!(user);
    }
    if let Some(pass) = cfg.password.as_deref() {
        outbound["password"] = json!(pass);
    }
    if cfg.tls.is_configured() {
        outbound["tls"] = build_tls_block(&profile.address, &[], &cfg.tls);
    }
    Ok(outbound)
}

/// Build SSH outbound.
pub(super) fn build_ssh_outbound(profile: &Profile, cfg: &SshConfig) -> anyhow::Result<Value> {
    let mut outbound = json!({
        "type": "ssh",
        "tag": "proxy",
        "server": profile.address,
        "server_port": profile.port,
        "user": cfg.user,
    });
    if let Some(pass) = cfg.password.as_deref() {
        outbound["password"] = json!(pass);
    }
    if let Some(pk) = cfg.private_key.as_deref() {
        outbound["private_key"] = json!(pk);
    }
    if let Some(pkp) = cfg.private_key_path.as_deref() {
        outbound["private_key_path"] = json!(pkp);
    }
    if let Some(passphrase) = cfg.private_key_passphrase.as_deref() {
        outbound["private_key_passphrase"] = json!(passphrase);
    }
    if !cfg.host_key.is_empty() {
        outbound["host_key"] = json!(cfg.host_key);
    }
    if !cfg.host_key_algorithms.is_empty() {
        outbound["host_key_algorithms"] = json!(cfg.host_key_algorithms);
    }
    Ok(outbound)
}

pub(super) fn proxy_carries_udp(config: &ProtocolConfig) -> bool {
    match config {
        ProtocolConfig::Http(_) | ProtocolConfig::Ssh(_) | ProtocolConfig::Shadowtls(_) => false,
        ProtocolConfig::Socks(cfg) => cfg.version == SocksVersion::V5,
        ProtocolConfig::Vless(_)
        | ProtocolConfig::Vmess(_)
        | ProtocolConfig::Trojan(_)
        | ProtocolConfig::Shadowsocks(_)
        | ProtocolConfig::Hysteria2(_)
        | ProtocolConfig::Tuic(_)
        | ProtocolConfig::Anytls(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socks(version: SocksVersion) -> ProtocolConfig {
        ProtocolConfig::Socks(SocksConfig {
            version,
            ..SocksConfig::default()
        })
    }

    #[test]
    fn proxy_carries_udp_only_where_the_generated_outbound_relays_it() {
        let cases = [
            (ProtocolConfig::Vless(VlessConfig::default()), true),
            (ProtocolConfig::Vmess(VmessConfig::default()), true),
            (ProtocolConfig::Trojan(TrojanConfig::default()), true),
            (
                ProtocolConfig::Shadowsocks(ShadowsocksConfig::default()),
                true,
            ),
            (ProtocolConfig::Hysteria2(Hysteria2Config::default()), true),
            (ProtocolConfig::Tuic(TuicConfig::default()), true),
            (ProtocolConfig::Anytls(AnytlsConfig::default()), true),
            (socks(SocksVersion::V5), true),
            (socks(SocksVersion::V4), false),
            (socks(SocksVersion::V4a), false),
            (ProtocolConfig::Http(HttpConfig::default()), false),
            (ProtocolConfig::Ssh(SshConfig::default()), false),
            (ProtocolConfig::Shadowtls(ShadowtlsConfig::default()), false),
        ];
        for (config, carries_udp) in cases {
            assert_eq!(proxy_carries_udp(&config), carries_udp, "{config:?}");
        }
    }
}

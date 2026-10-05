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

use serde_json::{Map, Value, json};

use crate::config::profile::{
    AnytlsConfig, HttpConfig, Hysteria2Config, Profile, ProtocolConfig, ShadowsocksConfig,
    ShadowtlsConfig, ShadowtlsVersion, SocksConfig, SocksVersion, SshConfig, TlsCommon,
    TransportConfig, TransportType, TrojanConfig, TuicConfig, VlessConfig, VmessConfig,
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
fn build_transport_block(t: &TransportConfig) -> Value {
    let mut obj = Map::new();
    obj.insert("type".to_string(), json!(transport_type_str(&t.kind)));
    if let Some(path) = t.path.as_deref() {
        obj.insert("path".to_string(), json!(path));
    }
    if let Some(host) = t.host.as_deref() {
        obj.insert("host".to_string(), json!(host));
    }
    if let Some(service_name) = t.service_name.as_deref() {
        obj.insert("service_name".to_string(), json!(service_name));
    }
    if t.kind == TransportType::Grpc {
        obj.entry("idle_timeout").or_insert(json!("15s"));
        obj.entry("ping_timeout").or_insert(json!("15s"));
    }
    if !t.headers.is_empty() {
        obj.insert("headers".to_string(), json!(t.headers));
    }
    Value::Object(obj)
}

fn transport_type_str(kind: &TransportType) -> &'static str {
    match kind {
        TransportType::Grpc => "grpc",
        TransportType::Ws => "ws",
        TransportType::Http => "http",
    }
}

/// Build VLESS outbound with optional REALITY / XTLS Vision / ECH.
pub(super) fn build_vless_outbound(profile: &Profile, cfg: &VlessConfig) -> anyhow::Result<Value> {
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

    if let Some(ref flow) = cfg.flow {
        outbound["flow"] = json!(flow);
    }

    if let Some(ref transport_type) = cfg.transport_type {
        let mut transport = json!({ "type": transport_type_str(transport_type) });
        if *transport_type == TransportType::Grpc {
            if let Some(ref service_name) = cfg.transport_service_name {
                transport["service_name"] = json!(service_name);
            }
            transport["idle_timeout"] = json!("15s");
            transport["ping_timeout"] = json!("15s");
        }
        outbound["transport"] = transport;
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
    if let Some(padding) = cfg.global_padding {
        outbound["global_padding"] = json!(padding);
    }
    if let Some(transport) = cfg.transport.as_ref() {
        outbound["transport"] = build_transport_block(transport);
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
        outbound["transport"] = build_transport_block(transport);
    }
    Ok(outbound)
}

/// Build Shadowsocks outbound (AEAD/AEAD-2022 ciphers only).
pub(super) fn build_shadowsocks_outbound(
    profile: &Profile,
    cfg: &ShadowsocksConfig,
) -> anyhow::Result<Value> {
    Ok(json!({
        "type": "shadowsocks",
        "tag": "proxy",
        "server": profile.address,
        "server_port": profile.port,
        "method": cfg.method.as_str(),
        "password": cfg.password,
    }))
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
    Ok(outbound)
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
    if tls_is_enabled(&cfg.tls) {
        outbound["tls"] = build_tls_block(&profile.address, &[], &cfg.tls);
    }
    Ok(outbound)
}

/// `TlsCommon::default()` represents "no TLS" — we treat a TLS block as
/// enabled when the user supplied at least one TLS-related field.
fn tls_is_enabled(tls: &TlsCommon) -> bool {
    tls.server_name.is_some()
        || tls.insecure
        || !tls.alpn.is_empty()
        || tls.utls_fingerprint.is_some()
        || tls.reality.is_some()
        || tls.ech.as_ref().is_some_and(|e| e.enabled)
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

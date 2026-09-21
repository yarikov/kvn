use anyhow::Result;

use crate::config::profile::*;

/// Encode a Profile to a share link URI. Inverse of [`parse_share_link`]:
/// `parse_share_link(encode_share_link(&p)?)` reproduces `p` modulo `id`
/// (which is regenerated on parse) and fields the parser does not extract.
pub fn encode_share_link(profile: &Profile) -> Result<String> {
    match &profile.config {
        ProtocolConfig::Vless(cfg) => Ok(encode_vless(profile, cfg)),
        ProtocolConfig::Vmess(cfg) => Ok(encode_vmess(profile, cfg)),
        ProtocolConfig::Trojan(cfg) => Ok(encode_trojan(profile, cfg)),
        ProtocolConfig::Shadowsocks(cfg) => Ok(encode_shadowsocks(profile, cfg)),
        ProtocolConfig::Hysteria2(cfg) => Ok(encode_hysteria2(profile, cfg)),
        ProtocolConfig::Tuic(cfg) => Ok(encode_tuic(profile, cfg)),
        ProtocolConfig::Socks(cfg) => Ok(encode_socks(profile, cfg)),
        ProtocolConfig::Http(cfg) => Ok(encode_http(profile, cfg)),
        ProtocolConfig::Ssh(cfg) => Ok(encode_ssh(profile, cfg)),
        ProtocolConfig::Anytls(cfg) => Ok(encode_anytls(profile, cfg)),
        ProtocolConfig::Shadowtls(cfg) => Ok(encode_shadowtls(profile, cfg)),
    }
}

fn build_query(pairs: &[(&str, String)]) -> String {
    let parts: Vec<String> = pairs
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

fn fragment_for(name: &str) -> String {
    if name.is_empty() {
        String::new()
    } else {
        format!("#{}", urlencoding::encode(name))
    }
}

fn host_for_uri(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn append_tls_common_query(pairs: &mut Vec<(&'static str, String)>, tls: &TlsCommon) {
    if let Some(reality) = &tls.reality {
        pairs.push(("security", "reality".to_string()));
        if !reality.server_name.is_empty() {
            pairs.push(("sni", reality.server_name.clone()));
        } else if let Some(sni) = &tls.server_name {
            pairs.push(("sni", sni.clone()));
        }
        pairs.push(("pbk", reality.public_key.clone()));
        if !reality.short_id.is_empty() {
            pairs.push(("sid", reality.short_id.clone()));
        }
        if !reality.spider_x.is_empty() {
            pairs.push(("spx", reality.spider_x.clone()));
        }
    } else if let Some(sni) = &tls.server_name {
        pairs.push(("sni", sni.clone()));
    }
    if !tls.alpn.is_empty() {
        pairs.push(("alpn", tls.alpn.join(",")));
    }
    if let Some(fp) = &tls.utls_fingerprint {
        pairs.push(("fp", fp.clone()));
    }
    if tls.insecure {
        pairs.push(("insecure", "1".to_string()));
    }
}

fn append_transport_query(pairs: &mut Vec<(&'static str, String)>, t: &TransportConfig) {
    let kind = match t.kind {
        TransportType::Grpc => "grpc",
        TransportType::Ws => "ws",
        TransportType::Http => "http",
    };
    pairs.push(("type", kind.to_string()));
    if let Some(p) = &t.path {
        pairs.push(("path", p.clone()));
    }
    if let Some(h) = &t.host {
        pairs.push(("host", h.clone()));
    }
    if let Some(s) = &t.service_name {
        pairs.push(("serviceName", s.clone()));
    }
}

fn encode_vless(profile: &Profile, cfg: &VlessConfig) -> String {
    let mut pairs: Vec<(&'static str, String)> = Vec::new();
    if cfg.flow == Some(Flow::XtlsRprxVision) {
        pairs.push(("flow", "xtls-rprx-vision".to_string()));
    }
    // `append_tls_common_query` already emits `security=reality` when
    // `tls.reality` is set; only the plain-TLS case needs an explicit hint.
    if cfg.tls.reality.is_none() && cfg.security == Some(Security::Tls) {
        pairs.push(("security", "tls".to_string()));
    }
    if let Some(tt) = &cfg.transport_type {
        let kind = match tt {
            TransportType::Grpc => "grpc",
            TransportType::Ws => "ws",
            TransportType::Http => "http",
        };
        pairs.push(("type", kind.to_string()));
    }
    if let Some(svc) = &cfg.transport_service_name {
        pairs.push(("serviceName", svc.clone()));
    }
    append_tls_common_query(&mut pairs, &cfg.tls);
    format!(
        "vless://{}@{}:{}{}{}",
        urlencoding::encode(&cfg.uuid),
        host_for_uri(&profile.address),
        profile.port,
        build_query(&pairs),
        fragment_for(&profile.name),
    )
}

fn encode_vmess(profile: &Profile, cfg: &VmessConfig) -> String {
    let mut pairs: Vec<(&'static str, String)> = Vec::new();
    if cfg.security != VmessSecurity::Auto {
        pairs.push(("scy", cfg.security.as_str().to_string()));
    }
    if cfg.alter_id != 0 {
        pairs.push(("aid", cfg.alter_id.to_string()));
    }
    append_tls_common_query(&mut pairs, &cfg.tls);
    if let Some(t) = &cfg.transport {
        append_transport_query(&mut pairs, t);
    }
    format!(
        "vmess://{}@{}:{}{}{}",
        urlencoding::encode(&cfg.uuid),
        host_for_uri(&profile.address),
        profile.port,
        build_query(&pairs),
        fragment_for(&profile.name),
    )
}

fn encode_trojan(profile: &Profile, cfg: &TrojanConfig) -> String {
    let mut pairs: Vec<(&'static str, String)> = Vec::new();
    append_tls_common_query(&mut pairs, &cfg.tls);
    if let Some(t) = &cfg.transport {
        append_transport_query(&mut pairs, t);
    }
    format!(
        "trojan://{}@{}:{}{}{}",
        urlencoding::encode(&cfg.password),
        host_for_uri(&profile.address),
        profile.port,
        build_query(&pairs),
        fragment_for(&profile.name),
    )
}

fn encode_shadowsocks(profile: &Profile, cfg: &ShadowsocksConfig) -> String {
    use base64::Engine;
    let creds = format!("{}:{}", cfg.method.as_str(), cfg.password);
    let userinfo = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(creds.as_bytes());
    format!(
        "ss://{}@{}:{}{}",
        userinfo,
        host_for_uri(&profile.address),
        profile.port,
        fragment_for(&profile.name),
    )
}

fn encode_hysteria2(profile: &Profile, cfg: &Hysteria2Config) -> String {
    let mut pairs: Vec<(&'static str, String)> = Vec::new();
    if let Some(obfs) = &cfg.obfs {
        let kind = match obfs.kind {
            Hysteria2ObfsType::Salamander => "salamander",
        };
        pairs.push(("obfs", kind.to_string()));
        pairs.push(("obfs-password", obfs.password.clone()));
    }
    if let Some(up) = cfg.up_mbps {
        pairs.push(("up", up.to_string()));
    }
    if let Some(down) = cfg.down_mbps {
        pairs.push(("down", down.to_string()));
    }
    append_tls_common_query(&mut pairs, &cfg.tls);
    format!(
        "hysteria2://{}@{}:{}{}{}",
        urlencoding::encode(&cfg.password),
        host_for_uri(&profile.address),
        profile.port,
        build_query(&pairs),
        fragment_for(&profile.name),
    )
}

fn encode_tuic(profile: &Profile, cfg: &TuicConfig) -> String {
    let mut pairs: Vec<(&'static str, String)> = Vec::new();
    if cfg.congestion_control != TuicCongestion::default() {
        pairs.push((
            "congestion_control",
            cfg.congestion_control.as_str().to_string(),
        ));
    }
    if cfg.udp_relay_mode != TuicUdpRelayMode::default() {
        pairs.push(("udp_relay_mode", cfg.udp_relay_mode.as_str().to_string()));
    }
    if cfg.zero_rtt_handshake {
        pairs.push(("zero_rtt_handshake", "1".to_string()));
    }
    append_tls_common_query(&mut pairs, &cfg.tls);
    format!(
        "tuic://{}:{}@{}:{}{}{}",
        urlencoding::encode(&cfg.uuid),
        urlencoding::encode(&cfg.password),
        host_for_uri(&profile.address),
        profile.port,
        build_query(&pairs),
        fragment_for(&profile.name),
    )
}

fn encode_socks(profile: &Profile, cfg: &SocksConfig) -> String {
    let mut userinfo = String::new();
    if let Some(u) = &cfg.username {
        userinfo.push_str(&urlencoding::encode(u));
        if let Some(p) = &cfg.password {
            userinfo.push(':');
            userinfo.push_str(&urlencoding::encode(p));
        }
        userinfo.push('@');
    }
    format!(
        "socks5://{}{}:{}{}",
        userinfo,
        host_for_uri(&profile.address),
        profile.port,
        fragment_for(&profile.name),
    )
}

fn encode_http(profile: &Profile, cfg: &HttpConfig) -> String {
    let tls_enabled = cfg.tls.server_name.is_some()
        || !cfg.tls.alpn.is_empty()
        || cfg.tls.utls_fingerprint.is_some()
        || cfg.tls.reality.is_some();
    let scheme = if tls_enabled { "https" } else { "http" };
    let mut userinfo = String::new();
    if let Some(u) = &cfg.username {
        userinfo.push_str(&urlencoding::encode(u));
        if let Some(p) = &cfg.password {
            userinfo.push(':');
            userinfo.push_str(&urlencoding::encode(p));
        }
        userinfo.push('@');
    }
    format!(
        "{}://{}{}:{}{}",
        scheme,
        userinfo,
        host_for_uri(&profile.address),
        profile.port,
        fragment_for(&profile.name),
    )
}

fn encode_ssh(profile: &Profile, cfg: &SshConfig) -> String {
    let mut pairs: Vec<(&'static str, String)> = Vec::new();
    if let Some(p) = &cfg.password {
        pairs.push(("password", p.clone()));
    }
    if let Some(p) = &cfg.private_key_path {
        pairs.push(("private_key_path", p.clone()));
    }
    if let Some(p) = &cfg.private_key_passphrase {
        pairs.push(("private_key_passphrase", p.clone()));
    }
    format!(
        "ssh://{}@{}:{}{}{}",
        urlencoding::encode(&cfg.user),
        host_for_uri(&profile.address),
        profile.port,
        build_query(&pairs),
        fragment_for(&profile.name),
    )
}

fn encode_anytls(profile: &Profile, cfg: &AnytlsConfig) -> String {
    let mut pairs: Vec<(&'static str, String)> = Vec::new();
    append_tls_common_query(&mut pairs, &cfg.tls);
    format!(
        "anytls://{}@{}:{}{}{}",
        urlencoding::encode(&cfg.password),
        host_for_uri(&profile.address),
        profile.port,
        build_query(&pairs),
        fragment_for(&profile.name),
    )
}

fn encode_shadowtls(profile: &Profile, cfg: &ShadowtlsConfig) -> String {
    let mut pairs: Vec<(&'static str, String)> = vec![
        ("version", cfg.version.as_u8().to_string()),
        ("ss-method", cfg.method.as_str().to_string()),
        ("ss-password", cfg.ss_password.clone()),
    ];
    append_tls_common_query(&mut pairs, &cfg.tls);
    format!(
        "shadowtls://{}@{}:{}{}{}",
        urlencoding::encode(&cfg.password),
        host_for_uri(&profile.address),
        profile.port,
        build_query(&pairs),
        fragment_for(&profile.name),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use uuid::Uuid;

    // ---- Round-trip: encode → parse must reproduce the input profile
    // (modulo `id`, which is regenerated on every parse).

    fn assert_roundtrip(mut profile: Profile) {
        let link = encode_share_link(&profile).expect("encode");
        let mut parsed =
            parse_share_link(&link).unwrap_or_else(|e| panic!("parse failed for `{link}`: {e}"));
        parsed.id = profile.id;
        // `tags` and `subscription_id` are not transported via share links;
        // strip them from both sides for the comparison.
        profile.tags.clear();
        profile.subscription_id = None;
        assert_eq!(parsed, profile, "round-trip mismatch for `{link}`");
    }

    #[test]
    fn encode_vless_roundtrip_plain() {
        let p = Profile::new_vless(
            "VLESS plain".to_string(),
            "1.1.1.1".to_string(),
            443,
            "vless-uuid".to_string(),
        );
        assert_roundtrip(p);
    }

    #[test]
    fn encode_vless_roundtrip_reality() {
        let mut p = Profile::new_vless(
            "VLESS reality".to_string(),
            "rt.example".to_string(),
            443,
            "vless-uuid".to_string(),
        );
        let ProtocolConfig::Vless(ref mut cfg) = p.config else {
            unreachable!()
        };
        cfg.flow = Some(Flow::XtlsRprxVision);
        cfg.security = Some(Security::Reality);
        cfg.tls.utls_fingerprint = Some("chrome".to_string());
        cfg.tls.reality = Some(RealitySettings {
            public_key: "pbk-value".to_string(),
            short_id: "sid-value".to_string(),
            server_name: "rt.example".to_string(),
            spider_x: "/spx".to_string(),
        });
        cfg.transport_type = Some(TransportType::Grpc);
        cfg.transport_service_name = Some("svc".to_string());
        assert_roundtrip(p);
    }

    #[test]
    fn encode_vmess_roundtrip_with_tls_and_ws() {
        let p = Profile {
            id: Uuid::new_v4(),
            name: "VMess WS".to_string(),
            address: "vm.example".to_string(),
            port: 8443,
            config: ProtocolConfig::Vmess(VmessConfig {
                uuid: "vm-uuid".to_string(),
                alter_id: 0,
                security: VmessSecurity::Aes128Gcm,
                tls: TlsCommon {
                    server_name: Some("sni.example".to_string()),
                    alpn: vec!["h2".to_string(), "http/1.1".to_string()],
                    utls_fingerprint: Some("chrome".to_string()),
                    ..TlsCommon::default()
                },
                transport: Some(TransportConfig {
                    kind: TransportType::Ws,
                    path: Some("/ws".to_string()),
                    host: Some("host.example".to_string()),
                    service_name: None,
                    headers: HashMap::new(),
                }),
                ..VmessConfig::default()
            }),
            tags: Vec::new(),
            subscription_id: None,
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_trojan_roundtrip() {
        let p = Profile {
            id: Uuid::new_v4(),
            name: "Trojan-1".to_string(),
            address: "tr.example".to_string(),
            port: 443,
            config: ProtocolConfig::Trojan(TrojanConfig {
                password: "hello world".to_string(),
                tls: TlsCommon {
                    server_name: Some("sni.example".to_string()),
                    ..TlsCommon::default()
                },
                transport: None,
            }),
            tags: Vec::new(),
            subscription_id: None,
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_shadowsocks_roundtrip_sip002() {
        let p = Profile {
            id: Uuid::new_v4(),
            name: "SS AEAD-2022".to_string(),
            address: "ss.example".to_string(),
            port: 8388,
            config: ProtocolConfig::Shadowsocks(ShadowsocksConfig {
                method: ShadowsocksCipher::Blake3Aes128Gcm,
                password: "p4ssw0rd".to_string(),
            }),
            tags: Vec::new(),
            subscription_id: None,
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_hysteria2_roundtrip_with_obfs() {
        let p = Profile {
            id: Uuid::new_v4(),
            name: "Hy2".to_string(),
            address: "hy.example".to_string(),
            port: 443,
            config: ProtocolConfig::Hysteria2(Hysteria2Config {
                password: "secret".to_string(),
                up_mbps: Some(100),
                down_mbps: Some(500),
                obfs: Some(Hysteria2Obfs {
                    kind: Hysteria2ObfsType::Salamander,
                    password: "obfs-pw".to_string(),
                }),
                tls: TlsCommon {
                    server_name: Some("hy.example".to_string()),
                    insecure: true,
                    ..TlsCommon::default()
                },
            }),
            tags: Vec::new(),
            subscription_id: None,
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_tuic_roundtrip() {
        let p = Profile {
            id: Uuid::new_v4(),
            name: "TUIC".to_string(),
            address: "tuic.example".to_string(),
            port: 443,
            config: ProtocolConfig::Tuic(TuicConfig {
                uuid: "tuic-uuid".to_string(),
                password: "tuic-pass".to_string(),
                congestion_control: TuicCongestion::Cubic,
                udp_relay_mode: TuicUdpRelayMode::Quic,
                zero_rtt_handshake: true,
                tls: TlsCommon {
                    server_name: Some("tuic.example".to_string()),
                    alpn: vec!["h3".to_string()],
                    ..TlsCommon::default()
                },
            }),
            tags: Vec::new(),
            subscription_id: None,
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_socks_roundtrip_with_auth() {
        let p = Profile {
            id: Uuid::new_v4(),
            name: "Socks".to_string(),
            address: "socks.example".to_string(),
            port: 1080,
            config: ProtocolConfig::Socks(SocksConfig {
                version: SocksVersion::V5,
                username: Some("alice".to_string()),
                password: Some("pa ss".to_string()),
            }),
            tags: Vec::new(),
            subscription_id: None,
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_http_roundtrip_https_tls() {
        let p = Profile {
            id: Uuid::new_v4(),
            name: "HTTPS proxy".to_string(),
            address: "proxy.example".to_string(),
            port: 443,
            config: ProtocolConfig::Http(HttpConfig {
                username: Some("u".to_string()),
                password: Some("p".to_string()),
                tls: TlsCommon {
                    server_name: Some("proxy.example".to_string()),
                    ..TlsCommon::default()
                },
            }),
            tags: Vec::new(),
            subscription_id: None,
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_ssh_roundtrip_key_path() {
        let p = Profile {
            id: Uuid::new_v4(),
            name: "SSH".to_string(),
            address: "ssh.example".to_string(),
            port: 22,
            config: ProtocolConfig::Ssh(SshConfig {
                user: "root".to_string(),
                password: None,
                private_key_path: Some("/home/me/.ssh/id_ed25519".to_string()),
                private_key_passphrase: Some("kp".to_string()),
                ..SshConfig::default()
            }),
            tags: Vec::new(),
            subscription_id: None,
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_anytls_roundtrip() {
        let p = Profile {
            id: Uuid::new_v4(),
            name: "AnyTLS".to_string(),
            address: "at.example".to_string(),
            port: 443,
            config: ProtocolConfig::Anytls(AnytlsConfig {
                password: "anytls-pw".to_string(),
                tls: TlsCommon {
                    server_name: Some("at.example".to_string()),
                    ..TlsCommon::default()
                },
                ..AnytlsConfig::default()
            }),
            tags: Vec::new(),
            subscription_id: None,
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_shadowtls_roundtrip_v3() {
        let p = Profile {
            id: Uuid::new_v4(),
            name: "ShadowTLS v3".to_string(),
            address: "st.example".to_string(),
            port: 443,
            config: ProtocolConfig::Shadowtls(ShadowtlsConfig {
                version: ShadowtlsVersion::V3,
                password: "stls-pw".to_string(),
                method: ShadowsocksCipher::Aes128Gcm,
                ss_password: "inner-ss-pw".to_string(),
                tls: TlsCommon {
                    server_name: Some("st.example".to_string()),
                    ..TlsCommon::default()
                },
            }),
            tags: Vec::new(),
            subscription_id: None,
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_vless_plain_tls_roundtrip() {
        let mut p = Profile::new_vless(
            "VLESS-TLS".to_string(),
            "1.2.3.4".to_string(),
            443,
            "vless-uuid".to_string(),
        );
        if let ProtocolConfig::Vless(ref mut cfg) = p.config {
            cfg.security = Some(Security::Tls);
            cfg.tls.server_name = Some("cdn.example.com".to_string());
            cfg.tls.alpn = vec!["h2".to_string(), "http/1.1".to_string()];
            cfg.tls.insecure = true;
            cfg.tls.utls_fingerprint = Some("chrome".to_string());
        }
        assert_roundtrip(p);
    }
}

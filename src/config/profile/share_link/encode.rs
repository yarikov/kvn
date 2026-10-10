use anyhow::Result;

use super::parse::vmess_b64_field_keys;
use crate::config::profile::*;

/// Encode a Profile to a share link URI. Inverse of [`parse_share_link`]:
/// `parse_share_link(encode_share_link(&p)?)` reproduces `p` modulo `id`
/// (which is regenerated on parse) and fields the parser does not extract.
pub fn encode_share_link(profile: &Profile) -> Result<String> {
    let link: Result<String> = match &profile.config {
        ProtocolConfig::Vless(cfg) => Ok(encode_vless(profile, cfg)),
        ProtocolConfig::Vmess(cfg) => return Ok(encode_vmess(profile, cfg, link_params(profile))),
        ProtocolConfig::Trojan(cfg) => Ok(encode_trojan(profile, cfg)),
        ProtocolConfig::Shadowsocks(cfg) => Ok(encode_shadowsocks(profile, cfg)),
        ProtocolConfig::Hysteria2(cfg) => Ok(encode_hysteria2(profile, cfg)),
        ProtocolConfig::Tuic(cfg) => Ok(encode_tuic(profile, cfg)),
        ProtocolConfig::Socks(cfg) => Ok(encode_socks(profile, cfg)),
        ProtocolConfig::Http(cfg) => Ok(encode_http(profile, cfg)),
        ProtocolConfig::Ssh(cfg) => Ok(encode_ssh(profile, cfg)),
        ProtocolConfig::Anytls(cfg) => Ok(encode_anytls(profile, cfg)),
        ProtocolConfig::Naive(cfg) => Ok(encode_naive(profile, cfg)),
        ProtocolConfig::Shadowtls(cfg) => Ok(encode_shadowtls(profile, cfg)),
    };
    Ok(append_share_link_params(link?, &link_params(profile)))
}

fn link_params(profile: &Profile) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut params = profile.share_link_params.clone();
    if let Some(tls) = profile.config.tls().filter(|tls| tls.ech.is_some()) {
        let ech_is_dns_query = exported_ech(tls).is_some_and(|ech| ech.config.is_empty());
        if !ech_is_dns_query {
            params.remove("ech");
        }
    }
    params
}

fn exported_ech(tls: &TlsCommon) -> Option<&EchSettings> {
    tls.ech
        .as_ref()
        .filter(|ech| ech.enabled || tls.reality.is_some())
}

fn append_share_link_params(
    link: String,
    params: &std::collections::BTreeMap<String, serde_json::Value>,
) -> String {
    if params.is_empty() {
        return link;
    }
    let (body, fragment) = match link.split_once('#') {
        Some((body, fragment)) => (body.to_string(), format!("#{fragment}")),
        None => (link, String::new()),
    };
    let separator = if body.contains('?') { '&' } else { '?' };
    let query: Vec<String> = params
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                urlencoding::encode(key),
                urlencoding::encode(&query_value(value))
            )
        })
        .collect();
    format!("{body}{separator}{}{fragment}", query.join("&"))
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

fn append_tls_common_query(pairs: &mut Vec<(&str, String)>, tls: &TlsCommon) {
    if let Some(reality) = &tls.reality {
        if !pairs.iter().any(|(key, _)| *key == "security") {
            pairs.push(("security", "reality".to_string()));
        }
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
    if let Some(ech) = exported_ech(tls).and_then(EchSettings::link_value) {
        pairs.push(("ech", ech));
    }
}

fn append_effective_sni(
    pairs: &mut Vec<(&str, String)>,
    tls: &TlsCommon,
    profile: &Profile,
    transport: Option<&TransportConfig>,
) {
    let host_would_become_sni = transport.is_some_and(|t| t.host.is_some());
    if tls.reality.is_none() && tls.server_name.is_none() && host_would_become_sni {
        pairs.push(("sni", profile.address.clone()));
    }
}

fn append_transport_query(
    pairs: &mut Vec<(&str, String)>,
    t: &TransportConfig,
    params: &std::collections::BTreeMap<String, serde_json::Value>,
) {
    pairs.push(("type", share_link_transport_type(t, params).to_string()));
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

fn query_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn share_link_transport_type<'a>(
    t: &'a TransportConfig,
    params: &std::collections::BTreeMap<String, serde_json::Value>,
) -> &'a str {
    match (
        &t.kind,
        params.get("headerType").and_then(serde_json::Value::as_str),
    ) {
        (TransportType::Http, Some(header)) if header != "none" => "tcp",
        (kind, _) => kind.as_str(),
    }
}

fn encode_vless(profile: &Profile, cfg: &VlessConfig) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    if cfg.flow == Some(Flow::XtlsRprxVision) {
        pairs.push(("flow", "xtls-rprx-vision".to_string()));
    }
    // `append_tls_common_query` already emits `security=reality` when
    // `tls.reality` is set; only the plain-TLS case needs an explicit hint.
    if cfg.security == Some(Security::None) {
        if cfg.tls.reality.is_some() {
            pairs.push(("security", "none".to_string()));
        }
    } else if cfg.tls.reality.is_none() {
        pairs.push(("security", "tls".to_string()));
        append_effective_sni(&mut pairs, &cfg.tls, profile, cfg.transport.as_ref());
    }
    append_tls_common_query(&mut pairs, &cfg.tls);
    if let Some(t) = &cfg.transport {
        append_transport_query(&mut pairs, t, &profile.share_link_params);
    }
    format!(
        "vless://{}@{}:{}{}{}",
        urlencoding::encode(&cfg.uuid),
        host_for_uri(&profile.address),
        profile.port,
        build_query(&pairs),
        fragment_for(&profile.name),
    )
}

fn encode_vmess(
    profile: &Profile,
    cfg: &VmessConfig,
    params: std::collections::BTreeMap<String, serde_json::Value>,
) -> String {
    use base64::Engine;
    use serde_json::json;

    let (net, mut object) = vmess_b64_transport_fields(cfg, params);
    object.extend(vmess_b64_tls_fields(&cfg.tls, cfg.stream_security.as_ref()));
    let host_would_become_sni = vmess_b64_field_keys(&net).1 == "host";
    if object.get("tls") == Some(&json!("tls"))
        && !object.contains_key("sni")
        && object.contains_key("host")
        && host_would_become_sni
    {
        object.insert("sni".into(), json!(profile.address));
    }
    object.extend([
        ("v".to_string(), json!(2)),
        ("ps".to_string(), json!(profile.name)),
        ("add".to_string(), json!(profile.address)),
        ("port".to_string(), json!(profile.port)),
        ("id".to_string(), json!(cfg.uuid)),
        ("aid".to_string(), json!(cfg.alter_id)),
        ("scy".to_string(), json!(cfg.security.as_str())),
        ("net".to_string(), json!(net)),
    ]);
    let body =
        serde_json::to_vec(&serde_json::Value::Object(object)).expect("JSON object serializes");
    format!(
        "vmess://{}",
        base64::engine::general_purpose::STANDARD.encode(body)
    )
}

fn vmess_b64_transport_fields(
    cfg: &VmessConfig,
    mut params: std::collections::BTreeMap<String, serde_json::Value>,
) -> (String, serde_json::Map<String, serde_json::Value>) {
    let net = cfg
        .transport
        .as_ref()
        .map_or("tcp", |t| share_link_transport_type(t, &params))
        .to_string();
    let transport_field = |key: &str| {
        let t = cfg.transport.as_ref()?;
        match key {
            "host" => t.host.clone(),
            "path" => t.path.clone(),
            "serviceName" => t.service_name.clone(),
            _ => None,
        }
    };
    let (type_key, host_key, path_key) = vmess_b64_field_keys(&net);
    let fields: Vec<(&str, serde_json::Value)> =
        [("type", type_key), ("host", host_key), ("path", path_key)]
            .into_iter()
            .filter_map(|(field, key)| {
                let value = transport_field(key)
                    .map(serde_json::Value::String)
                    .or_else(|| params.remove(key))?;
                Some((field, value))
            })
            .collect();
    let mut object: serde_json::Map<String, serde_json::Value> = params.into_iter().collect();
    object.extend(
        fields
            .into_iter()
            .map(|(field, value)| (field.to_string(), value)),
    );
    (net, object)
}

fn vmess_b64_tls_fields(
    tls: &TlsCommon,
    stream_security: Option<&Security>,
) -> serde_json::Map<String, serde_json::Value> {
    use serde_json::json;

    let mut object = serde_json::Map::new();
    let mode = match (stream_security, &tls.reality) {
        (Some(Security::None), Some(_)) => Some("none"),
        (Some(Security::None), None) => None,
        (_, Some(_)) | (Some(Security::Reality), None) => Some("reality"),
        _ => Some("tls"),
    };
    if let Some(mode) = mode {
        object.insert("tls".into(), json!(mode));
    }
    match &tls.reality {
        Some(reality) => {
            for (field, value) in [
                ("sni", &reality.server_name),
                ("pbk", &reality.public_key),
                ("sid", &reality.short_id),
                ("spx", &reality.spider_x),
            ] {
                if !value.is_empty() {
                    object.insert(field.into(), json!(value));
                }
            }
        }
        None => {
            if let Some(sni) = &tls.server_name {
                object.insert("sni".into(), json!(sni));
            }
        }
    }
    if !tls.alpn.is_empty() {
        object.insert("alpn".into(), json!(tls.alpn.join(",")));
    }
    if let Some(fp) = &tls.utls_fingerprint {
        object.insert("fp".into(), json!(fp));
    }
    if tls.insecure {
        object.insert("insecure".into(), json!("1"));
    }
    if let Some(ech) = exported_ech(tls).and_then(EchSettings::link_value) {
        object.insert("ech".into(), json!(ech));
    }
    object
}

fn encode_trojan(profile: &Profile, cfg: &TrojanConfig) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    append_effective_sni(&mut pairs, &cfg.tls, profile, cfg.transport.as_ref());
    append_tls_common_query(&mut pairs, &cfg.tls);
    if let Some(t) = &cfg.transport {
        append_transport_query(&mut pairs, t, &profile.share_link_params);
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
    let userinfo = if cfg.method.is_aead_2022() {
        format!(
            "{}:{}",
            urlencoding::encode(cfg.method.as_str()),
            urlencoding::encode(&cfg.password)
        )
    } else {
        let creds = format!("{}:{}", cfg.method.as_str(), cfg.password);
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(creds.as_bytes())
    };
    let plugin_slash = if profile.share_link_params.is_empty() {
        ""
    } else {
        "/"
    };
    format!(
        "ss://{}@{}:{}{}{}",
        userinfo,
        host_for_uri(&profile.address),
        profile.port,
        plugin_slash,
        fragment_for(&profile.name),
    )
}

fn encode_hysteria2(profile: &Profile, cfg: &Hysteria2Config) -> String {
    let mut pairs: Vec<(&'static str, String)> = Vec::new();
    if let Some(obfs) = &cfg.obfs {
        let kind = match obfs.kind {
            Hysteria2ObfsType::Salamander => "salamander",
            Hysteria2ObfsType::Gecko => "gecko",
        };
        pairs.push(("obfs", kind.to_string()));
        pairs.push(("obfs-password", obfs.password.clone()));
        if obfs.kind == Hysteria2ObfsType::Gecko {
            if let Some(min) = obfs.min_packet_size {
                pairs.push(("minPacketSize", min.to_string()));
            }
            if let Some(max) = obfs.max_packet_size {
                pairs.push(("maxPacketSize", max.to_string()));
            }
        }
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
    let (scheme, password) = match cfg.version {
        SocksVersion::V4 => ("socks4", None),
        SocksVersion::V4a => ("socks4a", None),
        SocksVersion::V5 => ("socks5", cfg.password.as_ref()),
    };
    let mut userinfo = String::new();
    if let Some(u) = &cfg.username {
        userinfo.push_str(&urlencoding::encode(u));
        if let Some(p) = password {
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

fn encode_naive(profile: &Profile, cfg: &NaiveConfig) -> String {
    let scheme = if cfg.quic {
        "naive+quic"
    } else {
        "naive+https"
    };
    let mut userinfo = String::new();
    if let Some(u) = &cfg.username {
        userinfo.push_str(&urlencoding::encode(u));
        if let Some(p) = &cfg.password {
            userinfo.push(':');
            userinfo.push_str(&urlencoding::encode(p));
        }
        userinfo.push('@');
    }
    let mut pairs: Vec<(&str, String)> = Vec::new();
    if let Some(sni) = &cfg.tls.server_name {
        pairs.push(("peer", sni.clone()));
    }
    if cfg.tls.insecure {
        pairs.push(("insecure", "1".to_string()));
    }
    if let Some(ech) = exported_ech(&cfg.tls).and_then(EchSettings::link_value) {
        pairs.push(("ech", ech));
    }
    format!(
        "{}://{}{}:{}{}{}",
        scheme,
        userinfo,
        host_for_uri(&profile.address),
        profile.port,
        build_query(&pairs),
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

    fn assert_link_round_trips(link: &str) {
        let parsed = parse_share_link(link).unwrap();
        let mut reparsed = parse_share_link(&encode_share_link(&parsed).unwrap()).unwrap();
        reparsed.id = parsed.id;
        assert_eq!(reparsed, parsed, "{link}");
    }

    #[test]
    fn encode_ech_config_roundtrip_in_uri_and_vmess_base64() {
        let tls = TlsCommon {
            server_name: Some("sni.example".to_string()),
            ech: Some(EchSettings::from_link_value("AEb+DQBC/w==", true)),
            ..TlsCommon::default()
        };
        assert_roundtrip(Profile {
            id: Uuid::new_v4(),
            name: "Trojan ECH".to_string(),
            address: "t.example".to_string(),
            port: 443,
            config: ProtocolConfig::Trojan(TrojanConfig {
                password: "pw".to_string(),
                tls: tls.clone(),
                ..TrojanConfig::default()
            }),
            tags: Vec::new(),
            subscription_id: None,
            share_link_params: Default::default(),
        });
        assert_roundtrip(Profile {
            id: Uuid::new_v4(),
            name: "VMess ECH".to_string(),
            address: "m.example".to_string(),
            port: 443,
            config: ProtocolConfig::Vmess(VmessConfig {
                uuid: "u".to_string(),
                stream_security: Some(Security::Tls),
                tls,
                ..VmessConfig::default()
            }),
            tags: Vec::new(),
            subscription_id: None,
            share_link_params: Default::default(),
        });
    }

    fn ech_of(profile: &Profile) -> Option<EchSettings> {
        profile.config.tls().and_then(|tls| tls.ech.clone())
    }

    fn set_ech(profile: &mut Profile, ech: EchSettings) {
        let ProtocolConfig::Trojan(cfg) = &mut profile.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        cfg.tls.ech = Some(ech);
    }

    #[test]
    fn encode_omits_a_disabled_ech_config() {
        let mut profile = parse_share_link("trojan://pw@t.example:443?ech=AEb%2BDQBC#T").unwrap();
        set_ech(
            &mut profile,
            EchSettings::from_link_value("AEb+DQBC", false),
        );
        let link = encode_share_link(&profile).unwrap();
        assert!(!link.contains("ech="), "{link}");
    }

    #[test]
    fn encode_keeps_an_ech_link_param_when_the_profile_has_no_ech_block() {
        for link in ["trojan://pw@t.example:443#T", "https://u:p@h.example:443#H"] {
            let mut profile = parse_share_link(link).unwrap();
            profile
                .share_link_params
                .insert("ech".into(), "AEb+DQBC".into());
            let exported = encode_share_link(&profile).unwrap();
            assert!(exported.contains("ech=AEb%2BDQBC"), "{exported}");
        }
    }

    #[test]
    fn encode_keeps_the_ech_dns_query_only_while_ech_still_uses_it() {
        let dns_query = "trojan://pw@t.example:443?ech=https%3A%2F%2F1.1.1.1%2Fdns-query#T";
        let mut profile = parse_share_link(dns_query).unwrap();
        assert_roundtrip(profile.clone());

        set_ech(&mut profile, EchSettings::from_link_value("AEb+DQBC", true));
        let reparsed = parse_share_link(&encode_share_link(&profile).unwrap()).unwrap();
        assert_eq!(ech_of(&reparsed), ech_of(&profile));
        assert!(!reparsed.share_link_params.contains_key("ech"));
    }

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
        let mut p = Profile::new_vless(
            "VLESS plain".to_string(),
            "1.1.1.1".to_string(),
            443,
            "vless-uuid".to_string(),
        );
        if let ProtocolConfig::Vless(cfg) = &mut p.config {
            cfg.security = Some(Security::None);
        }
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
        cfg.transport = Some(TransportConfig {
            kind: TransportType::Grpc,
            path: None,
            host: None,
            service_name: Some("svc".to_string()),
            headers: Default::default(),
        });
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
                stream_security: Some(Security::Tls),
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
            share_link_params: Default::default(),
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
            share_link_params: Default::default(),
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_trojan_roundtrip_with_httpupgrade() {
        let p = Profile {
            id: Uuid::new_v4(),
            name: "Trojan-HU".to_string(),
            address: "tr.example".to_string(),
            port: 443,
            config: ProtocolConfig::Trojan(TrojanConfig {
                password: "secret".to_string(),
                tls: TlsCommon {
                    server_name: Some("sni.example".to_string()),
                    ..TlsCommon::default()
                },
                transport: Some(TransportConfig {
                    kind: TransportType::HttpUpgrade,
                    path: Some("/up".to_string()),
                    host: Some("cdn.example".to_string()),
                    service_name: None,
                    headers: Default::default(),
                }),
            }),
            tags: Vec::new(),
            subscription_id: None,
            share_link_params: Default::default(),
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_keeps_unknown_transports_and_tcp_http_headers() {
        for link in [
            "vless://uuid@x.example:443?type=xhttp&path=%2Fx&extra=%7B%7D&mode=auto#X",
            "vless://uuid@h.example:80?sni=a.example&type=tcp&path=%2F&host=a.example&headerType=http#H",
            "vless://uuid@k.example:443?type=kcp&headerType=none&mtu=1350&seed=s&tti=20#K",
            "vless://uuid@w.example:443?type=ws&path=%2Fws&encryption=none&heartbeatPeriod=30#W",
        ] {
            let profile = parse_share_link(link).unwrap();
            assert_eq!(encode_share_link(&profile).unwrap(), link);
        }
    }

    #[test]
    fn encode_keeps_the_unmapped_params_of_every_protocol() {
        for link in [
            "vless://uuid@r.example:443?security=reality&pbk=k&sni=s.example&encryption=mlkem768&pqv=v#R",
            "hy2://p@h.example:443?sni=h.example&pinSHA256=AB%3ACD&mport=1000-2000#H",
            "ss://YWVzLTEyOC1nY206cA@s.example:8388?plugin=obfs-local%3Bobfs%3Dhttp#S",
        ] {
            assert_link_round_trips(link);
        }
    }

    #[test]
    fn encode_vmess_base64_round_trips_field_types_and_tls_flags() {
        let bodies = [
            serde_json::json!({
                "add": "k.example", "port": 443, "id": "u", "net": "kcp",
                "type": "wechat-video", "path": "seed", "mtu": 1350, "tti": 20
            }),
            serde_json::json!({
                "add": "x.example", "port": 443, "id": "u", "net": "xhttp", "path": "/x",
                "mode": "packet-up", "extra": { "headers": { "X-Test": "value" } },
                "tls": "tls", "sni": "x.example", "allowInsecure": 1
            }),
            serde_json::json!({
                "add": "g.example", "port": "443", "id": "u", "net": "grpc", "type": "multi",
                "host": "a.example", "path": "svc", "tls": "tls", "insecure": "1"
            }),
            serde_json::json!({
                "add": "q.example", "port": 443, "id": "u", "net": "quic",
                "type": "none", "host": "aes-128-gcm", "path": "secret"
            }),
        ];
        for body in bodies {
            assert_link_round_trips(&crate::test_helpers::vmess_b64_link(&body));
        }
    }

    #[test]
    fn vmess_uri_export_keeps_reality_and_grpc_authority_out_of_sni() {
        for link in [
            "vmess://u@vpn.example:443?security=reality&sni=r.example&pbk=key&sid=ab&spx=%2F&fp=chrome&type=grpc&serviceName=svc#R",
            "vmess://u@vpn.example:443?type=grpc&serviceName=svc&authority=cdn.example&fp=chrome#A",
        ] {
            assert_link_round_trips(link);
        }
    }

    #[test]
    fn vmess_export_keeps_the_reality_server_name_over_the_tls_one() {
        let mut profile = parse_share_link(
            "vmess://u@vpn.example:443?security=reality&sni=sni.example&pbk=key#R",
        )
        .unwrap();
        let ProtocolConfig::Vmess(cfg) = &mut profile.config else {
            panic!("expected VMess")
        };
        cfg.tls.server_name = Some("ordinary.example".to_string());

        let reparsed = parse_share_link(&encode_share_link(&profile).unwrap()).unwrap();

        let ProtocolConfig::Vmess(cfg) = reparsed.config else {
            panic!("expected VMess")
        };
        assert_eq!(cfg.tls.reality.unwrap().server_name, "sni.example");
    }

    #[test]
    fn export_keeps_an_explicit_none_beside_reality_params() {
        for link in [
            "vless://u@origin.example:443?security=none&pbk=abc&sni=sni.example&sid=ab&spx=%2F#V",
            "vmess://u@origin.example:443?security=none&pbk=abc&sni=sni.example&sid=ab&spx=%2F#M",
        ] {
            assert_link_round_trips(link);
        }
    }

    #[test]
    fn export_keeps_the_effective_sni_when_a_transport_host_is_set() {
        let ws = Some(TransportConfig {
            kind: TransportType::Ws,
            path: Some("/ws".to_string()),
            host: Some("cdn.example".to_string()),
            service_name: None,
            headers: HashMap::new(),
        });
        let profile = |config: ProtocolConfig| Profile {
            id: Uuid::new_v4(),
            name: "P".to_string(),
            address: "origin.example".to_string(),
            port: 443,
            config,
            tags: Vec::new(),
            subscription_id: None,
            share_link_params: Default::default(),
        };
        let profiles = [
            profile(ProtocolConfig::Vmess(VmessConfig {
                uuid: "u".to_string(),
                transport: ws.clone(),
                ..VmessConfig::default()
            })),
            profile(ProtocolConfig::Vless(VlessConfig {
                uuid: "u".to_string(),
                transport: ws.clone(),
                ..VlessConfig::default()
            })),
            profile(ProtocolConfig::Trojan(TrojanConfig {
                password: "p".to_string(),
                tls: TlsCommon::default(),
                transport: ws,
            })),
        ];
        for original in profiles {
            let reparsed = parse_share_link(&encode_share_link(&original).unwrap()).unwrap();
            let server_name = match &reparsed.config {
                ProtocolConfig::Vmess(cfg) => cfg.tls.server_name.clone(),
                ProtocolConfig::Vless(cfg) => cfg.tls.server_name.clone(),
                ProtocolConfig::Trojan(cfg) => cfg.tls.server_name.clone(),
                _ => unreachable!(),
            };
            assert_eq!(
                server_name.as_deref(),
                Some("origin.example"),
                "{original:?}"
            );
        }
    }

    #[test]
    fn vmess_base64_insecure_reads_the_v2rayn_and_marzban_fields() {
        let insecure = |flag: serde_json::Value| {
            let mut body = serde_json::json!({
                "add": "v.example", "port": 443, "id": "u", "tls": "tls"
            });
            body.as_object_mut()
                .unwrap()
                .extend(flag.as_object().unwrap().clone());
            let link = crate::test_helpers::vmess_b64_link(&body);
            let ProtocolConfig::Vmess(cfg) = parse_share_link(&link).unwrap().config else {
                panic!("expected VMess")
            };
            cfg.tls.insecure
        };
        assert!(insecure(serde_json::json!({ "insecure": "1" })));
        assert!(insecure(serde_json::json!({ "allowInsecure": 1 })));
        assert!(insecure(serde_json::json!({ "allowInsecure": true })));
        assert!(!insecure(serde_json::json!({ "insecure": "0" })));
        assert!(!insecure(serde_json::json!({ "allowInsecure": 0 })));
    }

    #[test]
    fn encode_shadowsocks_reproduces_the_sip002_examples() {
        for link in [
            "ss://YWVzLTEyOC1nY206dGVzdA@192.168.100.1:8888#Example1",
            "ss://2022-blake3-aes-256-gcm:YctPZ6U7xPPcU%2Bgp3u%2B0tx%2FtRizJN9K8y%2BuKlW2qjlI%3D@192.168.100.1:8888#Example3",
            "ss://2022-blake3-aes-256-gcm:YctPZ6U7xPPcU%2Bgp3u%2B0tx%2FtRizJN9K8y%2BuKlW2qjlI%3D@192.168.100.1:8888/?plugin=v2ray-plugin%3Bserver#Example3",
        ] {
            assert_eq!(
                encode_share_link(&parse_share_link(link).unwrap()).unwrap(),
                link
            );
        }
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
            share_link_params: Default::default(),
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
                    ..Default::default()
                }),
                tls: TlsCommon {
                    server_name: Some("hy.example".to_string()),
                    insecure: true,
                    ..TlsCommon::default()
                },
            }),
            tags: Vec::new(),
            subscription_id: None,
            share_link_params: Default::default(),
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_hysteria2_roundtrip_with_gecko() {
        let p = parse_share_link(
            "hysteria2://secret@hy.example:443?obfs=gecko&obfs-password=ob&minPacketSize=512&maxPacketSize=1200#Hy2",
        )
        .unwrap();
        let link = encode_share_link(&p).unwrap();
        assert!(
            link.contains("obfs=gecko&obfs-password=ob&minPacketSize=512&maxPacketSize=1200"),
            "{link}"
        );
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
            share_link_params: Default::default(),
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
            share_link_params: Default::default(),
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_socks4_roundtrip_keeps_version() {
        for version in [SocksVersion::V4, SocksVersion::V4a] {
            assert_roundtrip(Profile {
                id: Uuid::new_v4(),
                name: "Socks4".to_string(),
                address: "socks.example".to_string(),
                port: 1080,
                config: ProtocolConfig::Socks(SocksConfig {
                    version,
                    username: Some("alice".to_string()),
                    password: None,
                }),
                tags: Vec::new(),
                subscription_id: None,
                share_link_params: Default::default(),
            });
        }
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
            share_link_params: Default::default(),
        };
        assert_roundtrip(p);
    }

    #[test]
    fn encode_naive_roundtrip() {
        for quic in [false, true] {
            assert_roundtrip(Profile {
                id: Uuid::new_v4(),
                name: "Naive".to_string(),
                address: "n.example".to_string(),
                port: 8443,
                config: ProtocolConfig::Naive(NaiveConfig {
                    username: Some("alice".to_string()),
                    password: Some("p@ss word".to_string()),
                    quic,
                    tls: TlsCommon {
                        server_name: Some("sni.example".to_string()),
                        insecure: true,
                        ech: Some(EchSettings::from_link_value("AEb+DQBC", true)),
                        ..TlsCommon::default()
                    },
                }),
                tags: Vec::new(),
                subscription_id: None,
                share_link_params: [("padding".to_string(), "1".into())].into(),
            });
        }
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
            share_link_params: Default::default(),
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
            share_link_params: Default::default(),
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
            share_link_params: Default::default(),
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

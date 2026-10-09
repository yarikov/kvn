use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result};
use url::Url;
use uuid::Uuid;

use crate::config::profile::*;

/// Parse a share link text into a Profile. Dispatches on URI scheme.
pub fn parse_share_link(text: &str) -> Result<Profile> {
    let trimmed = text.trim();
    let scheme_end = trimmed.find("://").context("Missing URI scheme")?;
    let scheme = trimmed[..scheme_end].to_ascii_lowercase();
    let scheme = scheme.as_str();
    let rest = &trimmed[scheme_end + 3..];

    let mut profile = match scheme {
        "vless" => parse_vless(rest),
        "vmess" => parse_vmess(rest),
        "trojan" => parse_trojan(rest),
        "ss" => parse_shadowsocks(rest),
        "hysteria2" | "hy2" => parse_hysteria2(rest),
        "tuic" => parse_tuic(rest),
        "socks" | "socks5" | "socks5h" => parse_socks(scheme, rest, SocksVersion::V5),
        "socks4" => parse_socks(scheme, rest, SocksVersion::V4),
        "socks4a" => parse_socks(scheme, rest, SocksVersion::V4a),
        "http" | "https" => parse_http(scheme, rest),
        "ssh" => parse_ssh(rest),
        "anytls" => parse_anytls(rest),
        "shadowtls" => parse_shadowtls(rest),
        other => anyhow::bail!("Unsupported share link scheme: {other}://"),
    }?;
    let vmess_base64 = scheme == "vmess" && !rest.contains('@');
    if !vmess_base64 {
        profile.share_link_params = unmapped_query_params(rest, scheme);
    }
    Ok(profile)
}

fn parse_uri(scheme: &str, rest: &str) -> Result<Url> {
    Url::parse(&format!("{scheme}://{rest}")).with_context(|| format!("Invalid {scheme} URL"))
}

fn query_map(url: &Url) -> std::collections::HashMap<String, String> {
    url.query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn fragment_name(url: &Url, fallback: &str) -> Result<String> {
    Ok(match url.fragment().filter(|name| !name.is_empty()) {
        Some(f) => urlencoding::decode(f)?.to_string(),
        None => fallback.to_string(),
    })
}

pub(crate) fn decode_b64_lenient(s: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    // Try URL-safe-no-pad first (ss://, vmess JSON often), then standard.
    if let Ok(b) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&cleaned) {
        return Ok(b);
    }
    if let Ok(b) = base64::engine::general_purpose::URL_SAFE.decode(&cleaned) {
        return Ok(b);
    }
    if let Ok(b) = base64::engine::general_purpose::STANDARD_NO_PAD.decode(&cleaned) {
        return Ok(b);
    }
    base64::engine::general_purpose::STANDARD
        .decode(&cleaned)
        .context("base64 decode failed")
}

fn parse_transport_type(s: &str) -> Option<TransportType> {
    Some(match s {
        "" | "tcp" | "raw" | "none" => return None,
        "grpc" => TransportType::Grpc,
        "ws" => TransportType::Ws,
        "http" | "h2" => TransportType::Http,
        "httpupgrade" => TransportType::HttpUpgrade,
        "quic" => TransportType::Quic,
        other => TransportType::Other(other.to_string()),
    })
}

const TLS_QUERY_KEYS: [&str; 10] = [
    "ech",
    "sni",
    "host",
    "alpn",
    "fp",
    "allowInsecure",
    "insecure",
    "pbk",
    "sid",
    "spx",
];

const TRANSPORT_QUERY_KEYS: [&str; 4] = ["type", "path", "host", "serviceName"];

fn mapped_query_keys(scheme: &str) -> Vec<&'static str> {
    let (tls, transport, own): (bool, bool, &[&'static str]) = match scheme {
        "vless" => (true, true, &["flow", "security"]),
        "vmess" => (true, true, &["scy", "encryption", "security", "aid"]),
        "trojan" => (true, true, &[]),
        "hysteria2" | "hy2" => (
            true,
            false,
            &["obfs", "obfs-password", "up", "down", "upmbps", "downmbps"],
        ),
        "tuic" => (
            true,
            false,
            &["congestion_control", "udp_relay_mode", "zero_rtt_handshake"],
        ),
        "anytls" => (true, false, &[]),
        "shadowtls" => (
            true,
            false,
            &[
                "version",
                "ss-method",
                "method",
                "ss-password",
                "ss_password",
            ],
        ),
        "ssh" => (
            false,
            false,
            &["password", "private_key_path", "private_key_passphrase"],
        ),
        _ => (false, false, &[]),
    };
    let mut keys = own.to_vec();
    if tls {
        keys.extend(TLS_QUERY_KEYS);
    }
    if transport {
        keys.extend(TRANSPORT_QUERY_KEYS);
    }
    keys
}

fn unmapped_query_params(rest: &str, scheme: &str) -> BTreeMap<String, serde_json::Value> {
    let without_fragment = rest.split_once('#').map_or(rest, |(body, _)| body);
    let Some((_, query)) = without_fragment.split_once('?') else {
        return BTreeMap::new();
    };
    let mapped = mapped_query_keys(scheme);
    url::form_urlencoded::parse(query.as_bytes())
        .filter(|(key, value)| !is_mapped_param(&mapped, key, value))
        .map(|(key, value)| {
            (
                key.into_owned(),
                serde_json::Value::String(value.into_owned()),
            )
        })
        .collect()
}

fn is_mapped_param(mapped: &[&str], key: &str, value: &str) -> bool {
    mapped.contains(&key) && !(key == "ech" && EchSettings::is_dns_query_link_value(value))
}

const VMESS_B64_MAPPED_FIELDS: [&str; 21] = [
    "ech",
    "v",
    "ps",
    "add",
    "port",
    "id",
    "aid",
    "scy",
    "tls",
    "sni",
    "alpn",
    "fp",
    "insecure",
    "allowInsecure",
    "pbk",
    "sid",
    "spx",
    "net",
    "type",
    "host",
    "path",
];

pub(super) fn vmess_b64_field_keys(net: &str) -> (&'static str, &'static str, &'static str) {
    match net {
        "grpc" => ("mode", "authority", "serviceName"),
        "kcp" | "mkcp" => ("headerType", "host", "seed"),
        "xhttp" | "splithttp" => ("mode", "host", "path"),
        "quic" => ("headerType", "quicSecurity", "key"),
        _ => ("headerType", "host", "path"),
    }
}

fn parse_alpn(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

fn parse_bool_param(s: &str) -> bool {
    matches!(s, "1" | "true" | "yes")
}

/// Apply transport/SNI/utls/reality/ech parameters that VLESS/VMess/Trojan
/// share when their share-link is in plain URI form (not VMess base64-JSON).
fn extract_tls_common_from_query(q: &std::collections::HashMap<String, String>) -> TlsCommon {
    let mut tls = TlsCommon::default();
    if let Some(sni) = q.get("sni") {
        tls.server_name = Some(sni.clone());
    } else if let Some(host) = q.get("host") {
        tls.server_name = Some(host.clone());
    }
    if let Some(v) = q.get("alpn") {
        tls.alpn = parse_alpn(v);
    }
    if let Some(fp) = q.get("fp") {
        tls.utls_fingerprint = Some(fp.clone());
    }
    if q.get("allowInsecure")
        .map(|s| parse_bool_param(s))
        .unwrap_or(false)
        || q.get("insecure")
            .map(|s| parse_bool_param(s))
            .unwrap_or(false)
    {
        tls.insecure = true;
    }
    if let Some(pbk) = q.get("pbk") {
        tls.reality = Some(RealitySettings {
            public_key: pbk.clone(),
            short_id: q.get("sid").cloned().unwrap_or_default(),
            server_name: q.get("sni").cloned().unwrap_or_default(),
            spider_x: q.get("spx").cloned().unwrap_or_default(),
        });
        // sing-box honors `reality.server_name` exclusively when REALITY is
        // enabled; keeping a duplicate in `tls.server_name` would break the
        // encode→parse round-trip (one input `sni` becomes two destinations).
        tls.server_name = None;
    }
    tls.ech = q.get("ech").map(|value| ech_from_link(value, &tls));
    tls
}

fn ech_from_link(value: &str, tls: &TlsCommon) -> EchSettings {
    EchSettings::from_link_value(value, tls.reality.is_none())
}

fn extract_transport_from_query(
    q: &std::collections::HashMap<String, String>,
) -> Option<TransportConfig> {
    transport_from_params(
        &q.iter()
            .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
            .collect(),
    )
}

fn transport_from_params(params: &BTreeMap<String, serde_json::Value>) -> Option<TransportConfig> {
    let text = |key: &str| match params.get(key) {
        Some(serde_json::Value::String(value)) => Some(value.clone()),
        Some(other) => Some(other.to_string()),
        None => None,
    };
    let kind = match parse_transport_type(text("type").as_deref().unwrap_or("tcp")) {
        Some(kind) => kind,
        None => match text("headerType").as_deref() {
            None | Some("" | "none") => return None,
            Some("http") => TransportType::Http,
            Some(_) => TransportType::Other("tcp".to_string()),
        },
    };
    Some(TransportConfig {
        kind,
        path: text("path"),
        host: text("host"),
        service_name: text("serviceName"),
        headers: HashMap::new(),
    })
}

/// Parse a VLESS URI fragment.
fn parse_vless(rest: &str) -> Result<Profile> {
    let url = parse_uri("vless", rest)?;
    let uuid = url.username().to_string();
    let host = url
        .host_str()
        .context("Missing host in VLESS URL")?
        .to_string();
    let port = url.port().unwrap_or(443);
    let name = fragment_name(&url, &host)?;
    let mut profile = Profile::new_vless(name, host, port, uuid);

    let query = query_map(&url);

    let ProtocolConfig::Vless(ref mut cfg) = profile.config else {
        unreachable!("Profile::new_vless constructs a Vless variant");
    };

    if let Some(flow) = query.get("flow") {
        cfg.flow = match flow.as_str() {
            "xtls-rprx-vision" => Some(Flow::XtlsRprxVision),
            _ => None,
        };
    }
    cfg.transport = extract_transport_from_query(&query);
    // Shared with VMess/Trojan: handles sni / alpn / fp / insecure and
    // routes pbk+sid+spx+sni into a `RealitySettings` block when present.
    cfg.tls = extract_tls_common_from_query(&query);
    cfg.security = stream_security(query.get("security").map(String::as_str), &cfg.tls);

    Ok(profile)
}

/// Parse a VMess share link. Supports both formats:
/// (a) v2rayN/Shadowrocket: `vmess://<base64 JSON>` with v/ps/add/port/id/aid/scy/net/type/host/path/tls/sni/alpn/fp.
/// (b) Plain URI: `vmess://uuid@host:port?security=tls&type=ws&path=&host=#name`.
fn parse_vmess(rest: &str) -> Result<Profile> {
    if !rest.contains('@') {
        return parse_vmess_b64(rest);
    }
    let url = parse_uri("vmess", rest)?;
    let uuid = url.username().to_string();
    let host = url
        .host_str()
        .context("Missing host in VMess URL")?
        .to_string();
    let port = url.port().unwrap_or(443);
    let name = fragment_name(&url, &host)?;
    let query = query_map(&url);

    let cipher = query
        .get("scy")
        .or_else(|| query.get("encryption"))
        .map(String::as_str);
    let tls = extract_tls_common_from_query(&query);
    let cfg = VmessConfig {
        uuid,
        alter_id: query.get("aid").and_then(|v| v.parse().ok()).unwrap_or(0),
        security: parse_vmess_security(cipher),
        stream_security: stream_security(query.get("security").map(String::as_str), &tls),
        tls,
        transport: extract_transport_from_query(&query),
        ..VmessConfig::default()
    };
    Ok(Profile {
        id: Uuid::new_v4(),
        name,
        address: host,
        port,
        config: ProtocolConfig::Vmess(cfg),
        tags: Vec::new(),
        subscription_id: None,
        share_link_params: Default::default(),
    })
}

fn parse_vmess_b64(b64: &str) -> Result<Profile> {
    let bytes = decode_b64_lenient(b64).context("VMess base64 payload")?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).context("VMess JSON payload")?;

    let host = v["add"]
        .as_str()
        .context("VMess: missing 'add'")?
        .to_string();
    let port = v["port"]
        .as_u64()
        .or_else(|| v["port"].as_str().and_then(|s| s.parse().ok()))
        .context("VMess: missing 'port'")?;
    let port =
        u16::try_from(port).with_context(|| format!("VMess: port {port} is out of range"))?;
    let uuid = v["id"].as_str().context("VMess: missing 'id'")?.to_string();
    let name = v["ps"]
        .as_str()
        .filter(|name| !name.is_empty())
        .unwrap_or(&host)
        .to_string();
    let aid = v["aid"]
        .as_u64()
        .or_else(|| v["aid"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0) as u32;
    let security = v["scy"].as_str();

    let tls = vmess_b64_tls(&v);

    let transport_params = vmess_b64_transport_params(&v);
    let transport = transport_from_params(&transport_params);
    let share_link_params = transport_params
        .into_iter()
        .filter(|(key, _)| !TRANSPORT_QUERY_KEYS.contains(&key.as_str()))
        .collect();

    Ok(Profile {
        id: Uuid::new_v4(),
        name,
        address: host,
        port,
        config: ProtocolConfig::Vmess(VmessConfig {
            uuid,
            alter_id: aid,
            security: parse_vmess_security(security),
            stream_security: stream_security(v["tls"].as_str(), &tls),
            tls,
            transport,
            ..VmessConfig::default()
        }),
        tags: Vec::new(),
        subscription_id: None,
        share_link_params,
    })
}

fn stream_security(value: Option<&str>, tls: &TlsCommon) -> Option<Security> {
    match value {
        Some("reality") => Some(Security::Reality),
        Some("tls") => Some(Security::Tls),
        Some("none") => Some(Security::None),
        Some("") | None if tls.reality.is_some() => Some(Security::Reality),
        Some("") | None => Some(Security::None),
        Some(_) => None,
    }
}

fn json_flag(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(flag) => *flag,
        serde_json::Value::Number(number) => number.as_u64() == Some(1),
        serde_json::Value::String(text) => parse_bool_param(text),
        _ => false,
    }
}

fn vmess_b64_tls(v: &serde_json::Value) -> TlsCommon {
    let mut tls = TlsCommon::default();
    let mode = v["tls"].as_str();
    let text = |field: &str| {
        v[field]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(String::from)
    };
    let (_, host_field, _) = vmess_b64_field_keys(v["net"].as_str().unwrap_or("tcp"));
    let host_is_sni_fallback = host_field == "host" && mode == Some("tls");
    let sni = text("sni");
    if let Some(alpn) = v["alpn"].as_str() {
        tls.alpn = parse_alpn(alpn);
    }
    tls.utls_fingerprint = text("fp");
    tls.insecure = ["insecure", "allowInsecure"]
        .iter()
        .any(|field| json_flag(&v[*field]));
    if mode == Some("reality") || text("pbk").is_some() {
        tls.reality = Some(RealitySettings {
            public_key: text("pbk").unwrap_or_default(),
            short_id: text("sid").unwrap_or_default(),
            server_name: sni.unwrap_or_default(),
            spider_x: text("spx").unwrap_or_default(),
        });
    } else {
        tls.server_name = sni.or_else(|| text("host").filter(|_| host_is_sni_fallback));
    }
    tls.ech = text("ech").map(|value| ech_from_link(&value, &tls));
    tls
}

fn vmess_b64_transport_params(v: &serde_json::Value) -> BTreeMap<String, serde_json::Value> {
    let (type_key, host_key, path_key) = vmess_b64_field_keys(v["net"].as_str().unwrap_or("tcp"));
    let mut params: BTreeMap<String, serde_json::Value> = v
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(field, value)| {
            !is_mapped_param(
                &VMESS_B64_MAPPED_FIELDS,
                field,
                value.as_str().unwrap_or_default(),
            )
        })
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect();
    for (param, field) in [
        ("type", "net"),
        (type_key, "type"),
        (host_key, "host"),
        (path_key, "path"),
    ] {
        if let Some(value) = v[field].as_str().filter(|value| !value.is_empty()) {
            params
                .entry(param.to_string())
                .or_insert_with(|| serde_json::Value::String(value.to_string()));
        }
    }
    params
}

fn parse_vmess_security(s: Option<&str>) -> VmessSecurity {
    match s.unwrap_or("auto") {
        "auto" | "" => VmessSecurity::Auto,
        "none" => VmessSecurity::None,
        "zero" => VmessSecurity::Zero,
        "aes-128-gcm" => VmessSecurity::Aes128Gcm,
        "chacha20-poly1305" => VmessSecurity::Chacha20Poly1305,
        _ => VmessSecurity::Auto,
    }
}

/// Parse `trojan://password@host:port?sni=&type=&path=&host=#name`. TLS is implicit.
fn parse_trojan(rest: &str) -> Result<Profile> {
    let url = parse_uri("trojan", rest)?;
    let password = urlencoding::decode(url.username())?.to_string();
    if password.is_empty() {
        anyhow::bail!("Trojan share link missing password");
    }
    let host = url
        .host_str()
        .context("Missing host in Trojan URL")?
        .to_string();
    let port = url.port().unwrap_or(443);
    let name = fragment_name(&url, &host)?;
    let query = query_map(&url);
    Ok(Profile {
        id: Uuid::new_v4(),
        name,
        address: host,
        port,
        config: ProtocolConfig::Trojan(TrojanConfig {
            password,
            tls: extract_tls_common_from_query(&query),
            transport: extract_transport_from_query(&query),
        }),
        tags: Vec::new(),
        subscription_id: None,
        share_link_params: Default::default(),
    })
}

/// Parse Shadowsocks share link. Supports SIP002 (`ss://b64(method:pw)@host:port#name`)
/// and the legacy fully-base64 form (`ss://b64(method:pw@host:port)#name`).
fn parse_shadowsocks(rest: &str) -> Result<Profile> {
    // Split fragment off so we don't accidentally base64-decode the name.
    let (body, fragment) = match rest.split_once('#') {
        Some((body, fragment)) => (body, Some(fragment)),
        None => (rest, None),
    };
    let body = body.split_once('?').map_or(body, |(body, _)| body);
    let body = body.strip_suffix('/').unwrap_or(body);

    let (method, password, host, port) = if let Some((userinfo, hostport)) = body.rsplit_once('@') {
        let (method, password) = shadowsocks_userinfo(userinfo)?;
        let (host, port) = shadowsocks_host_port(hostport)?;
        (method, password, host, port)
    } else {
        // Legacy: entire body is base64.
        let bytes = decode_b64_lenient(body).context("Shadowsocks base64")?;
        let s = String::from_utf8(bytes).context("Shadowsocks base64 utf8")?;
        let (creds, hostport) = s
            .rsplit_once('@')
            .context("Shadowsocks legacy form: expected method:pw@host:port")?;
        let (m, p) = creds
            .split_once(':')
            .context("Shadowsocks: expected method:password")?;
        let (host, port) = shadowsocks_host_port(hostport)?;
        (m.to_string(), p.to_string(), host, port)
    };

    let cipher = parse_shadowsocks_cipher(&method)
        .with_context(|| format!("Unsupported Shadowsocks cipher: {method}"))?;
    let name = match fragment.filter(|name| !name.is_empty()) {
        Some(f) => urlencoding::decode(f)?.to_string(),
        None => host.clone(),
    };
    Ok(Profile {
        id: Uuid::new_v4(),
        name,
        address: host,
        port,
        config: ProtocolConfig::Shadowsocks(ShadowsocksConfig {
            method: cipher,
            password,
        }),
        tags: Vec::new(),
        subscription_id: None,
        share_link_params: Default::default(),
    })
}

fn shadowsocks_userinfo(userinfo: &str) -> Result<(String, String)> {
    if let Some((method, password)) = userinfo.split_once(':') {
        return Ok((
            urlencoding::decode(method)?.into_owned(),
            urlencoding::decode(password)?.into_owned(),
        ));
    }
    let creds = String::from_utf8(decode_b64_lenient(userinfo).context("Shadowsocks userinfo")?)
        .context("Shadowsocks userinfo utf8")?;
    let (method, password) = creds
        .split_once(':')
        .context("Shadowsocks: expected method:password")?;
    Ok((method.to_string(), password.to_string()))
}

fn shadowsocks_host_port(hostport: &str) -> Result<(String, u16)> {
    let (host, port) = hostport
        .rsplit_once(':')
        .context("Shadowsocks: expected host:port")?;
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    let port = port.parse().context("Shadowsocks: invalid port")?;
    Ok((host.to_string(), port))
}

fn parse_shadowsocks_cipher(s: &str) -> Option<ShadowsocksCipher> {
    Some(match s {
        "chacha20-ietf-poly1305" => ShadowsocksCipher::Chacha20IetfPoly1305,
        "aes-128-gcm" => ShadowsocksCipher::Aes128Gcm,
        "aes-256-gcm" => ShadowsocksCipher::Aes256Gcm,
        "2022-blake3-aes-128-gcm" => ShadowsocksCipher::Blake3Aes128Gcm,
        "2022-blake3-aes-256-gcm" => ShadowsocksCipher::Blake3Aes256Gcm,
        "2022-blake3-chacha20-poly1305" => ShadowsocksCipher::Blake3Chacha20Poly1305,
        "none" | "plain" => ShadowsocksCipher::None,
        _ => return None,
    })
}

/// Parse `hysteria2://password@host:port?obfs=&obfs-password=&sni=&insecure=&alpn=#name`.
fn parse_hysteria2(rest: &str) -> Result<Profile> {
    let url = parse_uri("hysteria2", rest)?;
    let password = urlencoding::decode(url.username())?.to_string();
    if password.is_empty() {
        anyhow::bail!("Hysteria2 share link missing password");
    }
    let host = url
        .host_str()
        .context("Missing host in Hysteria2 URL")?
        .to_string();
    let port = url.port().unwrap_or(443);
    let name = fragment_name(&url, &host)?;
    let query = query_map(&url);

    let obfs = match (query.get("obfs"), query.get("obfs-password")) {
        (Some(kind), Some(p)) if kind == "salamander" => Some(Hysteria2Obfs {
            kind: Hysteria2ObfsType::Salamander,
            password: p.clone(),
        }),
        _ => None,
    };

    Ok(Profile {
        id: Uuid::new_v4(),
        name,
        address: host,
        port,
        config: ProtocolConfig::Hysteria2(Hysteria2Config {
            password,
            up_mbps: mbps(&query, "up", "upmbps"),
            down_mbps: mbps(&query, "down", "downmbps"),
            obfs,
            tls: extract_tls_common_from_query(&query),
        }),
        tags: Vec::new(),
        subscription_id: None,
        share_link_params: Default::default(),
    })
}

fn mbps(query: &HashMap<String, String>, key: &str, alias: &str) -> Option<u32> {
    query
        .get(key)
        .or_else(|| query.get(alias))
        .and_then(|s| s.parse().ok())
}

/// Parse `tuic://uuid:password@host:port?congestion_control=&udp_relay_mode=&alpn=&sni=#name`.
fn parse_tuic(rest: &str) -> Result<Profile> {
    let url = parse_uri("tuic", rest)?;
    let uuid = urlencoding::decode(url.username())?.to_string();
    let password = urlencoding::decode(url.password().unwrap_or(""))?.to_string();
    if uuid.is_empty() || password.is_empty() {
        anyhow::bail!("TUIC share link missing uuid or password");
    }
    let host = url
        .host_str()
        .context("Missing host in TUIC URL")?
        .to_string();
    let port = url.port().unwrap_or(443);
    let name = fragment_name(&url, &host)?;
    let query = query_map(&url);

    let cc = match query.get("congestion_control").map(String::as_str) {
        Some("cubic") => TuicCongestion::Cubic,
        Some("new_reno") | Some("new-reno") | Some("newreno") => TuicCongestion::NewReno,
        _ => TuicCongestion::Bbr,
    };
    let udp = match query.get("udp_relay_mode").map(String::as_str) {
        Some("quic") => TuicUdpRelayMode::Quic,
        _ => TuicUdpRelayMode::Native,
    };
    let zero_rtt = query
        .get("zero_rtt_handshake")
        .map(|s| parse_bool_param(s))
        .unwrap_or(false);

    Ok(Profile {
        id: Uuid::new_v4(),
        name,
        address: host,
        port,
        config: ProtocolConfig::Tuic(TuicConfig {
            uuid,
            password,
            congestion_control: cc,
            udp_relay_mode: udp,
            zero_rtt_handshake: zero_rtt,
            tls: extract_tls_common_from_query(&query),
        }),
        tags: Vec::new(),
        subscription_id: None,
        share_link_params: Default::default(),
    })
}

/// Parse `socks5://user:pass@host:port#name` (also `socks://`).
fn parse_socks(scheme: &str, rest: &str, version: SocksVersion) -> Result<Profile> {
    let url = parse_uri(scheme, rest)?;
    let host = url
        .host_str()
        .context("Missing host in SOCKS URL")?
        .to_string();
    let port = url.port().context("Missing port in SOCKS URL")?;
    let name = fragment_name(&url, &host)?;
    let user = url.username();
    let pass = url.password();
    if pass.is_some() && version != SocksVersion::V5 {
        anyhow::bail!("SOCKS4 share link cannot carry a password");
    }
    Ok(Profile {
        id: Uuid::new_v4(),
        name,
        address: host,
        port,
        config: ProtocolConfig::Socks(SocksConfig {
            version,
            username: (!user.is_empty())
                .then(|| urlencoding::decode(user).unwrap_or_default().to_string()),
            password: pass.map(|p| urlencoding::decode(p).unwrap_or_default().to_string()),
        }),
        tags: Vec::new(),
        subscription_id: None,
        share_link_params: Default::default(),
    })
}

/// Parse `http://user:pass@host:port#name` / `https://...#name`. The `tls` flag
/// is set when the scheme is `https`.
fn parse_http(scheme: &str, rest: &str) -> Result<Profile> {
    let tls_enabled = scheme == "https";
    let url = parse_uri(scheme, rest)?;
    let host = url
        .host_str()
        .context("Missing host in HTTP URL")?
        .to_string();
    let port = url.port().unwrap_or(if tls_enabled { 443 } else { 80 });
    let name = fragment_name(&url, &host)?;
    let user = url.username();
    let pass = url.password();
    let tls = if tls_enabled {
        TlsCommon {
            server_name: Some(host.clone()),
            ..TlsCommon::default()
        }
    } else {
        TlsCommon::default()
    };
    Ok(Profile {
        id: Uuid::new_v4(),
        name,
        address: host,
        port,
        config: ProtocolConfig::Http(HttpConfig {
            username: (!user.is_empty())
                .then(|| urlencoding::decode(user).unwrap_or_default().to_string()),
            password: pass.map(|p| urlencoding::decode(p).unwrap_or_default().to_string()),
            tls,
        }),
        tags: Vec::new(),
        subscription_id: None,
        share_link_params: Default::default(),
    })
}

/// Parse `ssh://user@host:port#name` (optional `?password=&private_key_path=`).
fn parse_ssh(rest: &str) -> Result<Profile> {
    let url = parse_uri("ssh", rest)?;
    let host = url
        .host_str()
        .context("Missing host in SSH URL")?
        .to_string();
    let port = url.port().unwrap_or(22);
    let name = fragment_name(&url, &host)?;
    let user = url.username().to_string();
    if user.is_empty() {
        anyhow::bail!("SSH share link missing user");
    }
    let url_pass = url
        .password()
        .map(|p| urlencoding::decode(p).unwrap_or_default().to_string());
    let q = query_map(&url);
    Ok(Profile {
        id: Uuid::new_v4(),
        name,
        address: host,
        port,
        config: ProtocolConfig::Ssh(SshConfig {
            user,
            password: url_pass.or_else(|| q.get("password").cloned()),
            private_key_path: q.get("private_key_path").cloned(),
            private_key_passphrase: q.get("private_key_passphrase").cloned(),
            ..SshConfig::default()
        }),
        tags: Vec::new(),
        subscription_id: None,
        share_link_params: Default::default(),
    })
}

/// Parse `anytls://password@host:port?sni=#name`.
fn parse_anytls(rest: &str) -> Result<Profile> {
    let url = parse_uri("anytls", rest)?;
    let password = urlencoding::decode(url.username())?.to_string();
    if password.is_empty() {
        anyhow::bail!("AnyTLS share link missing password");
    }
    let host = url
        .host_str()
        .context("Missing host in AnyTLS URL")?
        .to_string();
    let port = url.port().unwrap_or(443);
    let name = fragment_name(&url, &host)?;
    let q = query_map(&url);
    Ok(Profile {
        id: Uuid::new_v4(),
        name,
        address: host,
        port,
        config: ProtocolConfig::Anytls(AnytlsConfig {
            password,
            tls: extract_tls_common_from_query(&q),
            ..AnytlsConfig::default()
        }),
        tags: Vec::new(),
        subscription_id: None,
        share_link_params: Default::default(),
    })
}

/// Parse `shadowtls://stpassword@host:port?version=3&ss-method=&ss-password=&sni=#name`.
/// There is no widely-deployed standard URI for ShadowTLS; this form follows the
/// closest community convention. The inner Shadowsocks cipher and password are
/// required (they configure the detour outbound that carries actual traffic).
fn parse_shadowtls(rest: &str) -> Result<Profile> {
    let url = parse_uri("shadowtls", rest)?;
    let password = urlencoding::decode(url.username())?.to_string();
    let host = url
        .host_str()
        .context("Missing host in ShadowTLS URL")?
        .to_string();
    let port = url.port().unwrap_or(443);
    let name = fragment_name(&url, &host)?;
    let q = query_map(&url);

    let version = match q.get("version").and_then(|s| s.parse::<u8>().ok()) {
        Some(1) => ShadowtlsVersion::V1,
        Some(2) => ShadowtlsVersion::V2,
        _ => ShadowtlsVersion::V3,
    };
    let method = q
        .get("ss-method")
        .or_else(|| q.get("method"))
        .map(String::as_str)
        .and_then(parse_shadowsocks_cipher)
        .unwrap_or(ShadowsocksCipher::Chacha20IetfPoly1305);
    let ss_password = q
        .get("ss-password")
        .or_else(|| q.get("ss_password"))
        .cloned()
        .unwrap_or_default();
    if ss_password.is_empty() {
        anyhow::bail!("ShadowTLS share link missing ss-password (inner Shadowsocks detour)");
    }

    Ok(Profile {
        id: Uuid::new_v4(),
        name,
        address: host,
        port,
        config: ProtocolConfig::Shadowtls(ShadowtlsConfig {
            version,
            password,
            method,
            ss_password,
            tls: extract_tls_common_from_query(&q),
        }),
        tags: Vec::new(),
        subscription_id: None,
        share_link_params: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::vless_cfg;

    // ---- decode_b64_lenient: accept all four base64 variants users find
    // in the wild (Shadowsocks legacy, SIP002, VMess JSON payloads).

    #[test]
    fn decode_b64_lenient_accepts_url_safe_no_pad() {
        // "hello" → aGVsbG8 (URL-safe, no padding)
        let out = decode_b64_lenient("aGVsbG8").unwrap();
        assert_eq!(out, b"hello");
    }

    #[test]
    fn decode_b64_lenient_accepts_url_safe_padded() {
        // "hello" → aGVsbG8= (URL-safe, padded)
        let out = decode_b64_lenient("aGVsbG8=").unwrap();
        assert_eq!(out, b"hello");
    }

    #[test]
    fn decode_b64_lenient_accepts_standard_no_pad() {
        // 0xFB,0xFF → +/8 (standard alphabet uses +/, URL-safe uses -_)
        let out = decode_b64_lenient("+/8").unwrap();
        assert_eq!(out, vec![0xFB, 0xFF]);
    }

    #[test]
    fn decode_b64_lenient_accepts_standard_padded() {
        let out = decode_b64_lenient("+/8=").unwrap();
        assert_eq!(out, vec![0xFB, 0xFF]);
    }

    #[test]
    fn decode_b64_lenient_strips_whitespace() {
        // Wrapped base64 from email/Telegram pastes.
        let out = decode_b64_lenient("aGVs\nbG8=\n").unwrap();
        assert_eq!(out, b"hello");
    }

    #[test]
    fn decode_b64_lenient_rejects_garbage() {
        // `!` is not in any base64 alphabet — must error, not panic.
        assert!(decode_b64_lenient("not!!base64").is_err());
    }

    #[test]
    fn decode_b64_lenient_empty_input_is_empty_output() {
        assert_eq!(decode_b64_lenient("").unwrap(), Vec::<u8>::new());
    }

    // ---- parse_alpn: comma-separated list, trims whitespace, drops empties.

    #[test]
    fn parse_alpn_single_entry() {
        assert_eq!(parse_alpn("h2"), vec!["h2".to_string()]);
    }

    #[test]
    fn parse_alpn_multiple_entries() {
        assert_eq!(
            parse_alpn("h2,http/1.1"),
            vec!["h2".to_string(), "http/1.1".to_string()],
        );
    }

    #[test]
    fn parse_alpn_trims_whitespace_around_entries() {
        assert_eq!(
            parse_alpn(" h2 , http/1.1 "),
            vec!["h2".to_string(), "http/1.1".to_string()],
        );
    }

    #[test]
    fn parse_alpn_drops_empty_entries() {
        // Trailing / repeated commas in user-pasted URIs.
        assert_eq!(parse_alpn("h2,,"), vec!["h2".to_string()]);
        assert_eq!(parse_alpn(""), Vec::<String>::new());
    }

    // ---- parse_bool_param: accept the wire values producers actually use.

    #[test]
    fn parse_bool_param_accepts_truthy_strings() {
        assert!(parse_bool_param("1"));
        assert!(parse_bool_param("true"));
        assert!(parse_bool_param("yes"));
    }

    #[test]
    fn parse_bool_param_rejects_other_values() {
        assert!(!parse_bool_param("0"));
        assert!(!parse_bool_param("false"));
        assert!(!parse_bool_param("no"));
        assert!(!parse_bool_param(""));
        // Case-sensitive: producers normalise to lowercase.
        assert!(!parse_bool_param("True"));
        assert!(!parse_bool_param("YES"));
    }

    #[test]
    fn parse_long_vless_uri() {
        let uri = r#"vless://671c62c7-6768-4b98-ac6b-572c9c707be0@203.0.113.42:59431?type=grpc&encryption=none&serviceName=&authority=&security=reality&pbk=0IO3LodsrMnhOWh4ogwgdVqYg30CS5-snhFMwldOuAQ&fp=chrome&sni=google.com&sid=f04debc34cbc48a4&spx=%2F#Example-2873vb06"#;
        let profile = parse_share_link(uri).unwrap();
        assert_eq!(profile.protocol(), Protocol::Vless);
        assert_eq!(profile.address, "203.0.113.42");
        assert_eq!(profile.port, 59431);
        assert_eq!(profile.name, "Example-2873vb06");
        let cfg = vless_cfg(&profile);
        assert_eq!(cfg.uuid, "671c62c7-6768-4b98-ac6b-572c9c707be0");
        assert!(cfg.security.is_some());
        let reality = cfg.tls.reality.as_ref().unwrap();
        assert_eq!(
            reality.public_key,
            "0IO3LodsrMnhOWh4ogwgdVqYg30CS5-snhFMwldOuAQ"
        );
        assert_eq!(reality.server_name, "google.com");
        assert_eq!(reality.short_id, "f04debc34cbc48a4");
        assert_eq!(reality.spider_x, "/");
        assert_eq!(cfg.tls.utls_fingerprint.as_deref(), Some("chrome"));
    }

    #[test]
    fn parse_vless_minimal() {
        let uri = "vless://uuid@1.2.3.4:443#Name";
        let profile = parse_share_link(uri).unwrap();
        assert_eq!(profile.address, "1.2.3.4");
        assert_eq!(profile.port, 443);
        assert_eq!(profile.name, "Name");
        let cfg = vless_cfg(&profile);
        assert_eq!(cfg.uuid, "uuid");
        assert!(cfg.tls.reality.is_none());
        assert!(cfg.flow.is_none());
        assert!(cfg.tls.utls_fingerprint.is_none());
        assert!(cfg.transport.is_none());
    }

    #[test]
    fn parse_vless_keeps_ws_path_and_host() {
        let uri = "vless://uuid@cdn.example.com:443?security=tls&type=ws&path=%2Fnl&host=nl.example.com&sni=cdn.example.com#NL";
        let cfg = vless_cfg(&parse_share_link(uri).unwrap()).clone();
        assert_eq!(
            cfg.transport,
            Some(TransportConfig {
                kind: TransportType::Ws,
                path: Some("/nl".into()),
                host: Some("nl.example.com".into()),
                service_name: None,
                headers: HashMap::new(),
            })
        );
    }

    #[test]
    fn parse_share_links_keep_httpupgrade_and_quic() {
        let trojan = parse_share_link(
            "trojan://p@tr.example:443?sni=tr.example&type=httpupgrade&path=%2Fup&host=cdn.example#T",
        )
        .unwrap();
        let ProtocolConfig::Trojan(trojan) = trojan.config else {
            panic!("expected Trojan")
        };
        let vless =
            parse_share_link("vless://uuid@q.example:443?security=tls&type=quic#Q").unwrap();
        let vmess = vmess_b64(serde_json::json!({
            "add": "vm.example", "port": "443", "id": "u", "net": "httpupgrade",
            "host": "cdn.example", "path": "/up", "tls": "tls"
        }));
        let ProtocolConfig::Vmess(vmess) = vmess.config else {
            panic!("expected VMess")
        };

        let httpupgrade = Some(TransportConfig {
            kind: TransportType::HttpUpgrade,
            path: Some("/up".into()),
            host: Some("cdn.example".into()),
            service_name: None,
            headers: HashMap::new(),
        });
        assert_eq!(trojan.transport, httpupgrade);
        assert_eq!(vmess.transport, httpupgrade);
        assert_eq!(
            vless_cfg(&vless).transport.as_ref().map(|t| &t.kind),
            Some(&TransportType::Quic)
        );
    }

    fn params(pairs: &[(&str, serde_json::Value)]) -> BTreeMap<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect()
    }

    fn vmess_b64(body: serde_json::Value) -> Profile {
        parse_share_link(&crate::test_helpers::vmess_b64_link(&body)).unwrap()
    }

    fn vmess_transport(profile: &Profile) -> TransportConfig {
        let ProtocolConfig::Vmess(cfg) = &profile.config else {
            panic!("expected VMess")
        };
        cfg.transport.clone().expect("transport")
    }

    #[test]
    fn parse_share_links_keep_transports_sing_box_lacks() {
        let xhttp = parse_share_link(
            "vless://uuid@x.example:443?security=tls&type=xhttp&path=%2Fx&mode=auto#X",
        )
        .unwrap();
        let kcp = vmess_b64(serde_json::json!({
            "add": "k.example", "port": "443", "id": "u", "net": "kcp",
            "type": "wechat-video", "path": "seed"
        }));

        assert_eq!(
            vless_cfg(&xhttp).transport,
            Some(TransportConfig {
                kind: TransportType::Other("xhttp".into()),
                path: Some("/x".into()),
                host: None,
                service_name: None,
                headers: HashMap::new(),
            })
        );
        assert_eq!(xhttp.share_link_params, params(&[("mode", "auto".into())]));
        assert_eq!(
            vmess_transport(&kcp).kind,
            TransportType::Other("kcp".into())
        );
        assert_eq!(
            kcp.share_link_params,
            params(&[
                ("headerType", "wechat-video".into()),
                ("seed", "seed".into())
            ])
        );
    }

    #[test]
    fn parse_share_links_keep_every_unmapped_param_on_the_profile() {
        let tcp = parse_share_link(
            "vless://uuid@r.example:443?security=reality&pbk=k&sni=s.example&encryption=mlkem768x25519plus.native.0rtt.key&pqv=v#R",
        )
        .unwrap();
        let hysteria2 = parse_share_link(
            "hy2://p@h.example:443?sni=h.example&pinSHA256=AB%3ACD&mport=1000-2000#H",
        )
        .unwrap();
        let shadowsocks = parse_share_link(
            "ss://YWVzLTEyOC1nY206cA@s.example:8388?plugin=obfs-local%3Bobfs%3Dhttp#S",
        )
        .unwrap();

        assert_eq!(
            tcp.share_link_params,
            params(&[
                ("encryption", "mlkem768x25519plus.native.0rtt.key".into()),
                ("pqv", "v".into()),
            ])
        );
        assert_eq!(
            hysteria2.share_link_params,
            params(&[("mport", "1000-2000".into()), ("pinSHA256", "AB:CD".into())])
        );
        assert_eq!(
            shadowsocks.share_link_params,
            params(&[("plugin", "obfs-local;obfs=http".into())])
        );
    }

    #[test]
    fn parse_vmess_b64_keeps_every_unmapped_field_with_its_json_value() {
        let grpc = vmess_b64(serde_json::json!({
            "add": "g.example", "port": 443, "id": "u", "net": "grpc",
            "type": "multi", "path": "svc", "authority": "a.example"
        }));
        let xhttp = vmess_b64(serde_json::json!({
            "add": "x.example", "port": 443, "id": "u", "net": "xhttp", "path": "/x",
            "mode": "packet-up", "x_padding_bytes": "100-1000",
            "extra": { "xPaddingBytes": "100-1000", "headers": { "X": "y" } }
        }));
        let kcp = vmess_b64(serde_json::json!({
            "add": "k.example", "port": 443, "id": "u", "net": "kcp", "mtu": 1350, "tti": 20
        }));

        assert_eq!(vmess_transport(&grpc).service_name.as_deref(), Some("svc"));
        assert_eq!(
            grpc.share_link_params,
            params(&[("authority", "a.example".into()), ("mode", "multi".into())])
        );
        assert_eq!(
            xhttp.share_link_params,
            params(&[
                (
                    "extra",
                    serde_json::json!({ "xPaddingBytes": "100-1000", "headers": { "X": "y" } })
                ),
                ("mode", "packet-up".into()),
                ("x_padding_bytes", "100-1000".into()),
            ])
        );
        assert_eq!(
            kcp.share_link_params,
            params(&[("mtu", 1350.into()), ("tti", 20.into())])
        );
    }

    #[test]
    fn parse_vmess_b64_legacy_quic_moves_encryption_into_quic_keys() {
        let quic = vmess_b64(serde_json::json!({
            "add": "q.example", "port": 443, "id": "u", "net": "quic",
            "type": "none", "host": "aes-128-gcm", "path": "secret"
        }));
        let transport = vmess_transport(&quic);
        assert_eq!(transport.kind, TransportType::Quic);
        assert_eq!((transport.host, transport.path), (None, None));
        assert_eq!(
            quic.share_link_params,
            params(&[
                ("headerType", "none".into()),
                ("key", "secret".into()),
                ("quicSecurity", "aes-128-gcm".into()),
            ])
        );
    }

    #[test]
    fn parse_tcp_http_header_as_the_sing_box_http_transport() {
        let profile = parse_share_link(
            "vless://uuid@h.example:80?type=tcp&headerType=http&host=a.example&path=%2F#H",
        )
        .unwrap();
        assert_eq!(
            vless_cfg(&profile).transport,
            Some(TransportConfig {
                kind: TransportType::Http,
                path: Some("/".into()),
                host: Some("a.example".into()),
                service_name: None,
                headers: HashMap::new(),
            })
        );
        assert_eq!(
            profile.share_link_params,
            params(&[("headerType", "http".into())])
        );
        let plain =
            parse_share_link("vless://uuid@h.example:80?type=tcp&headerType=none#P").unwrap();
        assert!(vless_cfg(&plain).transport.is_none());
    }

    #[test]
    fn parse_share_links_take_tls_from_security_and_cipher_from_encryption() {
        let vless_security =
            |link: &str| vless_cfg(&parse_share_link(link).unwrap()).security.clone();
        let vmess = |link: &str| {
            let ProtocolConfig::Vmess(cfg) = parse_share_link(link).unwrap().config else {
                panic!("expected VMess")
            };
            (cfg.stream_security, cfg.security)
        };

        assert_eq!(
            vless_security("vless://u@v.example:443#V"),
            Some(Security::None)
        );
        assert_eq!(
            vless_security("vless://u@v.example:443?security=none#V"),
            Some(Security::None)
        );
        assert_eq!(
            vless_security("vless://u@v.example:443?pbk=k&sni=s.example#V"),
            Some(Security::Reality)
        );
        assert_eq!(
            vless_security("vless://u@v.example:443?security=none&pbk=k&sni=s.example#V"),
            Some(Security::None)
        );
        assert_eq!(
            vmess("vmess://u@v.example:443?security=none&pbk=k#V").0,
            Some(Security::None)
        );
        assert_eq!(
            vmess("vmess://u@v.example:443?security=tls&encryption=aes-128-gcm#V"),
            (Some(Security::Tls), VmessSecurity::Aes128Gcm)
        );
        assert_eq!(
            vmess(&crate::test_helpers::vmess_b64_link(&serde_json::json!({
                "add": "v.example", "port": 443, "id": "u", "tls": ""
            }))),
            (Some(Security::None), VmessSecurity::Auto)
        );
    }

    #[test]
    fn parse_shadowsocks_follows_sip002() {
        let shadowsocks = |link: &str| {
            let profile = parse_share_link(link).unwrap();
            let ProtocolConfig::Shadowsocks(cfg) = profile.config else {
                panic!("expected Shadowsocks")
            };
            (profile.address, profile.port, cfg.method, cfg.password)
        };

        assert_eq!(
            shadowsocks(
                "ss://2022-blake3-aes-256-gcm:YctPZ6U7xPPcU%2Bgp3u%2B0tx%2FtRizJN9K8y%2BuKlW2qjlI%3D@192.168.100.1:8888#Example3"
            ),
            (
                "192.168.100.1".to_string(),
                8888,
                ShadowsocksCipher::Blake3Aes256Gcm,
                "YctPZ6U7xPPcU+gp3u+0tx/tRizJN9K8y+uKlW2qjlI=".to_string()
            )
        );
        assert_eq!(
            shadowsocks("ss://YWVzLTEyOC1nY206dGVzdA@192.168.100.1:8888/#Example1"),
            (
                "192.168.100.1".to_string(),
                8888,
                ShadowsocksCipher::Aes128Gcm,
                "test".to_string()
            )
        );
        assert_eq!(
            shadowsocks("ss://YWVzLTEyOC1nY206dGVzdA@[2001:db8::1]:8388#V6").0,
            "2001:db8::1"
        );
    }

    #[test]
    fn parse_vless_default_port() {
        let uri = "vless://uuid@example.com#Test";
        let profile = parse_share_link(uri).unwrap();
        assert_eq!(profile.port, 443);
        assert_eq!(profile.address, "example.com");
    }

    #[test]
    fn parse_vless_partial_reality() {
        let uri = "vless://uuid@1.2.3.4:8443?security=reality&pbk=pk123&sni=sni.test#Partial";
        let profile = parse_share_link(uri).unwrap();
        let cfg = vless_cfg(&profile);
        assert_eq!(cfg.security, Some(Security::Reality));
        let reality = cfg.tls.reality.as_ref().unwrap();
        assert_eq!(reality.public_key, "pk123");
        assert_eq!(reality.server_name, "sni.test");
        assert!(reality.short_id.is_empty());
        assert!(reality.spider_x.is_empty());
    }

    #[test]
    fn parse_vless_url_encoded_spx() {
        let uri = "vless://uuid@1.2.3.4?pbk=k&spx=%2Fpath%2Fhere#N";
        let profile = parse_share_link(uri).unwrap();
        let cfg = vless_cfg(&profile);
        assert_eq!(cfg.tls.reality.as_ref().unwrap().spider_x, "/path/here");
    }

    #[test]
    fn parse_accepts_scheme_in_any_case() {
        let profile = parse_share_link("SOCKS5://1.2.3.4:1080#Upper").unwrap();
        assert!(matches!(profile.config, ProtocolConfig::Socks(_)));
    }

    #[test]
    fn parse_empty_name_names_the_profile_after_its_host() {
        use base64::Engine;
        let profile = parse_share_link("socks5://1.2.3.4:1080#").unwrap();
        assert_eq!(profile.name, "1.2.3.4");

        let creds = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("aes-256-gcm:ssecret");
        let profile = parse_share_link(&format!("ss://{creds}@ss.example:8388#")).unwrap();
        assert_eq!(profile.name, "ss.example");

        let body =
            serde_json::json!({ "ps": "", "add": "1.2.3.4", "port": "10086", "id": "vm-id" });
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&body).unwrap());
        let profile = parse_share_link(&format!("vmess://{encoded}")).unwrap();
        assert_eq!(profile.name, "1.2.3.4");
    }

    #[test]
    fn parse_rejects_unknown_scheme() {
        let result = parse_share_link("snake-oil://whatever");
        assert!(result.is_err());
    }

    #[test]
    fn parse_vless_missing_host_fails() {
        let result = parse_share_link("vless://");
        assert!(result.is_err());
    }

    // ---- VMess ----

    #[test]
    fn parse_vmess_uri_form() {
        let uri = "vmess://vm-uuid@1.1.1.1:443?security=tls&type=ws&path=/ws&host=host.example&sni=sni.example#VMess-1";
        let p = parse_share_link(uri).unwrap();
        assert_eq!(p.protocol(), Protocol::Vmess);
        assert_eq!(p.address, "1.1.1.1");
        assert_eq!(p.port, 443);
        assert_eq!(p.name, "VMess-1");
        let ProtocolConfig::Vmess(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(cfg.uuid, "vm-uuid");
        assert_eq!(cfg.tls.server_name.as_deref(), Some("sni.example"));
        let t = cfg.transport.as_ref().unwrap();
        assert_eq!(t.kind, TransportType::Ws);
        assert_eq!(t.path.as_deref(), Some("/ws"));
        assert_eq!(t.host.as_deref(), Some("host.example"));
    }

    #[test]
    fn parse_vmess_b64_json_form() {
        use base64::Engine;
        let body = serde_json::json!({
            "v": "2", "ps": "VMessB64", "add": "1.2.3.4", "port": "10086",
            "id": "vm-id", "aid": "0", "scy": "aes-128-gcm",
            "net": "ws", "type": "none", "host": "h.example", "path": "/wp",
            "tls": "tls", "sni": "sni.example", "alpn": "h2,http/1.1", "fp": "chrome",
        });
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&body).unwrap());
        let p = parse_share_link(&format!("vmess://{}", encoded)).unwrap();
        assert_eq!(p.name, "VMessB64");
        assert_eq!(p.address, "1.2.3.4");
        assert_eq!(p.port, 10086);
        let ProtocolConfig::Vmess(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(cfg.uuid, "vm-id");
        assert_eq!(cfg.security, VmessSecurity::Aes128Gcm);
        assert_eq!(cfg.tls.server_name.as_deref(), Some("sni.example"));
        assert_eq!(cfg.tls.alpn, vec!["h2".to_string(), "http/1.1".to_string()]);
        assert_eq!(cfg.tls.utls_fingerprint.as_deref(), Some("chrome"));
        let t = cfg.transport.as_ref().unwrap();
        assert_eq!(t.kind, TransportType::Ws);
        assert_eq!(t.path.as_deref(), Some("/wp"));
    }

    // ---- Trojan ----

    #[test]
    fn parse_trojan_basic() {
        let uri = "trojan://secret@trojan.example:443?sni=sni.example&type=ws&path=/p#Trojan-1";
        let p = parse_share_link(uri).unwrap();
        assert_eq!(p.protocol(), Protocol::Trojan);
        let ProtocolConfig::Trojan(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(cfg.password, "secret");
        assert_eq!(cfg.tls.server_name.as_deref(), Some("sni.example"));
        assert_eq!(cfg.transport.as_ref().unwrap().kind, TransportType::Ws);
    }

    #[test]
    fn parse_trojan_url_decodes_password() {
        let uri = "trojan://hello%20world@trojan.example:443#T";
        let p = parse_share_link(uri).unwrap();
        let ProtocolConfig::Trojan(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(cfg.password, "hello world");
    }

    // ---- Shadowsocks ----

    #[test]
    fn parse_shadowsocks_sip002() {
        use base64::Engine;
        let creds = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("aes-256-gcm:ssecret");
        let uri = format!("ss://{}@ss.example:8388#SS-1", creds);
        let p = parse_share_link(&uri).unwrap();
        assert_eq!(p.protocol(), Protocol::Shadowsocks);
        let ProtocolConfig::Shadowsocks(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(cfg.method, ShadowsocksCipher::Aes256Gcm);
        assert_eq!(cfg.password, "ssecret");
        assert_eq!(p.address, "ss.example");
        assert_eq!(p.port, 8388);
        assert_eq!(p.name, "SS-1");
    }

    #[test]
    fn parse_shadowsocks_legacy_form() {
        use base64::Engine;
        let blob = base64::engine::general_purpose::STANDARD
            .encode("chacha20-ietf-poly1305:pw@1.1.1.1:8388");
        let uri = format!("ss://{}#Legacy", blob);
        let p = parse_share_link(&uri).unwrap();
        let ProtocolConfig::Shadowsocks(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(cfg.method, ShadowsocksCipher::Chacha20IetfPoly1305);
        assert_eq!(cfg.password, "pw");
        assert_eq!(p.address, "1.1.1.1");
        assert_eq!(p.port, 8388);
        assert_eq!(p.name, "Legacy");
    }

    #[test]
    fn parse_shadowsocks_unsupported_cipher_fails() {
        use base64::Engine;
        let creds = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("aes-128-cfb:pw");
        let uri = format!("ss://{}@1.2.3.4:8388#X", creds);
        assert!(parse_share_link(&uri).is_err());
    }

    // ---- Hysteria2 ----

    #[test]
    fn parse_hysteria2_with_obfs_and_alias() {
        let uri = "hy2://hp@hy.example:443?obfs=salamander&obfs-password=ob&sni=sni.example&insecure=1&alpn=h3#H2";
        let p = parse_share_link(uri).unwrap();
        assert_eq!(p.protocol(), Protocol::Hysteria2);
        let ProtocolConfig::Hysteria2(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(cfg.password, "hp");
        let obfs = cfg.obfs.as_ref().unwrap();
        assert_eq!(obfs.kind, Hysteria2ObfsType::Salamander);
        assert_eq!(obfs.password, "ob");
        assert_eq!(cfg.tls.server_name.as_deref(), Some("sni.example"));
        assert!(cfg.tls.insecure);
        assert_eq!(cfg.tls.alpn, vec!["h3".to_string()]);
    }

    #[test]
    fn parse_hysteria2_reads_s_ui_speeds() {
        let p = parse_share_link("hysteria2://hp@hy.example:443?downmbps=200&upmbps=50&security=tls&sni=sni.example&fastopen=0#S-UI").unwrap();
        let ProtocolConfig::Hysteria2(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!((cfg.up_mbps, cfg.down_mbps), (Some(50), Some(200)));
        assert!(!p.share_link_params.contains_key("upmbps"));
        assert!(!p.share_link_params.contains_key("downmbps"));
    }

    const ECH_CONFIG_LIST: &str = "AEb+DQBCAAAgACB+GRNFbWuFxJAhX6jcFCDTTdUF4NIBCuehfnWnGNsNaQAMAAEAAQABAAIAAQADAAtlY2guZXhhbXBsZQAA";

    fn ech_pem() -> Vec<String> {
        vec![
            "-----BEGIN ECH CONFIGS-----".to_string(),
            ECH_CONFIG_LIST.to_string(),
            "-----END ECH CONFIGS-----".to_string(),
        ]
    }

    #[test]
    fn parse_ech_config_list_from_3x_ui_links() {
        let ech = urlencoding::encode(ECH_CONFIG_LIST);
        let vless = parse_share_link(&format!(
            "vless://671c62c7-6768-4b98-ac6b-572c9c707be0@v.example:443?type=tcp&security=tls&sni=v.example&ech={ech}#3x-ui"
        ))
        .unwrap();
        let ProtocolConfig::Vless(cfg) = &vless.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        let expected = Some(EchSettings {
            enabled: true,
            config: ech_pem(),
        });
        assert_eq!(cfg.tls.ech, expected);
        assert!(vless.share_link_params.is_empty());

        use base64::Engine;
        let json = format!(
            r#"{{"v":"2","ps":"3x-ui","add":"m.example","port":"443","id":"u","net":"tcp","tls":"tls","sni":"m.example","ech":"{ECH_CONFIG_LIST}"}}"#
        );
        let vmess = parse_share_link(&format!(
            "vmess://{}",
            base64::engine::general_purpose::STANDARD.encode(json)
        ))
        .unwrap();
        let ProtocolConfig::Vmess(cfg) = &vmess.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(cfg.tls.ech, expected);
        assert!(vmess.share_link_params.is_empty());
    }

    #[test]
    fn parse_ech_dns_query_enables_ech_and_keeps_the_query() {
        let query = "ech.example+https://1.1.1.1/dns-query";
        let p = parse_share_link(&format!(
            "trojan://pw@t.example:443?security=tls&ech={}#T",
            urlencoding::encode(query)
        ))
        .unwrap();
        let ProtocolConfig::Trojan(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(
            cfg.tls.ech,
            Some(EchSettings {
                enabled: true,
                config: Vec::new()
            })
        );
        assert_eq!(p.share_link_params.get("ech"), Some(&query.into()));
    }

    #[test]
    fn parse_ech_beside_reality_keeps_it_disabled() {
        let p = parse_share_link(&format!(
            "vless://u@v.example:443?security=reality&pbk=key&sni=v.example&ech={}#R",
            urlencoding::encode(ECH_CONFIG_LIST)
        ))
        .unwrap();
        let ProtocolConfig::Vless(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(
            cfg.tls.ech,
            Some(EchSettings {
                enabled: false,
                config: ech_pem()
            })
        );
        assert!(cfg.tls.diagnostics().is_empty());
    }

    // ---- TUIC ----

    #[test]
    fn parse_tuic_basic() {
        let uri = "tuic://tu-uuid:tp@tuic.example:443?congestion_control=cubic&udp_relay_mode=quic&zero_rtt_handshake=1&alpn=h3&sni=sni.example#TUIC";
        let p = parse_share_link(uri).unwrap();
        assert_eq!(p.protocol(), Protocol::Tuic);
        let ProtocolConfig::Tuic(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(cfg.uuid, "tu-uuid");
        assert_eq!(cfg.password, "tp");
        assert_eq!(cfg.congestion_control, TuicCongestion::Cubic);
        assert_eq!(cfg.udp_relay_mode, TuicUdpRelayMode::Quic);
        assert!(cfg.zero_rtt_handshake);
        assert_eq!(cfg.tls.alpn, vec!["h3".to_string()]);
    }

    // ---- SOCKS / HTTP / SSH / AnyTLS / ShadowTLS ----

    #[test]
    fn parse_socks5_with_auth() {
        let p = parse_share_link("socks5://u:p@s.example:1080#S5").unwrap();
        assert_eq!(p.protocol(), Protocol::Socks);
        let ProtocolConfig::Socks(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(cfg.version, SocksVersion::V5);
        assert_eq!(cfg.username.as_deref(), Some("u"));
        assert_eq!(cfg.password.as_deref(), Some("p"));
    }

    #[test]
    fn parse_https_enables_tls() {
        let plain = parse_share_link("http://h.example:8080#HTTP").unwrap();
        let ProtocolConfig::Http(cfg) = &plain.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert!(!tls_has_anything(&cfg.tls));

        let secure = parse_share_link("https://u:p@h.example#HTTPS").unwrap();
        assert_eq!(secure.port, 443);
        assert_eq!(
            parse_share_link("https://h.example:80#HTTPS").unwrap().port,
            80
        );
        let ProtocolConfig::Http(cfg) = &secure.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert!(cfg.tls.server_name.is_some());
        assert_eq!(cfg.username.as_deref(), Some("u"));
        assert_eq!(cfg.password.as_deref(), Some("p"));
    }

    fn tls_has_anything(tls: &TlsCommon) -> bool {
        tls.server_name.is_some()
            || tls.insecure
            || !tls.alpn.is_empty()
            || tls.utls_fingerprint.is_some()
            || tls.reality.is_some()
            || tls.ech.is_some()
    }

    #[test]
    fn parse_ssh_with_password_in_query() {
        let p = parse_share_link("ssh://alice@ssh.example:2222?password=p#SSH").unwrap();
        let ProtocolConfig::Ssh(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(cfg.user, "alice");
        assert_eq!(cfg.password.as_deref(), Some("p"));
        assert_eq!(p.port, 2222);
    }

    #[test]
    fn parse_anytls_basic() {
        let p = parse_share_link("anytls://pp@a.example:443?sni=sni.example#A").unwrap();
        let ProtocolConfig::Anytls(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(cfg.password, "pp");
        assert_eq!(cfg.tls.server_name.as_deref(), Some("sni.example"));
    }

    #[test]
    fn parse_shadowtls_basic() {
        let uri = "shadowtls://stp@st.example:443?version=3&ss-method=2022-blake3-aes-256-gcm&ss-password=isp&sni=sni.example#ST";
        let p = parse_share_link(uri).unwrap();
        let ProtocolConfig::Shadowtls(cfg) = &p.config else {
            panic!("ProtocolConfig variant mismatch")
        };
        assert_eq!(cfg.version, ShadowtlsVersion::V3);
        assert_eq!(cfg.password, "stp");
        assert_eq!(cfg.method, ShadowsocksCipher::Blake3Aes256Gcm);
        assert_eq!(cfg.ss_password, "isp");
        assert_eq!(cfg.tls.server_name.as_deref(), Some("sni.example"));
    }

    #[test]
    fn parse_shadowtls_requires_inner_ss_password() {
        // No ss-password means we can't build the SS detour.
        let uri = "shadowtls://stp@st.example:443?version=3&ss-method=aes-128-gcm#X";
        assert!(parse_share_link(uri).is_err());
    }

    // ---- P0 regression: VLESS plain-TLS share links must preserve
    // sni / alpn / insecure end-to-end.

    #[test]
    fn parse_vless_plain_tls_preserves_sni() {
        let uri = "vless://uuid@1.2.3.4:443?security=tls&sni=cdn.example.com#X";
        let p = parse_share_link(uri).unwrap();
        let cfg = vless_cfg(&p);
        assert_eq!(cfg.security, Some(Security::Tls));
        assert_eq!(cfg.tls.server_name.as_deref(), Some("cdn.example.com"));
        assert!(cfg.tls.reality.is_none());
    }

    #[test]
    fn parse_vless_plain_tls_preserves_alpn_and_insecure() {
        let uri = "vless://uuid@1.2.3.4:443?security=tls&alpn=h2,http/1.1&insecure=1&fp=chrome#X";
        let p = parse_share_link(uri).unwrap();
        let cfg = vless_cfg(&p);
        assert_eq!(cfg.tls.alpn, vec!["h2".to_string(), "http/1.1".to_string()]);
        assert!(cfg.tls.insecure);
        assert_eq!(cfg.tls.utls_fingerprint.as_deref(), Some("chrome"));
    }

    // ---- Error-path coverage: malformed share links must surface as
    // Result::Err rather than panic, and must not silently construct a
    // profile with empty credentials. Closes the "user pasted a broken
    // URI from a Telegram channel" gap that share_link.rs guards.

    #[test]
    fn parse_vmess_b64_rejects_invalid_base64() {
        // `!` is outside every base64 alphabet.
        assert!(parse_share_link("vmess://not!!base64").is_err());
    }

    #[test]
    fn parse_vmess_b64_rejects_missing_id() {
        use base64::Engine;
        // Valid base64 + valid JSON, but no `id` field.
        let json = r#"{"add":"1.1.1.1","port":443,"ps":"X"}"#;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes());
        let err = parse_share_link(&format!("vmess://{b64}"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing 'id'"), "Error was: {err}");
    }

    #[test]
    fn parse_vmess_b64_rejects_missing_address() {
        use base64::Engine;
        let json = r#"{"port":443,"id":"u","ps":"X"}"#;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes());
        let err = parse_share_link(&format!("vmess://{b64}"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing 'add'"), "Error was: {err}");
    }

    #[test]
    fn parse_vmess_b64_rejects_out_of_range_port() {
        use base64::Engine;
        let json = r#"{"add":"1.1.1.1","port":"70000","id":"u","ps":"X"}"#;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes());
        let err = parse_share_link(&format!("vmess://{b64}"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("port 70000 is out of range"),
            "Error was: {err}"
        );
    }

    #[test]
    fn parse_trojan_rejects_empty_password() {
        assert!(parse_share_link("trojan://@trojan.example:443#X").is_err());
    }

    #[test]
    fn parse_hysteria2_rejects_empty_password() {
        assert!(parse_share_link("hysteria2://@hy.example:443#X").is_err());
    }

    #[test]
    fn parse_tuic_rejects_missing_password() {
        // `uuid:` with empty password.
        assert!(parse_share_link("tuic://uuid:@tuic.example:443#X").is_err());
    }

    #[test]
    fn parse_anytls_rejects_empty_password() {
        assert!(parse_share_link("anytls://@a.example:443#X").is_err());
    }

    #[test]
    fn parse_ssh_rejects_missing_user() {
        assert!(parse_share_link("ssh://@ssh.example:22#X").is_err());
    }

    #[test]
    fn parse_socks_version_follows_scheme() {
        for (link, version) in [
            ("socks4://user@s.example:1080#S4", SocksVersion::V4),
            ("socks4a://s.example:1080#S4a", SocksVersion::V4a),
            ("socks5h://s.example:1080#S5h", SocksVersion::V5),
        ] {
            let p = parse_share_link(link).unwrap();
            let ProtocolConfig::Socks(cfg) = &p.config else {
                panic!("ProtocolConfig variant mismatch")
            };
            assert_eq!(cfg.version, version, "{link}");
        }
    }

    #[test]
    fn parse_socks4_rejects_password() {
        assert!(parse_share_link("socks4://user:pass@s.example:1080#S4").is_err());
    }

    #[test]
    fn parse_socks_rejects_missing_port() {
        // socks5:// without an explicit port — there's no protocol default.
        assert!(parse_share_link("socks5://user:pass@socks.example#X").is_err());
    }
}

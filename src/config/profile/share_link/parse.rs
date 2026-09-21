use std::collections::HashMap;

use anyhow::{Context, Result};
use url::Url;
use uuid::Uuid;

use crate::config::profile::*;

/// Parse a share link text into a Profile. Dispatches on URI scheme.
pub fn parse_share_link(text: &str) -> Result<Profile> {
    let trimmed = text.trim();
    let scheme_end = trimmed.find("://").context("Missing URI scheme")?;
    let scheme = &trimmed[..scheme_end];
    let rest = &trimmed[scheme_end + 3..];

    match scheme {
        "vless" => parse_vless(rest),
        "vmess" => parse_vmess(rest),
        "trojan" => parse_trojan(rest),
        "ss" => parse_shadowsocks(rest),
        "hysteria2" | "hy2" => parse_hysteria2(rest),
        "tuic" => parse_tuic(rest),
        "socks" | "socks5" => parse_socks(rest),
        "http" | "https" => parse_http(rest, scheme == "https"),
        "ssh" => parse_ssh(rest),
        "anytls" => parse_anytls(rest),
        "shadowtls" => parse_shadowtls(rest),
        other => anyhow::bail!("Unsupported share link scheme: {other}://"),
    }
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
    Ok(match url.fragment() {
        Some(f) => urlencoding::decode(f)?.to_string(),
        None => fallback.to_string(),
    })
}

fn decode_b64_lenient(s: &str) -> Result<Vec<u8>> {
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
    match s {
        "grpc" => Some(TransportType::Grpc),
        "ws" => Some(TransportType::Ws),
        "http" => Some(TransportType::Http),
        _ => None,
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
    tls
}

fn extract_transport_from_query(
    q: &std::collections::HashMap<String, String>,
) -> Option<TransportConfig> {
    let kind = parse_transport_type(q.get("type")?)?;
    Some(TransportConfig {
        kind,
        path: q.get("path").cloned(),
        host: q.get("host").cloned(),
        service_name: q.get("serviceName").cloned(),
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
    if let Some(security) = query.get("security") {
        cfg.security = match security.as_str() {
            "reality" => Some(Security::Reality),
            "tls" => Some(Security::Tls),
            _ => None,
        };
    }
    if let Some(transport) = query.get("type") {
        cfg.transport_type = parse_transport_type(transport);
    }
    if let Some(service_name) = query.get("serviceName") {
        cfg.transport_service_name = Some(service_name.clone());
    }
    // Shared with VMess/Trojan: handles sni / alpn / fp / insecure and
    // routes pbk+sid+spx+sni into a `RealitySettings` block when present.
    cfg.tls = extract_tls_common_from_query(&query);

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

    let security = query
        .get("scy")
        .or_else(|| query.get("security"))
        .map(String::as_str);
    let cfg = VmessConfig {
        uuid,
        alter_id: query.get("aid").and_then(|v| v.parse().ok()).unwrap_or(0),
        security: parse_vmess_security(security),
        tls: extract_tls_common_from_query(&query),
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
        .context("VMess: missing 'port'")? as u16;
    let uuid = v["id"].as_str().context("VMess: missing 'id'")?.to_string();
    let name = v["ps"].as_str().unwrap_or(&host).to_string();
    let aid = v["aid"]
        .as_u64()
        .or_else(|| v["aid"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0) as u32;
    let security = v["scy"].as_str();

    let mut tls = TlsCommon::default();
    if v["tls"].as_str() == Some("tls") {
        tls.server_name = v["sni"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| v["host"].as_str().filter(|s| !s.is_empty()))
            .map(|s| s.to_string());
        if let Some(alpn) = v["alpn"].as_str() {
            tls.alpn = parse_alpn(alpn);
        }
        if let Some(fp) = v["fp"].as_str().filter(|s| !s.is_empty()) {
            tls.utls_fingerprint = Some(fp.to_string());
        }
    }

    let net = v["net"].as_str().unwrap_or("tcp");
    let transport = match net {
        "ws" => Some(TransportConfig {
            kind: TransportType::Ws,
            path: v["path"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from),
            host: v["host"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from),
            service_name: None,
            headers: HashMap::new(),
        }),
        "grpc" => Some(TransportConfig {
            kind: TransportType::Grpc,
            path: None,
            host: None,
            service_name: v["path"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from),
            headers: HashMap::new(),
        }),
        "h2" | "http" => Some(TransportConfig {
            kind: TransportType::Http,
            path: v["path"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from),
            host: v["host"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from),
            service_name: None,
            headers: HashMap::new(),
        }),
        _ => None,
    };

    Ok(Profile {
        id: Uuid::new_v4(),
        name,
        address: host,
        port,
        config: ProtocolConfig::Vmess(VmessConfig {
            uuid,
            alter_id: aid,
            security: parse_vmess_security(security),
            tls,
            transport,
            ..VmessConfig::default()
        }),
        tags: Vec::new(),
        subscription_id: None,
    })
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
    })
}

/// Parse Shadowsocks share link. Supports SIP002 (`ss://b64(method:pw)@host:port#name`)
/// and the legacy fully-base64 form (`ss://b64(method:pw@host:port)#name`).
fn parse_shadowsocks(rest: &str) -> Result<Profile> {
    // Split fragment off so we don't accidentally base64-decode the name.
    let (body, fragment) = match rest.find('#') {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    // Strip query (and plugin) — we don't model SS plugins yet.
    let body = match body.find('?') {
        Some(i) => &body[..i],
        None => body,
    };

    let (method, password, host, port) = if let Some(at) = body.rfind('@') {
        let userinfo = &body[..at];
        let hostport = &body[at + 1..];
        let creds = decode_b64_lenient(userinfo)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_else(|| userinfo.to_string());
        let (m, p) = creds
            .split_once(':')
            .context("Shadowsocks: expected method:password")?;
        let (h, port_s) = hostport
            .rsplit_once(':')
            .context("Shadowsocks: expected host:port")?;
        let port: u16 = port_s.parse().context("Shadowsocks: invalid port")?;
        (m.to_string(), p.to_string(), h.to_string(), port)
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
        let (h, port_s) = hostport
            .rsplit_once(':')
            .context("Shadowsocks: expected host:port")?;
        let port: u16 = port_s.parse().context("Shadowsocks: invalid port")?;
        (m.to_string(), p.to_string(), h.to_string(), port)
    };

    let cipher = parse_shadowsocks_cipher(&method)
        .with_context(|| format!("Unsupported Shadowsocks cipher: {method}"))?;
    let name = match fragment {
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
    })
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
            up_mbps: query.get("up").and_then(|s| s.parse().ok()),
            down_mbps: query.get("down").and_then(|s| s.parse().ok()),
            obfs,
            tls: extract_tls_common_from_query(&query),
        }),
        tags: Vec::new(),
        subscription_id: None,
    })
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
    })
}

/// Parse `socks5://user:pass@host:port#name` (also `socks://`).
fn parse_socks(rest: &str) -> Result<Profile> {
    let url = parse_uri("socks5", rest)?;
    let host = url
        .host_str()
        .context("Missing host in SOCKS URL")?
        .to_string();
    let port = url.port().context("Missing port in SOCKS URL")?;
    let name = fragment_name(&url, &host)?;
    let user = url.username();
    let pass = url.password();
    Ok(Profile {
        id: Uuid::new_v4(),
        name,
        address: host,
        port,
        config: ProtocolConfig::Socks(SocksConfig {
            version: SocksVersion::V5,
            username: (!user.is_empty())
                .then(|| urlencoding::decode(user).unwrap_or_default().to_string()),
            password: pass.map(|p| urlencoding::decode(p).unwrap_or_default().to_string()),
        }),
        tags: Vec::new(),
        subscription_id: None,
    })
}

/// Parse `http://user:pass@host:port#name` / `https://...#name`. The `tls` flag
/// is set when the scheme is `https`.
fn parse_http(rest: &str, tls_enabled: bool) -> Result<Profile> {
    let url = parse_uri("http", rest)?;
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
        assert!(cfg.transport_type.is_none());
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
    fn parse_socks_rejects_missing_port() {
        // socks5:// without an explicit port — there's no protocol default.
        assert!(parse_share_link("socks5://user:pass@socks.example#X").is_err());
    }
}

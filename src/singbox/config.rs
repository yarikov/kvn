use anyhow::{Context, bail};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use crate::config::profile::{
    ActiveDns, DnsRule, DnsServer, DnsStrategy, FINALMASK_PARAM, FakeIpServer, GeoRegion,
    NaiveConfig, Profile, ProtocolConfig, RoutedService, RoutingMode, ServiceRoute, Settings,
    SocksConfig, SocksVersion, read_stream_finalmask,
};
use crate::singbox::outbound::{
    build_anytls_outbound, build_http_outbound, build_hysteria2_outbound, build_naive_outbound,
    build_shadowsocks_outbound, build_shadowtls_outbounds, build_socks_outbound,
    build_ssh_outbound, build_trojan_outbound, build_tuic_outbound, build_vless_outbound,
    build_vmess_outbound, proxy_carries_udp,
};

/// Availability of local geoip/geosite rule-sets used when building routes.
/// A region is present only when both its files exist.
#[derive(Debug, Clone, Default)]
pub struct GeoAvailability {
    /// `(geoip_path, geosite_path)` per region. Populated by the runner from
    /// disk; `build_route` reads via `get`. Regions without rule-sets
    /// (`GeoRegion::Global`) never appear here.
    pub regions: HashMap<GeoRegion, (PathBuf, PathBuf)>,
    /// `(tag, path)` per rule-set, in rule order, for each service whose
    /// defined rule-set files all exist on disk.
    pub services: HashMap<RoutedService, Vec<(&'static str, PathBuf)>>,
}

impl GeoAvailability {
    pub fn get(&self, region: GeoRegion) -> Option<&(PathBuf, PathBuf)> {
        self.regions.get(&region)
    }

    /// All rule-sets available with dummy paths under `/geo/`, for tests.
    #[cfg(test)]
    pub fn all() -> Self {
        let mut regions = HashMap::new();
        for region in GeoRegion::ALL {
            if let Some(a) = crate::geo::region_assets(region) {
                regions.insert(
                    region,
                    (
                        PathBuf::from("/geo").join(a.geoip.filename),
                        PathBuf::from("/geo").join(a.geosite.filename),
                    ),
                );
            }
        }
        let mut services = HashMap::new();
        for service in RoutedService::ALL {
            services.insert(
                service,
                crate::geo::service_assets(service)
                    .present()
                    .into_iter()
                    .map(|a| (a.tag(), PathBuf::from("/geo").join(a.filename)))
                    .collect(),
            );
        }
        Self { regions, services }
    }
}

/// Generate a minimal sing-box config for latency testing: one SOCKS5 inbound
/// on `socks_port` (localhost only), the profile's outbound, and a route that
/// sends all traffic to that outbound.
pub fn generate_test_config(profile: &Profile, socks_port: u16) -> anyhow::Result<Value> {
    let outbounds = build_outbound(profile)?;
    Ok(json!({
        "log": { "level": "warn" },
        "inbounds": [{
            "type": "socks",
            "tag": "socks-in",
            "listen": "127.0.0.1",
            "listen_port": socks_port
        }],
        "outbounds": outbounds,
        "route": { "final": "proxy", "default_mark": 666, "auto_detect_interface": true }
    }))
}

/// Generate a complete sing-box JSON configuration from a profile.
/// Uses the sing-box 1.14+ format.
pub fn generate_config(
    profile: &Profile,
    settings: &Settings,
    geo: &GeoAvailability,
    clash_api_port: u16,
) -> anyhow::Result<Value> {
    let mut proxy_outbounds = build_outbound(profile)?;
    proxy_outbounds.push(json!({ "type": "direct", "tag": "direct" }));
    let routing_mode = settings.geo_routing.mode();
    let active_dns = settings.dns.active().with_context(|| {
        format!(
            "dns.current_preset {:?} does not name a DNS preset",
            settings.dns.current_preset
        )
    })?;
    let upstreams = DnsUpstreams::new(active_dns, &routing_mode, profile);
    let bootstrap = upstreams.bootstrap();
    let resolvers = RouteResolvers {
        default_domain: json!({
            "server": bootstrap.tag,
            "strategy": settings.dns.strategy.as_str(),
        }),
        destination: json!({
            "server": upstreams.active.final_server,
            "strategy": settings.dns.strategy.as_str(),
        }),
    };
    let (route, rule_sets) = build_route(
        &routing_mode,
        resolvers,
        geo,
        &settings.geo_routing.service_routes,
    );
    let available_rule_sets: HashSet<&str> = rule_sets
        .iter()
        .filter_map(|rule_set| rule_set["tag"].as_str())
        .collect();
    let dns = build_dns(
        &upstreams,
        bootstrap,
        &settings.dns.strategy,
        &available_rule_sets,
        &ech_dns_lookup_names(&proxy_outbounds),
    )?;

    let mut cache_file = json!({ "enabled": true });
    if let Some(path) = crate::paths::singbox_cache_path() {
        cache_file["path"] = json!(path.to_string_lossy());
    }
    if upstreams.active.fakeip_catch_all {
        cache_file["store_fakeip"] = json!(true);
    }

    let mut config = json!({
        "log": {
            "level": crate::config::profile::normalized_log_level(&settings.logs.level),
            "output": crate::paths::singbox_log_path().to_string_lossy(),
            "timestamp": true
        },
        "dns": dns,
        "inbounds": [
            {
                "type": "tun",
                "tag": "tun-in",
                "interface_name": settings.tun_interface.clone(),
                "address": ["10.222.0.1/30"],
                "mtu": 1420,
                "auto_route": true,
                "strict_route": true,
                "endpoint_independent_nat": true,
                "stack": "gvisor"
            }
        ],
        "outbounds": proxy_outbounds,
        "route": route,
        "experimental": {
            "cache_file": cache_file,
            "clash_api": {
                "external_controller": format!("127.0.0.1:{clash_api_port}")
            }
        }
    });

    // Merge rule_sets into route if any exist.
    if !rule_sets.is_empty() {
        config["route"]["rule_set"] = json!(rule_sets);
    }

    Ok(config)
}

/// Build the `dns` section from the active DNS preset. Maps onto sing-box
/// 1.14's `dns` schema: the fake-IP server carries its own ranges (no legacy
/// top-level `dns.fakeip` block), and when fake-IP is on we append an
/// `A`/`AAAA`-routing rule so the fake-IP server actually receives queries.
fn build_dns(
    upstreams: &DnsUpstreams,
    bootstrap: BootstrapResolver,
    strategy: &DnsStrategy,
    available_rule_sets: &HashSet<&str>,
    ech_lookup_names: &[String],
) -> anyhow::Result<Value> {
    let active = &upstreams.active;
    let mut servers: Vec<Value> = active
        .servers
        .iter()
        .map(|server| {
            build_dns_server(
                server,
                upstreams.hostname_resolver(server, &bootstrap),
                upstreams.is_tunnelled(server),
            )
        })
        .collect();
    servers.extend(active.fakeip.as_ref().map(fakeip_server_value));
    let active_rules = active_dns_rules(active, available_rule_sets);
    let ech_rule = ech_lookup_rule(
        ech_lookup_names,
        &bootstrap.tag,
        uses_ip_rule_set(&active_rules),
    );
    servers.extend(bootstrap.server);
    let mut block = Map::new();
    block.insert("servers".to_string(), Value::Array(servers));

    let mut rules: Vec<Value> = ech_rule.into_iter().collect();
    rules.extend(active_rules.iter().map(|(_, rule)| build_dns_rule(rule)));
    rules.extend(fakeip_catch_all_rule(active, &active_rules)?);
    if !rules.is_empty() {
        block.insert("rules".to_string(), Value::Array(rules));
    }

    block.insert("final".to_string(), json!(active.final_server));
    block.insert("strategy".to_string(), json!(strategy.as_str()));
    Ok(Value::Object(block))
}

fn ech_dns_lookup_names(outbounds: &[Value]) -> Vec<String> {
    outbounds
        .iter()
        .map(|outbound| &outbound["tls"])
        .filter(|tls| tls["ech"]["enabled"] == json!(true) && tls["ech"].get("config").is_none())
        .filter_map(|tls| tls["server_name"].as_str())
        .map(|name| name.trim_end_matches('.').to_ascii_lowercase())
        .filter(|name| !name.is_empty() && name.parse::<std::net::IpAddr>().is_err())
        .collect()
}

fn ech_lookup_rule(
    names: &[String],
    bootstrap_tag: &str,
    legacy_address_filters: bool,
) -> Option<Value> {
    if names.is_empty() {
        return None;
    }
    let mut rule = json!({
        "domain": names,
        "server": bootstrap_tag,
    });
    if !legacy_address_filters {
        rule["query_type"] = json!(["HTTPS"]);
    }
    Some(rule)
}

fn uses_ip_rule_set(active_rules: &[(usize, &DnsRule)]) -> bool {
    active_rules.iter().any(|(_, rule)| {
        rule.rule_set
            .iter()
            .any(|tag| crate::geo::is_ip_rule_set_tag(tag))
    })
}

fn active_dns_rules<'a>(
    active: &'a ActiveDns,
    available_rule_sets: &HashSet<&str>,
) -> Vec<(usize, &'a DnsRule)> {
    active
        .rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| {
            rule.rule_set
                .iter()
                .all(|tag| available_rule_sets.contains(tag.as_str()))
        })
        .collect()
}

fn fakeip_catch_all_rule(
    active: &ActiveDns,
    active_rules: &[(usize, &DnsRule)],
) -> anyhow::Result<Option<Value>> {
    let Some(fakeip) = active.fakeip.as_ref().filter(|_| active.fakeip_catch_all) else {
        return Ok(None);
    };
    let fakeip_tag = fakeip.tag.as_str();
    if active_rules
        .iter()
        .any(|(_, rule)| rule.server == fakeip_tag)
    {
        return Ok(None);
    }
    // TODO: translate IP rule-set DNS rules to `evaluate` + `match_response`
    // before supporting sing-box 1.16, which removes legacy address filters.
    let ip_rule_set = active_rules.iter().find_map(|(idx, rule)| {
        rule.rule_set
            .iter()
            .find(|tag| crate::geo::is_ip_rule_set_tag(tag))
            .map(|tag| (idx, tag))
    });
    if let Some((idx, tag)) = ip_rule_set {
        bail!(
            "rules[{idx}] of the active DNS preset uses the IP rule-set {tag:?}, which sing-box cannot combine with the automatic fake-IP rule; add a rule with \"server\": {fakeip_tag:?} to that preset, or remove {tag:?} from the rule"
        );
    }
    Ok(Some(json!({
        "query_type": ["A", "AAAA"],
        "server": fakeip_tag,
    })))
}

struct DnsUpstreams {
    active: ActiveDns,
    through_proxy: bool,
    carries_udp: bool,
}

struct BootstrapResolver {
    tag: String,
    server: Option<Value>,
}

impl DnsUpstreams {
    fn new(active: ActiveDns, routing_mode: &RoutingMode, profile: &Profile) -> Self {
        Self {
            active,
            through_proxy: final_outbound(routing_mode) == "proxy",
            carries_udp: proxy_carries_udp(&profile.config),
        }
    }

    fn udp_servers_kept_out_of_tunnel(&self) -> impl Iterator<Item = &DnsServer> {
        self.active.servers.iter().filter(|server| {
            !self.carries_udp && is_udp_server(server) && self.is_public_behind_proxy(server)
        })
    }

    fn is_tunnelled(&self, server: &DnsServer) -> bool {
        self.is_public_behind_proxy(server) && (self.carries_udp || !is_udp_server(server))
    }

    fn is_public_behind_proxy(&self, server: &DnsServer) -> bool {
        server
            .address()
            .is_some_and(|address| self.through_proxy && !is_local_address(address))
    }

    fn bootstrap(&self) -> BootstrapResolver {
        match self.active.final_server_entry() {
            Some(final_server) if self.is_tunnelled(final_server) => {
                let tag = self.active.unused_tag("bootstrap");
                let mut server = dns_server_value(final_server);
                server["tag"] = json!(tag);
                if final_server.hostname().is_some()
                    && let Some(local) = self.active.local_server_tag()
                {
                    server["domain_resolver"] = json!(local);
                }
                BootstrapResolver {
                    tag,
                    server: Some(server),
                }
            }
            _ => BootstrapResolver {
                tag: self.active.final_server.clone(),
                server: None,
            },
        }
    }

    fn hostname_resolver<'b>(
        &'b self,
        server: &DnsServer,
        bootstrap: &'b BootstrapResolver,
    ) -> Option<&'b str> {
        if server.tag() == bootstrap.tag {
            self.active.local_server_tag()
        } else {
            Some(&bootstrap.tag)
        }
    }
}

const CERTIFICATE_FINGERPRINT_PIN_KEYS: [&str; 2] = ["pinSHA256", "pcs"];

pub fn certificate_pin_warning(profile: &Profile) -> Option<String> {
    let key = CERTIFICATE_FINGERPRINT_PIN_KEYS.into_iter().find(|key| {
        profile
            .share_link_params
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|pin| !pin.is_empty())
    })?;
    Some(format!(
        "{key} is not checked: sing-box pins certificate public keys, not the certificate fingerprint the link gives, so the server certificate is verified only as the profile's TLS settings allow (not at all with insecure)"
    ))
}

pub fn ech_dns_warning(profile: &Profile, settings: &Settings) -> Option<String> {
    let outbounds = build_outbound(profile).ok()?;
    let name = ech_dns_lookup_names(&outbounds).into_iter().next()?;
    let active = settings.dns.active()?;
    let final_server = active.final_server_entry()?;
    let encrypted = matches!(
        final_server,
        DnsServer::Tls { .. } | DnsServer::Https { .. } | DnsServer::Quic { .. }
    );
    (!encrypted).then(|| {
        format!(
            "ECH config for {name:?} is looked up over unencrypted DNS ({}) before connecting, so the server name ECH hides is visible on the network; choose a DoH, DoT or DoQ DNS preset, or use a link that carries the ECH config",
            final_server.kind_label()
        )
    })
}

pub fn dns_bypass_warning(profile: &Profile, settings: &Settings) -> Option<String> {
    let active = settings.dns.active()?;
    let upstreams = DnsUpstreams::new(active, &settings.geo_routing.mode(), profile);
    let servers: Vec<String> = upstreams
        .udp_servers_kept_out_of_tunnel()
        .map(|server| format!("{:?} ({})", server.tag(), server.kind_label()))
        .collect();
    (!servers.is_empty()).then(|| {
        format!(
            "DNS server {} bypasses the VPN: the {} profile {:?} cannot carry UDP, so its queries are sent directly; use an https, tls or tcp DNS server to keep them in the tunnel",
            servers.join(", "),
            profile.protocol(),
            profile.name,
        )
    })
}

fn is_udp_server(server: &DnsServer) -> bool {
    matches!(server, DnsServer::Udp { .. } | DnsServer::Quic { .. })
}

fn is_local_address(address: &str) -> bool {
    match address.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || is_cgnat(ip)
        }
        Ok(IpAddr::V6(ip)) => {
            ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
        }
        Err(_) => false,
    }
}

fn is_cgnat(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    first == 100 && (64..=127).contains(&second)
}

fn build_dns_server(server: &DnsServer, hostname_resolver: Option<&str>, tunnelled: bool) -> Value {
    let mut value = dns_server_value(server);
    if server.hostname().is_some()
        && let Some(resolver) = hostname_resolver
    {
        value["domain_resolver"] = json!(resolver);
    }
    if tunnelled {
        value["detour"] = json!("proxy");
    }
    value
}

fn fakeip_server_value(fakeip: &FakeIpServer) -> Value {
    json!({
        "tag": fakeip.tag,
        "type": "fakeip",
        "inet4_range": fakeip.ranges.inet4_range,
        "inet6_range": fakeip.ranges.inet6_range,
    })
}

fn dns_server_value(server: &DnsServer) -> Value {
    match server {
        DnsServer::Local { tag } => json!({ "tag": tag, "type": "local" }),
        DnsServer::Udp {
            tag,
            server,
            server_port,
        } => server_with_port("udp", tag, server, *server_port, None),
        DnsServer::Tcp {
            tag,
            server,
            server_port,
        } => server_with_port("tcp", tag, server, *server_port, None),
        DnsServer::Tls {
            tag,
            server,
            server_port,
        } => server_with_port("tls", tag, server, *server_port, None),
        DnsServer::Https {
            tag,
            server,
            server_port,
            path,
        } => server_with_port("https", tag, server, *server_port, Some(path.as_str())),
        DnsServer::Quic {
            tag,
            server,
            server_port,
        } => server_with_port("quic", tag, server, *server_port, None),
    }
}

fn server_with_port(
    ty: &str,
    tag: &str,
    server: &str,
    port: Option<u16>,
    doh_path: Option<&str>,
) -> Value {
    let mut obj = Map::new();
    obj.insert("tag".to_string(), json!(tag));
    obj.insert("type".to_string(), json!(ty));
    obj.insert("server".to_string(), json!(server));
    if let Some(p) = port {
        obj.insert("server_port".to_string(), json!(p));
    }
    if let Some(path) = doh_path {
        obj.insert("path".to_string(), json!(path));
    }
    Value::Object(obj)
}

fn build_dns_rule(rule: &DnsRule) -> Value {
    let mut obj = Map::new();
    if !rule.domain.is_empty() {
        obj.insert("domain".to_string(), json!(rule.domain));
    }
    if !rule.domain_suffix.is_empty() {
        obj.insert("domain_suffix".to_string(), json!(rule.domain_suffix));
    }
    if !rule.domain_keyword.is_empty() {
        obj.insert("domain_keyword".to_string(), json!(rule.domain_keyword));
    }
    if !rule.domain_regex.is_empty() {
        obj.insert("domain_regex".to_string(), json!(rule.domain_regex));
    }
    if !rule.rule_set.is_empty() {
        obj.insert("rule_set".to_string(), json!(rule.rule_set));
    }
    obj.insert("server".to_string(), json!(rule.server));
    if rule.disable_cache {
        obj.insert("disable_cache".to_string(), json!(true));
    }
    Value::Object(obj)
}

struct RouteResolvers {
    default_domain: Value,
    destination: Value,
}

#[derive(Default)]
struct RuleSetRules {
    domain: Vec<Value>,
    ip: Vec<Value>,
    definitions: Vec<Value>,
}

impl RuleSetRules {
    fn push(&mut self, tag: &str, path: &PathBuf, outbound: &str) {
        let rule = json!({ "rule_set": [tag], "outbound": outbound });
        if crate::geo::is_ip_rule_set_tag(tag) {
            self.ip.push(rule);
        } else {
            self.domain.push(rule);
        }
        self.definitions.push(json!({
            "tag": tag,
            "type": "local",
            "format": "binary",
            "path": path,
        }));
    }
}

/// Build route object and local rule-sets based on routing mode.
/// Returns (route_value, rule_sets_vec).
fn build_route(
    routing_mode: &RoutingMode,
    resolvers: RouteResolvers,
    geo: &GeoAvailability,
    service_routes: &HashMap<RoutedService, ServiceRoute>,
) -> (Value, Vec<Value>) {
    let mut rules = vec![
        json!({
            "ip_version": 6,
            "action": "reject"
        }),
        json!({
            "inbound": ["tun-in"],
            "port": 53,
            "action": "hijack-dns"
        }),
        json!({
            "ip_cidr": ["10.222.0.0/30"],
            "outbound": "direct"
        }),
    ];

    // Per-service routing overrides, placed before the regional geo rules so
    // an explicit override wins in every routing mode (`Direct` beats a
    // region's `Only` → proxy; `Proxy` beats its `Bypass` → direct). A
    // service is skipped while its rule-set files are missing — a pending
    // download must never fail the connection. Iterates `RoutedService::ALL`,
    // not the map: `HashMap` iteration order is nondeterministic and would
    // make the generated config unstable across runs.
    let mut services = RuleSetRules::default();
    for service in RoutedService::ALL {
        let outbound = match service_routes.get(&service).copied().unwrap_or_default() {
            ServiceRoute::Disabled => continue,
            ServiceRoute::Proxy => "proxy",
            ServiceRoute::Direct => "direct",
        };
        let Some(entries) = geo.services.get(&service) else {
            continue;
        };
        for (tag, path) in entries {
            services.push(tag, path, outbound);
        }
    }

    let mut region = RuleSetRules::default();
    let mut region_private_rule = None;
    match routing_mode {
        RoutingMode::Global => {}
        RoutingMode::Bypass(geo_region) | RoutingMode::Only(geo_region)
            if !matches!(geo_region, GeoRegion::Global) =>
        {
            // `Bypass` sends matching traffic out the direct outbound; `Only`
            // sends matching traffic through the proxy.
            let outbound = if matches!(routing_mode, RoutingMode::Bypass(_)) {
                "direct"
            } else {
                "proxy"
            };
            region_private_rule = Some(json!({
                "ip_is_private": true,
                "outbound": "direct",
            }));
            if let (Some(assets), Some((geoip_path, geosite_path))) =
                (crate::geo::region_assets(*geo_region), geo.get(*geo_region))
            {
                // Existing order: geosite rule first, then geoip. Tags are
                // derived from filenames (`geoip-ru.srs` → `geoip-ru`).
                region.push(assets.geosite.tag(), geosite_path, outbound);
                region.push(assets.geoip.tag(), geoip_path, outbound);
            }
        }
        // Bypass(Global) / Only(Global) are unreachable through `available()`
        // but the enum permits them — degrade to a no-op.
        RoutingMode::Bypass(_) | RoutingMode::Only(_) => {}
    }

    // NOTE: TUN connections carry only a destination IP, so domain rule-sets
    // match only after `sniff`; with fake-IP the destination is a domain, so
    // IP rule-sets match only after `resolve`. A sniffed name takes precedence
    // over the fake-IP name in sing-box's domain matching, so `sniff` runs only
    // while the destination has no name; an SNI that differs from the queried
    // name (ECH, domain fronting) must not move a connection to another rule.
    if !services.domain.is_empty() || !region.domain.is_empty() {
        rules.push(json!({
            "domain_regex": ["."],
            "invert": true,
            "action": "sniff",
        }));
    }
    rules.append(&mut services.domain);
    let mut resolve = (!services.ip.is_empty() || !region.ip.is_empty()).then(|| {
        let mut rule = resolvers.destination;
        rule["action"] = json!("resolve");
        rule
    });
    if !services.ip.is_empty() {
        rules.extend(resolve.take());
    }
    rules.append(&mut services.ip);
    rules.extend(region_private_rule);
    rules.append(&mut region.domain);
    rules.extend(resolve);
    rules.append(&mut region.ip);
    let mut rule_sets = services.definitions;
    rule_sets.append(&mut region.definitions);

    // `default_mark` tags every packet sing-box sends to the network with a
    // Linux fwmark. The kvn-tui kill switch's nft ruleset allowlists this mark,
    // so traffic from sing-box's `direct` outbound (used by Bypass/Only routing
    // modes) can reach the physical interface while everything else is dropped.
    let route = json!({
        "default_domain_resolver": resolvers.default_domain,
        "rules": rules,
        "auto_detect_interface": true,
        "default_mark": 666,
        "final": final_outbound(routing_mode)
    });

    (route, rule_sets)
}

fn final_outbound(routing_mode: &RoutingMode) -> &'static str {
    match routing_mode {
        RoutingMode::Only(_) => "direct",
        _ => "proxy",
    }
}

/// Build the proxy outbound list for a profile. Most protocols return a
/// single outbound tagged `proxy`; ShadowTLS returns two (the wrapper plus
/// an inner Shadowsocks detour, with the SS half tagged `proxy`).
fn build_outbound(profile: &Profile) -> anyhow::Result<Vec<Value>> {
    let mut outbounds = match &profile.config {
        ProtocolConfig::Vless(cfg) => vec![build_vless_outbound(profile, cfg)?],
        ProtocolConfig::Vmess(cfg) => vec![build_vmess_outbound(profile, cfg)?],
        ProtocolConfig::Trojan(cfg) => vec![build_trojan_outbound(profile, cfg)?],
        ProtocolConfig::Shadowsocks(cfg) => vec![build_shadowsocks_outbound(profile, cfg)?],
        ProtocolConfig::Hysteria2(cfg) => vec![build_hysteria2_outbound(profile, cfg)?],
        ProtocolConfig::Tuic(cfg) => vec![build_tuic_outbound(profile, cfg)?],
        ProtocolConfig::Shadowtls(cfg) => build_shadowtls_outbounds(profile, cfg)?,
        ProtocolConfig::Anytls(cfg) => vec![build_anytls_outbound(profile, cfg)?],
        ProtocolConfig::Naive(cfg) => vec![build_naive_outbound(profile, cfg)?],
        ProtocolConfig::Socks(cfg) => vec![build_socks_outbound(profile, cfg)?],
        ProtocolConfig::Http(cfg) => vec![build_http_outbound(profile, cfg)?],
        ProtocolConfig::Ssh(cfg) => vec![build_ssh_outbound(profile, cfg)?],
    };
    apply_stream_finalmask(profile, &mut outbounds)?;
    refuse_unverifiable_certificate_name(profile, &outbounds)?;
    Ok(outbounds)
}

fn refuse_unverifiable_certificate_name(
    profile: &Profile,
    outbounds: &[Value],
) -> anyhow::Result<()> {
    let Some(vcn) = profile.share_link_params.get("vcn").and_then(Value::as_str) else {
        return Ok(());
    };
    let names: Vec<&str> = vcn
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    for outbound in outbounds {
        let Some(tls) = outbound.get("tls") else {
            continue;
        };
        if tls.get("insecure") == Some(&Value::Bool(true)) || tls.get("reality").is_some() {
            continue;
        }
        let sni = tls
            .get("server_name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .or_else(|| outbound.get("server").and_then(Value::as_str))
            .unwrap_or("");
        if let Some(name) = names
            .iter()
            .find(|name| certificate_name(name) != certificate_name(sni))
        {
            anyhow::bail!(
                "sing-box verifies the server certificate only against the SNI {sni:?}, but the link asks to verify it as {name:?} (vcn={vcn}), so the connection would fail certificate verification"
            );
        }
    }
    Ok(())
}

fn certificate_name(name: &str) -> String {
    name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase()
}

fn apply_stream_finalmask(profile: &Profile, outbounds: &mut [Value]) -> anyhow::Result<()> {
    if matches!(profile.config, ProtocolConfig::Hysteria2(_)) {
        return Ok(());
    }
    let fm = match profile.share_link_params.get(FINALMASK_PARAM) {
        Some(Value::String(fm)) => fm.clone(),
        Some(fm @ Value::Object(_)) => fm.to_string(),
        _ => return Ok(()),
    };
    let protocol_over_quic = matches!(
        profile.config,
        ProtocolConfig::Tuic(_) | ProtocolConfig::Naive(NaiveConfig { quic: true, .. })
    );
    let relays_udp_natively = matches!(
        profile.config,
        ProtocolConfig::Shadowsocks(_)
            | ProtocolConfig::Socks(SocksConfig {
                version: SocksVersion::V5,
                ..
            })
    );
    let refuses_tls_fragment = matches!(profile.config, ProtocolConfig::Naive(_));
    for outbound in outbounds {
        let over_quic =
            protocol_over_quic || outbound.pointer("/transport/type") == Some(&json!("quic"));
        let mask = read_stream_finalmask(&fm, over_quic || relays_udp_natively)
            .map_err(|error| anyhow::anyhow!("{} link: {error:#}", profile.config.protocol()))?;
        if mask.fragments_tls_hello
            && !over_quic
            && !refuses_tls_fragment
            && let Some(tls) = outbound.get_mut("tls")
        {
            tls["fragment"] = json!(true);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::{
        CustomDnsPreset, DnsConfig, DnsRule, DnsStrategy, GeoRegion, Profile, ProtocolConfig,
        RealitySettings, ShadowtlsVersion, TransportConfig, TransportType, VlessConfig,
        VmessConfig,
    };
    use serde_json::json;

    const TEST_CLASH_PORT: u16 = 41390;

    fn test_profile() -> Profile {
        let mut p = Profile::new_vless(
            "Example".to_string(),
            "203.0.113.42".to_string(),
            59431,
            "671c62c7-6768-4b98-ac6b-572c9c707be0".to_string(),
        );
        if let ProtocolConfig::Vless(ref mut cfg) = p.config {
            cfg.security = Some(crate::config::profile::Security::Reality);
            cfg.tls.reality = Some(RealitySettings {
                public_key: "0IO3LodsrMnhOWh4ogwgdVqYg30CS5-snhFMwldOuAQ".to_string(),
                short_id: "f04debc34cbc48a4".to_string(),
                server_name: "google.com".to_string(),
                spider_x: "/".to_string(),
            });
            cfg.transport = Some(TransportConfig {
                kind: TransportType::Grpc,
                path: None,
                host: None,
                service_name: None,
                headers: Default::default(),
                early_data: None,
            });
            cfg.tls.utls_fingerprint = Some("chrome".to_string());
        }
        p
    }

    fn vless_cfg_mut(profile: &mut Profile) -> &mut VlessConfig {
        match &mut profile.config {
            ProtocolConfig::Vless(c) => c,
            _ => panic!("expected VLESS"),
        }
    }

    fn vless_cfg(profile: &Profile) -> &VlessConfig {
        match &profile.config {
            ProtocolConfig::Vless(c) => c,
            _ => panic!("expected VLESS"),
        }
    }

    #[test]
    fn generated_config_has_required_keys() {
        let profile = test_profile();
        let settings = Settings::default();
        let config = generate_config(
            &profile,
            &settings,
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();

        assert!(config.get("log").is_some());
        assert!(config.get("dns").is_some());
        assert!(config.get("inbounds").is_some());
        assert!(config.get("outbounds").is_some());
        assert!(config.get("route").is_some());
        assert!(config.get("experimental").is_some());
    }

    #[test]
    fn generated_config_log_level_defaults_to_info() {
        let profile = test_profile();
        let settings = Settings::default();
        let config = generate_config(
            &profile,
            &settings,
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        assert_eq!(config["log"]["level"].as_str(), Some("info"));
    }

    #[test]
    fn generated_config_log_level_follows_settings() {
        let profile = test_profile();
        let mut settings = Settings::default();
        settings.logs.level = "debug".to_string();
        let config = generate_config(
            &profile,
            &settings,
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        assert_eq!(config["log"]["level"].as_str(), Some("debug"));
    }

    #[test]
    fn generated_config_log_level_falls_back_on_garbage() {
        let profile = test_profile();
        let mut settings = Settings::default();
        settings.logs.level = "verbose".to_string();
        let config = generate_config(
            &profile,
            &settings,
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        assert_eq!(config["log"]["level"].as_str(), Some("info"));
    }

    #[test]
    fn generated_config_enables_clash_api() {
        let profile = test_profile();
        let settings = Settings::default();
        let config = generate_config(
            &profile,
            &settings,
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        assert_eq!(
            config["experimental"]["clash_api"]["external_controller"],
            format!("127.0.0.1:{TEST_CLASH_PORT}"),
            "clash_api must be enabled so the TUI can poll traffic stats"
        );
    }

    #[test]
    fn generated_config_clash_api_port_follows_argument() {
        let profile = test_profile();
        let settings = Settings::default();
        for port in [31234u16, 45678u16] {
            let config =
                generate_config(&profile, &settings, &GeoAvailability::all(), port).unwrap();
            assert_eq!(
                config["experimental"]["clash_api"]["external_controller"],
                format!("127.0.0.1:{port}")
            );
        }
    }

    #[test]
    fn clash_api_client_targets_the_generated_external_controller() {
        let profile = test_profile();
        let settings = Settings::default();
        let config = generate_config(
            &profile,
            &settings,
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        let controller = config["experimental"]["clash_api"]["external_controller"]
            .as_str()
            .unwrap();
        assert_eq!(
            crate::singbox::clash_api::connections_url(TEST_CLASH_PORT),
            format!("http://{controller}/connections")
        );
    }

    #[test]
    fn vless_outbound_with_reality() {
        let profile = test_profile();
        let outbound = build_vless_outbound(&profile, vless_cfg(&profile)).unwrap();

        assert_eq!(outbound["type"], "vless");
        assert_eq!(outbound["tag"], "proxy");
        assert_eq!(outbound["server"], "203.0.113.42");
        assert_eq!(outbound["server_port"], 59431);
        assert_eq!(outbound["uuid"], "671c62c7-6768-4b98-ac6b-572c9c707be0");

        let tls = &outbound["tls"];
        assert_eq!(tls["enabled"], true);
        assert_eq!(tls["server_name"], "google.com");
        assert!(tls.get("reality").is_some());
        assert_eq!(
            tls["reality"]["public_key"],
            "0IO3LodsrMnhOWh4ogwgdVqYg30CS5-snhFMwldOuAQ"
        );
        assert_eq!(tls["reality"]["short_id"], "f04debc34cbc48a4");
        assert_eq!(tls["utls"]["enabled"], true);
        assert_eq!(tls["utls"]["fingerprint"], "chrome");
    }

    #[test]
    fn vless_outbound_without_reality() {
        let profile = Profile::new_vless(
            "Simple".to_string(),
            "1.2.3.4".to_string(),
            443,
            "uuid".to_string(),
        );
        let outbound = build_vless_outbound(&profile, vless_cfg(&profile)).unwrap();

        let tls = &outbound["tls"];
        assert_eq!(tls["enabled"], true);
        assert_eq!(tls["server_name"], "1.2.3.4");
        // sing-box treats absent `insecure` as false; we now omit it when
        // unset (shared shape with VMess/Trojan via `build_tls_block`).
        assert!(
            tls.get("insecure")
                .is_none_or(|v| v.as_bool() == Some(false))
        );
        assert!(tls.get("reality").is_none());
    }

    #[test]
    fn vless_outbound_plain_tls_uses_custom_sni() {
        // P0 regression: a profile with cfg.tls.server_name = Some("cdn...")
        // must emit `tls.server_name == "cdn..."`, not profile.address.
        let mut profile = Profile::new_vless(
            "CDN".to_string(),
            "1.2.3.4".to_string(),
            443,
            "uuid".to_string(),
        );
        vless_cfg_mut(&mut profile).tls.server_name = Some("cdn.example.com".to_string());
        vless_cfg_mut(&mut profile).tls.alpn = vec!["h2".to_string()];
        vless_cfg_mut(&mut profile).tls.insecure = true;
        let outbound = build_vless_outbound(&profile, vless_cfg(&profile)).unwrap();
        let tls = &outbound["tls"];
        assert_eq!(tls["server_name"], "cdn.example.com");
        assert_eq!(tls["alpn"], serde_json::json!(["h2"]));
        assert_eq!(tls["insecure"], true);
    }

    #[test]
    fn vless_outbound_with_flow() {
        let mut profile = test_profile();
        vless_cfg_mut(&mut profile).flow = Some(crate::config::profile::Flow::XtlsRprxVision);
        let outbound = build_vless_outbound(&profile, vless_cfg(&profile)).unwrap();
        assert_eq!(outbound["flow"], "xtls-rprx-vision");
    }

    #[test]
    fn vless_outbound_with_grpc_transport() {
        let profile = test_profile();
        let outbound = build_vless_outbound(&profile, vless_cfg(&profile)).unwrap();

        assert!(outbound.get("transport").is_some());
        let transport = &outbound["transport"];
        assert_eq!(transport["type"], "grpc");
        assert_eq!(transport["idle_timeout"], "15s");
        assert_eq!(transport["ping_timeout"], "15s");
    }

    #[test]
    fn vless_outbound_with_grpc_service_name() {
        let mut profile = test_profile();
        vless_cfg_mut(&mut profile)
            .transport
            .as_mut()
            .unwrap()
            .service_name = Some("my-service".to_string());
        let outbound = build_vless_outbound(&profile, vless_cfg(&profile)).unwrap();
        assert_eq!(outbound["transport"]["service_name"], "my-service");
    }

    #[test]
    fn vless_outbound_without_transport() {
        let mut profile = test_profile();
        vless_cfg_mut(&mut profile).transport = None;
        let outbound = build_vless_outbound(&profile, vless_cfg(&profile)).unwrap();
        assert!(outbound.get("transport").is_none());
    }

    fn profile_with(config: ProtocolConfig, address: &str, port: u16) -> Profile {
        Profile {
            id: uuid::Uuid::nil(),
            name: "T".into(),
            address: address.into(),
            port,
            config,
            tags: Vec::new(),
            subscription_id: None,
            share_link_params: Default::default(),
        }
    }

    fn build_one(profile: &Profile) -> Value {
        let mut outs = build_outbound(profile).unwrap();
        assert_eq!(outs.len(), 1, "expected a single outbound");
        outs.remove(0)
    }

    #[test]
    fn vmess_outbound_basic_shape() {
        use crate::config::profile::{VmessConfig, VmessSecurity};
        let profile = profile_with(
            ProtocolConfig::Vmess(VmessConfig {
                uuid: "vm-uuid".into(),
                alter_id: 0,
                security: VmessSecurity::Aes128Gcm,
                ..Default::default()
            }),
            "1.1.1.1",
            443,
        );
        let outbound = build_one(&profile);
        assert_eq!(outbound["type"], "vmess");
        assert_eq!(outbound["uuid"], "vm-uuid");
        assert_eq!(outbound["security"], "aes-128-gcm");
        assert_eq!(outbound["alter_id"], 0);
        assert_eq!(outbound["packet_encoding"], "xudp");
        assert!(outbound.get("tls").is_some());
        // Sing-box 1.12 forbids the legacy stream cipher.
        assert_ne!(outbound["security"], "aes-128-cfb");
    }

    #[test]
    fn vmess_outbound_with_ws_transport() {
        use crate::config::profile::{TransportConfig, TransportType, VmessConfig};
        let profile = profile_with(
            ProtocolConfig::Vmess(VmessConfig {
                uuid: "u".into(),
                transport: Some(TransportConfig {
                    kind: TransportType::Ws,
                    path: Some("/ws".into()),
                    host: Some("example.com".into()),
                    service_name: None,
                    headers: Default::default(),
                    early_data: None,
                }),
                ..Default::default()
            }),
            "1.1.1.1",
            443,
        );
        let outbound = build_one(&profile);
        assert_eq!(
            outbound["transport"],
            json!({"type": "ws", "path": "/ws", "headers": {"Host": "example.com"}})
        );
    }

    fn vmess_with_transport(transport: TransportConfig) -> Profile {
        profile_with(
            ProtocolConfig::Vmess(VmessConfig {
                uuid: "u".into(),
                transport: Some(transport),
                ..Default::default()
            }),
            "1.1.1.1",
            443,
        )
    }

    #[test]
    fn ws_transport_keeps_an_explicit_host_header() {
        let profile = vmess_with_transport(TransportConfig {
            kind: TransportType::Ws,
            path: None,
            host: Some("link.example.com".into()),
            service_name: None,
            headers: [("host".to_string(), "edited.example.com".to_string())].into(),
            early_data: None,
        });
        let outbound = build_one(&profile);
        assert_eq!(
            outbound["transport"]["headers"],
            json!({"host": "edited.example.com"})
        );
    }

    #[test]
    fn httpupgrade_transport_emits_host_and_path_and_quic_emits_only_its_type() {
        let httpupgrade = vmess_with_transport(TransportConfig {
            kind: TransportType::HttpUpgrade,
            path: Some("/up".into()),
            host: Some("cdn.example.com".into()),
            service_name: None,
            headers: Default::default(),
            early_data: None,
        });
        let quic = vmess_with_transport(TransportConfig {
            kind: TransportType::Quic,
            path: Some("/ignored".into()),
            host: Some("ignored.example.com".into()),
            service_name: None,
            headers: Default::default(),
            early_data: None,
        });
        assert_eq!(
            build_one(&httpupgrade)["transport"],
            json!({"type": "httpupgrade", "host": "cdn.example.com", "path": "/up"})
        );
        assert_eq!(build_one(&quic)["transport"], json!({"type": "quic"}));
    }

    fn with_params(mut profile: Profile, pairs: &[(&str, &str)]) -> Profile {
        profile.share_link_params = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), json!(value)))
            .collect();
        profile
    }

    fn transport(kind: TransportType, path: Option<&str>, host: Option<&str>) -> TransportConfig {
        TransportConfig {
            kind,
            path: path.map(Into::into),
            host: host.map(Into::into),
            service_name: None,
            headers: Default::default(),
            early_data: None,
        }
    }

    #[test]
    fn unknown_transport_emits_only_its_type_and_link_params_never_reach_sing_box() {
        let xhttp = with_params(
            vmess_with_transport(transport(
                TransportType::Other("xhttp".into()),
                Some("/x"),
                None,
            )),
            &[("mode", "auto")],
        );
        let ws = with_params(
            vmess_with_transport(transport(TransportType::Ws, Some("/ws"), None)),
            &[("mode", "auto")],
        );
        assert_eq!(build_one(&xhttp)["transport"], json!({"type": "xhttp"}));
        assert_eq!(
            build_one(&ws)["transport"],
            json!({"type": "ws", "path": "/ws"})
        );
    }

    #[test]
    fn tcp_http_header_is_refused_over_tls_and_sent_as_http_without_it() {
        let http = transport(TransportType::Http, Some("/"), Some("a.example.com"));
        let profile = with_params(
            vmess_with_transport(http.clone()),
            &[("headerType", "http")],
        );
        let error = build_outbound(&profile).unwrap_err().to_string();
        assert!(error.contains("HTTP/2"), "{error}");
        let mut plain = profile;
        if let ProtocolConfig::Vmess(cfg) = &mut plain.config {
            cfg.stream_security = Some(crate::config::profile::Security::None);
        }
        assert_eq!(
            build_one(&plain)["transport"],
            json!({"type": "http", "host": ["a.example.com"], "path": "/"})
        );
    }

    #[test]
    fn explicit_no_tls_drops_the_tls_block_and_unset_keeps_it() {
        use crate::config::profile::Security;
        let mut vless = test_profile();
        vless_cfg_mut(&mut vless).security = Some(Security::None);
        let mut vmess = vmess_with_transport(transport(TransportType::Ws, Some("/ws"), None));
        let legacy_vmess = vmess.clone();
        if let ProtocolConfig::Vmess(cfg) = &mut vmess.config {
            cfg.stream_security = Some(Security::None);
        }

        assert!(build_one(&vless).get("tls").is_none());
        assert!(build_one(&vmess).get("tls").is_none());
        assert_eq!(build_one(&legacy_vmess)["tls"]["enabled"], true);
    }

    #[test]
    fn quic_transport_refuses_options_sing_box_lacks() {
        let profile = with_params(
            vmess_with_transport(transport(TransportType::Quic, None, None)),
            &[("quicSecurity", "aes-128-gcm")],
        );
        let error = build_outbound(&profile).unwrap_err().to_string();
        assert!(error.contains("quicSecurity=aes-128-gcm"), "{error}");
    }

    #[test]
    fn vless_encryption_is_refused_and_none_is_accepted() {
        let encrypted = with_params(test_profile(), &[("encryption", "mlkem768x25519plus")]);
        let plain = with_params(test_profile(), &[("encryption", "none")]);
        let error = build_outbound(&encrypted).unwrap_err().to_string();
        assert!(error.contains("VLESS Encryption"), "{error}");
        assert!(build_outbound(&plain).is_ok());
    }

    #[test]
    fn http_transport_emits_hosts_as_a_list() {
        let profile = vmess_with_transport(TransportConfig {
            kind: TransportType::Http,
            path: Some("/h2".into()),
            host: Some("a.example.com, b.example.com".into()),
            service_name: Some("ignored".into()),
            headers: Default::default(),
            early_data: None,
        });
        let outbound = build_one(&profile);
        assert_eq!(
            outbound["transport"],
            json!({"type": "http", "host": ["a.example.com", "b.example.com"], "path": "/h2"})
        );
    }

    #[test]
    fn vmess_tls_emits_ech_when_enabled() {
        use crate::config::profile::{EchSettings, TlsCommon, VmessConfig};
        let profile = profile_with(
            ProtocolConfig::Vmess(VmessConfig {
                uuid: "u".into(),
                tls: TlsCommon {
                    ech: Some(EchSettings {
                        enabled: true,
                        config: vec!["base64-blob".into()],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            }),
            "ech.example.com",
            443,
        );
        let outbound = build_one(&profile);
        let ech = &outbound["tls"]["ech"];
        assert_eq!(ech["enabled"], true);
        assert_eq!(ech["config"][0], "base64-blob");
    }

    #[test]
    fn trojan_outbound_shape() {
        use crate::config::profile::TrojanConfig;
        let profile = profile_with(
            ProtocolConfig::Trojan(TrojanConfig {
                password: "secret".into(),
                ..Default::default()
            }),
            "trojan.example",
            443,
        );
        let outbound = build_one(&profile);
        assert_eq!(outbound["type"], "trojan");
        assert_eq!(outbound["password"], "secret");
        assert_eq!(outbound["tls"]["enabled"], true);
        assert_eq!(outbound["tls"]["server_name"], "trojan.example");
    }

    #[test]
    fn shadowsocks_outbound_passes_the_link_plugin_to_sing_box() {
        use crate::config::profile::{ShadowsocksCipher, ShadowsocksConfig};
        let shadowsocks = || {
            profile_with(
                ProtocolConfig::Shadowsocks(ShadowsocksConfig {
                    method: ShadowsocksCipher::Aes128Gcm,
                    password: "p".into(),
                }),
                "ss.example",
                8388,
            )
        };
        let obfs = with_params(
            shadowsocks(),
            &[("plugin", "obfs-local;obfs=http;obfs-host=a.example")],
        );
        let v2ray = with_params(shadowsocks(), &[("plugin", "v2ray-plugin")]);

        let obfs = build_one(&obfs);
        let v2ray = build_one(&v2ray);

        assert_eq!(obfs["plugin"], "obfs-local");
        assert_eq!(obfs["plugin_opts"], "obfs=http;obfs-host=a.example");
        assert_eq!(v2ray["plugin"], "v2ray-plugin");
        assert!(v2ray.get("plugin_opts").is_none());
    }

    #[test]
    fn shadowsocks_outbound_uses_aead_cipher() {
        use crate::config::profile::{ShadowsocksCipher, ShadowsocksConfig};
        let profile = profile_with(
            ProtocolConfig::Shadowsocks(ShadowsocksConfig {
                method: ShadowsocksCipher::Blake3Aes256Gcm,
                password: "ss-pass".into(),
            }),
            "ss.example",
            8388,
        );
        let outbound = build_one(&profile);
        assert_eq!(outbound["type"], "shadowsocks");
        assert_eq!(outbound["method"], "2022-blake3-aes-256-gcm");
        assert_eq!(outbound["password"], "ss-pass");
        assert!(
            outbound.get("tls").is_none(),
            "Shadowsocks must not carry a tls block"
        );
    }

    fn hysteria2(params: &[(&str, &str)]) -> Profile {
        with_params(
            profile_with(
                ProtocolConfig::Hysteria2(crate::config::profile::Hysteria2Config {
                    password: "p".into(),
                    ..Default::default()
                }),
                "hy2.example",
                443,
            ),
            params,
        )
    }

    #[test]
    fn hysteria2_mport_becomes_server_port_ranges() {
        let outbound = build_one(&hysteria2(&[("mport", "1000-2000, 3000")]));
        assert!(outbound.get("server_port").is_none());
        assert_eq!(outbound["server_ports"], json!(["1000:2000", "3000:3000"]));
    }

    #[test]
    fn certificate_pin_warning_names_an_unchecked_pin_sha256() {
        assert!(
            certificate_pin_warning(&hysteria2(&[("pinSHA256", "AB:CD")]))
                .unwrap()
                .contains("pinSHA256 is not checked")
        );
        assert_eq!(certificate_pin_warning(&hysteria2(&[])), None);
    }

    #[test]
    fn certificate_pin_warning_names_an_unchecked_pcs() {
        assert!(
            certificate_pin_warning(&hysteria2(&[("pcs", "ab01")]))
                .unwrap()
                .starts_with("pcs is not checked")
        );
    }

    #[test]
    fn hysteria2_outbound_shape() {
        use crate::config::profile::{Hysteria2Config, Hysteria2Obfs, Hysteria2ObfsType};
        let profile = profile_with(
            ProtocolConfig::Hysteria2(Hysteria2Config {
                password: "hy2-pass".into(),
                up_mbps: Some(100),
                down_mbps: Some(200),
                obfs: Some(Hysteria2Obfs {
                    kind: Hysteria2ObfsType::Salamander,
                    password: "obfs-pass".into(),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            "hy2.example",
            443,
        );
        let outbound = build_one(&profile);
        assert_eq!(outbound["type"], "hysteria2");
        assert_eq!(outbound["password"], "hy2-pass");
        assert_eq!(outbound["up_mbps"], 100);
        assert_eq!(outbound["down_mbps"], 200);
        assert_eq!(outbound["obfs"]["type"], "salamander");
        assert_eq!(outbound["obfs"]["password"], "obfs-pass");
        // The legacy top-level obfs_password key must not appear.
        assert!(outbound.get("obfs_password").is_none());
        // Hysteria2 is QUIC-based — ALPN defaults to h3 when not set.
        assert_eq!(outbound["tls"]["alpn"][0], "h3");
    }

    #[test]
    fn hysteria2_outbound_emits_gecko_packet_sizes() {
        use crate::config::profile::{Hysteria2Config, Hysteria2Obfs, Hysteria2ObfsType};
        let profile = profile_with(
            ProtocolConfig::Hysteria2(Hysteria2Config {
                password: "hy2-pass".into(),
                obfs: Some(Hysteria2Obfs {
                    kind: Hysteria2ObfsType::Gecko,
                    password: "obfs-pass".into(),
                    min_packet_size: Some(512),
                    max_packet_size: Some(1200),
                }),
                ..Default::default()
            }),
            "hy2.example",
            443,
        );
        assert_eq!(
            build_one(&profile)["obfs"],
            json!({
                "type": "gecko",
                "password": "obfs-pass",
                "min_packet_size": 512,
                "max_packet_size": 1200,
            })
        );
    }

    #[test]
    fn hysteria2_outbound_refuses_obfuscation_the_link_could_not_model() {
        let error = build_outbound(&hysteria2(&[("obfs", "other"), ("obfs-password", "ob")]))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Hysteria 2 obfuscation (obfs=other)"),
            "{error}"
        );
    }

    #[test]
    fn hysteria2_outbound_emits_the_hop_interval_with_server_ports() {
        let mut profile = hysteria2(&[("mport", "20000-30000")]);
        if let ProtocolConfig::Hysteria2(cfg) = &mut profile.config {
            cfg.hop_interval_secs = Some(10);
            cfg.hop_interval_max_secs = Some(20);
        }
        let outbound = build_one(&profile);
        assert_eq!(outbound["hop_interval"], "10s");
        assert_eq!(outbound["hop_interval_max"], "20s");
    }

    #[test]
    fn hysteria2_outbound_checks_the_stored_fm() {
        let tuning = r#"{"quicParams":{"congestion":"bbr"}}"#;
        assert!(build_outbound(&hysteria2(&[("fm", tuning)])).is_ok());

        let noise = r#"{"udp":[{"type":"noise","settings":{}}]}"#;
        let error = build_outbound(&hysteria2(&[("fm", noise)]))
            .unwrap_err()
            .to_string();
        assert!(error.contains("UDP mask \"noise\""), "{error}");

        let salamander = r#"{"udp":[{"type":"salamander","settings":{"password":"pw"}}]}"#;
        let error = build_outbound(&hysteria2(&[("fm", salamander)]))
            .unwrap_err()
            .to_string();
        assert!(error.contains("re-import"), "{error}");
    }

    fn vcn_profile(query: &str) -> Profile {
        crate::config::profile::parse_share_link(&format!(
            "vless://00000000-0000-4000-8000-000000000001@a.example:443?type=tcp&{query}#V"
        ))
        .unwrap()
    }

    #[test]
    fn certificate_name_the_sni_covers_is_accepted() {
        for query in [
            "security=tls&sni=a.example&vcn=a.example",
            "security=tls&sni=a.example&vcn=A.Example,a.example",
            "security=tls&vcn=a.example",
            "security=tls&sni=&vcn=a.example",
            "security=tls&sni=a.example.&vcn=a.example",
            "security=tls&sni=a.example&vcn=a.example.",
            "security=tls&sni=a.example&vcn=",
            "security=tls&sni=a.example&allowInsecure=1&vcn=b.example",
            "security=reality&pbk=key&sni=a.example&vcn=b.example",
            "security=none&vcn=b.example",
        ] {
            assert!(build_outbound(&vcn_profile(query)).is_ok(), "{query}");
        }
    }

    #[test]
    fn certificate_name_the_sni_does_not_cover_is_refused() {
        for (query, name) in [
            ("security=tls&sni=a.example&vcn=b.example", "b.example"),
            (
                "security=tls&sni=a.example&vcn=a.example,b.example",
                "b.example",
            ),
            ("security=tls&sni=a.example&vcn=a.example..", "a.example.."),
        ] {
            let error = build_outbound(&vcn_profile(query)).unwrap_err().to_string();
            assert!(
                error.contains(&format!("verify it as \"{name}\"")),
                "{query}: {error}"
            );
        }
    }

    #[test]
    fn tls_block_disables_sni_and_keeps_the_name_for_verification() {
        let profile = crate::config::profile::parse_share_link(
            "trojan://pw@t.example:443?security=tls&sni=cover.example&disable_sni=1#T",
        )
        .unwrap();
        let tls = &build_one(&profile)["tls"];
        assert_eq!(tls["disable_sni"], true);
        assert_eq!(tls["server_name"], "cover.example");
    }

    #[test]
    fn websocket_transport_emits_early_data() {
        let profile = crate::config::profile::parse_share_link(
            "vless://00000000-0000-4000-8000-000000000001@v.example:443?type=ws&security=tls&path=%2Fws%3Fed%3D2048#S",
        )
        .unwrap();
        let transport = &build_one(&profile)["transport"];
        assert_eq!(transport["path"], "/ws");
        assert_eq!(transport["max_early_data"], 2048);
        assert_eq!(
            transport["early_data_header_name"],
            "Sec-WebSocket-Protocol"
        );
    }

    fn with_fm(link: &str, fm: serde_json::Value) -> Profile {
        let fm = urlencoding::encode(&fm.to_string()).into_owned();
        let separator = if link.contains('?') { '&' } else { '?' };
        crate::config::profile::parse_share_link(
            &link.replace("#", &format!("{separator}fm={fm}#")),
        )
        .unwrap()
    }

    fn tls_hello_fragment() -> serde_json::Value {
        json!({ "tcp": [{ "type": "fragment", "settings": { "packets": "tlshello", "length": "100-200" } }] })
    }

    #[test]
    fn stream_fm_fragments_the_tls_hello_when_the_outbound_has_tls() {
        let vless = "vless://00000000-0000-4000-8000-000000000001@v.example:443?type=tcp&security=tls&sni=v.example#V";
        let outbound = build_one(&with_fm(vless, tls_hello_fragment()));
        assert_eq!(outbound["tls"]["fragment"], true);

        let plain = vless.replace("security=tls", "security=none");
        assert!(
            build_one(&with_fm(&plain, tls_hello_fragment()))
                .get("tls")
                .is_none()
        );

        let vmess = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            json!({
                "v": "2", "add": "m.example", "port": "443", "id": "00000000-0000-4000-8000-000000000001",
                "net": "tcp", "tls": "tls", "fm": tls_hello_fragment(),
            })
            .to_string(),
        );
        let profile =
            crate::config::profile::parse_share_link(&format!("vmess://{vmess}")).unwrap();
        assert_eq!(build_one(&profile)["tls"]["fragment"], true);
    }

    #[test]
    fn stream_fm_fragments_only_tcp_tls_that_sing_box_can_split() {
        let naive = with_fm(
            "naive+https://u:p@n.example:443?peer=n.example#N",
            tls_hello_fragment(),
        );
        let tuic = with_fm(
            "tuic://00000000-0000-4000-8000-000000000001:p@t.example:443?sni=t.example#T",
            tls_hello_fragment(),
        );
        for profile in [naive, tuic] {
            let outbound = build_one(&profile);
            assert!(outbound["tls"].get("fragment").is_none(), "{outbound}");
        }
    }

    #[test]
    fn stream_fm_refuses_udp_masks_where_udp_goes_straight_to_the_server() {
        let salamander =
            json!({ "udp": [{ "type": "salamander", "settings": { "password": "pw" } }] });
        for link in [
            "ss://YWVzLTI1Ni1nY206cHc@s.example:8388#S",
            "socks5://u:p@s.example:1080#K",
        ] {
            let error = build_outbound(&with_fm(link, salamander.clone()))
                .unwrap_err()
                .to_string();
            assert!(error.contains("UDP mask \"salamander\""), "{link}: {error}");
        }
        let fragment = with_fm(
            "ss://YWVzLTI1Ni1nY206cHc@s.example:8388#S",
            tls_hello_fragment(),
        );
        assert!(build_outbound(&fragment).is_ok());
        for link in [
            "socks4://u@s.example:1080#K",
            "socks4a://u@s.example:1080#K",
        ] {
            assert!(
                build_outbound(&with_fm(link, salamander.clone())).is_ok(),
                "{link}"
            );
        }
    }

    #[test]
    fn stream_fm_refuses_masks_sing_box_cannot_reproduce() {
        let vless =
            "vless://00000000-0000-4000-8000-000000000001@v.example:443?type=tcp&security=tls#V";
        let error = build_outbound(&with_fm(
            vless,
            json!({ "tcp": [{ "type": "header-custom" }] }),
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("TCP mask \"header-custom\""), "{error}");

        let noise = json!({ "udp": [{ "type": "noise", "settings": {} }] });
        assert!(build_outbound(&with_fm(vless, noise.clone())).is_ok());
        let tuic = "tuic://00000000-0000-4000-8000-000000000001:p@t.example:443?sni=t.example#T";
        let error = build_outbound(&with_fm(tuic, noise))
            .unwrap_err()
            .to_string();
        assert!(error.contains("UDP mask \"noise\""), "{error}");
    }

    #[test]
    fn tuic_outbound_shape() {
        use crate::config::profile::{TuicConfig, TuicCongestion, TuicUdpRelayMode};
        let profile = profile_with(
            ProtocolConfig::Tuic(TuicConfig {
                uuid: "tuic-uuid".into(),
                password: "tuic-pass".into(),
                congestion_control: TuicCongestion::Bbr,
                udp_relay_mode: TuicUdpRelayMode::Native,
                zero_rtt_handshake: true,
                ..Default::default()
            }),
            "tuic.example",
            443,
        );
        let outbound = build_one(&profile);
        assert_eq!(outbound["type"], "tuic");
        assert_eq!(outbound["uuid"], "tuic-uuid");
        assert_eq!(outbound["password"], "tuic-pass");
        assert_eq!(outbound["congestion_control"], "bbr");
        assert_eq!(outbound["udp_relay_mode"], "native");
        assert_eq!(outbound["zero_rtt_handshake"], true);
        assert_eq!(outbound["tls"]["alpn"][0], "h3");
    }

    #[test]
    fn shadowtls_emits_wrapper_and_detour_pair() {
        use crate::config::profile::{ShadowsocksCipher, ShadowtlsConfig};
        let profile = profile_with(
            ProtocolConfig::Shadowtls(ShadowtlsConfig {
                version: ShadowtlsVersion::V3,
                password: "st-pass".into(),
                method: ShadowsocksCipher::Chacha20IetfPoly1305,
                ss_password: "inner-ss".into(),
                ..Default::default()
            }),
            "st.example",
            443,
        );
        let outs = build_outbound(&profile).unwrap();
        assert_eq!(outs.len(), 2, "ShadowTLS emits wrapper + detour");
        let wrap = outs.iter().find(|o| o["type"] == "shadowtls").unwrap();
        let inner = outs.iter().find(|o| o["type"] == "shadowsocks").unwrap();
        assert_eq!(wrap["version"], 3);
        assert_eq!(wrap["password"], "st-pass");
        assert_eq!(inner["tag"], "proxy");
        assert_eq!(inner["server"], "st.example");
        assert_eq!(inner["server_port"], 443);
        assert_eq!(inner["password"], "inner-ss");
        assert_eq!(inner["detour"], wrap["tag"]);
    }

    #[test]
    fn shadowtls_v1_omits_password() {
        use crate::config::profile::{ShadowsocksCipher, ShadowtlsConfig};
        let profile = profile_with(
            ProtocolConfig::Shadowtls(ShadowtlsConfig {
                version: ShadowtlsVersion::V1,
                password: "ignored".into(),
                method: ShadowsocksCipher::Chacha20IetfPoly1305,
                ss_password: "inner-ss".into(),
                ..Default::default()
            }),
            "st.example",
            443,
        );
        let outs = build_outbound(&profile).unwrap();
        let wrap = outs.iter().find(|o| o["type"] == "shadowtls").unwrap();
        assert_eq!(wrap["version"], 1);
        assert!(wrap.get("password").is_none(), "v1 must not emit password");
    }

    #[test]
    fn anytls_outbound_shape() {
        use crate::config::profile::AnytlsConfig;
        let profile = profile_with(
            ProtocolConfig::Anytls(AnytlsConfig {
                password: "anytls-pass".into(),
                idle_session_timeout: Some("30s".into()),
                ..Default::default()
            }),
            "anytls.example",
            443,
        );
        let outbound = build_one(&profile);
        assert_eq!(outbound["type"], "anytls");
        assert_eq!(outbound["password"], "anytls-pass");
        assert_eq!(outbound["idle_session_timeout"], "30s");
        assert_eq!(outbound["tls"]["enabled"], true);
    }

    #[test]
    fn naive_outbound_shape() {
        use crate::config::profile::NaiveConfig;
        let outbound = build_one(&profile_with(
            ProtocolConfig::Naive(NaiveConfig {
                username: Some("alice".into()),
                password: Some("pw".into()),
                quic: true,
                ..Default::default()
            }),
            "n.example",
            443,
        ));
        assert_eq!(
            outbound,
            json!({
                "type": "naive",
                "tag": "proxy",
                "server": "n.example",
                "server_port": 443,
                "username": "alice",
                "password": "pw",
                "quic": true,
                "tls": { "enabled": true, "server_name": "n.example" }
            })
        );
    }

    #[test]
    fn socks_outbound_with_auth() {
        use crate::config::profile::{SocksConfig, SocksVersion};
        let profile = profile_with(
            ProtocolConfig::Socks(SocksConfig {
                version: SocksVersion::V5,
                username: Some("u".into()),
                password: Some("p".into()),
            }),
            "socks.example",
            1080,
        );
        let outbound = build_one(&profile);
        assert_eq!(outbound["type"], "socks");
        assert_eq!(outbound["version"], "5");
        assert_eq!(outbound["username"], "u");
        assert_eq!(outbound["password"], "p");
        assert!(outbound.get("tls").is_none());
    }

    #[test]
    fn http_outbound_emits_tls_only_when_user_opts_in() {
        use crate::config::profile::{HttpConfig, TlsCommon};
        let plain = profile_with(
            ProtocolConfig::Http(HttpConfig::default()),
            "http.example",
            8080,
        );
        let plain_out = build_one(&plain);
        assert!(plain_out.get("tls").is_none(), "plain HTTP omits TLS");

        let secure = profile_with(
            ProtocolConfig::Http(HttpConfig {
                tls: TlsCommon {
                    server_name: Some("proxy.example".into()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            "http.example",
            8443,
        );
        let secure_out = build_one(&secure);
        assert_eq!(secure_out["tls"]["enabled"], true);
        assert_eq!(secure_out["tls"]["server_name"], "proxy.example");
    }

    #[test]
    fn ssh_outbound_password_and_key() {
        use crate::config::profile::SshConfig;
        let profile = profile_with(
            ProtocolConfig::Ssh(SshConfig {
                user: "alice".into(),
                password: Some("p".into()),
                private_key_path: Some("/keys/id_ed25519".into()),
                host_key_algorithms: vec!["ssh-ed25519".into()],
                ..Default::default()
            }),
            "ssh.example",
            22,
        );
        let outbound = build_one(&profile);
        assert_eq!(outbound["type"], "ssh");
        assert_eq!(outbound["user"], "alice");
        assert_eq!(outbound["password"], "p");
        assert_eq!(outbound["private_key_path"], "/keys/id_ed25519");
        assert_eq!(outbound["host_key_algorithms"][0], "ssh-ed25519");
    }

    #[test]
    fn generated_config_includes_direct_outbound() {
        let profile = test_profile();
        let settings = Settings::default();
        let config = generate_config(
            &profile,
            &settings,
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        let outbounds = config["outbounds"].as_array().unwrap();
        assert!(outbounds.iter().any(|o| o["type"] == "direct"));
        assert!(outbounds.iter().any(|o| o["tag"] == "proxy"));
    }

    fn default_resolver() -> RouteResolvers {
        RouteResolvers {
            default_domain: json!({ "server": "bootstrap", "strategy": "prefer_ipv4" }),
            destination: json!({ "server": "remote", "strategy": "prefer_ipv4" }),
        }
    }

    fn rule_set_rule(tag: &str, server: &str) -> DnsRule {
        DnsRule {
            rule_set: vec![tag.to_string()],
            server: server.to_string(),
            ..Default::default()
        }
    }

    fn udp(tag: &str, server: &str) -> DnsServer {
        DnsServer::Udp {
            tag: tag.to_string(),
            server: server.to_string(),
            server_port: None,
        }
    }

    fn doh(tag: &str, server: &str) -> DnsServer {
        DnsServer::Https {
            tag: tag.to_string(),
            server: server.to_string(),
            server_port: None,
            path: "/dns-query".to_string(),
        }
    }

    fn http_profile() -> Profile {
        Profile {
            config: ProtocolConfig::Http(Default::default()),
            ..test_profile()
        }
    }

    fn local() -> DnsServer {
        DnsServer::Local {
            tag: "local".to_string(),
        }
    }

    fn active(servers: Vec<DnsServer>, final_server: &str) -> ActiveDns {
        ActiveDns {
            servers,
            rules: Vec::new(),
            final_server: final_server.to_string(),
            fakeip: None,
            fakeip_catch_all: false,
        }
    }

    fn default_active() -> ActiveDns {
        DnsConfig::default().active().unwrap()
    }

    fn with_fakeip(mut dns: ActiveDns, rules: Vec<DnsRule>) -> ActiveDns {
        dns.rules = rules;
        dns.fakeip = Some(FakeIpServer {
            tag: "fakeip".to_string(),
            ranges: Default::default(),
        });
        dns.fakeip_catch_all = true;
        dns
    }

    fn direct(active: ActiveDns) -> DnsUpstreams {
        DnsUpstreams {
            active,
            through_proxy: false,
            carries_udp: true,
        }
    }

    fn through_proxy(active: ActiveDns) -> DnsUpstreams {
        DnsUpstreams {
            active,
            through_proxy: true,
            carries_udp: true,
        }
    }

    fn build(
        upstreams: &DnsUpstreams,
        available_rule_sets: &HashSet<&str>,
    ) -> anyhow::Result<Value> {
        build_dns(
            upstreams,
            upstreams.bootstrap(),
            &DnsStrategy::PreferIpv4,
            available_rule_sets,
            &[],
        )
    }

    fn settings_with_preset(preset: CustomDnsPreset) -> Settings {
        let mut settings = Settings::default();
        settings.dns.current_preset = preset.name.clone();
        settings.dns.custom_presets = vec![preset];
        settings
    }

    fn custom_preset(
        servers: Vec<DnsServer>,
        rules: Vec<DnsRule>,
        final_server: &str,
    ) -> CustomDnsPreset {
        CustomDnsPreset {
            name: "home".to_string(),
            servers,
            rules,
            final_server: final_server.to_string(),
        }
    }

    #[test]
    fn route_has_default_mark_for_killswitch() {
        let (route, _) = build_route(
            &RoutingMode::Global,
            default_resolver(),
            &GeoAvailability::all(),
            &no_services(),
        );
        assert_eq!(
            route["default_mark"].as_u64(),
            Some(666),
            "default_mark must match the kill-switch nft rule (0x29a)"
        );
    }

    #[test]
    fn build_route_global_has_basic_rules() {
        let (route, rule_sets) = build_route(
            &RoutingMode::Global,
            default_resolver(),
            &GeoAvailability::all(),
            &no_services(),
        );
        assert!(rule_sets.is_empty());
        let rules = route["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 3); // ipv6 reject, dns hijack, direct cidr
        assert_eq!(route["final"], "proxy");
        assert_eq!(
            route["default_domain_resolver"],
            default_resolver().default_domain
        );
    }

    #[test]
    fn build_route_only_ru_has_private_rule_and_final_direct() {
        let (route, _rule_sets) = build_route(
            &RoutingMode::Only(GeoRegion::Ru),
            default_resolver(),
            &GeoAvailability::all(),
            &no_services(),
        );
        let rules = route["rules"].as_array().unwrap();
        assert!(rules.len() >= 4); // basic 3 + ip_is_private
        assert_eq!(route["final"], "direct");
    }

    #[test]
    fn build_route_bypass_cn_has_private_rule() {
        let (route, _rule_sets) = build_route(
            &RoutingMode::Bypass(GeoRegion::Cn),
            default_resolver(),
            &GeoAvailability::all(),
            &no_services(),
        );
        let rules = route["rules"].as_array().unwrap();
        assert!(rules.len() >= 4); // basic 3 + ip_is_private
        assert_eq!(route["final"], "proxy");
    }

    #[test]
    fn build_route_only_cn_has_private_rule_and_final_direct() {
        let (route, _rule_sets) = build_route(
            &RoutingMode::Only(GeoRegion::Cn),
            default_resolver(),
            &GeoAvailability::all(),
            &no_services(),
        );
        let rules = route["rules"].as_array().unwrap();
        assert!(rules.len() >= 4); // basic 3 + ip_is_private
        assert_eq!(route["final"], "direct");
    }

    #[test]
    fn default_dns_block_matches_legacy_layout() {
        let dns = build(&direct(default_active()), &HashSet::new()).unwrap();
        let servers = dns["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0]["tag"], "local");
        assert_eq!(servers[0]["type"], "local");
        assert_eq!(servers[1]["tag"], "remote");
        assert_eq!(servers[1]["type"], "https");
        assert_eq!(servers[1]["server"], "1.1.1.1");
        assert_eq!(servers[1]["path"], "/dns-query");
        assert!(servers[1].get("server_port").is_none());
        assert_eq!(dns["final"], "remote");
        assert_eq!(dns["strategy"], "prefer_ipv4");
        assert!(dns.get("rules").is_none());
        assert!(dns.get("fakeip").is_none());
    }

    #[test]
    fn build_dns_emits_dot_server_with_port() {
        let dns = active(
            vec![
                local(),
                DnsServer::Tls {
                    tag: "google-dot".to_string(),
                    server: "8.8.8.8".to_string(),
                    server_port: Some(853),
                },
            ],
            "google-dot",
        );
        let block = build(&direct(dns), &HashSet::new()).unwrap();
        let s = &block["servers"].as_array().unwrap()[1];
        assert_eq!(s["type"], "tls");
        assert_eq!(s["server"], "8.8.8.8");
        assert_eq!(s["server_port"], 853);
        assert_eq!(block["final"], "google-dot");
    }

    #[test]
    fn build_dns_emits_fakeip_server_and_auto_rule_when_enabled() {
        let lan_rule = DnsRule {
            domain_suffix: vec!["lan".to_string()],
            server: "local".to_string(),
            ..Default::default()
        };
        let dns = with_fakeip(active(vec![local()], "local"), vec![lan_rule]);
        let block = build(&direct(dns), &HashSet::new()).unwrap();
        // Sing-box 1.12 no longer accepts the top-level `dns.fakeip` block;
        // the ranges live inside the server entry instead.
        assert!(block.get("fakeip").is_none());
        let fake_server = block["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["type"] == "fakeip")
            .unwrap();
        assert_eq!(fake_server["tag"], "fakeip");
        assert_eq!(fake_server["inet4_range"], "198.18.0.0/15");
        assert_eq!(fake_server["inet6_range"], "fc00::/18");

        let rules = block["rules"].as_array().unwrap();
        assert_eq!(
            rules.len(),
            2,
            "auto-rule must be appended after user rules when fakeip is on"
        );
        assert_eq!(rules[0]["server"], "local");
        assert_eq!(rules[1]["server"], "fakeip");
        assert_eq!(rules[1]["query_type"][0], "A");
        assert_eq!(rules[1]["query_type"][1], "AAAA");
        assert!(block.get("independent_cache").is_none());
    }

    #[test]
    fn build_dns_does_not_duplicate_fakeip_rule_when_user_added_one() {
        let user_rule = DnsRule {
            domain_suffix: vec!["example.com".to_string()],
            server: "fakeip".to_string(),
            ..Default::default()
        };
        let dns = with_fakeip(active(vec![local()], "local"), vec![user_rule]);
        let block = build(&direct(dns), &HashSet::new()).unwrap();
        let rules = block["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["domain_suffix"][0], "example.com");
        assert_eq!(rules[0]["server"], "fakeip");
    }

    #[test]
    fn build_dns_keeps_explicit_fakeip_rules_without_the_catch_all() {
        let user_rule = DnsRule {
            domain_suffix: vec!["example.com".to_string()],
            server: "fakeip".to_string(),
            ..Default::default()
        };
        let mut dns = with_fakeip(active(vec![local()], "local"), vec![user_rule]);
        dns.fakeip_catch_all = false;
        let block = build(&direct(dns), &HashSet::new()).unwrap();
        let servers = block["servers"].as_array().unwrap();
        assert!(servers.iter().any(|server| server["type"] == "fakeip"));
        let rules = block["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["server"], "fakeip");
    }

    #[test]
    fn build_dns_drops_rules_whose_rule_set_is_not_routed() {
        let mut dns = default_active();
        dns.rules = vec![
            rule_set_rule("geosite-category-ru", "local"),
            DnsRule {
                domain_suffix: vec!["lan".to_string()],
                server: "local".to_string(),
                ..Default::default()
            },
        ];
        let upstreams = direct(dns);

        let block = build(&upstreams, &HashSet::new()).unwrap();
        let rules = block["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["domain_suffix"][0], "lan");

        let block = build(&upstreams, &HashSet::from(["geosite-category-ru"])).unwrap();
        assert_eq!(block["rules"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn build_dns_emits_fakeip_rule_when_the_user_fakeip_rule_is_not_routed() {
        let dns = with_fakeip(
            active(vec![local()], "local"),
            vec![rule_set_rule("geosite-category-ru", "fakeip")],
        );
        let block = build(&direct(dns), &HashSet::new()).unwrap();
        let rules = block["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["query_type"][0], "A");
        assert_eq!(rules[0]["server"], "fakeip");
    }

    #[test]
    fn build_dns_rejects_ip_rule_set_alongside_the_automatic_fakeip_rule() {
        let dns = with_fakeip(
            active(vec![local()], "local"),
            vec![rule_set_rule("geoip-ru", "local")],
        );
        let error = build(&direct(dns), &HashSet::from(["geoip-ru"])).unwrap_err();
        assert!(error.to_string().contains("rules[0]"));
        assert!(error.to_string().contains("geoip-ru"));
    }

    #[test]
    fn build_dns_accepts_rule_sets_that_cannot_conflict_with_fakeip() {
        let explicit_fakeip_rule = DnsRule {
            domain_suffix: vec!["example.com".to_string()],
            server: "fakeip".to_string(),
            ..Default::default()
        };
        for rules in [
            vec![rule_set_rule("geoip-ru", "local"), explicit_fakeip_rule],
            vec![rule_set_rule("geosite-category-ru", "local")],
        ] {
            let available = HashSet::from(["geoip-ru", "geosite-category-ru"]);
            let dns = with_fakeip(active(vec![local()], "local"), rules);
            assert!(build(&direct(dns), &available).is_ok());
        }
    }

    #[test]
    fn build_dns_emits_rules_with_domain_suffix() {
        let mut dns = default_active();
        dns.rules = vec![DnsRule {
            domain_suffix: vec!["example.com".to_string()],
            server: "local".to_string(),
            ..Default::default()
        }];
        let block = build(&direct(dns), &HashSet::new()).unwrap();
        let rules = block["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["domain_suffix"][0], "example.com");
        assert_eq!(rules[0]["server"], "local");
        assert!(rules[0].get("disable_cache").is_none());
    }

    #[test]
    fn dns_upstreams_tunnel_only_public_servers() {
        let udp_proxy = through_proxy(default_active());
        let no_proxy = direct(default_active());
        let cases = [
            (&udp_proxy, doh("doh", "1.1.1.1"), true),
            (&udp_proxy, doh("doh", "dns.google"), true),
            (&udp_proxy, udp("udp", "8.8.8.8"), true),
            (&udp_proxy, udp("router", "192.168.1.1"), false),
            (&udp_proxy, udp("loopback", "127.0.0.1"), false),
            (&udp_proxy, udp("cgnat", "100.64.0.1"), false),
            (&udp_proxy, udp("link-local", "169.254.1.1"), false),
            (&udp_proxy, udp("ula", "fd00::1"), false),
            (&udp_proxy, udp("v6-link-local", "fe80::1"), false),
            (
                &udp_proxy,
                DnsServer::Local {
                    tag: "local".into(),
                },
                false,
            ),
            (&no_proxy, doh("doh", "1.1.1.1"), false),
        ];
        for (upstreams, server, tunnelled) in cases {
            assert_eq!(upstreams.is_tunnelled(&server), tunnelled, "{server:?}");
        }
    }

    #[test]
    fn dns_upstreams_send_public_udp_servers_directly_on_a_profile_without_udp() {
        let doq = DnsServer::Quic {
            tag: "doq".into(),
            server: "94.140.14.14".into(),
            server_port: None,
        };
        let dns = active(vec![doh("remote", "1.1.1.1")], "remote");
        let upstreams = DnsUpstreams::new(dns, &RoutingMode::Global, &http_profile());
        assert!(upstreams.is_tunnelled(&doh("remote", "1.1.1.1")));
        for server in [udp("plain", "8.8.8.8"), doq] {
            assert!(!upstreams.is_tunnelled(&server), "{server:?}");
        }
    }

    #[test]
    fn dns_bypass_warning_names_public_udp_servers_a_profile_cannot_tunnel() {
        let preset =
            |server| custom_preset(vec![doh("remote", "1.1.1.1"), server], vec![], "remote");
        let public = settings_with_preset(preset(udp("plain", "8.8.8.8")));
        let warning = dns_bypass_warning(&http_profile(), &public).unwrap();
        assert!(warning.contains("\"plain\" (UDP)"), "{warning}");
        assert!(warning.contains("http"), "{warning}");

        let lan = settings_with_preset(preset(udp("router", "192.168.1.1")));
        let mut only = public.clone();
        only.geo_routing.current_region = Some(GeoRegion::Ru);
        only.geo_routing
            .selected_region_modes
            .insert(GeoRegion::Ru, RoutingMode::Only(GeoRegion::Ru));
        for (profile, settings) in [
            (http_profile(), &lan),
            (http_profile(), &only),
            (test_profile(), &public),
        ] {
            assert_eq!(dns_bypass_warning(&profile, settings), None);
        }
    }

    #[test]
    fn ech_dns_lookup_names_are_the_server_names_whose_ech_config_comes_from_dns() {
        let outbound = |ech: Value, server_name: &str| json!({ "tls": { "server_name": server_name, "ech": ech } });
        let outbounds = [
            outbound(json!({ "enabled": true }), "dns.example"),
            outbound(
                json!({ "enabled": true, "config": ["pem"] }),
                "static.example",
            ),
            outbound(json!({ "enabled": true }), "203.0.113.7"),
            outbound(json!({ "enabled": true }), "Mixed.Example."),
            json!({ "type": "direct" }),
        ];
        assert_eq!(
            ech_dns_lookup_names(&outbounds),
            ["dns.example", "mixed.example"]
        );
    }

    fn trojan_with_ech(config: Vec<String>) -> Profile {
        use crate::config::profile::{EchSettings, TlsCommon, TrojanConfig};
        profile_with(
            ProtocolConfig::Trojan(TrojanConfig {
                password: "pw".into(),
                tls: TlsCommon {
                    server_name: Some("ech.example".into()),
                    ech: Some(EchSettings {
                        enabled: true,
                        config,
                    }),
                    ..Default::default()
                },
                ..Default::default()
            }),
            "203.0.113.7",
            443,
        )
    }

    #[test]
    fn ech_dns_warning_names_an_unencrypted_lookup() {
        let doh = Settings::default();
        let mut plain = Settings::default();
        plain.dns.current_preset = "system_local".into();
        assert_eq!(ech_dns_warning(&trojan_with_ech(Vec::new()), &doh), None);
        assert_eq!(
            ech_dns_warning(&trojan_with_ech(vec!["pem".into()]), &plain),
            None
        );
        assert!(
            ech_dns_warning(&trojan_with_ech(Vec::new()), &plain)
                .unwrap()
                .starts_with(
                    "ECH config for \"ech.example\" is looked up over unencrypted DNS (local)"
                )
        );
    }

    #[test]
    fn build_dns_sends_the_ech_lookup_to_the_direct_bootstrap_first() {
        let ech_rule = |dns: ActiveDns| {
            let upstreams = through_proxy(dns);
            build_dns(
                &upstreams,
                upstreams.bootstrap(),
                &DnsStrategy::PreferIpv4,
                &HashSet::from(["geoip-ru"]),
                &["ech.example".to_string()],
            )
            .unwrap()["rules"][0]
                .clone()
        };
        assert_eq!(
            ech_rule(active(vec![doh("remote", "1.1.1.1")], "remote")),
            json!({ "query_type": ["HTTPS"], "domain": ["ech.example"], "server": "bootstrap" })
        );

        let mut with_ip_rule_set = active(vec![local(), doh("remote", "1.1.1.1")], "remote");
        with_ip_rule_set.rules = vec![DnsRule {
            rule_set: vec!["geoip-ru".into()],
            server: "local".into(),
            ..Default::default()
        }];
        assert_eq!(
            ech_rule(with_ip_rule_set),
            json!({ "domain": ["ech.example"], "server": "bootstrap" })
        );
    }

    #[test]
    fn generated_config_looks_up_a_dns_ech_config_outside_the_tunnel() {
        let profile = trojan_with_ech(Vec::new());
        let config = generate_config(
            &profile,
            &Settings::default(),
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        let rule = &config["dns"]["rules"][0];
        assert_eq!(rule["domain"], json!(["ech.example"]));
        assert_ne!(rule["server"], config["dns"]["final"]);
        let server = config["dns"]["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|server| server["tag"] == rule["server"])
            .unwrap();
        assert!(server.get("detour").is_none());
    }

    #[test]
    fn bootstrap_is_a_direct_copy_of_a_tunnelled_final_server() {
        let without_local = active(vec![doh("remote", "1.1.1.1")], "remote");
        let bootstrap = through_proxy(without_local).bootstrap();
        assert_eq!(bootstrap.tag, "bootstrap");
        let server = bootstrap.server.unwrap();
        assert_eq!(server["server"], "1.1.1.1");
        assert!(server.get("detour").is_none());

        let hostname = active(vec![local(), doh("remote", "dns.google")], "remote");
        let server = through_proxy(hostname).bootstrap().server.unwrap();
        assert_eq!(server["domain_resolver"], "local");

        let taken = active(
            vec![doh("remote", "1.1.1.1"), udp("bootstrap", "9.9.9.9")],
            "remote",
        );
        assert_eq!(through_proxy(taken).bootstrap().tag, "bootstrap-2");
    }

    #[test]
    fn bootstrap_is_the_final_server_itself_when_it_stays_direct() {
        for dns in [
            active(vec![local()], "local"),
            active(vec![udp("router", "192.168.1.1")], "router"),
        ] {
            let final_server = dns.final_server.clone();
            let bootstrap = through_proxy(dns).bootstrap();
            assert_eq!(bootstrap.tag, final_server);
            assert!(bootstrap.server.is_none());
        }
    }

    #[test]
    fn build_dns_tunnels_upstreams_and_resolves_hostnames_through_the_bootstrap() {
        let dns = active(
            vec![
                local(),
                doh("remote", "dns.google"),
                udp("router", "192.168.1.1"),
            ],
            "remote",
        );
        let block = build(&through_proxy(dns), &HashSet::new()).unwrap();
        let servers = block["servers"].as_array().unwrap();
        let tags: Vec<&str> = servers.iter().map(|s| s["tag"].as_str().unwrap()).collect();
        assert_eq!(tags, ["local", "remote", "router", "bootstrap"]);
        assert_eq!(servers[1]["detour"], "proxy");
        assert_eq!(servers[1]["domain_resolver"], "bootstrap");
        assert!(servers[2].get("detour").is_none());
        assert_eq!(servers[3]["domain_resolver"], "local");
    }

    #[test]
    fn build_dns_resolves_a_direct_hostname_final_server_through_local() {
        let dns = active(
            vec![
                local(),
                doh("remote", "dns.google"),
                doh("other", "dns.quad9.net"),
            ],
            "remote",
        );
        let block = build(&direct(dns), &HashSet::new()).unwrap();
        let servers = block["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 3);
        assert_eq!(servers[1]["domain_resolver"], "local");
        assert_eq!(servers[2]["domain_resolver"], "remote");
    }

    #[test]
    fn build_dns_server_resolves_hostnames_through_the_given_server() {
        let hostname = DnsServer::Https {
            tag: "remote".to_string(),
            server: "dns.google".to_string(),
            server_port: None,
            path: "/dns-query".to_string(),
        };
        assert_eq!(
            build_dns_server(&hostname, Some("local"), false)["domain_resolver"],
            "local"
        );
        let ip = DnsServer::Https {
            tag: "remote".to_string(),
            server: "1.1.1.1".to_string(),
            server_port: None,
            path: "/dns-query".to_string(),
        };
        assert!(
            build_dns_server(&ip, Some("local"), false)
                .get("domain_resolver")
                .is_none()
        );
    }

    #[test]
    fn generated_config_routes_dns_rules_only_with_their_rule_sets() {
        let mut settings = settings_with_preset(custom_preset(
            vec![local(), doh("remote", "1.1.1.1")],
            vec![rule_set_rule("geosite-category-ru", "local")],
            "remote",
        ));
        let mut rules_in = |mode: RoutingMode| {
            settings.geo_routing.current_region = Some(GeoRegion::Ru);
            settings
                .geo_routing
                .selected_region_modes
                .insert(GeoRegion::Ru, mode);
            let config = generate_config(
                &test_profile(),
                &settings,
                &GeoAvailability::all(),
                TEST_CLASH_PORT,
            )
            .unwrap();
            config["dns"].get("rules").is_some()
        };
        assert!(rules_in(RoutingMode::Bypass(GeoRegion::Ru)));
        assert!(!rules_in(RoutingMode::Global));
    }

    #[test]
    fn generated_config_sets_store_fakeip_when_enabled() {
        let mut settings = Settings::default();
        settings.dns.fakeip_enabled = true;
        let config = generate_config(
            &test_profile(),
            &settings,
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        assert_eq!(
            config["experimental"]["cache_file"]["store_fakeip"], true,
            "store_fakeip must persist the v4/v6→domain map across restarts"
        );
    }

    #[test]
    fn generated_config_keeps_the_cache_file_in_the_kvn_state_directory() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let state = tempfile::tempdir().unwrap();
        let _state = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", state.path());
        let config = generate_config(
            &test_profile(),
            &Settings::default(),
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        assert_eq!(
            config["experimental"]["cache_file"]["path"],
            state
                .path()
                .join("kvn/singbox-cache.db")
                .to_string_lossy()
                .as_ref()
        );
    }

    #[test]
    fn generated_config_omits_store_fakeip_when_disabled() {
        let profile = test_profile();
        let settings = Settings::default();
        let config = generate_config(
            &profile,
            &settings,
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        assert!(
            config["experimental"]["cache_file"]
                .get("store_fakeip")
                .is_none()
        );
    }

    #[test]
    fn generated_config_uses_the_active_custom_preset() {
        let settings = settings_with_preset(custom_preset(
            vec![local(), doh("quad9", "9.9.9.9")],
            Vec::new(),
            "quad9",
        ));
        let config = generate_config(
            &test_profile(),
            &settings,
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        assert_eq!(config["dns"]["final"], "quad9");
        let servers = config["dns"]["servers"].as_array().unwrap();
        assert_eq!(servers[1]["detour"], "proxy");
        assert_eq!(
            config["route"]["default_domain_resolver"]["server"],
            "bootstrap"
        );
        assert_eq!(servers[2]["tag"], "bootstrap");
        assert_eq!(servers[2]["server"], "9.9.9.9");
        assert!(servers[2].get("detour").is_none());
    }

    #[test]
    fn generated_config_keeps_dns_direct_when_only_matching_traffic_is_tunnelled() {
        let mut settings = Settings::default();
        settings.geo_routing.current_region = Some(GeoRegion::Ru);
        settings
            .geo_routing
            .selected_region_modes
            .insert(GeoRegion::Ru, RoutingMode::Only(GeoRegion::Ru));
        let config = generate_config(
            &test_profile(),
            &settings,
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        let servers = config["dns"]["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 2);
        assert!(servers.iter().all(|server| server.get("detour").is_none()));
        assert_eq!(
            config["route"]["default_domain_resolver"]["server"],
            "remote"
        );
    }

    // ---- Iran routing modes (previously only Ru/Cn covered) ----

    #[test]
    fn build_route_bypass_ir_has_private_rule_and_final_proxy() {
        let (route, rule_sets) = build_route(
            &RoutingMode::Bypass(GeoRegion::Ir),
            default_resolver(),
            &GeoAvailability::all(),
            &no_services(),
        );
        let rules = route["rules"].as_array().unwrap();
        assert!(rules.len() >= 4);
        assert_eq!(route["final"], "proxy");
        // BypassIr with geo data must register both geoip-ir and geosite-category-ir.
        let tags: Vec<&str> = rule_sets
            .iter()
            .map(|rs| rs["tag"].as_str().unwrap())
            .collect();
        assert!(tags.contains(&"geoip-ir"));
        assert!(tags.contains(&"geosite-category-ir"));
    }

    #[test]
    fn build_route_only_ir_has_private_rule_and_final_direct() {
        let (route, rule_sets) = build_route(
            &RoutingMode::Only(GeoRegion::Ir),
            default_resolver(),
            &GeoAvailability::all(),
            &no_services(),
        );
        let rules = route["rules"].as_array().unwrap();
        assert!(rules.len() >= 4);
        assert_eq!(route["final"], "direct");
        assert_eq!(rule_sets.len(), 2);
    }

    #[test]
    fn build_route_bypass_ru_has_private_rule_and_final_proxy() {
        // Only the OnlyRu variant was tested explicitly; Bypass had no direct test.
        let (route, rule_sets) = build_route(
            &RoutingMode::Bypass(GeoRegion::Ru),
            default_resolver(),
            &GeoAvailability::all(),
            &no_services(),
        );
        assert_eq!(route["final"], "proxy");
        assert_eq!(rule_sets.len(), 2);
    }

    // ---- Geo availability missing: rule_sets must stay empty ----

    #[test]
    fn build_route_bypass_ru_without_geo_emits_no_rule_sets() {
        let (_route, rule_sets) = build_route(
            &RoutingMode::Bypass(GeoRegion::Ru),
            default_resolver(),
            &GeoAvailability::default(),
            &no_services(),
        );
        assert!(rule_sets.is_empty());
    }

    #[test]
    fn build_route_only_cn_without_geo_emits_no_rule_sets() {
        let (_route, rule_sets) = build_route(
            &RoutingMode::Only(GeoRegion::Cn),
            default_resolver(),
            &GeoAvailability::default(),
            &no_services(),
        );
        assert!(rule_sets.is_empty());
    }

    #[test]
    fn build_route_bypass_ir_without_geo_emits_no_rule_sets() {
        let (_route, rule_sets) = build_route(
            &RoutingMode::Bypass(GeoRegion::Ir),
            default_resolver(),
            &GeoAvailability::default(),
            &no_services(),
        );
        assert!(rule_sets.is_empty());
    }

    #[test]
    fn build_route_only_ir_without_geo_emits_no_rule_sets() {
        let (_route, rule_sets) = build_route(
            &RoutingMode::Only(GeoRegion::Ir),
            default_resolver(),
            &GeoAvailability::default(),
            &no_services(),
        );
        assert!(rule_sets.is_empty());
    }

    // ---- Per-service routing overrides ----

    fn no_services() -> HashMap<RoutedService, ServiceRoute> {
        HashMap::new()
    }

    fn routes(entries: &[(RoutedService, ServiceRoute)]) -> HashMap<RoutedService, ServiceRoute> {
        entries.iter().copied().collect()
    }

    #[test]
    fn build_route_service_direct_adds_rule_and_rule_sets_in_global() {
        let (route, rule_sets) = build_route(
            &RoutingMode::Global,
            default_resolver(),
            &GeoAvailability::all(),
            &routes(&[(RoutedService::Steam, ServiceRoute::Direct)]),
        );
        let rules = route["rules"].as_array().unwrap();
        for tag in ["geosite-steam", "geoip-steam"] {
            let steam_rule = rules
                .iter()
                .find(|r| r["rule_set"] == json!([tag]))
                .expect("steam rule present");
            assert_eq!(steam_rule["outbound"], "direct");
        }
        let tags: Vec<&str> = rule_sets.iter().filter_map(|r| r["tag"].as_str()).collect();
        assert_eq!(tags, ["geosite-steam", "geoip-steam"]);
        assert!(
            rule_sets
                .iter()
                .all(|r| r["type"] == "local" && r["format"] == "binary")
        );
    }

    #[test]
    fn build_route_service_proxy_uses_proxy_outbound() {
        let (route, rule_sets) = build_route(
            &RoutingMode::Bypass(GeoRegion::Ru),
            default_resolver(),
            &GeoAvailability::all(),
            &routes(&[(RoutedService::Telegram, ServiceRoute::Proxy)]),
        );
        let rules = route["rules"].as_array().unwrap();
        for tag in ["geosite-telegram", "geoip-telegram"] {
            let tg_rule = rules
                .iter()
                .find(|r| r["rule_set"] == json!([tag]))
                .expect("telegram rule present");
            assert_eq!(tg_rule["outbound"], "proxy");
        }
        // Telegram's 2 rule-sets + the region's 2.
        assert_eq!(rule_sets.len(), 4);
    }

    #[test]
    fn build_route_service_rules_precede_geo_rules_under_only() {
        let (route, rule_sets) = build_route(
            &RoutingMode::Only(GeoRegion::Ru),
            default_resolver(),
            &GeoAvailability::all(),
            &routes(&[(RoutedService::Steam, ServiceRoute::Direct)]),
        );
        let rules = route["rules"].as_array().unwrap();
        let steam_idx = rules
            .iter()
            .position(|r| r["rule_set"] == json!(["geoip-steam"]))
            .unwrap();
        let geo_idx = rules
            .iter()
            .position(|r| r["rule_set"] == json!(["geosite-category-ru"]))
            .unwrap();
        assert!(
            steam_idx < geo_idx,
            "service overrides must win over the regional rules"
        );
        assert_eq!(rule_sets.len(), 4);
    }

    #[test]
    fn build_route_service_rules_follow_all_order() {
        // Same config must always yield the same rule order, regardless of
        // HashMap internals — services appear in RoutedService::ALL order.
        let (route, _) = build_route(
            &RoutingMode::Global,
            default_resolver(),
            &GeoAvailability::all(),
            &routes(&[
                (RoutedService::Telegram, ServiceRoute::Direct),
                (RoutedService::Steam, ServiceRoute::Direct),
            ]),
        );
        let rules = route["rules"].as_array().unwrap();
        let service_rules: Vec<&Value> = rules
            .iter()
            .filter(|r| r.get("rule_set").is_some())
            .collect();
        let tags: Vec<&Value> = service_rules.iter().map(|r| &r["rule_set"][0]).collect();
        assert_eq!(
            tags,
            [
                "geosite-steam",
                "geosite-telegram",
                "geoip-steam",
                "geoip-telegram"
            ]
        );
    }

    #[test]
    fn build_route_sniffs_before_domain_rules_and_resolves_before_ip_rules() {
        let (route, _) = build_route(
            &RoutingMode::Bypass(GeoRegion::Ru),
            default_resolver(),
            &GeoAvailability::all(),
            &routes(&[(RoutedService::Steam, ServiceRoute::Proxy)]),
        );
        let rules = route["rules"].as_array().unwrap();
        let position =
            |predicate: &dyn Fn(&Value) -> bool| rules.iter().position(predicate).unwrap();
        let sniff = position(&|r| r["action"] == "sniff");
        let resolve = position(&|r| r["action"] == "resolve");
        let first_domain_rule = position(&|r| r["rule_set"] == json!(["geosite-steam"]));
        let first_ip_rule = position(&|r| r["rule_set"] == json!(["geoip-steam"]));
        assert!(sniff < first_domain_rule);
        assert_eq!(rules[sniff]["domain_regex"], json!(["."]));
        assert_eq!(rules[sniff]["invert"], true);
        assert!(first_domain_rule < resolve && resolve < first_ip_rule);
        assert_eq!(rules[resolve]["server"], "remote");
    }

    #[test]
    fn build_route_disabled_services_emit_no_entries() {
        let (route, rule_sets) = build_route(
            &RoutingMode::Global,
            default_resolver(),
            &GeoAvailability::all(),
            &routes(&[(RoutedService::Steam, ServiceRoute::Disabled)]),
        );
        assert!(rule_sets.is_empty());
        assert!(
            route["rules"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r.get("rule_set").is_none())
        );
    }

    #[test]
    fn build_route_service_enabled_without_files_is_noop() {
        let (_route, rule_sets) = build_route(
            &RoutingMode::Global,
            default_resolver(),
            &GeoAvailability::default(),
            &routes(&[(RoutedService::Steam, ServiceRoute::Direct)]),
        );
        assert!(rule_sets.is_empty());
    }

    #[test]
    fn generated_config_includes_service_rule_sets_when_enabled() {
        let profile = test_profile();
        let mut settings = Settings::default();
        settings
            .geo_routing
            .service_routes
            .insert(RoutedService::Steam, ServiceRoute::Direct);
        let config = generate_config(
            &profile,
            &settings,
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        let tags: Vec<&str> = config["route"]["rule_set"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["tag"].as_str())
            .collect();
        assert!(tags.contains(&"geosite-steam"));
        assert!(tags.contains(&"geoip-steam"));
    }

    #[test]
    fn generated_config_omits_service_rule_sets_by_default() {
        let profile = test_profile();
        let settings = Settings::default();
        let config = generate_config(
            &profile,
            &settings,
            &GeoAvailability::all(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        assert!(config["route"].get("rule_set").is_none());
    }

    // ---- dns_server_value for every variant ----

    #[test]
    fn dns_server_value_emits_local_shape() {
        let v = dns_server_value(&DnsServer::Local {
            tag: "loc".to_string(),
        });
        assert_eq!(v["tag"], "loc");
        assert_eq!(v["type"], "local");
    }

    #[test]
    fn dns_server_value_emits_udp_and_tcp_with_optional_port() {
        let udp = dns_server_value(&DnsServer::Udp {
            tag: "u".to_string(),
            server: "1.1.1.1".to_string(),
            server_port: Some(53),
        });
        assert_eq!(udp["type"], "udp");
        assert_eq!(udp["server_port"], 53);
        let udp_no_port = dns_server_value(&DnsServer::Udp {
            tag: "u".to_string(),
            server: "1.1.1.1".to_string(),
            server_port: None,
        });
        assert!(udp_no_port.get("server_port").is_none());

        let tcp = dns_server_value(&DnsServer::Tcp {
            tag: "t".to_string(),
            server: "1.1.1.1".to_string(),
            server_port: None,
        });
        assert_eq!(tcp["type"], "tcp");
    }

    #[test]
    fn dns_server_value_emits_quic_shape() {
        let v = dns_server_value(&DnsServer::Quic {
            tag: "q".to_string(),
            server: "9.9.9.9".to_string(),
            server_port: Some(853),
        });
        assert_eq!(v["type"], "quic");
        assert_eq!(v["server_port"], 853);
    }

    #[test]
    fn fakeip_server_value_emits_inet_ranges() {
        let v = fakeip_server_value(&FakeIpServer {
            tag: "fakeip".to_string(),
            ranges: Default::default(),
        });
        assert_eq!(v["type"], "fakeip");
        assert_eq!(v["inet4_range"], "198.18.0.0/15");
        assert_eq!(v["inet6_range"], "fc00::/18");
    }

    // ---- build_dns_rule branches ----

    #[test]
    fn build_dns_rule_emits_only_populated_fields() {
        let rule = DnsRule {
            domain: vec!["a.example".to_string()],
            server: "remote".to_string(),
            ..DnsRule::default()
        };
        let v = build_dns_rule(&rule);
        assert!(v.get("domain").is_some());
        assert!(v.get("domain_suffix").is_none());
        assert!(v.get("domain_keyword").is_none());
        assert!(v.get("domain_regex").is_none());
        assert!(v.get("rule_set").is_none());
        assert_eq!(v["server"], "remote");
    }

    #[test]
    fn build_dns_rule_includes_domain_keyword_regex_and_ruleset() {
        let rule = DnsRule {
            domain_keyword: vec!["youtube".to_string()],
            domain_regex: vec!["^ads\\.".to_string()],
            rule_set: vec!["geosite-ads".to_string()],
            server: "remote".to_string(),
            ..DnsRule::default()
        };
        let v = build_dns_rule(&rule);
        assert_eq!(v["domain_keyword"], json!(["youtube"]));
        assert_eq!(v["domain_regex"], json!(["^ads\\."]));
        assert_eq!(v["rule_set"], json!(["geosite-ads"]));
    }

    #[test]
    fn build_dns_rule_emits_disable_cache_when_set() {
        let rule = DnsRule {
            domain: vec!["x.test".to_string()],
            server: "remote".to_string(),
            disable_cache: true,
            ..DnsRule::default()
        };
        let v = build_dns_rule(&rule);
        assert_eq!(v["disable_cache"], true);
    }

    #[test]
    fn generate_test_config_has_socks_inbound_on_given_port() {
        let profile = test_profile();
        let cfg = generate_test_config(&profile, 12345).unwrap();
        let inbounds = cfg["inbounds"].as_array().unwrap();
        assert_eq!(inbounds.len(), 1);
        assert_eq!(inbounds[0]["type"], "socks");
        assert_eq!(inbounds[0]["listen"], "127.0.0.1");
        assert_eq!(inbounds[0]["listen_port"], 12345);
    }

    #[test]
    fn generate_test_config_log_level_is_warn() {
        let profile = test_profile();
        let cfg = generate_test_config(&profile, 9999).unwrap();
        assert_eq!(cfg["log"]["level"], "warn");
    }

    #[test]
    fn generate_test_config_includes_profile_outbound() {
        let profile = test_profile();
        let cfg = generate_test_config(&profile, 9999).unwrap();
        let outbounds = cfg["outbounds"].as_array().unwrap();
        assert!(!outbounds.is_empty());
        // The primary outbound should have tag "proxy" (VLESS convention).
        assert!(outbounds.iter().any(|o| o["tag"] == "proxy"));
    }

    #[test]
    fn generate_test_config_has_route_to_proxy_and_no_dns() {
        let profile = test_profile();
        let cfg = generate_test_config(&profile, 9999).unwrap();
        assert_eq!(
            cfg["route"]["final"], "proxy",
            "route must send traffic to proxy outbound"
        );
        assert_eq!(
            cfg["route"]["default_mark"].as_u64(),
            Some(666),
            "default_mark must match killswitch nft rule (0x29a)"
        );
        assert_eq!(
            cfg["route"]["auto_detect_interface"].as_bool(),
            Some(true),
            "auto_detect_interface needed to bypass TUN via SO_BINDTODEVICE"
        );
        assert!(
            cfg.get("dns").is_none(),
            "no dns section needed in test config"
        );
    }
}

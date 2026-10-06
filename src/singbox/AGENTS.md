# Agent Guide: sing-box configuration

This guide extends the root [`AGENTS.md`](../../AGENTS.md), whose rules apply here too. It covers `src/singbox/` and the geo rule-sets it consumes (`src/geo.rs`): config generation, the Clash API port, routing modes, and service routing overrides. DNS generation is described with the DNS model in `src/config/AGENTS.md` § DNS Configuration. Read it before changing that code.

## sing-box Config Generation
- `singbox::config::generate_config` builds a complete sing-box 1.14+ JSON object from a `Profile` and `Settings`.
- The config is written atomically with mode `0600` to `$XDG_RUNTIME_DIR/kvn-tui/singbox.json`, validated with `sing-box check`, and only then is `sing-box run` spawned. The private `kvn-tui/` runtime directory has mode `0700`; a desktop user session with `XDG_RUNTIME_DIR` is required.
- The Clash API `external_controller` port is allocated per start by `net::allocate_loopback_port` and passed into `generate_config`; it is never a fixed number and never a user setting. `sing-box check` does **not** validate bindability (it exits 0 against an occupied controller port), so a conflict only appears when `sing-box run` fails. `runner::start` therefore retries up to `CLASH_API_START_ATTEMPTS` (3) times with a fresh port, but only when the failure names `address already in use` for that exact port — every other failure is returned immediately, and an exhausted retry keeps the original stderr in the error chain. The chosen port lives on `ProcessHandle`, so it cannot outlive the process; the daemon reads it from the process slot when executing `Effect::FetchTrafficStats`.
- If the process exits immediately, stderr is captured and surfaced to the user.
- `build_outbound(profile)` dispatches to a per-protocol builder and returns `Vec<serde_json::Value>` (most protocols return one outbound; ShadowTLS returns two — a `shadowtls` wrapper tagged `shadowtls-wrap` plus a `shadowsocks` detour tagged `proxy`).
- Shared helpers: `build_tls_block` (TLS + ECH + REALITY), `build_transport_block` (WebSocket / gRPC / HTTP upgrade). No deprecated sing-box fields (no `obfs_password`, no `aes-128-cfb`, no top-level `dns.fakeip`, no WireGuard outbound).

## Routing Modes
- `RoutingMode` is `Global | Bypass(GeoRegion) | Only(GeoRegion)`, serialized as `global`, `bypass_<region>`, `only_<region>` (hand-written serde).
- `RoutingMode::Global` — all traffic through VPN.
- `RoutingMode::Bypass(region)` — the region's IPs/domains bypass the VPN (direct).
- `RoutingMode::Only(region)` — only the region's IPs/domains go through the VPN; everything else is direct.
- The available routing modes depend on the selected **geo region** (`Ru`, `Cn`, `Ir`, or `Global`). `RoutingMode::available(region)` returns the list dynamically.
- Geo-region and routing-mode preferences are grouped under `settings.geo_routing: GeoRouting`. It stores `current_region: Option<GeoRegion>` and `selected_region_modes: HashMap<GeoRegion, RoutingMode>`. The active mode is derived from `selected_region_modes[current_region]` and falls back to `Global`. Switching back to a previously used region restores its last routing mode.
- Rule-sets are local `.srs` binary files downloaded to `~/.config/kvn-tui/geo/`.
- **Service routing overrides** (`geo_routing.service_routes: HashMap<RoutedService, ServiceRoute>`, absent = `Disabled` / opt-in): orthogonal to the routing mode — each of the predefined services (`Steam`, `Telegram`) can be forced to `Direct` (real network location; e.g. Steam CDN downloads) or `Proxy` (always through the tunnel, even under `Bypass`).
  - `build_route` emits one `rule_set → outbound` rule per rule-set file, every service rule ahead of the geo rules so an override wins in every mode, in the order `sniff` → service domain rules → `resolve` → service IP rules → `ip_is_private` → region geosite → `resolve` (when no service IP rule took it) → region geoip.
  - A TUN connection carries only an IP, so domain rule-sets match only after `sniff`, which runs only while the destination has no name (`domain_regex: ["."]` + `invert`): sing-box matches a sniffed name ahead of the fake-IP name, so an SNI that differs from the queried name (ECH, domain fronting) would otherwise move the connection to another rule; with fake-IP the destination is a domain, so IP rule-sets match only after `resolve`, which queries the active preset's final server (tunnelled where DNS is) — never the direct `bootstrap` copy, and never the fake-IP server.
  - Each action is emitted only when a rule of its kind follows, so Global without service overrides keeps the three base rules.
  - Services iterate `RoutedService::ALL` (never the map — HashMap order is nondeterministic).
  - Assets are declared in `geo::service_assets()` as a `ServiceAssets { geoip: Option<GeoAsset>, geosite: Option<GeoAsset> }` descriptor per service, all sourced from MetaCubeX/meta-rules-dat (one provider, one branch layout).
  - They are fetched *through the tunnel* (`Effect::DownloadServiceRuleSetsIfMissing`) — never pre-connect, where the kill switch or ISP blocks would stall the fetch — and refreshed with the periodic geo updates.
  - Two triggers: after `Msg::Connected` (backstop), and on a service-routing commit while connected, where the reconnect is DEFERRED until the download pass reports back (`Model::pending_service_reconnect` → `Msg::ServiceRuleSetsReady`) so a first-enabled service's rules are live on the very next connection rather than requiring a second reconnect.
  - Missing files degrade to "no rule for that service", never a failed connection.
  - Edited on Settings › Routing (`Space r`; the deprecated `S` still opens `Overlay::ServiceRouting`) (draft map in `Model::service_routing_draft`; cycling a route back to `Disabled` removes its entry — absent = Disabled — so a full cycle commits as a no-op; committed atomically on Enter).

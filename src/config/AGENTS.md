# Agent Guide: Configuration

This guide extends the root [`AGENTS.md`](../../AGENTS.md), whose rules apply here too. It covers the module layout of `src/config/profile/`, `src/config/` and the external editor (`tui_client/editor*`): share-link parsing, the DNS data model, validation and schema migrations, and editor sessions with JSON Schema checking and conflict resolution. Read it before changing that code.

## Share-Link Parsing
- Entry point: `config::profile::parse_share_link(uri)` dispatches on the URI scheme.
- Supported schemes: `vless://`, `vmess://`, `trojan://`, `ss://`, `hysteria2://`, `hy2://`, `tuic://`, `shadowtls://`, `anytls://`, `socks://`, `socks5://`, `http://`, `https://`, `ssh://`.
- All supported schemes are listed in `SUPPORTED_SHARE_SCHEMES` (used by both dispatch and the subscription Base64 heuristic in `config::subscription`).
- VLESS: extracts UUID, host, port, fragment (name), `flow`, `security`, `fp`, transport type, and REALITY params (`pbk`, `sid`, `sni`, `spx`). ECH config also parsed when present.
- VMess: handles both base64-JSON (v2rayN / Shadowrocket) and inline URI forms.
- Shadowsocks: handles SIP002 (`ss://base64(method:password)@host:port`) and legacy fully-base64 forms.
- Hysteria 2: `hy2://` is an alias for `hysteria2://`.
- SOCKS: `socks5://` is an alias for `socks://`.
- TLS and transport parameters map onto the shared `TlsCommon` / `TransportConfig` types (root `AGENTS.md` § Serialization); VLESS keeps its fields flat for backward compatibility. `TransportConfig` is shared across VLESS, VMess, Trojan, AnyTLS.

## DNS Configuration
- **Data model**: `settings.dns: DnsConfig` holds `current_preset: String`, `custom_presets: Vec<CustomDnsPreset>`, `strategy: DnsStrategy`, `fakeip_enabled: bool` and `fakeip_ranges: FakeIpRanges`.
  - A preset is `{ servers, rules, final_server }`; the active one is selected **by name**, never copied, so selecting a preset cannot overwrite another.
  - Built-in presets (`DnsPreset`: `cloudflare_doh` default, `google_dot`, `quad9_doh`, `system_local`) live in code (`DnsPreset::canonical`) and have no rules; custom presets are written in `profiles.json` and their rules reference their own server tags.
  - `DnsConfig::active()` resolves the selection into `ActiveDns { servers, rules, final_server, fakeip }`, adding the global fake-IP server (`FakeIpServer`, tag `FAKEIP_SERVER_TAG` = `fakeip`) when `fakeip_enabled` or when a rule of the active preset targets it, as in v5, where the fake-IP server was always present and the flag only controlled the catch-all rule; `ActiveDns::fakeip_catch_all` carries the flag for that rule and `store_fakeip`; every reader — generation, the status badge, the kill-switch endpoints — goes through it.
  - Server variants map 1:1 onto sing-box 1.14 server types: `Local`, `Udp`, `Tcp`, `Tls` (DoT), `Https` (DoH, with optional `path`), `Quic` (DoQ); fake-IP is not a server variant.
- **Validation** (`DnsConfig::diagnostics`, collected by `Config::diagnostics`): `current_preset` names a built-in or custom preset; custom preset names are non-empty, unique and not a built-in name; within each preset (pointers under `/custom_presets/{i}`, messages labelled `dns.custom_presets[i]`) server tags are non-empty, unique and not the reserved `fakeip`, `final_server` references one of them and every `rule.server` references one of them or `fakeip`, and a server whose address is a hostname has a `Local` server to resolve it; `fakeip_ranges` are syntactically valid IP prefixes; `strategy` is `prefer_ipv4` or `ipv4_only` (the IPv6 values still deserialize so older files can be migrated, and the schema's `enum` mirrors the check).
  - Every save validates (`serialized_config_with_schema`), so a diagnostic blocks every TUI save, not only the editor: add one only for a config that `sing-box check` rejects in every routing state.
  - Never be stricter than sing-box — the fake-IP range check is syntax only, because sing-box accepts a range of the other address family, and `domain_regex` is left to `sing-box check` (Go RE2 syntax).
- **Schema v5 → v6** (`Config::migrate_v5_to_v6`): besides the DNS presets below, `prefer_ipv6` → `prefer_ipv4` and `ipv6_only` → `ipv4_only` (IPv4 strategies unchanged), with `settings.dns_strategy` synced from the result.
  - Presets (`DnsConfig::migrate_legacy_servers`): the old top-level `servers` / `rules` / `final_server` are read only through `#[serde(skip_serializing)]` + `#[schemars(skip)]` legacy fields (so the editor schema rejects them in a v6 document); because the step drains them, any that remain in a loaded v6 file were written by hand, and `DnsConfig::diagnostics` reports each one instead of letting the next save drop it silently.
  - If the old servers minus fake-IP equal a built-in's canonical set (ports normalized, order-insensitive) and there are no rules, `current_preset` becomes that built-in; otherwise they become custom preset `custom` (`custom-2`, … when taken), which is selected.
  - The first old fake-IP server's ranges move to `fakeip_ranges`, and rules that targeted an old fake-IP server's tag are pointed at the reserved `fakeip`; a regular server already tagged `fakeip` is renamed to an unused tag (`fakeip-2`, …) together with the rules and `final_server` that referenced it.
- **Legacy migration**: the old `settings.dns_strategy` field is still deserialized; the ordered `Config::migrate_to` schema steps promote it into `dns.strategy` if the new value is still at its default. `load_config_at` runs those steps implicitly and persists the result; a schema newer than the build supports is refused. On save, `save_config_at` mirrors `dns.strategy` back into `dns_strategy` so older kvn builds keep loading the file.
- **Config generation** (`singbox::config::build_dns`): emits the sing-box 1.14 schema — no legacy top-level `dns.fakeip` block and no deprecated `independent_cache`; the fake-IP server carries its own ranges.
  - A server whose address is a hostname gets `domain_resolver` = the first `Local` server.
  - A user rule is dropped while any of its `rule_set` tags is missing from the generated `route.rule_set` (wrong routing mode, files not downloaded) — the whole rule, never a single tag, so a rule is never widened.
  - When `fakeip_enabled` is set and a `fakeip` server exists, the builder appends an `{ query_type: ["A","AAAA"], server: <tag> }` rule after the surviving user rules, unless a surviving rule already targets that server, and sets `experimental.cache_file.store_fakeip: true` so the IP→domain map survives restarts; `cache_file.path` is always `paths::singbox_cache_path()` (`$XDG_STATE_HOME/kvn/singbox-cache.db`, created by `runner::write_config`), never sing-box's default `cache.db` in the daemon's working directory. sing-box 1.14 rejects that `query_type` rule next to a DNS rule using an IP rule-set (`geo::is_ip_rule_set_tag`), so that combination makes `generate_config` fail with the rule index and both fixes; this depends on the routing state and is deliberately not a diagnostic.
- **User-facing privacy model**: `docs/privacy.md` tells users which traffic and DNS goes through the tunnel per routing mode, the IPv4-only limits of the tunnel (IPv6 blocked; the VPN server, directly queried DNS servers — Only mode, local servers, the bootstrap copy — and direct destinations reachable over IPv4 only, while a tunnelled public DNS server may be IPv6 if the VPN server has IPv6 egress), the exact kill-switch exceptions (mirroring `contrib/killswitch.nft`), and a self-check with `curl --resolve` and `tcpdump`. Update it with any change to routing, DNS paths, the kill switch, or the TUN.
- **Upstream path** (`singbox::config::DnsUpstreams`): DNS servers without `detour` are dialed directly by sing-box, so when the routing mode's final outbound is `proxy` (Global, Bypass) every `udp`/`tcp`/`tls`/`https`/`quic` server gets `detour: "proxy"` — except servers on a local address (loopback, RFC 1918, link-local, CGNAT `100.64.0.0/10`, IPv6 unique-local; hostnames count as public).
  - Only mode tunnels nothing.
  - A public `udp`/`quic` server that would be tunnelled on a profile whose outbound cannot carry UDP (`singbox::outbound::proxy_carries_udp`: HTTP, SSH, ShadowTLS, SOCKS 4/4a) makes `DnsUpstreams::new`, and so the connect, fail with the server tag and the fix: sending it directly would leak every queried domain past the VPN, and the user must see that rather than have it hidden.
  - All of this reads the active preset (`ActiveDns`).
  - When the final server is tunnelled, a direct copy of it is appended under an unused `bootstrap` tag and used as `route.default_domain_resolver` and as the `domain_resolver` of hostname servers, so the VPN server's hostname is never resolved through the tunnel it is opening; a hostname final server's copy resolves through the first `Local` server.
  - When the final server stays direct, it is the bootstrap itself.
- **TUI overlay** (`Overlay::DnsSettings`, Settings › DNS / `Space d`): three rows — the preset, the strategy, and the fake-IP toggle. `h` / `l` on the Preset row cycle `DnsConfig::preset_names` (built-ins, then custom presets in file order) into `Model::dns_preset_draft: Option<String>`; built-ins render with their labels (Cloudflare DoH, …), custom presets with their names. Custom presets are written in `profiles.json` via `e`.
- **Commit** (`update/key/dns.rs::commit_dns_drafts`): applies all drafts (the preset draft sets `current_preset`). A preset draft naming a preset that no longer exists (the config changed while the overlay was open) is dropped with an error status, the overlay stays open, and nothing is saved or reconnected. Turning fake-IP on while a rule of the active preset uses an IP rule-set and none targets the fake-IP server shows a warning status.
- **Strategy draft**: `Model::dns_strategy_draft: Option<DnsStrategy>` previews strategy changes while the overlay is open. The label renders as `Strategy: ‹ value ›` with a trailing `*` when the draft differs from the saved setting. Enter commits the draft (clears it, triggers `SaveConfig` + reconnect-if-connected); Esc/q discards it. `h` / `l` toggle between `prefer_ipv4` and `ipv4_only` (`DnsStrategy::toggled`): the tunnel is IPv4-only, so the IPv6 strategies are not offered.
- **Status bar**: a `[DNS: <kind>]` badge derives its label from the active preset's final server `kind_label` (`DoH` / `DoT` / `DoQ` / `UDP` / `TCP` / `local`) or `fakeip` when `fakeip_enabled` is true.

## External Editor Sessions

- `e` edits a private snapshot. The TUI keeps terminal input paused until the
  editor session saves successfully or ends with cancellation or a failure.
- JSON and validation errors offer another editor pass or explicit cancellation.
  Validation is collected, not fail-fast: `Config::diagnostics` returns every
  problem as a `ConfigDiagnostic` with the JSON pointer of the offending value
  (each nested type reports pointers relative to itself; the parent prefixes
  them). The editor maps pointers to lines through one
  `config::json_pointer::JsonIndex` per document, lists the problems in file
  order and reopens at the first one. `Config::validate` stays the `anyhow`
  boundary for every other caller and joins all messages.
- serde stops at the first parse error, so `check_edit` first validates the raw
  JSON against the generated schema and lists every structural problem (messages
  never quote the document's values, so passwords are never echoed). When
  `schema_version` is current and the document deserializes,
  `Config::diagnostics` runs as well and its problems are added, skipping any at
  a pointer the schema already reported, so a schema error never hides a
  duplicate id or a broken reference. A wrong `schema_version` stops the check
  before deserializing: the schema pins it as required and equal to
  `CURRENT_SCHEMA_VERSION` (`json_schema::pin_current_schema_version`): serde
  defaults a missing version to 0, and loading would then silently re-run every
  migration over current data — v4→v5 alone resets the TUN name and the
  connectivity probe and re-enables HTTP subscriptions.
- Every config type derives `JsonSchema`; `ShadowtlsVersion` and `RoutingMode`
  have hand-written serde, so they implement it by hand from the same values.
  `#[serde(flatten)]` drops `additionalProperties: false` from the protocol
  configs that deny unknown fields (SOCKS, SSH, Shadowsocks), so
  `json_schema::share_profile_fields_with_protocol_branches` restores it on
  those `Profile` branches; the other protocols stay lenient, as serde is. It
  lists the shared profile fields in every branch, not only the strict ones: a
  JSON language server reports an unmatched `oneOf` through the branch that
  matches the most properties, and fields listed only in the strict branches
  made it report an unknown `protocol` as `must be "shadowsocks"`.
  `json_schema::check_branches_only_for_known_protocols` then moves the branches
  under `if`/`then`, checked only when `protocol` is one of their values, and
  adds a `Profile`-level `protocol` `enum` of those values, so an unknown
  protocol reports that list instead of a guessed branch's errors. The validator
  implements `if`/`then` for it.
- Value constraints that `Config::diagnostics` also enforces are mirrored in the
  schema with `#[schemars(...)]` field attributes so a JSON language server
  flags them while typing — `minLength: 1` on the profile and protocol fields
  that must not be empty, `minimum` on `port` and `logs.line_retention`, and
  `enum` for `settings.theme` and `logs.level`, built from the same constants. A
  schema problem blocks saving like any other, so such a constraint must never
  be stricter than the semantic check it mirrors (the ShadowTLS `password`,
  which only v3 requires, therefore carries none);
  `value_constraints_match_the_semantic_checks` pins both directions.
  `json_schema` tests pin that a document with every protocol and settings shape
  passes both serde and the schema, and fail when the generated schema starts
  using a keyword the validator does not implement.
- The snapshot carries `"$schema"` (`Config::json_schema`) pointing at
  `$XDG_RUNTIME_DIR/kvn/profiles.schema.json`, rewritten on every editor open.
  The editor session keeps the reference and writes it into a generated conflict
  document too (`editor::conflict_document`), so a resolution pass keeps
  completion and checking. Only `config::save_editor_snapshot_at` writes it;
  every `profiles.json` save drops it, because older builds reject the unknown
  key — `kvn config recover` on a preserved edit would otherwise make the file
  unreadable to them.
- Concurrent conflicts show their paths and offer to reopen with `YOUR EDIT` /
  `CURRENT` markers, generated by `config::merge::resolution` using the existing
  UUID-aware three-way merge. Independent changes survive in both alternatives.
  Each resolution uses the daemon's conflict-time configuration as its new base.
- Each save opens a separate IPC connection and uses a correlated
  `ApplyEditedConfig` request. `StateSnapshot::config_edit_result` is optional,
  appears only on the matching reply, and reports the result after persistence.
  Legacy uncorrelated requests retain their recovery-file behavior. Interactive
  sessions preserve unfinished work on cancellation or failure only when the
  bytes differ from the original snapshot. A generated conflict document counts
  as unfinished work even before further editing. Explicit cancellation is an
  informational client-local toast.
- Terminal input uses unbuffered descriptor reads. The paused event reader
  discards partial decoding state. Retry prompts use the shared decoder in raw
  mode and discard pending input on entry and exit. Enter continues editing;
  q or Esc cancels immediately, without an answer line. Paste and unrelated
  input are ignored.
- Each retry prompt shows only the current issue on a temporary alternate
  screen, preserving terminal history. Long messages scroll with j/k, arrows,
  or g/G; the action footer remains visible.
- Report the editor outcome even if restoring the terminal fails, and retain
  its diagnostics and recovery path in any terminal or IPC error returned.
- A missing reply does not prove that saving failed. Preserve the editor copy
  and tell the user to check the current configuration before retrying.

## Module Layout

- **`config::profile` submodules** (`src/config/profile/{diagnostic,json_schema,protocol,protocol_options,protocol_config,tls,entry,schedule,subscription,routing,settings,schema,migrate,dns}.rs`, `src/config/profile/share_link{.rs,/parse.rs,/encode.rs}`): `src/config/profile.rs` is a facade of `pub use` re-exports; each persisted type lives in one submodule — `ConfigDiagnostic` (a validation problem: JSON pointer + message), the draft-07 JSON Schema generated from the types with `schemars` and the in-house validator for exactly the keywords it emits, plus the `uuid` / `date` / `date-time` formats (`config_json_schema`, `schema_diagnostics`), protocol discriminant, per-protocol options and configs, shared TLS/transport blocks, `Profile`, auto-update schedules, `Subscription`, geo routing, `Settings`, the root `Config`, the ordered schema migrations, DNS, and share-link URI parsing/encoding

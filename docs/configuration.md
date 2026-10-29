# Configuration

`kvn` stores profiles, subscriptions, and application settings in:

```text
~/.config/kvn-tui/profiles.json
```

This is the default location; `XDG_CONFIG_HOME` is respected when set.
Rule-sets live in their own file, `~/.config/kvn-tui/routing.json` (see
[Rule-sets and modes](#rule-sets-and-modes)).

Press `e` in the TUI to edit a private copy with `$VISUAL` or `$EDITOR`.
VS Code, VSCodium, Cursor, Zed and Sublime Text get `--wait` unless the
command already has `--wait` or `-w`, so kvn waits until the editor is closed.
When the command is Omarchy's `omarchy-launch-editor`, kvn runs the editor
chosen with `omarchy default editor` itself, so the copy opens at the selected
profile or subscription. An editor kvn cannot wait for is reported instead of
being opened detached; set `$VISUAL` to a command that waits.
After the editor exits, kvn validates the copy and waits for the daemon to
confirm saving it. For JSON or validation errors, a temporary screen shows the
current problems, replacing any previous error. A JSON syntax error is reported
alone; every other problem — a missing field, a wrong type, an unknown key, an
invalid value — is listed with its line number, in file order.
Press Enter to reopen the same file at the first reported line, or press `q`
or Esc to cancel immediately, without Enter. Scroll long
messages with `j`/`k`, Up/Down, or `g`/`G` (start/end). The action
keys remain visible while scrolling.

The copy starts with a `"$schema"` reference to
`$XDG_RUNTIME_DIR/kvn/profiles.schema.json`, which kvn writes each time the
editor opens. Editors with a JSON language server (VS Code, Zed, Helix, Neovim
with `jsonls`) use it to complete fields and values and to flag mistakes while
you type. The reference is not saved into `profiles.json`. Leave
`schema_version` as it is: kvn manages it, and an edit that removes or changes
it is rejected.

Independent changes made while the editor was open are merged automatically.
If the same values changed on both sides, kvn shows the conflicting fields and
asks whether to continue editing or cancel. Press Enter to reopen the file with
`<<<<<<< YOUR EDIT`, `=======`, and `>>>>>>> CURRENT` markers. Keep the desired
values, remove all markers, then save and exit the editor again. Conflicts
involving deletion or array order show the whole enclosing object or array.
If markers remain, kvn offers to continue editing or cancel. Each save checks
for further changes.

Cancellation is shown as an informational message. Cancellation or a failure
preserves the unfinished copy in the recovery directory only if its contents
differ from the original snapshot. The snapshot ends with a newline, so saving
unchanged content with Vim or Neovim does not create a recovery copy on failure.
A generated conflict document already
contains unsaved edits and is preserved even if it has not been edited further.
The message includes the recovery path when a copy was saved.
If the daemon's reply is lost, saving may have
succeeded: check the current configuration before retrying. Saves performed by
kvn use an atomic temporary file and rename, so an interrupted write cannot
replace a valid configuration with a partial file.

## File structure

The current schema version is 7. A minimal configuration is:

```json
{
  "schema_version": 7,
  "profiles": [],
  "subscriptions": [],
  "settings": {}
}
```

VLESS `security` and VMess `stream_security` take `tls`, `reality` or `none`;
`none` connects without TLS, and a profile without the field keeps using TLS.

Each profile has common fields such as `id`, `name`, `address`, `port`, and
`tags`. `share_link_params` keeps the share-link parameters kvn does not map,
so exporting the profile reproduces its link; kvn passes none of them to
sing-box except the Shadowsocks `plugin` and the Hysteria 2 port range `mport`,
and refuses to connect when one of them names a feature sing-box lacks (for
example VLESS Encryption, a Hysteria 2 `obfs` that is not `salamander` or a
valid `gecko`, or a `vcn` certificate name other than the SNI: sing-box
verifies the server certificate only against the SNI, so kvn connects with
`vcn` only when every name it lists is the SNI, or when the certificate is not
verified at all — `insecure`, or REALITY). A Hysteria 2 link's `fm` (Xray's `finalMask`, sent by Remnawave)
is read when imported: its obfuscation, port hopping and Brutal speeds become
profile fields (`obfs`, `mport`, `hop_interval_secs` / `hop_interval_max_secs`,
`up_mbps` / `down_mbps`), and only client-side QUIC tuning stays in `fm`, unused
by sing-box. An `fm` with a mask sing-box cannot reproduce is kept as it is, and
connecting fails with the mask's name. On other links `fm` stays as it is: a
`fragment` mask that splits the TLS hello (`packets: "tlshello"`) turns on
sing-box's TLS `fragment` on TLS over TCP, NaiveProxy excepted, and sing-box
picks its own split points and delays; other
fragments and settings that do not change what the server receives are kept
but unused; `header-custom`, `sudoku` and `xmc`, and a UDP mask on a link that
runs over UDP or relays UDP straight to the server (Shadowsocks, SOCKS5), make
connecting fail with the mask's name. A certificate fingerprint pin — Hysteria 2
`pinSHA256`, or `pcs` on TUIC and AnyTLS links from S-UI — cannot be checked by
sing-box, which pins public keys rather than certificate fingerprints; kvn shows
a warning after connecting with such a profile. Protocol-specific fields are stored at the same level and selected by
the `protocol` discriminator. See the
[supported protocols](../README.md#supported-protocols) for available profile
types and share-link schemes.

## Subscriptions

| Field | Description |
|-------|-------------|
| `id` | UUID; generated when absent |
| `name` | Display name |
| `url` | Subscription URL; `http://` is rejected unless `allow_insecure_http_subscriptions` is on |
| `auto_update` | Update schedule (see below); `off` when absent |
| `last_updated` | Time of the last successful update; maintained by kvn |
| `next_auto_update` | Date of the next scheduled update; maintained by kvn |
| `retry_state` | Retry bookkeeping after a failed update; maintained by kvn |
| `send_hwid` | Send device identification headers with this subscription's requests; `false` when absent |
| `hwid` | Identifier sent instead of `settings.hwid`; used only with `send_hwid` |

Valid update schedules are `off`, `every_1d`, `every_3d`, and `every_7d`. An
imported subscription starts at `every_1d`; existing subscriptions keep whatever
they store, and `i` changes the schedule at any time. The legacy values
`every_1h` and `every_12h` behave as `every_1d`, and the schema migration
rewrites them.

kvn keeps the `profiles` array in the order the Profiles list shows it:
standalone profiles first, then the profiles of each subscription in the order
of the `subscriptions` array. A file in another order is rewritten in that
order when kvn loads or saves it, so the Omarchy bar widget, which lists
`profiles` as stored, shows the same order. `Alt+j` / `Alt+k` (or `Alt+↓` /
`Alt+↑`) move the selected profile within its group, or the selected
subscription among subscriptions. An update keeps the order of the servers a
subscription already had and adds new servers at its end.

Every subscription request carries `User-Agent: kvn/<version>`. With
`send_hwid: true` it also carries `X-Hwid` (the subscription's `hwid`, or
`settings.hwid`), `X-Device-Os: Linux`, `X-Ver-Os` (the kernel version),
`X-Device-Model: Desktop`, and `X-Device-Locale` (for example `ru-RU`). Some
providers require these headers to count devices; leave `send_hwid` off
otherwise. The HWID must be 10 to 64 Latin letters, digits, `-` and `=` — the
format Remnawave accepts, which 3x-ui accepts too — and kvn refuses to save a
configuration with any other HWID; the generated `lnx-` HWID fits. A
configuration saved earlier with another HWID still loads and opens with `e`,
but kvn cannot save it until that HWID is fixed. When a panel
still rejects the HWID kvn sent, kvn reports that the server did not accept it:
check the subscription's `hwid` with the provider rather than turning
`send_hwid` off.

kvn does not change its `User-Agent`. A panel picks the subscription format by
it, and some providers let only the apps they recommend download the
subscription: Remnawave, for example, answers `403` when its response rules
block the client or no rule matches it. kvn then reports that the server
refuses this client; ask the provider to allow kvn.

A subscription download gives up when resolving the server's address or
connecting to it takes longer than 15 seconds, or when the whole answer has not
arrived within 60 seconds. The update then fails like any other: the
subscription keeps its current servers, and a scheduled update is retried later.

When a subscription has expired, been disabled or run out of traffic, some
panels still answer with a list that holds only a notice dressed as a server.
kvn does not import those entries. If the answer has nothing else, the update
fails with the notice's text and the subscription keeps its current servers;
otherwise the notice is skipped and logged. kvn recognises:

- Remnawave's notice: a VLESS link to `0.0.0.0:1` with the all-zero UUID;
- 3x-ui's info node: a SOCKS5 `socks://127.0.0.1:1080` without credentials
  that is the only line of the answer, and only when the `subscription-userinfo` header says the subscription has
  expired (`expire` in the past and not `0`) or used up its traffic
  (`upload + download` reaching a non-zero `total`). A local SOCKS proxy in any
  other answer is imported as usual.

## Settings

| Field | Default | Description |
|-------|---------|-------------|
| `tun_interface` | `kvn0` | Name of the sing-box TUN interface; must start with `kvn` |
| `dns` | Cloudflare DoH | DNS presets, the active preset, and fake-IP state |
| `geo_routing` | `mode` `global`; `auto_update` every 7 days on a new installation, `off` when the field or section is absent (`I` changes it at any time) | Selected routing mode and rule-set updates |
| `auto_connect` | `false` | Connect to `last_connected_profile` at startup |
| `kill_switch` | `false` | Persisted kill-switch state |
| `last_connected_profile` | `null` | Last connected profile; maintained by the application |
| `theme` | `omarchy` on a first launch under Omarchy, `tokyo-night` otherwise | Bundled palette slug or `omarchy` |
| `icons` | `nerd` on Omarchy, `unicode` elsewhere | `nerd` for Nerd Font glyphs, or `unicode` for terminals without a Nerd Font |
| `hwid` | generated on first launch (`lnx-` + UUID) | Installation identifier sent only by subscriptions with `send_hwid` |
| `allow_insecure_http_subscriptions` | `false` (`true` after migration from v4) | Temporarily allow deprecated HTTP subscription URLs for backward compatibility |
| `connectivity_probe.enabled` | `true` | Enable the HTTP(S) endpoint used only by manual `t` / `T` latency tests |
| `connectivity_probe.url` | `https://connectivitycheck.gstatic.com/generate_204` | Probe endpoint; required and validated only when the probe is enabled |
| `logs.level` | `info` | `trace`, `debug`, `info`, `warn`, or `error` |
| `logs.line_retention.app` | `1000` | Physical lines retained in `app.log` |
| `logs.line_retention.singbox` | `100000` | Physical lines retained in `sing-box.log` |

The latency value is an end-to-end probe rather than a raw network RTT. For an
HTTPS endpoint it includes the TLS handshake, request, and time to the first
response byte through the tested VPN profile. The probe runs only when `t` or
`T` is pressed and does not affect normal VPN connection health.

```json
"connectivity_probe": {
  "enabled": true,
  "url": "https://connectivitycheck.gstatic.com/generate_204"
}
```

A field missing from `connectivity_probe` takes its default, so
`"connectivity_probe": {"enabled": false}` is enough to disable active probing.
While disabled, `url` may retain any value; it is ignored until probing is
enabled again.

New configurations reject HTTP subscription URLs by default. Configurations
migrated from schema v4 enable them to preserve compatibility with self-hosted
servers and emit a warning on every HTTP update. Set
`allow_insecure_http_subscriptions` to `false` after enabling HTTPS to reject
HTTP before any network request is made.

```json
"logs": {
  "level": "info",
  "line_retention": {
    "app": 1000,
    "singbox": 100000
  }
}
```

`RUST_LOG`, when set to a valid filter, overrides `logs.level` for application
logging. sing-box still uses `logs.level`.

## DNS

DNS settings are organized in presets. A preset is a set of DNS servers, the
rules that send some domains to a particular server, and the `final_server`
that answers everything else. One preset is active at a time; choose it in
**Settings › DNS** (`Space d`), next to the fake-IP toggle, or set
`current_preset` directly.

kvn ships four built-in presets: `cloudflare_doh` (the default), `google_dot`,
`quad9_doh`, and `system_local`. Write your own under `custom_presets`; they
appear in Settings › DNS by name after the built-in ones. The following
snippet belongs inside `settings`:

```json
{
  "dns": {
    "current_preset": "home",
    "custom_presets": [
      {
        "name": "home",
        "servers": [
          { "type": "local", "tag": "local" },
          { "type": "https", "tag": "adguard", "server": "94.140.14.14" },
          { "type": "udp", "tag": "router", "server": "192.168.1.1" }
        ],
        "rules": [{ "domain_suffix": ["lan"], "server": "router" }],
        "final_server": "adguard"
      }
    ],
    "fakeip_enabled": false
  }
}
```

The built-in presets look like this in the same format, which makes them a
convenient starting point for your own:

```json
[
  {
    "name": "cloudflare_doh",
    "servers": [
      { "type": "local", "tag": "local" },
      { "type": "https", "tag": "remote", "server": "1.1.1.1", "path": "/dns-query" }
    ],
    "final_server": "remote"
  },
  {
    "name": "google_dot",
    "servers": [
      { "type": "local", "tag": "local" },
      { "type": "tls", "tag": "remote", "server": "8.8.8.8", "server_port": 853 }
    ],
    "final_server": "remote"
  },
  {
    "name": "quad9_doh",
    "servers": [
      { "type": "local", "tag": "local" },
      { "type": "https", "tag": "remote", "server": "9.9.9.9", "path": "/dns-query" }
    ],
    "final_server": "remote"
  },
  {
    "name": "system_local",
    "servers": [{ "type": "local", "tag": "local" }],
    "final_server": "local"
  }
]
```

Supported server types are `local`, `udp`, `tcp`, `tls`, `https`, and `quic`.
Every type except `local` takes a `server` address and an optional
`server_port` (the protocol's standard port when absent); `https` also takes an
optional `path`, `/dns-query` by default.
Within a preset, server tags must be non-empty and unique, and `final_server`
and every rule's `server` must name one of that preset's servers. A server may
be addressed by hostname (`"server": "dns.google"`) only when the preset also
has a `local` server, which resolves it; servers given by IP address need no
`local` server. Preset names must be unique and cannot reuse a built-in name.

Fake-IP applies to whichever preset is active. kvn answers IPv6 address
lookups with no addresses, because the tunnel carries IPv4 only (see
[what kvn protects](privacy.md#ipv4-only)).

Fake-IP answers address lookups with addresses from `fakeip_ranges`
(`198.18.0.0/15` and `fc00::/18` unless set). A rule can send particular
domains to it with `"server": "fakeip"`, whether or not the Fake-IP toggle is
on; the tag `fakeip` is reserved for this and cannot name a preset's own
server. If no active rule already targets `fakeip`, the toggle adds a
catch-all for remaining `A` and `AAAA` queries. Both ranges must be IP
prefixes; putting an IPv6 prefix in `inet4_range`, or the reverse, is not
supported:

```json
{ "fakeip_ranges": { "inet4_range": "198.18.0.0/15", "inet6_range": "fc00::/18" } }
```

Configurations from kvn versions before presets are converted when kvn first
loads them, after a copy is saved to `~/.config/kvn-tui/recovery/`: DNS
servers that match a built-in preset select it, and anything else becomes a
custom preset named `custom`, which is selected. In a current configuration
the old `servers`, `rules` and `final_server` fields are reported as errors
rather than ignored; move them into a custom preset.

DNS rules can match `domain`, `domain_suffix`, `domain_keyword`,
`domain_regex`, or `rule_set`; each rule may also set `disable_cache`. A rule
matches a name when any of its domain conditions does: any entry of the
`domain*` fields or any domain rule-set such as `geosite-*`. A rule without an
IP rule-set applies to every query type.
`domain_regex` uses Go RE2 syntax and is checked by sing-box when connecting.

A `rule_set` entry names one of the rule-sets kvn downloads, such as
`geosite-category-ru` or `geoip-ru` for a region. A rule is skipped while any
of its rule-sets is not in use, because the routing mode does not use that
region or the file has not been downloaded yet.

A rule with an IP rule-set (`geoip-*`) works in two steps, for `A` and
`AAAA` queries only. kvn first asks the preset's `final_server`; only when
that answer has an address in the rule-set does it send the query to the
rule's `server`, and the program gets that server's answer. The rule's domain
conditions decide whether the first step runs, and without them it runs for
every name; any one IP rule-set is enough for the second step. An error,
an empty answer or an address outside the rule-set moves the query on to the
next rule and then to `final_server`; an error from the rule's own server is
returned as it is. `disable_cache` applies to both steps. Names that match
are resolved twice, and the two servers may answer differently: a name that
your `final_server` resolves outside the region is not caught by the rule.
[What kvn protects](privacy.md#dns) shows what each server sees.

With fake-IP on and no active rule already targeting `fakeip`, kvn adds a rule
that sends every remaining `A` and `AAAA` query to its fake-IP server, after
the active preset's rules.

In Global and Bypass modes, DNS queries to public servers go through the
tunnel; [what kvn protects](privacy.md#dns) summarizes where every query goes.
Servers on a local address, such as a router at `192.168.1.1` or a resolver on
`127.0.0.1`, are always queried directly, and so is every server in Only mode.
Names on a rule-set that is routed directly — the region's sites in Bypass, a
service set to `direct` — are asked from a direct copy of `final_server`, so
that a content network answers for your real location; a rule of the active
preset for the same rule-set takes precedence.
Profiles whose protocol cannot carry UDP (HTTP, SSH, ShadowTLS, and SOCKS
4/4a) query a public `udp` or `quic` server directly in Global and Bypass
modes, so its queries bypass the VPN; kvn shows a warning after connecting.
Use an `https`, `tls` or `tcp` server with these profiles to keep DNS in the
tunnel. kvn resolves the VPN
server's own hostname through a direct copy of the active preset's
`final_server`, so connecting never depends on the tunnel being up.

To resolve a region's sites through a DNS service inside that region instead,
give the preset a server for it and a rule for the region's list. The
service's resolvers are in the region, so a content network answers for the
region whichever way the query travels: through the tunnel in Bypass, where
your provider does not see it, and directly in Only. The rule replaces the
direct lookup described above, and is skipped in Global mode, where the list
is not in use. For Russia with Yandex DNS:

```json
{
  "name": "ru-yandex",
  "servers": [
    { "type": "https", "tag": "remote", "server": "1.1.1.1", "path": "/dns-query" },
    { "type": "https", "tag": "yandex", "server": "77.88.8.8", "path": "/dns-query" }
  ],
  "rules": [{ "rule_set": ["geosite-category-ru"], "server": "yandex" }],
  "final_server": "remote"
}
```

The regional service sees the names of the region's sites you visit, together
with the VPN server's address in Bypass and your own in Only.

## Geo routing

Open **Settings › Routing** (`Space r`) and pick the **Mode**: the row offers
every mode of [`routing.json`](#rule-sets-and-modes), in the file's order. The
region you choose in the first-run tour (`ru`, `cn`, `ir` or `global`) adds
its Bypass and Only modes to the file next to Global; `global` adds none.
Bypass and Only need the country's rule-sets: without them a connection runs
Bypass as Global and Only as Direct, downloads the files and reconnects in the
chosen mode (see [Routing modes](privacy.md#routing-modes)).

Below the Mode row the page lists the rules of the mode the row shows, as
`routing.json` has them — the key (`rule_set`, `domain` or `ip`), its value and
the route, top first, since the first rule that matches decides — and last the
mode's `final`, where everything else goes. The list follows the row before you
apply it. Move through the rules with `j`/`k`, `gg`/`G` or the mouse wheel; a
list longer than the window scrolls, with a scrollbar on the right. `h`/`l`
change the mode only on the Mode row, and `e` opens `routing.json` at the
selected rule (or at the mode on the Mode row).

`h` / `l` (or `←` / `→`) on a rule change its route, shown between `‹ ›` like
the Mode row, in the cycle `proxy` → `direct` → `block` → `disabled`; a
disabled rule is dimmed. The route of `final` has no `‹ ›`: change it in the
editor. `Alt+j` / `Alt+k` (or `Alt+↓` / `Alt+↑`) move the selected rule down
or up within its mode; on the Mode row and on the first rule upwards they do
nothing, and on `final`, which always stays last, or on the last rule
downwards they show an error. `d` deletes the selected rule from its mode and
selects the next row; the rule-set stays declared in `rule_sets` for other
modes. `final` cannot be deleted, so `d` on it or on the Mode row shows an
error; a mode may keep `final` alone. A deleted rule comes back only by
discarding the draft or in the editor.

Route changes, moves and deletions are a draft, marked `*` next to the rules
heading, kept for each mode until `Enter` writes them all to `routing.json`
together with the Mode row; `q`, `Esc` and `Backspace` discard it. The saved
file applies at once, and a changed active mode reconnects. When the rules of
an edited mode were changed in the file in the meantime, kvn opens the editor
with the `YOUR EDIT` / `CURRENT` conflict markers, as when saving from the
editor (see [Rule-sets and modes](#rule-sets-and-modes)); the draft stays until
the file is saved. `e` with a draft opens the file with it. Rules cannot be
changed while `routing.json` is invalid, and a reload that finds it invalid
discards the draft.

The following snippet belongs inside `settings`:

```json
{
  "geo_routing": {
    "mode": "bypass_ru",
    "auto_update": "every_1d"
  }
}
```

`mode` is the `id` of a mode in `routing.json`; one the file does not declare
runs as `global`. Geo update schedules are `off`, `every_1d`, `every_3d`, and
`every_7d`. The legacy value `every_12h` behaves as `every_1d`, and the schema
migration rewrites it.

Each country region uses two rule-sets: a `geosite-*` list of domains and a
`geoip-*` list of IP networks.

A downloaded file is kept only when it is a sing-box rule-set: a captive
portal or error page is rejected and the previous file stays. A damaged
rule-set file counts as missing and is downloaded again on the next update,
even when the server reports it unchanged.

### Rule-sets and modes

Rule-sets and routing modes are declared in `~/.config/kvn-tui/routing.json`,
next to — not inside — `profiles.json`. kvn creates the file the first time it
starts, with the Global mode plus the modes of the regions you have used and
only the rule-sets those modes use; from then on the file is yours, and kvn
adds to it only the modes and rule-sets of the region you choose in the
first-run tour, when they are missing:

```json
{
  "schema_version": 1,
  "rule_sets": {
    "geoip-ru": {
      "kind": "geoip",
      "url": "https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-ru.srs"
    },
    "geosite-category-ru": {
      "kind": "geosite",
      "url": "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-category-ru.srs"
    }
  },
  "modes": [
    { "id": "global", "name": "Global", "final": "proxy", "rules": [] },
    {
      "id": "bypass_ru",
      "name": "Bypass RU",
      "final": "proxy",
      "rules": [
        { "ip": "private", "route": "direct" },
        { "domain": "corp.example", "route": "direct" },
        { "rule_set": "geosite-category-ru", "route": "direct", "required": true },
        { "rule_set": "geoip-ru", "route": "direct", "required": true }
      ]
    }
  ]
}
```

`rule_sets`:

- The key is the rule-set's tag: the name rules and DNS rules use in
  `rule_set`, and the name of the downloaded file, `<tag>.srs` in the geo
  directory. Tags use lowercase letters, digits and inner hyphens.
- `kind` is `geoip` for a list of IP networks or `geosite` for a list of
  domains. kvn does not read the file to find out, so declare it correctly; the
  built-in rule-sets keep their `kind`.
- `url` is an `https://` address of a binary sing-box rule-set (`.srs`). Two
  rule-sets cannot share one address. Changing it discards the downloaded file:
  the next update downloads it from the new address.

`modes`:

- `id` is what the settings select (`global`, `bypass_<region>`,
  `only_<region>`); lowercase letters, digits and inner hyphens or
  underscores. `name` is shown for the mode.
- `final` is where traffic no rule matches goes: `proxy` (through the tunnel) or
  `direct`.
- `rules` apply in the order of the file, and the first rule that matches a
  connection decides its `route`: `proxy` (through the tunnel), `direct`,
  `block` (the connection is refused at once; its name is still looked up
  like a `proxy` one) or `disabled` (the rule keeps its place and matches
  nothing; a rule-set only disabled rules use is not downloaded, and DNS
  rules that name it are left out, as for a rule-set the mode does not use). `final`
  takes only `proxy` or `direct`. Each rule names one condition:
  - `rule_set` — the tag of a rule-set declared in `rule_sets`; every rule-set
    a mode uses is downloaded and kept up to date;
  - `domain` — a domain and all its subdomains: `example.com` matches
    `example.com` and `www.example.com`, not `badexample.com`;
  - `ip` — an IP address, a CIDR network such as `10.0.0.0/8`, or `private`
    for private networks (LAN, loopback). The Bypass and Only modes kvn writes
    start with `{ "ip": "private", "route": "direct" }`; move it or remove it
    to route a local network through the tunnel. `private` matches a connection
    made to a private address, not a name that resolves to one.
- Only `rule_set` rules can be `required`: the mode cannot run without such a
  rule-set. While the file of one is missing, the mode runs without that
  rule-set — what it would match goes to the mode's `final` outbound, so
  Bypass sends the region through the tunnel and Only keeps it out — and kvn
  reconnects once the file is downloaded. Your other rules, and the required
  rule-sets that are already on disk, keep working in the meantime. A rule-set
  that is not required is simply left out until its file is there.
- The `global` mode is required, its `final` is `proxy` and none of its rules
  is `required`: a selected mode that is missing from the file runs as Global,
  which must never wait for a download. A mode missing from the file cannot be
  selected.
- The Mode setting lists the modes in the order of the file. Every rule-set a
  mode in the file uses is downloaded, so switching between them needs no
  download.

To check an `ip` rule or an IP rule-set against a name, kvn looks the name up
first, and every rule below uses that address — the VPN server too receives the
address, not the name. With fake-IP on and DNS that does not use the tunnel
(Only mode, a local server), a name sent through the tunnel by a rule below an
IP rule is therefore looked up past the VPN. kvn warns about it when it
connects and in `kvn doctor`; move such rules above the first IP rule.

A rule-set that is not required and has no file yet is named in the status
line when a connection comes up (`Not routed until downloaded: …`); kvn
downloads it and reconnects once it is there. After a failed download it waits
for the next retry instead of trying again on every connection.

Steam and Telegram overrides set in an earlier release become rules: when the
daemon starts, every mode in the file gets `geosite-steam` / `geoip-steam`
(and `geosite-telegram` / `geoip-telegram`) with the override's route ahead of
its own rules, the rule-sets keep their sources and downloaded files, and the
old `service_routes` setting leaves `profiles.json`. While `routing.json` fails
its checks the move waits for the next start, and the overrides stay where they
are.

kvn reads `routing.json` when the daemon starts and when the configuration is
reloaded. A file that fails these checks is reported in the status line and
left untouched, and kvn does not connect until the file is fixed: at startup it
does not auto-connect, and `Enter`, `r`, `kvn connect`, `kvn reconnect` and
`kvn toggle` fail with `Connection blocked: routing.json is invalid`. A file
that breaks while the VPN is connected leaves that connection running; only new
connections and reconnects are refused.

Press `e` in **Settings › Routing** (`Space r e`) to edit `routing.json` the
same way as `profiles.json`: kvn opens a copy of the file in `$VISUAL` or
`$EDITOR` at the mode shown in the Mode row, or at the rule selected below it,
checks it when the editor exits, lists every problem with its line, and offers
to reopen it. The copy carries a
`"$schema"` reference to `$XDG_RUNTIME_DIR/kvn/routing.schema.json`, which is
not saved into the file. Changes made to `routing.json` while the editor was
open are merged by mode `id` and rule-set tag; the rules of one mode changed on
both sides are a conflict, reopened with the same `YOUR EDIT` / `CURRENT`
markers. The saved file applies at once: new rule-sets download, and a changed
active mode reconnects. kvn writes the file in its own format, so the order of
keys and the layout of a hand-written file are not kept; closing the editor
without a change leaves the file as it is.

When `routing.json` fails its checks, `e` opens the file itself, at the first
problem. Fixing it lifts the block on connecting; kvn saves it only if nobody
changed the file in the meantime, and keeps the broken version in
`~/.config/kvn-tui/recovery/`.

To start over, `kvn config reset --routing ru` (any list of `ru`, `cn`, `ir`,
`global`) rebuilds `routing.json` the way a fresh install creates it: Global,
the modes of the listed countries, and only the rule-sets those modes use. Custom modes,
rules and rule-sets go, and so do the modes of other countries and Steam and
Telegram overrides not moved into the file yet; a broken file is replaced the
same way. A selected mode the new file lacks switches to Global. The previous
file is archived in `~/.config/kvn-tui/recovery/` and comes back with
`kvn config recover --routing <file>`; see [CLI](cli.md#config).

## Themes and logging

Open **Settings › Interface** (`Space i`) and change **Theme** to choose one of
the 22 palettes bundled from [`themes/`](../themes/).
See the [theme gallery](themes.md) for a full UI preview of every bundled palette.
The special `omarchy` value follows the active Omarchy theme. Fresh non-Omarchy
installations use `tokyo-night`.

The **Icons** row on the same screen switches between Nerd Font glyphs and
plain Unicode symbols. A new installation, or a config without the `icons`
field or the whole `settings` section, uses `nerd` on Omarchy, which ships a
Nerd Font, and `unicode` elsewhere.

`logs.level` controls both application and generated sing-box logging. Accepted
values are `trace`, `debug`, `info`, `warn`, and `error`. A valid `RUST_LOG`
filter takes precedence for application logging only; it does not change
sing-box logging.

`logs.line_retention.app` and `logs.line_retention.singbox` are line limits for
the two on-disk log files. Both values must be at least `1000`. Both files are
checked when the daemon starts; `sing-box.log` is additionally checked at a
safe reconnect point no more than once every 24 hours. Active sing-box logging
is never truncated in place.

## Validation and schema versions

Configuration is parsed and validated before use, and every problem is
reported at once. Validation checks:

- unique profile and subscription IDs, required profile values, and that each
  profile's `subscription_id` names an existing subscription;
- the HWID of every subscription with `send_hwid`;
- DNS presets, server tags and references, fake-IP range syntax, and leftover
  pre-preset DNS fields;
- the TUN interface, theme slug, log level, and minimum log limits;
- `connectivity_probe.url` while the probe is enabled: an `http` or `https`
  URL with a host, without credentials or a fragment, at most 2048 bytes.

The TUN interface must contain only ASCII letters, digits, `-`, or `_`, and be
at most 15 characters.

A configuration with an older `schema_version` is migrated automatically when
kvn loads it, after a copy is saved to `~/.config/kvn-tui/recovery/`. A
configuration with a newer `schema_version` is refused: older `kvn` releases
cannot read it.

## Runtime files

| Resource | Location |
|----------|----------|
| Profiles and settings | `~/.config/kvn-tui/profiles.json` |
| Rule-sets | `~/.config/kvn-tui/routing.json` |
| Geo rule-sets | `~/.config/kvn-tui/geo/` |
| Application log | `~/.config/kvn-tui/logs/app.log` |
| sing-box log | `~/.config/kvn-tui/logs/sing-box.log` |
| Recovery copies | `~/.config/kvn-tui/recovery/profiles.json.<kind>-*`, three per kind: `before-migration` (schema migration), `invalid` (archived by `kvn config reset`/`recover`), `conflict` and `conflict-invalid` (editor copies kvn preserved); `routing.json.invalid-*` (a broken `routing.json` replaced from the editor, or the file archived by `kvn config reset`/`recover`) and `routing.json.conflict-invalid-*` (unfinished `routing.json` edits), three each |
| First-run tour progress | `$XDG_STATE_HOME/kvn/onboarding.json` |
| Support prompt schedule | `$XDG_STATE_HOME/kvn/support-prompt.json` |
| sing-box cache (fake-IP map) | `$XDG_STATE_HOME/kvn/singbox-cache.db` |
| IPC socket | `$XDG_RUNTIME_DIR/kvn-tui.sock` |
| Generated sing-box config | `$XDG_RUNTIME_DIR/kvn-tui/singbox.json` |
| Latency-test configs | `$XDG_RUNTIME_DIR/kvn-tui/test-<uuid>.json` |
| Private editor copy | `$XDG_RUNTIME_DIR/kvn-tui/profiles-edit-<pid>.json` |
| Private routing editor copy | `$XDG_RUNTIME_DIR/kvn/routing-edit-<pid>.json` |
| Daemon instance lock | `$XDG_RUNTIME_DIR/kvn/daemon.lock` |
| Waybar and recovery state | `$XDG_RUNTIME_DIR/kvn/state.json` |
| Editor JSON Schema | `$XDG_RUNTIME_DIR/kvn/profiles.schema.json` |
| Routing editor JSON Schema | `$XDG_RUNTIME_DIR/kvn/routing.schema.json` |

The application-owned runtime directory is created with mode `0700`; generated
sing-box configs and the IPC socket use mode `0600`. No secret-bearing config is
written directly into the shared `/tmp` namespace. `XDG_RUNTIME_DIR` is required;
kvn fails with a clear error outside a desktop user session instead of
falling back to a shared or persistent location.

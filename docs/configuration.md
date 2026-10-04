# Configuration

`kvn` stores profiles, subscriptions, and application settings in:

```text
~/.config/kvn-tui/profiles.json
```

This is the default location; `XDG_CONFIG_HOME` is respected when set.

Press `e` in the TUI to edit a private copy with `$VISUAL` or `$EDITOR`.
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

The current schema version is 5. A minimal configuration is:

```json
{
  "schema_version": 5,
  "profiles": [],
  "subscriptions": [],
  "settings": {}
}
```

Each profile has common fields such as `id`, `name`, `address`, `port`, and
`tags`. Protocol-specific fields are stored at the same level and selected by
the `protocol` discriminator. See the
[supported protocols](../README.md#supported-protocols) for available profile
types and share-link schemes.

A subscription contains `id`, `name`, `url`, `auto_update`, and an optional
`last_updated` timestamp. Valid update schedules are `off`, `every_1h`,
`every_12h`, `every_1d`, and `every_7d`. An imported subscription starts at
`every_1d`; existing subscriptions keep whatever they store, and `i` changes the
schedule at any time.

## Settings

| Field | Default | Description |
|-------|---------|-------------|
| `default_profile` | `null` | UUID of the selected default profile |
| `tun_interface` | `kvn0` | Name of the sing-box TUN interface; must start with `kvn` |
| `dns` | Cloudflare DoH | DNS servers, rules, strategy, and fake-IP state |
| `geo_routing` | no region; `auto_update` every 7 days on a new installation, `off` when the field or section is absent (`I` changes it at any time) | Country modes, rule-set updates, and service overrides |
| `auto_connect` | `false` | Connect to `last_connected_profile` at startup |
| `kill_switch` | `false` | Persisted kill-switch state |
| `last_connected_profile` | `null` | Last connected profile; maintained by the application |
| `theme` | `tokyo-night` | Bundled palette slug or `omarchy` |
| `icons` | `nerd` | `nerd` for Nerd Font glyphs, or `unicode` for terminals without a Nerd Font |
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

Set `enabled` to `false` to disable active probing. While disabled, `url` may
be absent or retain any value; it is ignored until probing is enabled again.

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

`dns_strategy` is a legacy compatibility field mirrored from `dns.strategy`.
Edit `dns.strategy` instead. `RUST_LOG`, when set, overrides `logs.level`.

## DNS

Press `D` for the built-in Cloudflare DoH, Google DoT, Quad9 DoH, and system
resolver presets, strategy selection, and fake-IP toggle. Custom servers and
rules can be edited in JSON.

Supported server types are `local`, `udp`, `tcp`, `tls`, `https`, `quic`, and
`fake_ip`. Supported strategies are `prefer_ipv4`, `prefer_ipv6`, `ipv4_only`,
and `ipv6_only`.

The following snippet belongs inside `settings`:

```json
{
  "dns": {
    "servers": [
      { "type": "local", "tag": "local" },
      {
        "type": "https",
        "tag": "remote",
        "server": "1.1.1.1",
        "path": "/dns-query"
      }
    ],
    "rules": [
      {
        "domain_suffix": ["internal.example"],
        "server": "local"
      }
    ],
    "final_server": "remote",
    "strategy": "prefer_ipv4",
    "fakeip_enabled": false
  }
}
```

Server tags must be non-empty and unique. `final_server` and every rule's
`server` must reference an existing tag. Enabling `fakeip_enabled` requires a
`fake_ip` server in the same list.

DNS rules can match `domain`, `domain_suffix`, `domain_keyword`,
`domain_regex`, or `rule_set`; each rule may also set `disable_cache`.

## Geo and service routing

Press `o` to select `ru`, `cn`, `ir`, or `global`. Country regions offer
Global, Bypass, and Only modes; `global` skips country rule-set downloads and
offers Global mode only. The last mode selected for each country is retained.

The following snippet belongs inside `settings`:

```json
{
  "geo_routing": {
    "current_region": "ru",
    "selected_region_modes": {
      "ru": "bypass_ru"
    },
    "auto_update": "every_1d",
    "service_routes": {
      "steam": "direct",
      "telegram": "proxy"
    }
  }
}
```

Routing modes are serialized as `global`, `bypass_<region>`, or
`only_<region>`. Geo update schedules are `off`, `every_12h`, `every_1d`,
`every_3d`, and `every_7d`.

Press `S` to apply the predefined service overrides. `proxy` always uses the
tunnel; `direct` sends matching traffic through the real network, including
past the kill switch. An absent service entry means Disabled and follows the
country routing mode.

Service rule-sets are fetched through the active tunnel. Missing files do not
block a connection: the override remains inactive until the files are
downloaded and the connection is restarted.

## Themes and logging

Press `C` to choose one of the 22 palettes bundled from [`themes/`](../themes/).
See the [theme gallery](themes.md) for a full UI preview of every bundled palette.
The special `omarchy` value follows the active Omarchy theme. Fresh non-Omarchy
installations use `tokyo-night`.

`logs.level` controls both application and generated sing-box logging. Accepted
values are `trace`, `debug`, `info`, `warn`, and `error`; `RUST_LOG` takes
precedence when present.

`logs.line_retention.app` and `logs.line_retention.singbox` are line limits for
the two on-disk log files. Both values must be at least `1000`. Both files are
checked when the daemon starts; `sing-box.log` is additionally checked at a
safe reconnect point no more than once every 24 hours. Active sing-box logging
is never truncated in place.

## Validation and schema versions

Configuration is parsed and validated before use. Validation checks profile
references and required values, DNS tags and server references, the TUN
interface, theme slug, log level, and minimum log limits.

The TUN interface must contain only ASCII letters, digits, `-`, or `_`, and be
at most 15 characters. `default_profile`, when set, must reference an existing
profile.

Older schema versions must be migrated before use. Newer schema versions are
not supported by older `kvn` releases.

## Runtime files

| Resource | Location |
|----------|----------|
| Profiles and settings | `~/.config/kvn-tui/profiles.json` |
| Geo and service rule-sets | `~/.config/kvn-tui/geo/` |
| Application log | `~/.config/kvn-tui/logs/app.log` |
| sing-box log | `~/.config/kvn-tui/logs/sing-box.log` |
| Waybar and recovery state | `~/.config/kvn-tui/state.json` |
| Migration backups | `~/.config/kvn-tui/recovery/profiles.json.before-migration-*` |
| Applied migration markers | `$XDG_STATE_HOME/kvn/migrations/` |
| First-run tour progress | `$XDG_STATE_HOME/kvn/onboarding.json` |
| Support prompt schedule | `$XDG_STATE_HOME/kvn/support-prompt.json` |
| IPC socket | `$XDG_RUNTIME_DIR/kvn-tui.sock` |
| Generated sing-box config | `$XDG_RUNTIME_DIR/kvn-tui/singbox.json` |
| Migration lock | `$XDG_RUNTIME_DIR/kvn/migrate.lock` |
| Editor JSON Schema | `$XDG_RUNTIME_DIR/kvn/profiles.schema.json` |

The application-owned runtime directory is created with mode `0700`; generated
sing-box configs and the IPC socket use mode `0600`. No secret-bearing config is
written directly into the shared `/tmp` namespace. `XDG_RUNTIME_DIR` is required;
kvn fails with a clear error outside a desktop user session instead of
falling back to a shared or persistent location.

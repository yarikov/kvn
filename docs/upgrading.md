# Upgrading kvn

Upgrade `kvn` through your usual package manager:

```bash
yay -Syu
```

A running daemon keeps the previous version until it restarts. Opening `kvn`
restarts it and reconnects the current profile; without the TUI, restart it
yourself:

```bash
systemctl --user restart kvn-tui.service
```

Until then, `kvn connect`, `kvn toggle` and the other one-shot commands refuse
to talk to the outdated daemon. Run `kvn doctor` if an upgrade needs
attention.

## v0.36.0

Rule-sets and routing modes move to a new file,
`~/.config/kvn-tui/routing.json`, which kvn creates the first time it starts.
See [Rule-sets and modes](configuration.md#rule-sets-and-modes).

The region is no longer a setting. `profiles.json` keeps the selected mode as
`geo_routing.mode` — the mode you used in your current region — and
`routing.json` gets the modes of every region you have used. **Settings ›
Routing** (`Space r`) has a Mode row, which lists every mode of the file, and
below it the rules of the mode it shows.

Steam and Telegram overrides become rules of every mode in that file, with the
same route and the same rule-set sources, so the files already downloaded are
kept and both services are routed as before. The `service_routes` setting
leaves `profiles.json`; an older kvn started afterwards routes Steam and
Telegram by the mode, like any other traffic. If `routing.json` cannot be read,
kvn reports it and moves the overrides the next time the daemon starts after
the file is fixed (`systemctl --user restart kvn-tui.service`).

To keep only the modes of one country — for example, Global and the RU modes
after `routing.json` received the modes of every region you have used — reset
the file with the daemon stopped:

```bash
systemctl --user stop kvn-tui.service \
  && kvn config reset --routing ru \
  && systemctl --user start kvn-tui.service
```

The reset rebuilds `routing.json` as a fresh install creates it, so it
**removes the Steam and Telegram rules and rule-sets**, together with your own
modes, rules and rule-sets and the modes of other countries. Overrides that
have not moved into `routing.json` yet are dropped from `profiles.json` too, so
they do not come back. A selected mode of another country switches to Global.
The previous file is archived in `~/.config/kvn-tui/recovery/`, and
`kvn config recover --routing <file>` brings it back. To route Steam or Telegram
again afterwards, add their rule-sets and rules to `routing.json` (`Space r e`).
`kvn config reset` and `kvn config recover` now take `--profiles` and
`--routing`; see [CLI](cli.md#config).

`/usr/bin/kvn` is now the program itself and `/usr/bin/kvn-tui` a symlink to
it, so scripts that call either name keep working. The package built from
`pkg/arch` is now called `kvn`: on its first install pacman offers to remove
`kvn-tui`, which keeps your configuration and the daemon autostart. Log
targets are now named `kvn::…`, so a `RUST_LOG` filter written for `kvn_tui`
no longer matches; rename it to `kvn`.

## v0.35.0

The AUR package is renamed from `kvn-tui-bin` to `kvn-bin`. `kvn-tui-bin` still
receives updates for a while, but will be retired; switch now:

```bash
yay -S kvn-bin
```

Confirm the removal of `kvn-tui-bin` when asked. The new package installs the
same files, so profiles, settings and the daemon autostart are kept.

The shortcuts that have shown a deprecation notice since v0.30.0–v0.32.0 are
removed:

| Removed | Use instead |
|---------|-------------|
| `h` / `l`, `←` / `→` to switch panes | `Ctrl+h` / `Ctrl+←`, `Ctrl+l` / `Ctrl+→` |
| `m`, `o`, `S` | `Space r` (Settings › Routing) |
| `D` | `Space d` (Settings › DNS) |
| `C` | `Space i` (Settings › Interface) |
| `a` | `Shift+A` |

The configuration moves to schema v7. kvn converts it automatically the first
time it loads it and keeps the previous file in `~/.config/kvn-tui/recovery/`:

- The DNS strategy setting is removed from `dns` and from Settings › DNS. kvn
  always answers IPv6 address lookups with no addresses, as `ipv4_only` did,
  because the tunnel carries IPv4 only.

An older kvn cannot read a v7 configuration.

The polkit rule also allows `org.freedesktop.resolve1.revert`, which sing-box
uses to reset the tunnel's DNS settings when it stops. From this release on,
the package upgrade updates an installed polkit rule and kill switch itself;
there is nothing to rerun.

The status file for bars moves to `$XDG_RUNTIME_DIR/kvn/state.json`; the
daemon removes the old `~/.config/kvn-tui/state.json` when it starts. Scripts
that read the file directly need the new path; `kvn --waybar-status` follows it
on its own. `kvn status` no longer writes `last error: <message>` to stderr.

## v0.34.0

The configuration moves to schema v6. kvn converts it automatically the first
time it loads it and keeps the previous file in `~/.config/kvn-tui/recovery/`:

- DNS servers become **presets**. Servers that match a built-in preset select
  it; anything else becomes a custom preset named `custom`. The former
  top-level `dns.servers`, `dns.rules` and `dns.final_server` fields are
  reported as errors from then on. See [DNS](configuration.md#dns).
- The DNS strategies `prefer_ipv6` and `ipv6_only` become `prefer_ipv4` and
  `ipv4_only`, because the tunnel carries IPv4 only.
- VLESS profiles keep their transport in a `transport` block, like VMess and
  Trojan. The former `transport_type` and `transport_service_name` fields are
  reported as errors from then on.
- WebSocket early data written in a profile's path (`/ws?ed=2048`) moves out
  of the path, so sing-box sends it as early data, as it does for newly
  imported links.

An older kvn cannot read a v6 configuration. The sing-box cache now lives in
`~/.local/state/kvn/singbox-cache.db`; a `cache.db` that earlier versions left
in the daemon's working directory (your home directory under the systemd unit)
is no longer used and can be deleted.

If the kill switch is installed, refresh it after upgrading:

```bash
sudo kvn setup --killswitch
```

The new rules stop direct connections that were open before the kill switch
came on, and keep kvn's IPv6 fake-IP addresses (`fc00::/18`) from leaving
outside the tunnel. Until you run the command, the installed rules keep the
previous exceptions and `kvn doctor` reports the kill switch as outdated. An
active kill switch is updated in place, without a moment of lost protection.

## v0.29.0

- The command is now `kvn`. The legacy `kvn-tui` name still works but prints a
  deprecation warning for every command except plain `kvn-tui`,
  `kvn-tui --daemon` and `kvn-tui --waybar-status`; update scripts and
  keybindings.
- `settings.tun_interface` must start with `kvn`. A configuration with any
  other name, including the `tun` prefix that v0.28.0 still accepted, is
  rejected: set it to `kvn0` or another `kvn` name.

## v0.28.0

Users upgrading from v0.27.1 or earlier must refresh existing polkit and
kill-switch installations and apply the steps described in the
[v0.28.0 migration guide](migrations/v0.28.0.md).

## v0.27.0 on Omarchy 4

Follow the [v0.27.0 migration guide](migrations/v0.27.0.md) to install the
standalone `yarikov.omakvn` bar plugin.

## v0.22.0 on Omarchy

Follow the [v0.22.0 migration guide](migrations/v0.22.0.md) to refresh the
Omarchy desktop integration. Omarchy 3 is no longer supported since v0.32.0.

## v0.20.0

Follow the [v0.20.0 migration guide](migrations/v0.20.0.md) to move daemon
startup from Hyprland autostart to the systemd user service.

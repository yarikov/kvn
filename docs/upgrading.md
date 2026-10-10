# Upgrading kvn

Upgrade `kvn` through your usual package manager:

```bash
yay -Syu
```

Pending migrations run on the next `kvn` launch, or explicitly with
`kvn migrate`.

Use `kvn migrate --pending` to check pending migrations and `kvn doctor` if an
upgrade needs attention.

## Next release (unreleased)

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

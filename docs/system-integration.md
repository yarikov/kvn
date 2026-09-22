# System integration

This document describes every system or desktop change made by kvn, the
`kvn-tui` package, and its optional setup commands. Review it before running
commands with `sudo`.

## Package installation

The Arch package installs:

| Path | Mode | Purpose |
|------|------|---------|
| `/usr/bin/kvn-tui` | `0755` | Application binary |
| `/usr/bin/kvn` | symlink | Canonical command pointing to `kvn-tui` |
| `/usr/lib/systemd/user/kvn-tui.service` | `0644` | Per-user daemon service |
| `/usr/lib/kvn/migrations/` | `0755` | Root-owned, ordered breaking-migration scripts |
| `/var/lib/kvn/migration-baseline` | `0644` | Scripts included by the initial package installation |
| `/usr/share/libalpm/hooks/kvn-tui-sing-box-capabilities.hook` | `0644` | Restores sing-box capabilities after package updates |
| `/usr/share/licenses/kvn-tui/LICENSE` | `0644` | MIT license for the source package |
| `/usr/share/licenses/kvn-tui-bin/LICENSE` | `0644` | MIT license for the binary package |

The package post-install hook grants `/usr/bin/sing-box` the
`cap_net_admin,cap_net_raw+ep` capabilities required for TUN operation. It does
not enable the daemon service automatically. An ALPM hook restores these
capabilities whenever pacman installs or upgrades `/usr/bin/sing-box`; manually
replaced binaries are not covered. Enable the daemon as your regular user:

```bash
systemctl --user enable --now kvn-tui.service
```

Before removing the package, stop and disable the service:

```bash
systemctl --user disable --now kvn-tui.service
```

Removing the `kvn-tui` package removes the ALPM hook but does not remove
capabilities already set on sing-box. If no other software needs them, they can
be revoked explicitly:

```bash
sudo setcap -r /usr/bin/sing-box
```

Do not revoke them while another TUN client relies on the same sing-box binary.

## Package migrations

Package upgrades may include ordered migration scripts installed under
`/usr/lib/kvn/migrations/`. Updates installed through `yay` or `paru` are
detected on the next `kvn` launch, or run them explicitly with `kvn migrate`.

The runner takes `$XDG_RUNTIME_DIR/kvn/migrate.lock`, waits for any active
pacman transaction, backs `profiles.json` up into
`~/.config/kvn-tui/recovery/`, then runs the pending scripts in order against a
disposable copy of the config. Only once the whole queue and its config result
have landed is the queue recorded under `$XDG_STATE_HOME/kvn/migrations/`;
machine-wide operations use `/var/lib/kvn/migrations/`. A run that fails
anywhere records nothing and replays from the start next time, which is why
migration scripts must be idempotent.

The daemon, the VPN and the kill switch stay up while the scripts run. Once the
queue finishes, the daemon is still running the previous version and
configuration, so the runner restarts `kvn-tui.service` — unless a TUI session
is attached, in which case that session shows a modal prompt instead — `Enter`
restarts, `Ctrl+C` stops the daemon — and the daemon refuses to persist config
until it is restarted.

Use:

```bash
kvn migrate --pending
```

to inspect pending migrations, or:

```bash
kvn doctor
```

to check migration state and related issues.

## Polkit setup

```bash
sudo kvn setup --polkit
```

System integration setup and cleanup must be run through `sudo` from a
non-root user. Unprivileged invocations and commands run directly from a root
shell are rejected. Polkit and the kill switch may be handled together with
`--polkit --killswitch`.

The command:

- creates the dedicated system group `kvn-tui` and adds the invoking user if
  necessary;
- writes `/etc/polkit-1/rules.d/49-kvn-tui.rules` with mode `0644`;
- records the rule's SHA-256 in `/var/lib/kvn/integrations/polkit.sha256`
  (mode `0644`), because the polkit rules directory is not readable by
  regular users;
- leaves polkit to reload the changed rule automatically.

The rule allows every member of `kvn-tui` to perform these actions without an
authentication prompt:

- `org.freedesktop.resolve1.set-dns-servers`
- `org.freedesktop.resolve1.set-domains`
- `org.freedesktop.resolve1.set-default-route`
No NetworkManager actions are granted. This authorization is group-wide and is
not restricted to the kvn process. After being added to `kvn-tui`, reboot
to activate the `kvn-tui` group.

To remove the rule:

```bash
sudo kvn clean --polkit
```

Cleanup preserves the group while the kill switch still uses it. If neither
integration remains, cleanup removes the now-unused group and its membership
records automatically. Auto-connect is turned off in kvn settings: immediately
if the daemon is running, otherwise on its next start.

## Kill switch setup

```bash
sudo kvn setup --killswitch
```

The command requires `nftables`, adds the invoking user to the dedicated
`kvn-tui` group when needed, and installs:

| Path | Owner / mode | Purpose |
|------|--------------|---------|
| `/etc/kvn-tui/killswitch.nft` | `root`, `0644` | nftables ruleset |
| `/usr/lib/kvn-tui/killswitch-helper.sh` | `root:root`, `0755` | Validating privileged helper |
| `/etc/systemd/system/kvn-tui-killswitch.service` | `root`, `0644` | System kill-switch unit |
| `/etc/sudoers.d/kvn-tui-killswitch` | `root:root`, `0440` | Restricted NOPASSWD rule |
| `/var/lib/kvn/integrations/killswitch-sudoers.sha256` | `root`, `0644` | SHA-256 of the installed sudoers rule |

The sudoers rule permits members of `kvn-tui` to invoke only the fixed helper
path without a password. The root-owned helper rejects unknown operations and
accepts only:

- `check` (read-only authorization/installation probe)
- `enable`
- `disable`
- `revoke`
- `allow <ip> <tcp|udp> <port>`

The nftables policy drops other input, output, and forwarded traffic while
allowing loopback, `kvn*`, established connections, private LAN ranges,
DHCP, ICMP, and packets marked by sing-box. Temporary IPv4/IPv6 exceptions are
added for VPN and DNS handshakes, then revoked on disconnect.

Marked traffic includes the sing-box `direct` outbound. This is required for
Bypass/Only modes and means an explicit Direct service route can leave through
the physical network while the kill switch is active.

Toggling the kill switch with `K` runs `systemctl enable --now` or
`disable --now`; an enabled kill switch therefore loads again at boot.

### Remove the kill switch

Use the cleanup command, which stops the active unit before deleting any files:

```bash
sudo kvn clean --killswitch
```

If the active unit cannot be stopped, cleanup aborts before removing its files.
The command preserves the `kvn-tui` group while the polkit integration still
uses it. If neither integration remains, cleanup removes the group and its
membership records automatically. The kill switch is turned off in kvn
settings: immediately if the daemon is running, otherwise on its next start.

## Outdated integrations

`kvn doctor` compares the installed kill-switch helper, ruleset, and unit with
the versions embedded in the current kvn binary, and compares the recorded
SHA-256 stamps with the embedded sudoers and polkit rules. Any difference is
reported as an error; rerun `sudo kvn setup --killswitch` or
`sudo kvn setup --polkit` to update the files. Installations made before
stamps were introduced have no stamps and are reported as outdated once.
A polkit rule that grants the DNS actions without a stamp is treated the same
way. Cleanup removes the corresponding stamp.

## Omarchy setup

```bash
kvn setup --omarchy
```

Omarchy 4 or newer is required; the installer aborts on older releases, and
`kvn doctor` reports a warning when it finds an Omarchy 3 installation.

This command runs without sudo and changes only the current user's files.
Running it as root is rejected to prevent changes under `/root`. `--omarchy`
cannot be combined with `--polkit` or `--killswitch`; run user-level and
system-level commands separately. It installs this executable launcher:

```text
~/.local/bin/omarchy-launch-kvn-tui
```

The launcher mode is `0755`.

### What the installer updates

- `~/.config/omarchy/plugins/yarikov.omakvn/` — a Git-managed checkout of the
  standalone [omakvn](https://github.com/yarikov/omakvn) Quickshell plugin.
  The widget connects to the daemon's Unix socket
  (`$XDG_RUNTIME_DIR/kvn-tui.sock`) and exchanges NDJSON: snapshots in,
  semantic commands (`ConnectProfile`, `Disconnect`, `SetRoutingMode`,
  `SetGeoRegion`, `SetKillSwitch`, `SetAutoConnect`) out. Existing embedded
  copies and older `command` bar modules are migrated automatically. The
  plugin is mandatory: when the shell plugin registry is unavailable or the
  install fails, setup aborts and rolls back instead of installing a reduced
  integration;
- `~/.config/omarchy/shell.json` — the `yarikov.omakvn` bar entry, inserted
  before `omarchy.bluetooth`;
- `~/.config/hypr/bindings.lua` — optional launcher binding;
- `~/.config/hypr/hyprland.lua` — floating-window rule.
- `~/.local/share/applications/kvn-tui.desktop` — Apps menu entry searchable by
  `kvn`, `kvn-tui`, `tui`, and `vpn`; it opens or focuses the TUI directly.
- `~/.local/share/icons/hicolor/scalable/apps/kvn-tui.svg` — high-contrast Apps
  icon styled like Omarchy's bundled applications for light and dark themes.

When `omarchy-shell` is running, the installer also triggers a plugin rescan
and an idempotent `omarchy bar put yarikov.omakvn` so the widget appears without a
re-login.

The selected shortcut is explicitly unbound before being assigned to kvn.
The suggested `Super + Ctrl + K` shortcut replaces the default Herdr binding.
Changes are applied as a transaction and rolled back if setup fails or
Hyprland reports new configuration errors.

### Backups and removal

Changed configuration files receive timestamped backups such as:

```text
bindings.lua.bak.before-kvn-tui.20260821143012
```

At most five kvn backups are retained for each file. To fully remove the
integration, first remove the plugin with Omarchy's plugin manager:

```bash
omarchy plugin remove yarikov.omakvn
```

Then restore a suitable backup or manually remove the legacy `kvn-tui` module,
binding, and window-rule entries. Remove the launcher separately:

```bash
rm ~/.local/bin/omarchy-launch-kvn-tui
rm ~/.local/share/applications/kvn-tui.desktop
rm ~/.local/share/icons/hicolor/scalable/apps/kvn-tui.svg
```

After confirming the active configuration no longer needs the backups, delete
only the backups created by kvn with:

```bash
kvn clean --omarchy
```

`clean --omarchy` removes only these backups, including those left by earlier
Omarchy 3 installations. It does not remove the plugin, launcher, keybinding,
or window rule.

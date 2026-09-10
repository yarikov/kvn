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
| `/usr/lib/kvn-tui/migrations/` | `0755` | Root-owned, ordered breaking-migration scripts |
| `/var/lib/kvn-tui/migration-baseline` | `0644` | Scripts included by the initial package installation |
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

`kvn update` updates the AUR package and invokes the migration runner from the
new binary. Updates performed directly through `yay` or `paru` are detected on
the next `kvn` launch. Successful scripts are recorded per user under
`$XDG_STATE_HOME/kvn-tui/migrations/`; machine-wide operations use their own
root-owned markers under `/var/lib/kvn-tui/migrations/`.

Optional root-owned `*.resources.json` files (`0644`) declare Git resources
pinned to full commits. The runner prepares these in private per-user
`$XDG_STATE_HOME/kvn-tui/migration-resources/` storage before entering migration
mode. A manifest with `"when": "omarchy_4"` is prepared only when
`omarchy version` reports major version 4; on ordinary Arch without Omarchy,
or with another major version, its downloads are skipped before calling Git.
Preparation errors are recorded separately from the transaction journal
and shown by `kvn doctor`; they do not block the existing daemon or VPN.
Scripts use `KVN_MIGRATION_RESOURCES_DIR` and must not download during a
transaction. `setup --omarchy --plugin-source PATH` installs an independent
local plugin copy without remote add/update. Resources survive failed
transactions and are deleted only after successful daemon/VPN handoff.

The runner keeps the daemon IPC client through all progress acknowledgements
and requires a confirmed daemon exit before exchanging `profiles.json`. The
journal, rather than a freshly discovered package queue, controls recovery and
completion markers. Its file identities make a crash between atomic exchange
and phase persistence recoverable. During this brief daemon replacement, an
attached TUI retains the blocking overlay, reconnects without starting a daemon
itself, and re-executes the installed client after the journal is cleared.

The runner first sends the daemon a versioned migration command. The daemon
rejects config mutations and the TUI displays a non-dismissible overlay, while
the existing sing-box process and kill switch keep running. Only after the
daemon acknowledges this state does the runner copy the exact `profiles.json`
bytes to `~/.config/kvn-tui/recovery/profiles.json.before-migration-*.json`
(mode `0600`) and create a private `.profiles.json.migrating-*` candidate from
that backup. Scripts receive the candidate path through a runner-owned
environment variable; they never edit the live file.

A new daemon does not start while migrations are pending, except for the
explicit post-cutover start recorded in the private
`$XDG_STATE_HOME/kvn-tui/migration-session.json` journal. After the ordered
queue, the runner validates the candidate and verifies that the live source
still matches the backup. It then briefly stops the old daemon, atomically
exchanges the live and candidate files, starts the new daemon, releases migration
mode, and reconnects the prior profile. The former live file stays at the
private candidate path until the handoff succeeds, then is removed; the exact
backup under `recovery/` remains available. On failure before the exchange, the
existing daemon and tunnel keep running and the candidate/journal are retained
for `kvn migrate`. The journal also makes crashes on either side of the atomic
exchange resumable without guessing or overwriting a backup.
The schema version in `profiles.json` is checked separately from package
markers, so restoring an old config after reinstalling the package cannot skip
its schema migration.
Run `kvn migrate --pending` to inspect the queue or `kvn doctor` for a read-only
health check.

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
- leaves polkit to reload the changed rule automatically.

The rule allows every member of `kvn-tui` to perform these actions without an
authentication prompt:

- `org.freedesktop.resolve1.set-dns-servers`
- `org.freedesktop.resolve1.set-domains`
- `org.freedesktop.resolve1.set-default-route`
No NetworkManager actions are granted. This authorization is group-wide and is
not restricted to the kvn process. After being added to `kvn-tui`, log out
and back in and restart `kvn-tui.service`.

To remove the rule:

```bash
sudo kvn clean --polkit
```

Cleanup preserves the group while the kill switch still uses it. If neither
integration remains, cleanup removes the now-unused group and its membership
records automatically. Existing membership in the legacy `network` group is
never removed automatically; verify that no other software needs it before
changing it manually.

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

The sudoers rule permits members of `kvn-tui` to invoke only the fixed helper
path without a password. The root-owned helper rejects unknown operations and
accepts only:

- `check` (read-only authorization/installation probe)
- `enable`
- `disable`
- `revoke`
- `allow <ip> <tcp|udp> <port>`

The nftables policy drops other input, output, and forwarded traffic while
allowing loopback, `tun*`/`kvn*`, established connections, private LAN ranges,
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
membership records automatically. Restart the user daemon afterward so its
persisted state is reconciled.

## Omarchy setup

```bash
kvn setup --omarchy
```

This command runs without sudo and changes only the current user's files.
Running it as root is rejected to prevent changes under `/root`. `--omarchy`
cannot be combined with `--polkit` or `--killswitch`; run user-level and
system-level commands separately. Both Omarchy generations install this
executable launcher:

```text
~/.local/bin/omarchy-launch-kvn-tui
```

The launcher mode is `0755`.

### Omarchy 3

The installer may update:

- `~/.config/waybar/config.jsonc` — status module and click action;
- `~/.config/waybar/style.css` — module spacing;
- `~/.config/hypr/autostart.conf` — removes the legacy daemon autostart line;
- `~/.config/hypr/bindings.conf` — optional launcher binding;
- `~/.config/hypr/hyprland.conf` — floating-window rule.

It restarts Waybar and restores the Waybar configuration from the current
backup if the restart fails.

### Omarchy 4

The installer updates:

- `~/.config/omarchy/plugins/yarikov.omakvn/` — a Git-managed checkout of the
  standalone [omakvn](https://github.com/yarikov/omakvn) Quickshell plugin,
  installed when the shell plugin registry is available. The widget connects
  to the daemon's Unix socket (`$XDG_RUNTIME_DIR/kvn-tui.sock`) and exchanges
  NDJSON: snapshots in, semantic commands (`ConnectProfile`, `Disconnect`,
  `SetRoutingMode`, `SetGeoRegion`, `SetKillSwitch`, `SetAutoConnect`) out.
  Existing embedded copies are migrated automatically. Without the plugin
  registry or when a fresh remote install fails, the installer falls back to
  a `command` bar module running `kvn --waybar-status`;
- `~/.config/omarchy/shell.json` — the `yarikov.omakvn` bar entry (or the legacy
  `kvn-tui` command module on fallback), inserted before `omarchy.bluetooth`;
- `~/.config/hypr/bindings.lua` — optional launcher binding;
- `~/.config/hypr/hyprland.lua` — floating-window rule.
- `~/.local/share/applications/kvn-tui.desktop` — Apps menu entry searchable by
  `kvn`, `kvn-tui`, `tui`, and `vpn`; it opens or focuses the TUI directly.
- `~/.local/share/icons/hicolor/scalable/apps/kvn-tui.svg` — high-contrast Apps
  icon styled like Omarchy's bundled applications for light and dark themes.

When `omarchy-shell` is running, the installer also triggers a plugin rescan
and an idempotent `omarchy bar put yarikov.omakvn` so the widget appears without a
re-login. Upgrades from the command-module integration replace the old entry.

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
integration on Omarchy 4, first remove the plugin with Omarchy's plugin manager:

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

`clean --omarchy` removes only these backups. It does not remove the plugin,
launcher, keybinding, window rule, or undo the Omarchy 3 Waybar integration.

# Command-line interface

Running `kvn` without arguments opens the TUI, starting the background daemon
first when it is not running. Before that it runs any pending
[package migrations](#migrate) in the terminal. Every other command below runs
once and exits.

The package also installs the legacy `kvn-tui` name. It still works, but every
command run through it, except plain `kvn-tui`, `kvn-tui --daemon` and
`kvn-tui --waybar-status`, first prints a warning to stderr that the name will be
removed.

```bash
kvn --help              # list commands
kvn <command> --help    # options of one command
kvn --version
```

| Command | Purpose |
|---------|---------|
| [`kvn status`](#status) | Show the connection state |
| [`kvn connect`](#connect) | Connect to a profile |
| [`kvn disconnect`](#disconnect) | Disconnect or cancel a connection attempt |
| [`kvn reconnect`](#reconnect) | Restart the current connection |
| [`kvn toggle`](#toggle) | Connect the last profile, or disconnect |
| [`kvn enable` / `kvn disable`](#enable-and-disable) | Turn the kill switch on or off, or hide the support prompt |
| [`kvn doctor`](#doctor) | Check that kvn is ready to use |
| [`kvn migrate`](#migrate) | Run package migrations |
| [`kvn config`](#config) | Reset or recover `profiles.json` |
| [`kvn setup`](#setup) | Install optional integrations |
| [`kvn clean`](#clean) | Remove optional integrations or everything kvn created |
| [`kvn --waybar-status`](#--waybar-status) | Print status JSON for a Waybar module |
| [`kvn --daemon`](#--daemon) | Run the headless daemon |

## Connection control

These commands talk to the running daemon, the same one the TUI uses, so a
connection made from the command line is visible in the TUI and vice versa.
They fail when the daemon belongs to a different kvn version; restart it with
`systemctl --user restart kvn-tui.service` or by opening the TUI.

`connect`, `reconnect`, and a `toggle` that connects return as soon as the
daemon has started the connection, before the tunnel is up; they print
`Connecting to <profile>…` (or `Reconnecting to <profile>…`). A zero exit status therefore does not mean the VPN
is connected, and `kvn connect Work && some-command` can run `some-command`
outside the tunnel. A script that needs the tunnel should wait for
[`kvn status`](#status) to report it:

```bash
kvn connect Work &&
  timeout 30 sh -c "until kvn status | grep -q '^Connected'; do sleep 1; done"
```

A connection that fails later is reported by `kvn status`, not by the command
that started it.

### status

```bash
kvn status [--json]
```

Prints one line describing the connection:

```text
Connected to Work VPN (↑ 1.2 MiB/s · ↓ 3.4 MiB/s) [kill switch]
Connecting to Work VPN…
Disconnected
```

The transfer rates appear only while either of them is above zero, and
`[kill switch]` only while the kill switch is on. When the last action failed,
the line becomes `Disconnected — <message>` while kvn is disconnected, and the
message is written to stderr as `last error: <message>` in every state. `--json` prints the daemon's full state snapshot instead.
`status` never starts the daemon; it fails when the daemon is not running.

### connect

```bash
kvn connect <profile>
```

`<profile>` is matched in this order:

1. a profile UUID;
2. an exact profile name, ignoring case;
3. the start of exactly one profile name, ignoring case.

A query that matches several profiles, or none, is rejected with a message
naming the problem; use the UUID when two profiles share a name. The daemon is
started if it is not running.

### disconnect

```bash
kvn disconnect
```

Disconnects the active tunnel or cancels a connection in progress. It succeeds
without doing anything when the daemon is not running.

### reconnect

```bash
kvn reconnect
```

Restarts the active or in-progress connection with the same profile. It fails
when kvn is disconnected.

### toggle

```bash
kvn toggle
```

Disconnects (or cancels) when a connection is active or in progress; otherwise
connects to the last profile that connected successfully. The daemon is started
if it is not running. This is the command to bind to a key or a bar click.

When no profile has connected yet, it fails with
`no previous profile; run kvn connect <name> first`.

## enable and disable

```bash
kvn enable --killswitch
kvn disable --killswitch
kvn disable --support-prompt
```

`--killswitch` turns the kill switch on or off. When the daemon is running the
change goes through it and the setting is saved; otherwise only the kill-switch
unit is switched, and the daemon aligns `settings.kill_switch` with the unit on
its next start. Unlike
`Shift+K` in the TUI, disabling asks for no confirmation. The kill switch must
have been installed with [`sudo kvn setup --killswitch`](#setup) first; see
[Kill switch setup](system-integration.md#kill-switch-setup).

`--support-prompt` permanently hides the support prompt. It works whether or
not the daemon is running and cannot be combined with `--killswitch`.

## Diagnostics and upgrades

### doctor

```bash
kvn doctor
```

Runs a read-only check of sing-box, the configuration, pending package
migrations, the daemon, the clipboard, and the optional integrations, with a
remediation hint for each problem. It exits with a non-zero status when a
required check fails; warnings alone do not change the exit status.

### migrate

```bash
kvn migrate [--pending]
```

Runs the package migrations installed by a newer kvn package. They also run
automatically on the next `kvn` launch. While any migration is pending, the
daemon refuses to start (see [`--daemon`](#--daemon)). `--pending` only lists them, one
`<id>⇥<summary>` line each, without running anything. See
[Package migrations](system-integration.md#package-migrations) and
[Upgrading kvn](upgrading.md).

## config

```bash
kvn config reset [--yes]
kvn config recover <file>
```

Both commands replace `~/.config/kvn-tui/profiles.json` and refuse to run while
the daemon is running; stop it first with
`systemctl --user stop kvn-tui.service`. The previous file is archived under
`~/.config/kvn-tui/recovery/` as `profiles.json.invalid-*`, and the archive
path is printed. kvn keeps only the three newest archives and deletes older
ones as new archives are created; copy an archive elsewhere if you need to keep
it.

- `reset` creates a default configuration. It asks for confirmation unless
  `--yes` is given.
- `recover` installs `<file>` — for example an editor copy that kvn preserved
  in the recovery directory — after checking that it loads and is valid.
  Those copies are rotated the same way, three of each kind.

## Integrations

What each integration installs and changes is described in
[System integration](system-integration.md).

### setup

```bash
kvn setup --omarchy
sudo kvn setup --polkit
sudo kvn setup --killswitch
sudo kvn setup --polkit --killswitch
```

| Option | Run as | Installs |
|--------|--------|----------|
| `--omarchy` | your user, without `sudo` | The Omarchy bar plugin, launcher, keybinding, and window rule ([details](system-integration.md#omarchy-setup)) |
| `--polkit` | `sudo` | Passwordless DNS management, needed for auto-connect ([details](system-integration.md#polkit-setup)) |
| `--killswitch` | `sudo` | The nftables kill switch ([details](system-integration.md#kill-switch-setup)) |

`--omarchy` changes only user files and is rejected under `sudo`, so it cannot
be combined with the other two. `--polkit` and `--killswitch` must be run
through `sudo` from the account that uses kvn.

### clean

```bash
kvn clean --omarchy
kvn clean --omarchy-backups
sudo kvn clean --polkit
sudo kvn clean --killswitch
sudo kvn clean --polkit --killswitch
sudo kvn clean --all [--yes]
```

| Option | Run as | Removes |
|--------|--------|---------|
| `--omarchy` | your user | The whole Omarchy integration, backups included ([details](system-integration.md#backups-and-removal)) |
| `--omarchy-backups` | your user | Only the backups `setup --omarchy` created |
| `--polkit` | `sudo` | The polkit rule; auto-connect is turned off |
| `--killswitch` | `sudo` | The kill switch, after stopping it; the setting is turned off ([details](system-integration.md#remove-the-kill-switch)) |
| `--all` | `sudo` | Everything kvn created for you, profiles included ([details](system-integration.md#remove-everything)) |

`--polkit` and `--killswitch` turn the matching setting off in the running
daemon, or on its next start when it is not running. `--all` stops the daemon,
lists what it is about to delete, and asks for confirmation; `--yes` skips the
prompt. `--all` and `--yes` cannot be combined with any other option.

## Integration flags

### --waybar-status

```bash
kvn --waybar-status
```

Prints one line of JSON for a Waybar `custom` module:

```json
{"text":"󰦝","tooltip":"Connected: Work VPN","class":"connected"}
```

`class` is `connected` or `disconnected`; the icons are Nerd Font glyphs. The
state is read from `~/.config/kvn-tui/state.json` without contacting the
daemon, so the command is cheap enough to poll:

```jsonc
"custom/kvn": {
  "exec": "kvn --waybar-status",
  "return-type": "json",
  "interval": 5,
  "on-click": "kvn toggle"
}
```

### --daemon

```bash
kvn --daemon
```

Runs the headless daemon that owns sing-box, the configuration, and the
background services. It is normally started by `kvn-tui.service`, or on demand
by `kvn`, `kvn connect`, and `kvn toggle`; run it by hand only in a custom
service, with `ExecStart` pointing at this command.

While a package migration is pending, the daemon does not start: it prints
`N kvn migration(s) are pending. Launch kvn in a terminal to apply them` and
exits with status 0, so systemd does not restart it. Run `kvn` or
`kvn migrate` to apply them; until then no VPN connection can be made.

The daemon runs `sing-box` from `PATH`, or the binary named by the
`SING_BOX_PATH` environment variable. Set it in the service environment, for
example with `systemctl --user edit kvn-tui.service`.

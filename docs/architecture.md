# Architecture

`kvn` does not implement any VPN protocol. It is a configuration and process
manager around [sing-box](https://sing-box.sagernet.org/): it stores
profiles and settings, turns the active profile into a sing-box configuration,
runs `sing-box` as a child process and shows its state in a terminal UI.

- [Processes](#processes)
- [The core: The Elm Architecture](#the-core-the-elm-architecture)
- [Daemon threads](#daemon-threads)
- [IPC protocol](#ipc-protocol)
- [Logs](#logs)
- [TUI client](#tui-client)
- [sing-box lifecycle](#sing-box-lifecycle)
- [Configuration and state](#configuration-and-state)
- [Privileges](#privileges)
- [Source layout](#source-layout)
- [Technology stack](#technology-stack)
- [Testing](#testing)

## Processes

```text
 ┌──────────────┐ ┌──────────────────────┐ ┌──────────────────────────┐
 │ kvn (TUI)    │ │ kvn (CLI)            │ │ Omarchy plugin (omakvn)  │
 └──────┬───────┘ └──────────┬───────────┘ └────────────┬─────────────┘
        │   NDJSON over $XDG_RUNTIME_DIR/kvn-tui.sock   │
        └────────────────────┼──────────────────────────┘
                             ▼
              ┌───────────────────────────────┐
              │ kvn --daemon                  │   systemd user unit
              │ (kvn-tui.service)             │   kvn-tui.service
              │ canonical Model, config, geo, │
              │ suspend, logs                 │
              └───────┬───────────────┬───────┘
     spawn / kill,    │               │  sudo killswitch-helper.sh
     Clash API        │               │  enable / disable
     (loopback)       ▼               ▼
 ┌────────────────────────────┐ ┌────────────────────────────────┐
 │ sing-box run               │ │ kill switch: nftables table    │
 │ TUN, protocols, DNS,       │ │ kvn-tui-killswitch.service     │
 │ routing                    │ │ (systemd system unit)          │
 │ cap_net_admin,cap_net_raw  │ │ drops egress outside the       │
 │ marks packets 0x29a ───────┼─┼▶ tunnel, passes 0x29a          │
 └────────────────────────────┘ └────────────────────────────────┘
```

**The daemon** (`kvn --daemon`, started by the `kvn-tui.service` user unit, or
on demand by `kvn`, `kvn connect` and `kvn toggle`, which fall back to spawning
it directly when the unit cannot be started) controls the running VPN: while it runs, every change goes through it. It owns
the canonical application state, `profiles.json`, the sing-box process, geo rule-set downloads, suspend
and resume handling, and it turns the kill switch on and off. It runs headless, so the VPN does not
depend on a terminal being open.

At startup the daemon refuses to run while a package migration is pending. It
then reconciles settings with the system: `settings.kill_switch` follows
whether the kill-switch unit is actually active, and auto-connect is turned off
before the first connect when polkit denies the DNS actions it needs. On exit
it stops sing-box and flushes kill-switch exceptions an older kvn left behind.

**Clients** connect to the daemon's Unix socket and never touch sing-box
directly:

- the TUI (`kvn`) renders the state and forwards input;
- the one-shot CLI commands (`kvn status`, `connect`, `disconnect`,
  `reconnect`, `toggle`, `enable`/`disable`) send one command and exit;
- the [Omarchy bar plugin](https://github.com/yarikov/omakvn) shows the
  connection on the desktop bar and toggles it.

Some CLI commands also work without a daemon and then act on their own.
`kvn enable`/`disable --killswitch` call the kill-switch helper directly when
the daemon is unreachable, and `kvn disable --support-prompt` writes
`support-prompt.json` itself. `kvn config`, `kvn migrate`, `setup` and `clean`
work on files and system integration directly; see the
[CLI reference](cli.md).

Closing the TUI with `q` sends `Detach` and exits the TUI process. The daemon
and the tunnel keep running, and running `kvn` again reattaches instantly.
`Ctrl+C` in the TUI sends `Quit`, which stops sing-box and the daemon.

**sing-box** is a child of the daemon. It creates the TUN interface, speaks
the VPN protocol and does DNS and routing. The daemon reads live traffic
statistics from its Clash API on a loopback port chosen for each start.

**The kill switch** lives outside the daemon: an nftables table loaded by the
`kvn-tui-killswitch.service` system unit before the network comes up. It drops
traffic leaving outside the tunnel, but lets through packets sing-box marks
with fwmark `0x29a`, so its own connection to the VPN server gets through; the
full list of exceptions is in [What kvn protects](privacy.md#kill-switch). The
daemon only enables or disables the unit through `sudo killswitch-helper.sh`;
the table stays loaded when the daemon or sing-box stops or crashes, so traffic
is blocked rather than leaked.

## The core: The Elm Architecture

All business logic follows The Elm Architecture (TEA), in `src/app/`:

| Part | File | Role |
|------|------|------|
| `Model` | `app/model.rs` | All application state as plain data |
| `Msg` | `app/msg.rs` | Every event: IPC commands, ticks, process exits, download results, resume |
| `update` | `app/update.rs` + `app/update/` | `update(model, msg) -> Vec<Effect>`: changes the model, performs no I/O |
| `Effect` | `app/effect.rs` | A description of a side effect: `Connect`, `SaveConfig`, `DownloadGeo`, `BroadcastState`, … |

`update` never reads files, opens sockets, starts threads or runs processes.
It returns `Effect` values, and the daemon's main loop executes them. The few deliberate exceptions (small,
deterministic file reads that decide whether Omarchy-specific entries are
shown) are listed in [AGENTS.md](../AGENTS.md#side-effect-free-boundaries).

After each `update`, the main loop handles the effects in this order:

1. **`SaveConfig` first.** If the list contains it, the loop saves the new
   config before any other effect runs (`persist_config_unless_frozen`,
   revision-checked as described in
   [Configuration and state](#configuration-and-state)). On success the saved
   config replaces the model's and `SaveConfig` is dropped from the list. On
   failure the previous config, the support prompt state and, if the update
   moved it, the first-run tour are restored. The edited config is preserved
   in a recovery file, the error is shown and logged, and every other effect
   except `BroadcastState` and `AppendAppLog` is dropped, so nothing acts on a
   config that was never saved.
2. **The rest in order.** `daemon::effect::execute_daemon_effect` dispatches
   each remaining effect to a submodule of `src/daemon/`. A `SaveConfig` that
   reaches it was not committed above, so all it does there is report the
   error.

Connecting to a profile, end to end:

1. The TUI sends `IpcCommand::Key(Enter)`. The IPC server thread puts it on
   the daemon's channel as `Msg::IpcCommand`.
2. The key handler only queues the connection (`queue_connect`): the model
   becomes `Connecting` with the chosen profile, and `Effect::BroadcastState`
   shows that to the clients.
3. On the next `Msg::Tick`, the tick handler sees the queued connection and
   returns `Effect::Connect`. Auto-connect at startup and reconnect after
   resume use the same path.
4. The daemon marks the connection as pending, so later ticks do not start
   it again, and runs it in a worker thread (generate config, check, spawn).
   The worker puts the process into the shared process slot (see
   [Daemon threads](#daemon-threads)) and sends back `Msg::Connected`, or
   sends `Msg::ConnectFailed`.
5. `update` records the result and returns more effects: save the last used
   profile, write `state.json`, download missing service rule-sets through the
   tunnel, broadcast the new state.
6. Every attached client receives a `StateSnapshot` and redraws.

Because `update` is a pure function, every state transition can be tested by
building a `Model`, sending a `Msg` and checking the model and the effects.

## Daemon threads

The daemon has one `mpsc` channel of `Msg`, consumed by the main loop that
runs `update` and executes effects. Only the main loop changes the `Model`.
Other threads report to it with messages:

- **Ticker**: `Msg::Tick` every 250 ms. It starts queued connections and
  drives timeouts, auto-update schedules, sing-box exit detection and, once a second while
  connected, a Clash API traffic sample.
- **IPC server**: accepts socket connections and turns each NDJSON command into
  `Msg::IpcCommand`, or `Msg::IpcRequest` when it carries a request id.
- **Suspend watcher**: listens for systemd-logind `PrepareForSleep` over D-Bus
  and sends `Msg::SystemResumed`, so the tunnel is reconnected after resume
  even with no TUI open.
- **Signal handler**: turns `SIGTERM`/`SIGINT` into a clean quit.
- **Effect workers**: short-lived threads for slow effects (connecting,
  downloads, subscription fetches, profile latency tests, kill-switch and
  polkit checks), each reporting back with a `Msg`.

The sing-box process is the one piece of state shared outside the channel. It
lives in `ProcessSlot`, the process handle plus the current connect attempt id,
behind a mutex:

- the main loop writes the current attempt id after every `update`, so a new
  connect or a disconnect immediately invalidates older attempts;
- a connect worker installs its process only if its attempt id still matches.
  Otherwise it kills the process it just started, so a cancelled connect can
  never replace a newer one. A second mutex lets only one worker start sing-box
  at a time;
- disconnect takes the handle out of the slot and stops the process;
- the ticker checks whether the process has exited, and when it has, it
  empties the slot and sends `Msg::SingBoxExited` with that attempt id.

## IPC protocol

Clients talk to the daemon in newline-delimited JSON over
`$XDG_RUNTIME_DIR/kvn-tui.sock` (mode `0600`). The protocol is in `src/ipc.rs`.

- **Attach**: asks for the current `StateSnapshot`. The CLI does this too,
  for example in `kvn status`.
- **AttachSession**: a TUI sends it right after `Attach` to register as an
  open window. The daemon counts these connections in `tui_sessions` and
  decrements the counter when the connection closes, so a crashed TUI is not
  counted either. `kvn migrate` reads the counter: with an open TUI it
  shows a restart prompt in that window, otherwise it restarts the unit
  itself.
  A client that only sends `Attach` does not count as an open TUI.
- **Detach**: sent by a TUI when it exits with `q`.
- **Key**: the TUI forwards key presses. The daemon interprets them with the
  same `update` code the TUI would, so behavior does not depend on the client.
- **Semantic commands**: `ConnectProfile`, `Disconnect`, `Reconnect`,
  `SetRoutingMode`, `SetGeoRegion`, `SetKillSwitch`, `SetAutoConnect` and
  others are for clients without a keyboard model: the CLI and the Omarchy
  plugin. They share the commit helpers with the key handlers, so both paths
  run the same logic.
- **StateSnapshot**: after every state change the daemon pushes a full
  snapshot, including the complete config, to every attached client. There are
  no diffs to get out of sync.
- **Correlated requests**: saving an edited config (`ApplyEditedConfig`),
  wheel scrolling (`ScrollViewport`), pane focus (`SetMainPaneFocus`), CLI
  connection commands (`connect`, `disconnect`, `reconnect`, `toggle`) and
  `disable --support-prompt` carry a request id. Only the matching reply
  carries their result or error.
- **Restart required**: after package migrations finish while a TUI is
  attached, the daemon enters a frozen state until it is restarted. It ignores
  every command except `Quit` and `ClearErrorStatus`, answers connection
  requests with an error, and pauses scheduled downloads.

Every snapshot carries the daemon's version and `IPC_VERSION`. When a package
upgrade leaves an older daemon running, the TUI sends it `Quit`, waits up to
five seconds for it to exit, starts the new daemon (through the unit for the
packaged binary, directly for other builds), and reconnects the profile that
was connected. One-shot CLI commands do not restart it; they fail and ask the
user to restart the daemon.

## Logs

Logs do not go through IPC, and `StateSnapshot` carries no log lines. sing-box
writes its log file, and the daemon (and, for editor results, the TUI) appends
`[app]` messages to the app log. The daemon trims both files to the configured
line limits when it starts, and the sing-box log also at most once every 24
hours when connecting. The TUI
reads the two files itself: on its own 250 ms tick, its `LogTailer` reads the
new lines and shows them in the log pane.

```text
sing-box ──writes──▶ sing-box.log ─┐
daemon ────writes──▶ app log ──────┼──▶ TUI LogTailer ──▶ log pane
daemon ── trims both files ────────┘
```

## TUI client

The TUI (`src/tui_client.rs`, `src/tui_client/`) keeps a local copy of the
`Model`, replaced by each snapshot, and renders it with ratatui
(`src/ui/`). It forwards most input to the daemon but handles locally what
needs the terminal or the desktop:

- clipboard import and copy (`wl-paste`/`wl-copy` on Wayland, `xclip`/`xsel`
  on X11), bracketed paste;
- editing `profiles.json` in `$EDITOR`, with validation and conflict
  resolution in the same session;
- reading the log files and the log pane: cursor, mouse clicks, wheel
  scrolling and text selection;
- the theme: one of the bundled palettes, or the active Omarchy theme, which
  a file watcher follows live. OSC 10/11 make the terminal's own foreground
  and background match the palette.

## sing-box lifecycle

Connecting (`src/singbox/`):

1. `singbox::config::generate_config` builds the sing-box 1.14+ JSON from the
   profile and settings. It is pure: whether geo files exist is passed in.
2. The config is written atomically with mode `0600` to
   `$XDG_RUNTIME_DIR/kvn-tui/singbox.json`.
3. `sing-box check` validates it. A rejected config never starts.
4. `sing-box run` is spawned. If it exits immediately, its stderr is shown to
   the user. If the Clash API port was taken in the meantime, the start is
   retried with a new port, up to three times.

The generated config carries the routing mode (Global, Bypass or Only for the
selected country), per-service overrides, the DNS preset and the fwmark that
lets sing-box's own packets pass the kill switch. What goes through the tunnel
in each mode is described in [What kvn protects](privacy.md). The settings
are described in the [configuration guide](configuration.md).

## Configuration and state

- **`profiles.json`** holds profiles, subscriptions and settings. It carries a
  `schema_version`. Loading runs the ordered schema migrations and saves the
  result, and a file from a newer kvn is refused instead of being damaged.
- **Writes are atomic** (temporary file, fsync, rename) and checked against
  the revision the daemon last read. A file changed on disk in the meantime is
  merged three-way instead of overwritten, and real conflicts are reported.
- **Validation** collects every problem with its JSON pointer instead of
  stopping at the first one. The external editor also gets a generated JSON
  Schema, so a JSON language server can flag mistakes while typing.
- **Runtime and state files** (tour progress, support prompt schedule,
  sing-box cache, `state.json` for status bars, logs, geo rule-sets) live in
  the XDG directories. New files use the `kvn` namespace, while existing ones
  keep `kvn-tui` for compatibility. The full list is in the
  [configuration guide](configuration.md).
- **Package migrations** (`kvn migrate`) run root-owned scripts shipped with
  the package for changes that schema migrations cannot express, back up
  `profiles.json` first and record what has run.

## Privileges

Neither the daemon nor the TUI runs as root.

- **sing-box** gets `cap_net_admin,cap_net_raw` as file capabilities from the
  package, which is enough to create the TUN interface.
- **DNS**: sing-box sets the tunnel's DNS through systemd-resolved. The
  optional polkit rule (`sudo kvn setup --polkit`) allows exactly the three
  resolved actions it uses — `set-dns-servers`, `set-domains` and
  `set-default-route` — for members of the `kvn-tui` group, so connecting does not
  ask for a password.
- **Kill switch**: an nftables ruleset loaded by a systemd unit, toggled
  through one helper script that the `kvn-tui` group may run with sudo
  (`sudo kvn setup --killswitch`).

`kvn doctor` checks all of this, including whether installed files differ
from the ones the current build ships. Every file each setup command installs
is listed in [System integration](system-integration.md).

## Source layout

| Path | Contents |
|------|----------|
| `src/main.rs`, `src/cli.rs`, `src/cli/` | Entry point, command-line parsing, one-shot commands, setup and clean |
| `src/app/` | TEA core: model, messages, `update`, effects, scroll logic |
| `src/daemon.rs`, `src/daemon/` | Main loop, effect execution, sing-box process slot, ticker |
| `src/ipc.rs` | Socket server and client, commands, `StateSnapshot` |
| `src/tui_client.rs`, `src/tui_client/` | Terminal session, input handling, clipboard, editor, theme watcher |
| `src/ui/` | ratatui layout, widgets, overlays, palettes, icons |
| `src/config.rs`, `src/config/` | `profiles.json` I/O, profile and settings types, share links, schema, merge, subscriptions |
| `src/singbox/` | Config generation, process runner, Clash API client |
| `src/geo.rs` | Geo and service rule-set downloads and cache |
| `src/services/` | Log tailer, `state.json`, suspend watcher, kill switch |
| `src/onboarding.rs` | First-run tour |
| `src/doctor.rs`, `src/integration_files.rs` | Diagnostics and the embedded polkit and kill-switch files |
| `src/migrations.rs`, `contrib/migrations/` | Package migrations |
| `src/paths.rs`, `src/atomic_write.rs` | XDG paths and atomic writes |
| `contrib/` | systemd units, polkit rule, nftables ruleset, setup and clean scripts |
| `themes/`, `build.rs` | Bundled palettes, compiled into the binary at build time |

[AGENTS.md](../AGENTS.md#module-map) has the complete module map.

## Technology stack

`kvn` is built with Rust 2024 and requires Rust 1.88 or newer.

| Component | Library / Tool | Purpose |
|-----------|--------------|---------|
| Terminal UI | [ratatui](https://ratatui.rs/) + [crossterm](https://github.com/crossterm-rs/crossterm) | Rendering, keyboard and mouse input, terminal lifecycle |
| VPN backend | [sing-box](https://sing-box.sagernet.org/) 1.14+ | TUN, protocols, DNS, and traffic routing |
| Data formats | [serde](https://serde.rs/), `serde_json`, `toml` | Configuration, IPC messages, and bundled palettes |
| Config schema | [schemars](https://graham.cool/schemars/) | JSON Schema for editor completion and validation |
| Networking | [ureq](https://github.com/algesten/ureq) with rustls | Subscriptions, rule-sets, and Clash API statistics |
| Linux integration | [zbus](https://docs.rs/zbus/latest/zbus/), `notify`, `signal-hook` | Suspend/resume, theme watching, and Unix signals |
| CLI | [clap](https://docs.rs/clap/) | Commands, setup options, and diagnostics |
| Observability | [tracing](https://github.com/tokio-rs/tracing) | Filtered application and daemon logs |
| Core utilities | `anyhow`, `uuid`, `chrono`, `url`, `base64`, `dirs` | Errors, IDs, timestamps, share links, and XDG paths |
| Testing | [insta](https://insta.rs/) | Snapshot tests of rendered screens |

## Testing

State transitions are tested through `update`: build a `Model`, send a `Msg`,
check the model and the returned effects. Rendering is tested only with insta
snapshots. Configuration and protocol logic have unit tests; IPC, subscription
and geo downloads also use local socket and HTTP fixtures. Runtime
wrappers around processes, D-Bus and desktop services are kept thin and do not
require unit-test coverage. CI requires at least 85 % line and region coverage
overall. See [CONTRIBUTING.md](../CONTRIBUTING.md) for the commands.

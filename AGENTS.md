# Agent Guide: kvn

This document contains project-specific context and conventions for AI coding agents. It supplements `README.md` with architectural details, coding styles, and rules of thumb.

It is the only agent-instruction file in this repository — there is no `CLAUDE.md`. Agents that look for one (Claude Code among them) fall back to `AGENTS.md`, so keep every convention here rather than splitting it across files.

---

## Project Overview

`kvn` is a **terminal VPN client** for Arch Linux. It is a Rust TUI application that manages VPN profiles, generates sing-box configurations, and orchestrates the `sing-box` binary as a child process. Navigation is vim-style (`j`/`k`/`g`/`G`).

The app does **not** implement VPN protocols itself. It is a configuration generator and process manager around the external `sing-box` binary.

---

## Commands

```bash
cargo build --release          # release build
cargo test                     # run all tests
cargo test ui::layout          # run a single module's tests
INSTA_UPDATE=always cargo test # run tests and auto-accept snapshot changes
cargo insta test --check --unreferenced=reject  # what CI checks: snapshots match, none orphaned
cargo fmt                      # format (required before committing)
cargo clippy --all-targets --all-features  # lint (fix warnings before committing)
cargo llvm-cov --summary-only  # coverage report; both region & line totals must stay ≥ 85 % (CI gate)
```

The rules behind each gate live in § Testing Patterns, § Coverage Policy, and § Formatting & Linting.

---

## Module Map

| Module | Path | Responsibility |
|--------|------|----------------|
| `cli` | `src/cli.rs` | CLI argument parsing: `--daemon`, `--waybar-status`, `--version`; `status`/`connect`/`disconnect`/`reconnect`/`toggle` one-shot IPC clients; `enable`/`disable --killswitch`; `doctor`, `migrate`, `config {migrate,reset,recover}`; `setup` and `clean` for the `--omarchy` / `--polkit` / `--killswitch` integrations |
| `app` | `src/app.rs`, `src/app/model.rs`, `src/app/msg.rs`, `src/app/update.rs`, `src/app/effect.rs` | TEA core: Model, Msg, Update, Effect — pure data, messages, business logic, side-effect declarations |
| `model` | `src/app/model.rs` | Application state (`Model`), overlay + connection state + subscription state, input state — pure data, no side effects |
| `msg` | `src/app/msg.rs` | Message enum (`Msg`) — all external events (keys, ticks, logs, geo, resume, etc.) |
| `update` | `src/app/update.rs` + `src/app/update/` | Pure `update(model, msg) -> Vec<Effect>` — the top-level message dispatcher only; every handler lives in a submodule (see below) |
| `update` submodules | `src/app/update/{status,connection,traffic,config_reload,tick,routing,geo,subscription,paste,onboarding}.rs` | Non-keyboard message handlers: status/download-blocked helpers, connect lifecycle + kill-switch/polkit results, Clash-API traffic sampling, `ConfigReloaded`, the 250 ms tick and its auto-update schedules, routing-mode / geo-region / service-routing commits, geo download results, subscription fetch results, clipboard paste → profile or subscription, and the first-run tour's `handoff` / `advance` / `outcome` / `finish` / `open_when_idle` / `resume` transitions plus its `integration_setup_checked` probe result |
| `update::key` | `src/app/update/key.rs` + `src/app/update/key/` | Keyboard input: `handle_key` routes by `Model.overlay` to `sources`, `confirm_delete`, `confirm_disable`, `settings_menu`, `regions`, `dns`, `service_routing`, `theme`, `onboarding`; `key/ipc.rs` (+ `ipc/{semantic,support}.rs`) handles `IpcCommand` for non-TUI clients |
| `effect` | `src/app/effect.rs` | Effect enum — declarative description of side effects to be executed by runtime |
| `daemon` | `src/daemon.rs` | Headless daemon: owns sing-box process, config, mpsc channel, IPC server, background services; `run` / `run_loop`, `DaemonShared`, `build_snapshot`, and the startup reconciliation of kill-switch and auto-connect state |
| `daemon` submodules | `src/daemon/{effect,connection,geo,config_io,subscription,traffic,profile_test,process_slot}.rs` | Effect execution, mirroring the `app/update/` handler split: `effect.rs` is the `execute_daemon_effect` dispatcher only; `connection.rs` owns connect/disconnect, the kill-switch handshake window, the polkit check and the tour's integration probe; `geo.rs` the seven geo/service rule-set effects plus the shared refresh and result-finalizing helpers; `config_io.rs` the revision-checked `profiles.json` commit, the support-prompt and onboarding-progress writes (including the failed-write recovery in § First-Run Onboarding) and config reload; `subscription.rs`, `traffic.rs` and `profile_test.rs` one effect each (the last owns the temporary sing-box SOCKS5 latency probe); `process_slot.rs` the sing-box process slot, its poisoned-lock-safe accessors, the 250 ms ticker and exit polling |
| `tui_client` | `src/tui_client.rs` | TUI client orchestration: `run`, the daemon handshake (`connect_to_current_daemon`), terminal setup (`TerminalSession`, OSC colors), snapshot application, and the `run_loop` skeleton that feeds every `Msg` to the handler tree |
| `tui_client` submodules | `src/tui_client/handler.rs` + `src/tui_client/handler/`, `src/tui_client/docs_preview.rs` | `handler.rs` owns `ClientLoop` (the loop's terminal/pane/log/toast/pointer state) and dispatches each `Msg`; the short branches (paste, snapshot, tick, resize, theme) stay there, while `mouse.rs` handles clicks, drags and log selection, `pointer.rs` the pointer shape plus double-click tracking, and `toast.rs` the client-local status toast lifetime. `handler/key.rs` routes by `Model.overlay` to `key/{support,log_pane,clipboard,editor,quit}.rs` — the client-local half of `app/update/key/`. `docs_preview.rs` builds the fixed state used for documentation captures |
| `ipc` | `src/ipc.rs` | NDJSON protocol over Unix domain socket for daemon ↔ TUI client communication |
| `migrations` | `src/migrations.rs`, `contrib/migrations/*.sh` | Ordered package migrations: root-owned script discovery, per-user/machine markers written per script, runner lock, `profiles.json` backup, end-of-run daemon restart handoff |
| `test_helpers` | `src/test_helpers.rs` | Shared test utilities (e.g. `model_with_profiles`)
| `ui` | `src/ui.rs`, `src/ui/layout.rs`, `src/ui/widgets.rs`, `src/ui/styles.rs`, `src/ui/palette.rs`, `src/ui/icons.rs`, `src/ui/nav.rs` | ratatui rendering (used by TUI client only), layout splits, widget definitions, palette-driven `Theme`, Nerd Font / Unicode icon sets selected by `settings.icons`, navigation helpers |
| `ui::layout` submodules | `src/ui/layout.rs` + `src/ui/layout/` | `src/ui/layout.rs` is the facade: frame split, the `draw*` entry points, and the `pub(crate)` re-exports the TUI client calls. Each concern lives in one submodule — `text.rs` (Unicode width helpers), `log.rs` + `log/navigation.rs` (log formatting; cursor, viewport and selection state), `panes.rs` (pane geometry, mouse hit-testing, main/traffic/status rows), `sources.rs` (the Profiles list), and `overlay.rs` (dispatch on `Model.overlay` + the shared footer wording) over `overlay/` — `popup.rs` (popup geometry and the three modal renderers), `settings_row.rs` (the shared `Label ‹ value ›` row), and one file per overlay: `help`, `settings_menu`, `confirm_delete`, `confirm_disable`, `restart_required`, `routing`, `dns`, `theme`, `support`, `onboarding` |
| `palette` | `src/ui/palette.rs`, `themes/*.toml`, `build.rs` | 22 vendored Omarchy palettes; `build.rs` compiles `themes/*.toml` into a `BUNDLED` static at compile time (no runtime TOML parsing) |
| `config` | `src/config.rs`, `src/config/profile.rs`, `src/config/subscription.rs` | JSON config I/O, profile and subscription struct definitions, subscription fetcher |
| `config::profile` submodules | `src/config/profile/{protocol,protocol_options,protocol_config,tls,entry,schedule,subscription,routing,settings,schema,migrate,dns}.rs`, `src/config/profile/share_link{.rs,/parse.rs,/encode.rs}` | `src/config/profile.rs` is a facade of `pub use` re-exports; each persisted type lives in one submodule — protocol discriminant, per-protocol options and configs, shared TLS/transport blocks, `Profile`, auto-update schedules, `Subscription`, geo routing, `Settings`, the root `Config`, the ordered schema migrations, DNS, and share-link URI parsing/encoding |
| `singbox` | `src/singbox.rs`, `src/singbox/config.rs`, `src/singbox/runner.rs`, `src/singbox/clash_api.rs`, `src/singbox/process_handle.rs` | Process lifecycle: allocate a free Clash API port, write temp config, run `sing-box check`, spawn `sing-box run`, retry a lost port race, kill on disconnect; Clash API client for live traffic stats; `Child` wrapper carrying the process's Clash API port |
| `geo` | `src/geo.rs` | Download and cache geoip/geosite rule-sets for sing-box routing |
| `paths` | `src/paths.rs` | XDG directory resolution (`~/.config/kvn-tui/`), atomic path construction |
| `atomic_write` | `src/atomic_write.rs` | Atomic file write helper (write `.tmp` + fsync + rename + parent-dir fsync) |
| `net` | `src/net.rs` | Free loopback port allocation (bind `127.0.0.1:0`, read the OS-assigned port, drop the listener) for the Clash API control port and the profile-test SOCKS5 port |
| `systemd` | `src/systemd.rs` | The daemon's systemd user unit name and its restart helper, so a unit rename lands in one place |
| `onboarding` | `src/onboarding.rs` | First-run tour: the ordered `OnboardingStep` cards, their handoff screen and copyable command, the persisted `OnboardingState` and the session-local `OnboardingProgress`, the `IntegrationSetup` / `SetupState` the protection cards render, plus the grandfathering load for installs that predate the tour |
| `waybar` | `src/services/waybar.rs` | Read/write `state.json` for waybar integration and crash recovery |
| `suspend` | `src/services/suspend.rs` | D-Bus listener for `systemd-logind` `PrepareForSleep` signals (zbus) |
| `integration_files` | `src/integration_files.rs`, `contrib/killswitch.nft`, `contrib/kvn-tui-killswitch.{service,sudoers}`, `contrib/49-kvn-tui.rules` | Embedded kill-switch/polkit payloads passed to the setup scripts; detects outdated installed files and SHA-256 stamps in `/var/lib/kvn/integrations/` for `kvn doctor` |
| `killswitch` | `src/services/killswitch.rs` | nftables helper integration: enable/disable systemd unit, pre-allow VPN handshake IPs, reconcile state on startup |
| `services` | `src/services.rs`, `src/services/log_tailer.rs`, `src/services/waybar.rs`, `src/services/suspend.rs` | Background services: log tailer, waybar state I/O, suspend watcher (all run inside the daemon) |
| `clipboard` | `src/tui_client/clipboard.rs` | System clipboard integration; auto-detects Wayland (`wl-paste` / `wl-copy`) or X11 (`xclip`, falls back to `xsel`); reads clipboard content and passes it to `parse_share_link` or the subscription fetcher |
| `editor` | `src/tui_client/editor.rs` | Launch `$EDITOR` / `$VISUAL` on `profiles.json`, temporarily restore terminal |
| `theme_watch` | `src/tui_client/theme_watch.rs` | Resolves `settings.theme` slug to a `Theme` (with `"omarchy"` sentinel falling back to `tokyo-night`); watches Omarchy's XDG state current-theme directory and emits `Msg::ThemeChanged`; no-op when Omarchy isn't installed |
| `omarchy plugin` | `https://github.com/yarikov/omakvn` | Standalone Quickshell bar plugin (`yarikov.omakvn`), required by the Omarchy integration. `setup --omarchy` requires Omarchy 4+ and installs or updates its Git checkout in `~/.config/omarchy/plugins/yarikov.omakvn/`, aborting and rolling back when the plugin cannot be installed; the main project owns the semantic IPC API. |

---

## Build System & Dependencies

- **Rust**: edition 2024, minimum version 1.88
- **External binary**: `sing-box` must be installed separately and available on `$PATH` (or via `SING_BOX_PATH` env var)
- **Key crates**: `ratatui` + `crossterm` (TUI), `serde` + `serde_json` (config), `zbus` (D-Bus), `ureq` (HTTP), `tracing` (logs), `anyhow` + `thiserror` (errors)

Build release:

```bash
cargo build --release
```

Run (no root required when sing-box has capabilities):

```bash
./target/release/kvn-tui
```

Install polkit rule (avoids authentication dialogs on connect):

```bash
sudo ./target/release/kvn-tui setup --polkit
```

---

## Release Process

See the `release` skill in `.agents/skills/release/SKILL.md` for the full version-bump and tagging workflow. Supports auto-bump by semver level (major / minor / patch) or explicit version.

---

## Platform Constraints

**Arch Linux.** Both Wayland and X11 sessions are supported. Do not add generic Linux abstractions (other distros, BSDs, …) without explicit user request.

- Clipboard: auto-detected at startup in `src/tui_client/clipboard.rs` — prefers `wl-paste` / `wl-copy` on Wayland, falls back to `xclip` then `xsel` on X11
- Power events: listens to `org.freedesktop.login1.Manager.PrepareForSleep` via zbus (display-server-agnostic)
- TUN interface: created by sing-box; requires root privileges

---

## Code Conventions

### Comments and Self-Documenting Code
- Do not add line, block, or documentation comments to new or modified code.
- Make code self-documenting through precise names, small focused functions, explicit types, and clear structure.
- If intent is unclear without a comment, refactor the code until the intent is evident instead of explaining it with a comment.
- Do not remove existing comments unless their removal is directly required by the task.

**Exception — actionable markers.** `TODO`, `FIXME`, `HACK`, `PERF` and
`NOTE` comments are allowed and are not covered by the rule above. They record
work, risk or a constraint that does not live in the code, so a marker never
substitutes for a clearer name or a smaller function.

Each marker states what has to change and why it is not done yet. Name the
blocker when there is one — a protocol break, a cross-repository release, an
upstream fix. A marker that only says something is wrong is not useful:

```rust
// FIXME: sing-box 1.13 renames this field; rename after the PKGBUILD bump.
// TODO: fold into StateSnapshot once IPC_VERSION can be bumped together
// with the omakvn plugin, which reads status_is_error.
// PERF: re-parses every line on each tick; batch once the log pane paginates.
// NOTE: sing-box exits 0 on an occupied controller port, so a conflict only
// surfaces on run.
```

Rules of thumb:

- Use `TODO` for deferred work, `FIXME` for a known defect, `HACK` for a
  deliberate workaround, `PERF` for a known cost worth revisiting.
- `NOTE` is for a constraint a reader cannot derive from the code — external
  behavior, an upstream quirk, a protocol rule. It is not a licence to
  restate what the code does; if a `NOTE` explains the code itself, the rule
  above applies and the code should be clearer instead.
- Do not use a marker to defer work the current change should finish, or to
  park a decision the reviewer needs to see. Say it in the PR instead.
- Record debt that outlives one change in the plan document as well, so it
  stays visible outside the source.
- Keep markers current: delete one when its work lands, and update it when
  the blocker changes.

### Error Handling
- Use `anyhow::Result<T>` for fallible functions at the application / UI boundary.
- Use `thiserror` only if you need structured error enums (rare in this codebase).
- Prefer `.context("...")` and `.with_context(|| format!("..."))` to add descriptive messages.

### File I/O
- **Atomic writes are mandatory** for config files. Pattern: write to `.tmp`, then `fs::rename`.
- See `config::save_config_at` and `geo::GeoManager::write_atomic` for the canonical implementation.

### Logging
- Use `tracing::info!`, `tracing::warn!`, `tracing::error!` — not `println!`.
- The subscriber is initialized in `main.rs` with `EnvFilter` and `fmt::layer().without_time()`.

### Serialization
- All persistent data uses `serde` + `serde_json`.
- Config file: `profiles.json` (top-level `Config` struct with `profiles: Vec<Profile>` and `settings: Settings`).
- `Profile` stores common fields (`id`, `name`, `address`, `port`) plus `#[serde(flatten)] config: ProtocolConfig`. `ProtocolConfig` is an internally-tagged enum (`#[serde(tag = "protocol", rename_all = "lowercase")]`) with one variant per protocol (11 total). VLESS keeps its protocol-specific fields flat on `VlessConfig` for backward compatibility with configs written before the multi-protocol refactor.
- Shared TLS/transport types: `TlsCommon` (SNI, ALPN, fingerprint, insecure, `EchSettings`, `RealitySettings`), `TransportConfig` (WebSocket, gRPC, HTTP upgrade). ECH and REALITY are mutually exclusive and enforced in `ProtocolConfig::validate`.
- `Profile::dedup_key()` returns a stable string key (`"protocol:credential@host:port"`) used by the subscription importer to detect duplicate profiles across imports.
- Enums use `#[serde(rename_all = "snake_case")]` or `"lowercase"` as appropriate.

### Formatting & Linting
- Run `cargo fmt` before committing to keep the codebase consistent.
- Run `cargo clippy --all-targets --all-features` and fix any warnings before committing.
- The project uses the default `rustfmt` configuration (no `rustfmt.toml`).

### Naming
- Modules are snake_case (`singbox`, not `sing_box`).
- The binary name is `kvn-tui`; the crate name is `kvn-tui`. The package installs `/usr/bin/kvn` as a symlink to it, so user-facing docs and commands say `kvn`.

### Module Files
- Use the **new Rust module style**: a module `foo` with submodules lives in `src/foo.rs` (parent) and `src/foo/bar.rs` (children). Do **not** create `src/foo/mod.rs`; that is the old style and is not used in this project.

---

## Testing Patterns

- Tests are co-located in `#[cfg(test)] mod tests` blocks at the bottom of each source file.
- `src/test_helpers.rs` provides shared test utilities (e.g., `model_with_profiles`).
- Tests should not depend on external network or the `sing-box` binary unless explicitly marked `#[ignore]` (or guarded by a `command -v sing-box` runtime check).
- Use `tempfile` for file-system tests; use `NamedTempFile` / `tempdir()` for isolation.
- Tests that mutate process environment (`std::env::set_var`) **must** lock `crate::test_helpers::ENV_LOCK` to serialize against other env-touching tests. The same lock is required for tests that *read* the environment non-atomically — spawning a child process (execve/PATH lookup reads the env) or resolving an env-dependent path more than once — otherwise they race the mutating tests and fail intermittently.
- Snapshot tests use [insta](https://insta.rs/). Regenerate with `INSTA_UPDATE=always cargo test`, then review the diffs before committing; with `cargo-insta` installed, `cargo insta review` walks them interactively instead. Pending `.snap.new` files are gitignored.
- The CI `test` job follows `cargo test --locked` with `cargo insta test --check --unreferenced=reject`, so a snapshot orphaned by a deleted or renamed test fails the build. `.config/insta.yaml` applies the same policy to local `cargo insta test` runs.
- Test visual TUI rendering — dimensions, alignment, spacing, styles, and rendered text or row order — only with `insta` snapshots. Do not add granular unit tests that inspect coordinates, widths, styles, helper output, or individual cells in a `ratatui::Buffer`.
- Pin colors and modifiers with `test_helpers::buffer_to_styled_string`, which appends a per-cell style map and its legend to the rendered text; `snapshot_terminal` keeps the snapshots that are about layout readable. Do not assert on `Cell::style()` at a computed index — that is the granular style inspection the rule above rules out.
- Render snapshots at `test_helpers::APP_WINDOW_COLS` × `APP_WINDOW_ROWS` (113×35) unless the test is about a specific size. That is the grid the app actually opens with: the Omarchy apps menu launches it through the `floating-window` Hyprland tag, which sizes the window to 875×600 logical pixels, and Foot fills those with 113×35 cells at the default font. The column count shifts with monitor scale and font size; the row count is stable. Sizes that carry meaning of their own — `MIN_TERMINAL_WIDTH`/`MIN_TERMINAL_HEIGHT` for the smallest supported window, `TWO_PANE_MIN_WIDTH - 1` for the single-pane layout — stay explicit.
- Use regular unit tests for functional behavior only: input handling, model transitions, persistence, emitted effects, validation, and business logic.
- When changing existing UI rendering, update the relevant snapshot. Add a new snapshot only when no existing snapshot covers the state being changed.
- When changing existing logic, test only what the change actually changed. Do not add a test — or an assertion inside a new test — that re-verifies behavior the change left alone; the existing tests already cover it, and the duplicate only makes the diff look bigger than the change. Rebind or rename the existing test instead when a change moves behavior from one input to another.
- A behavior is asserted at exactly one layer — the one that implements it. A test of an outer layer asserts only what that layer adds, never the inner layer's semantics again: `handle_ipc_command` tests assert that the command reaches the shared `commit_*` helper and that `Effect::BroadcastState` is appended, while `update/key/regions.rs` owns the per-region commit semantics; `generate_config` tests assert composition, while the `build_route` / `build_dns` tests own block contents; `fetch_subscription` tests assert the wire round-trip, while the `build_request_headers` and `hwid_response_error` tests own header and message contents.
- Example pattern: create a default `Profile`, generate a config, assert on JSON structure.

### Coverage Policy

- **Minimum total coverage is 85 %** on both regions and lines. CI enforces this in the `coverage` job — see `.github/workflows/ci.yml`.
- Any change that lowers total region or line coverage below 85 % must add tests in the same PR to bring it back up. A code change that drops coverage is not "done."
- Check locally before pushing:

  ```bash
  # Requires cargo-llvm-cov (install once: cargo install cargo-llvm-cov)
  # and llvm-tools-preview. On distros without the rustup component, set
  # LLVM_COV=/usr/bin/llvm-cov LLVM_PROFDATA=/usr/bin/llvm-profdata.
  cargo llvm-cov --summary-only
  ```

  The `TOTAL` line shows region / function / line coverage. Both region and line numbers must be ≥ 85 % for CI to pass.
- The CI gate parses the `TOTAL` line directly because `cargo-llvm-cov --fail-under-*` flags are silently no-op in the 0.8.x series.
- 0 %-coverage I/O wrappers (`daemon.rs` and its `daemon/` submodules, `tui_client.rs` and its `tui_client/handler/` submodules, `main.rs`, `services/killswitch.rs`, `services/suspend.rs`, `tui_client/clipboard.rs`, `tui_client/theme_watch.rs` watcher thread, `singbox/clash_api.rs`, `systemd.rs`, install_* in `cli.rs`) are accepted as-is — they wrap subprocesses, DBus, Unix sockets, HTTP, and filesystem watchers, which need integration harnesses out of scope for unit tests. **Do not rewrite them just to add fake-based tests.** Cover new logic with pure-function tests instead.

---

## Key Design Decisions

### TEA Architecture
The application follows **The Elm Architecture (TEA)**:
1. **Model** (`app/model.rs`) holds all application state as pure data. UI state is split into `Overlay` (popup/modal) and `ConnectionState` (idle/connecting/connected).
2. **Messages** (`app/msg.rs`) represent every external event — keyboard input, timer ticks, log lines, geo updates, system resume.
3. **Update** (`app/update.rs` and its `app/update/` submodules) is a pure function `update(model, msg) -> Vec<Effect>`: no I/O, no threads, no system calls. All business logic lives here; `update.rs` itself only dispatches each `Msg` to a submodule handler.
4. **Effects** (`app/effect.rs`) are declarative descriptions of side effects (`Connect`, `DownloadGeo`, `SaveConfig`, `Quit`, etc.).
5. **Daemon** (`daemon.rs` + `daemon/`) owns the canonical `Model`, the `mpsc` channel, the sing-box `process_slot`, and all background services (ticker, suspend watcher, log tailer, IPC server). It exposes a Unix domain socket IPC server (`ipc.rs`) that accepts NDJSON commands from TUI clients.
6. **TUI Client** (`tui_client.rs`) connects to the daemon socket, enters the alternate screen, renders the UI using ratatui, and forwards keyboard input (plus clipboard/editor actions) as IPC commands. It has its own local `Model` that is kept in sync via `StateSnapshot` broadcasts from the daemon.
7. **IPC Protocol** (`ipc.rs`) uses newline-delimited JSON over a Unix socket. Commands: `Attach`, `Detach`, `Key`, `SelectSource`, `SetMainPaneFocus`, `GoFirst`, `ConnectProfile`, `Disconnect`, `Reconnect`, `SetRoutingMode`, `SetGeoRegion`, `SetKillSwitch`, `SetAutoConnect`, `CheckOnboarding`, `CheckSupportPrompt`, `Paste`, `Copied`, `ReloadConfig`, `Quit`, `ClientError`. Responses: `StateSnapshot` pushed by the daemon after every state change. The semantic commands (`ConnectProfile` through `SetAutoConnect`) exist for non-TUI clients — the Omarchy Quickshell module and the `kvn status/connect/disconnect/reconnect/toggle` CLI subcommands. Overlay commits (routing mode, geo region) are shared between the key handlers and IPC via `commit_routing_mode` / `commit_geo_region` in `update/routing.rs` so both paths run identical logic.

This separation makes the update tree fully synchronous and trivial to unit-test.

### Background Services
Background work is executed in dedicated threads spawned by the **daemon** (`daemon.rs`):
- **Ticker** — sends `Msg::Tick` every 250 ms to drive connection state machines.
- **Suspend watcher** — `services/suspend.rs` runs a blocking zbus listener that sends `Msg::SystemResumed`; the daemon auto-reconnects on resume even when no TUI is attached.
- **IPC server** — `ipc.rs` accepts Unix socket connections from TUI clients, parses NDJSON commands, and forwards them as `Msg::IpcCommand` into the daemon's mpsc channel.
- **Effects** — `Connect`, `DownloadGeo`, and `PasteClipboard` (via `IpcCommand`) each spawn a short-lived thread that sends the result back via the daemon's channel.
- **Log tailer** — `LogTailer` (`services/log_tailer.rs`) reads new lines from the shared log file on every `Tick` inside the daemon. App status messages are also written to the same file (with an `[app]` prefix) so both sing-box and app logs are visible in the TUI log panel.
- **State I/O** — `services/waybar.rs` writes `state.json` on connect/disconnect for waybar integration.

The **TUI client** (`tui_client.rs`) additionally spawns:
- **Event reader** — polls `crossterm` events and sends `Msg::Key` / `Msg::Resize` to the local TUI channel. Reading can be paused while `$EDITOR` is open.
- **Ticker** — sends `Msg::Tick` every 250 ms to drive the local log tailer.
- **IPC reader** — reads NDJSON state snapshots from the daemon socket and forwards them as `Msg::StateUpdate`.

### sing-box Config Generation
- `singbox::config::generate_config` builds a complete sing-box 1.12+ JSON object from a `Profile` and `Settings`.
- The config is written atomically with mode `0600` to `$XDG_RUNTIME_DIR/kvn-tui/singbox.json`, validated with `sing-box check`, and only then is `sing-box run` spawned. The private `kvn-tui/` runtime directory has mode `0700`; a desktop user session with `XDG_RUNTIME_DIR` is required.
- The Clash API `external_controller` port is allocated per start by `net::allocate_loopback_port` and passed into `generate_config`; it is never a fixed number and never a user setting. `sing-box check` does **not** validate bindability (it exits 0 against an occupied controller port), so a conflict only appears when `sing-box run` fails. `runner::start` therefore retries up to `CLASH_API_START_ATTEMPTS` (3) times with a fresh port, but only when the failure names `address already in use` for that exact port — every other failure is returned immediately, and an exhausted retry keeps the original stderr in the error chain. The chosen port lives on `ProcessHandle`, so it cannot outlive the process; the daemon reads it from the process slot when executing `Effect::FetchTrafficStats`.
- If the process exits immediately, stderr is captured and surfaced to the user.
- `build_outbound(profile)` dispatches to a per-protocol builder and returns `Vec<serde_json::Value>` (most protocols return one outbound; ShadowTLS returns two — a `shadowtls` wrapper tagged `shadowtls-wrap` plus a `shadowsocks` detour tagged `proxy`).
- Shared helpers: `build_tls_block` (TLS + ECH + REALITY), `build_transport_block` (WebSocket / gRPC / HTTP upgrade). No deprecated sing-box fields (no `obfs_password`, no `aes-128-cfb`, no top-level `dns.fakeip`, no WireGuard outbound).

### Routing Modes
- `RoutingMode::Global` — all traffic through VPN.
- `RoutingMode::BypassRu` — RU IPs/domains bypass VPN (direct).
- `RoutingMode::OnlyRu` — only RU IPs/domains go through VPN; everything else is direct.
- `RoutingMode::BypassCn` — CN IPs/domains bypass VPN (direct).
- `RoutingMode::OnlyCn` — only CN IPs/domains go through VPN; everything else is direct.
- The available routing modes depend on the selected **geo region** (`Ru`, `Cn`, `Ir`, or `Global`). `RoutingMode::available(region)` returns the list dynamically.
- Geo-region and routing-mode preferences are grouped under `settings.geo_routing: GeoRouting`. It stores `current_region: Option<GeoRegion>` and `selected_region_modes: HashMap<GeoRegion, RoutingMode>`. The active mode is derived from `selected_region_modes[current_region]` and falls back to `Global`. Switching back to a previously used region restores its last routing mode.
- Rule-sets are local `.srs` binary files downloaded to `~/.config/kvn-tui/geo/`.
- **Service routing overrides** (`geo_routing.service_routes: HashMap<RoutedService, ServiceRoute>`, absent = `Disabled` / opt-in): orthogonal to the routing mode — each of the predefined services (`Steam`, `Telegram`) can be forced to `Direct` (real network location; e.g. Steam CDN downloads) or `Proxy` (always through the tunnel, even under `Bypass`). `build_route` emits the per-service `rule_set → outbound` rules ahead of the geo rules so an override wins in every mode, iterating `RoutedService::ALL` (never the map — HashMap order is nondeterministic). Assets are declared in `geo::service_assets()` as a `ServiceAssets { geoip: Option<GeoAsset>, geosite: Option<GeoAsset> }` descriptor per service, all sourced from MetaCubeX/meta-rules-dat (one provider, one branch layout). They are fetched *through the tunnel* (`Effect::DownloadServiceRuleSetsIfMissing`) — never pre-connect, where the kill switch or ISP blocks would stall the fetch — and refreshed with the periodic geo updates. Two triggers: after `Msg::Connected` (backstop), and on a service-routing commit while connected, where the reconnect is DEFERRED until the download pass reports back (`Model::pending_service_reconnect` → `Msg::ServiceRuleSetsReady`) so a first-enabled service's rules are live on the very next connection rather than requiring a second reconnect. Missing files degrade to "no rule for that service", never a failed connection. Edited via the `S` overlay (draft map in `Model::service_routing_draft`; cycling a route back to `Disabled` removes its entry — absent = Disabled — so a full cycle commits as a no-op; committed atomically on Enter).

### Share-Link Parsing
- Entry point: `config::profile::parse_share_link(uri)` dispatches on the URI scheme.
- Supported schemes: `vless://`, `vmess://`, `trojan://`, `ss://`, `hysteria2://`, `hy2://`, `tuic://`, `shadowtls://`, `anytls://`, `socks://`, `socks5://`, `http://`, `https://`, `ssh://`.
- All supported schemes are listed in `SUPPORTED_SHARE_SCHEMES` (used by both dispatch and the subscription Base64 heuristic in `config::subscription`).
- VLESS: extracts UUID, host, port, fragment (name), `flow`, `security`, `fp`, transport type, and REALITY params (`pbk`, `sid`, `sni`, `spx`). ECH config also parsed when present.
- VMess: handles both base64-JSON (v2rayN / Shadowrocket) and inline URI forms.
- Shadowsocks: handles SIP002 (`ss://base64(method:password)@host:port`) and legacy fully-base64 forms.
- Hysteria 2: `hy2://` is an alias for `hysteria2://`.
- SOCKS: `socks5://` is an alias for `socks://`.
- TLS parameters shared across protocols (VLESS excluded — keeps fields flat for backward compat): `TlsCommon` with SNI, ALPN, fingerprint, insecure, ECH (`EchSettings`), and REALITY (`RealitySettings`). ECH and REALITY are mutually exclusive.
- Transport (WebSocket / gRPC / HTTP): `TransportConfig` shared across VLESS, VMess, Trojan, AnyTLS.

### Suspend / Resume
- `services/suspend.rs` runs a blocking zbus listener in a dedicated thread. On resume (`PrepareForSleep` with `false`), it sends `Msg::SystemResumed` through the `mpsc` channel so `update/connection.rs` can schedule a reconnect effect.

### Kill Switch
- Uses **nftables** + a systemd unit (`kvn-tui-killswitch.service`) that loads `/etc/kvn-tui/killswitch.nft`. The ruleset drops all outbound traffic except localhost, `kvn*` interfaces, and packets marked `0x29a` by sing-box.
- Privilege escalation via **sudoers NOPASSWD** (not polkit) — grants the `network` group passwordless access to `/usr/lib/kvn-tui/killswitch-helper.sh`. Installed with `sudo kvn setup --killswitch`.
- **Toggle flow**: `Shift+K` keybinding → `Effect::ApplyKillSwitch { enabled }` → daemon spawns thread calling `services::killswitch::apply(enabled)` → sends `Msg::KillSwitchApplied { enabled, error }` back. On success the boolean is flipped and config is saved; on error the boolean is unchanged and the error is shown. Disabling from the keybinding first opens `Overlay::ConfirmDisable(DisableTarget::KillSwitch)` (see § Disable Confirmation); the Connection settings screen and IPC go straight to `set_kill_switch`.
- **Group check on enable**: `apply(true)` reads `integration_group_status()` (`Active` / `PendingActivation` / `NotMember`). A pending group asks the user to reboot (logging out is not enough: `kvn-tui.service` inherits groups from the long-lived `systemd --user` manager); a missing membership points to `sudo kvn setup --killswitch`. `kvn doctor` reports the same two cases.
- **Reconciliation on startup**: daemon queries systemd to check whether the unit is actually active and aligns `settings.kill_switch` with the real state, preventing drift if the unit was manually disabled or the helper was uninstalled.
- **Outdated files**: the installed ruleset, unit, helper, and sudoers rule come from `integration_files`. `kvn doctor` fails when they differ from the embedded payloads. The sudoers file is unreadable to users, so it is checked through `/var/lib/kvn/integrations/killswitch-sudoers.sha256`; the polkit rule is checked the same way through `polkit.sha256`.
- **Disable without helper**: if the helper is missing and the unit is already inactive (e.g. after `sudo kvn clean --killswitch` with the daemon still running), disabling succeeds without calling the helper and just clears `settings.kill_switch`.
- **Handshake window**: before spawning `sing-box run`, the daemon pre-resolves the VPN endpoint's IP addresses via DNS and adds them as temporary nftables exceptions (`allow <ip> tcp <port>`), ensuring the initial TLS/REALITY handshake is not blocked. Every non-`local`, non-`fakeip` DNS upstream from `settings.dns.servers` is also resolved and allowlisted with its protocol-appropriate port (UDP/53, TCP/53, DoT/853, DoH/443, DoQ/853) so sing-box's bootstrap resolver can reach the user-configured DoH/DoT endpoint instead of a hard-coded 1.1.1.1. These exceptions are revoked on disconnect.
- **sing-box integration**: all sing-box packets carry `default_mark=666` (fwmark `0x29a`); the nftables rule `meta mark 0x29a accept` lets them through. This ensures Bypass/Only geo-routing modes work correctly even with the kill switch active.
- **UI**: the status bar shows a `[KS]` badge when the kill switch is enabled.

### DNS Configuration
- **Data model**: `settings.dns: DnsConfig` holds `servers: Vec<DnsServer>`, `rules: Vec<DnsRule>`, `final_server: String`, `strategy: DnsStrategy`, `fakeip_enabled: bool`. Server variants map 1:1 onto sing-box 1.12 server types: `Local`, `Udp`, `Tcp`, `Tls` (DoT), `Https` (DoH, with optional `path`), `Quic` (DoQ), `FakeIp` (with `inet4_range` / `inet6_range`).
- **Validation** (`Config::validate`): server tags are non-empty and unique, `final_server` and every `rule.server` reference an existing tag, and when `fakeip_enabled` at least one `FakeIp` server is present.
- **Legacy migration**: the old `settings.dns_strategy` field is still deserialized; the ordered `Config::migrate_to` schema steps promote it into `dns.strategy` if the new value is still at its default. `load_config_at` runs those steps implicitly and persists the result; a schema newer than the build supports is refused. On save, `save_config_at` mirrors `dns.strategy` back into `dns_strategy` so older kvn builds keep loading the file.
- **Config generation** (`singbox::config::build_dns`): emits the modern sing-box 1.12 schema — no legacy top-level `dns.fakeip` block; the fake-IP server carries its own ranges. When `fakeip_enabled` is set and a `fakeip` server exists, the builder auto-prepends an `{ query_type: ["A","AAAA"], server: <tag> }` rule (skipped if the user already added one), flips `dns.independent_cache: true`, and sets `experimental.cache_file.store_fakeip: true` so the IP→domain map survives restarts.
- **TUI overlay** (`Overlay::DnsSettings`, key `D`): six rows — four presets (Cloudflare DoH `1.1.1.1`, Google DoT `8.8.8.8`, Quad9 DoH `9.9.9.9`, system `local`), a strategy cycle, and the fake-IP toggle. Custom servers and per-domain rules are edited in `profiles.json` via `e`.
- **Strategy draft**: `Model::dns_strategy_draft: Option<DnsStrategy>` previews strategy changes while the overlay is open. `h` / `l` on the Strategy row cycle the draft (`DnsStrategy::prev` / `next`); the label renders as `Strategy: ‹ value ›` with a trailing `*` when the draft differs from the saved setting. Enter on the Strategy row commits the draft (clears it, triggers `SaveConfig` + reconnect-if-connected); Esc/q discards it.
- **Active-state detection**: `DnsPreset::detect` structurally matches `dns.servers + final_server` against the four presets, ignoring any fake-IP server alongside; `draw_selection_modal` (in `ui/layout/overlay/popup.rs`) paints the active item bold green via `Theme::success()`. This generic active-index parameter is also used by the routing-mode and geo-region overlays.
- **Status bar**: a `[DNS: <kind>]` badge derives its label from the final server's `kind_label` (`DoH` / `DoT` / `DoQ` / `UDP` / `TCP` / `local`) or `fakeip` when `fakeip_enabled` is true.

### Theme System
- **Data**: every UI style is derived from a `Palette` (16 ANSI colors + 6 semantic colors: accent, cursor, foreground, background, selection_foreground, selection_background). `Theme` holds a `Palette` and exposes `&self` methods (`accent`, `normal`, `status`, `error`, `success`, `border`, `selected`, `selected_connected`, `popup_bg`, `background`).
- **Bundling**: `themes/*.toml` contains all 22 Omarchy 4 semantic palettes, vendored from `/usr/share/omarchy/themes/<name>/colors.toml`. `build.rs` derives the ANSI and UI fields and compiles them into `OUT_DIR/bundled_palettes.rs` (build-dep `toml`). No runtime TOML parsing — `Palette::lookup(slug)` is a static array scan.
- **Active theme resolution**: `tui_client::theme_watch::resolve_active(slug)` is the single source of truth, called both at startup and on `Msg::ThemeChanged`. The reserved slug `"omarchy"` reads `$XDG_STATE_HOME/omarchy/current/theme.name`; any other slug looks up a bundled palette (with `Theme::legacy()` as the fallback for unknown names).
- **In-TUI picker** (`Overlay::ThemeSettings`, key `C`): mirrors the DNS overlay draft pattern. `j`/`k` update `Model.theme_selected` and `Model.theme_draft`; the TUI client recomputes `model.theme` from the draft on every snapshot apply (live preview). Enter persists `settings.theme = <slug>` and emits `Effect::SaveConfig`. Esc clears the draft and reverts. The Auto-entry (slug `"omarchy"`) is shown only when `detect_omarchy_theme()` returns `Some` — non-Omarchy users see only the 22 bundled palettes.
- **Settings › Interface** (`Space i`): the Theme row cycles the same `theme_picker_slugs()` with `h`/`l` into `Model.theme_draft`, so the live preview path is shared with the `C` picker; the Icons row drafts `settings.icons` into `Model.interface_settings_draft`, which `Model::icon_set()` prefers while rendering. Enter commits both with one `Effect::SaveConfig`; Esc/Backspace discard both drafts.
- **Watcher**: spawned only when the Omarchy `current/` state directory exists. It watches that directory because theme updates replace files and subtrees within it atomically. Emits `Msg::ThemeChanged(Theme)` to the TUI channel. The update reducer applies it only when `settings.theme == "omarchy"`; manual picker overrides win.
- **Frame background**: `draw()` paints the whole `frame.area()` with `theme.background()` before any other widget so cells with `Style::default()` (no explicit `bg`) inherit the palette color instead of falling through to the terminal default. Popups continue to use the same color via `theme.popup_bg()`; border-only blocks only set `fg`, so the fill survives.
- **Terminal defaults (OSC 10/11)**: ratatui can't reach the pixel padding between the character grid and the window border, and cells using `Style::default()` inherit the terminal foreground. `tui_client::apply_terminal_colors` emits OSC 10 and OSC 11 so both defaults follow the active palette. It runs at startup and whenever either effective color changes. On exit OSC 110/111 restore the user's terminal defaults. All calls are guarded by `io::stdout().is_terminal()` to stay silent in pipes/CI.

### State I/O
- `services/waybar.rs` writes a small JSON file (`state.json`) on every connect/disconnect. It stores connection status, active profile name, and sing-box PID.
- Used by the `--waybar-status` CLI flag and for crash recovery (state is cleared on startup).

### Daemon + TUI Client Architecture
- **Daemon** (`kvn --daemon`) runs headless. It owns the sing-box process, config, geo updates, suspend/resume handling, and log tailing. It binds a Unix domain socket for IPC.
- **TUI Client** (`kvn`) connects to the daemon socket, requests a state snapshot (`Attach`), enters the alternate screen, and renders the UI. Keyboard input is forwarded to the daemon as `IpcCommand::Key` (except `p` and `e`, which are handled locally because they need terminal/clipboard access). Bracketed paste (`\x1b[?2004h`) is enabled so a terminal paste arrives as a single `Msg::Paste` and is sent as `IpcCommand::Paste` instead of being decoded as shortcut keys.
- Pressing `q` (or `Esc`) when no overlay is shown sends `Detach` to the daemon, leaves the alternate screen, disables raw mode, and **exits the TUI process**. The daemon and sing-box keep running. Shell regains the prompt immediately because the foreground TUI process actually exits. If an overlay is open (Help, ConfirmDelete, RoutingMode, GeoRegions, Error), `q`/`Esc` is forwarded to the daemon as a normal key, which closes the overlay. `Overlay::RestartRequired` is the exception: it is modal, ignores `q`/`Esc`, and takes only `Enter` (exit and `systemctl --user restart kvn-tui.service`) or `Ctrl+C` (stop the daemon).
- Pressing `Ctrl+C` sends `Quit` to the daemon. The daemon stops sing-box, cleans up the Unix socket, and exits. The TUI waits briefly (300 ms) for cleanup to complete before exiting.
- Running `kvn` again connects to the same daemon and re-attaches, restoring the TUI instantly without restarting sing-box.
- The IPC protocol is NDJSON over a Unix socket. The daemon pushes a full `StateSnapshot` after every state change. The snapshot includes the complete config (`profiles` and `settings`) so the TUI client always renders the current data.
- `handle_ipc_command` unconditionally appends `Effect::BroadcastState` to every IPC command result, ensuring the daemon always pushes state after user interaction.
- `handle_geo_result`, `Msg::ConnectFailed`, and the `handle_tick` idle fallback also append `Effect::BroadcastState` so state mutations that don't produce other broadcast-triggering effects are still visible to the TUI.

### Geo Region Selection
- `settings.geo_routing.current_region` (`Option<GeoRegion>`) controls which country rule-sets are downloaded and which routing modes are shown.
- `GeoRegion::Ru` — download RU geoip/geosite, enable `Global` / `BypassRu` / `OnlyRu`.
- `GeoRegion::Cn` — download CN geoip/geosite, enable `Global` / `BypassCn` / `OnlyCn`.
- `GeoRegion::Global` — skip geo downloads, only `Global` mode is available.
- A region is mandatory: while `geo_routing.current_region` is `None` the region picker refuses `q`/`Esc`, so the main UI stays unreachable. On a brand-new install the first-run tour (see § First-Run Onboarding) owns that first choice and hands off to the same picker; the bare picker is still forced directly once the tour is complete but no region was chosen.
- The region can be changed at runtime with the `o` keybinding. When the region changes, the previous region's mode is saved into `geo_routing.selected_region_modes` and the new region's previously stored mode is restored (falling back to `Global`).

### First-Run Onboarding

- On a brand-new install `kvn` opens a guided tour instead of the bare region
  picker. Cards in order: `Welcome`, `Region`, `Routing`, `Profiles`,
  `Connected`, `Doctor`, `Omarchy`, `AutoConnect`, `KillSwitch`, `Finish`
  (`onboarding::OnboardingStep::ORDER`). `Connected` acknowledges the first
  working tunnel and separates the "get it running" half from the "tune it"
  half. `Omarchy` precedes the two protection cards on purpose:
  `kvn setup --omarchy` clones the plugin from GitHub, and a kill switch enabled
  one card earlier leaves the machine without internet until the reboot its
  pending group membership needs — the user could not finish that step, nor turn
  the kill switch back off before rebooting.
- **`Omarchy` is conditional**, so the tour is ten cards only on an Omarchy
  desktop that does not have the bar plugin yet, and nine otherwise.
  `OnboardingStep::sequence(include_omarchy_card)` filters `ORDER`, and `index` / `total` /
  `next` all read from it, so the `4/10` counter, the advance order and what is
  rendered cannot disagree. The flag is `OnboardingProgress::include_omarchy_card`
  — it says whether the card is in the sequence, not whether Omarchy is present,
  and an installed plugin makes it `false` — decided
  once per process by `model::show_omarchy_card()` —
  `omarchy::detect_omarchy_theme().is_some() && !omarchy::omakvn_plugin_installed()`,
  two cheap file reads of the kind `theme_picker_slugs` already does. It is an
  environment fact and never persisted, but it *is* sent over IPC as
  `StateSnapshot::onboarding_omarchy`: the card's own command installs the plugin
  that removes the card, so a client started after `kvn setup --omarchy` would
  otherwise count a shorter tour than the daemon and render `Omarchy` with a
  nonsense counter. The daemon's answer wins; `Model::from_config`'s own guess is
  only the pre-attach default. `OnboardingProgress::new` normalises a step
  recorded as `Omarchy` but resumed without that card.
- Progress lives in `$XDG_STATE_HOME/kvn/onboarding.json` as
  `OnboardingState { step, completed_at }` — deliberately outside the versioned
  `profiles.json` schema, like the support prompt. `Model.onboarding` wraps it in
  `OnboardingProgress`, adding the session-local `awaiting` (a handoff is in
  flight) and `card_pending` (a result arrived; show the card when the screen is
  free). The client reads the active card from `Overlay::Onboarding(step)`;
  `awaiting` is carried separately as `StateSnapshot::onboarding_awaiting`
  because the pickers render differently while the tour holds them.
- An install that already has a geo region is **grandfathered**:
  `onboarding::load_for_daemon` records it complete and persists that, so nobody
  who already uses kvn sees the tour.
- **Card keys**: `Enter` performs the step and moves on; there is no skip, no way
  back and no early exit, so every step is taken exactly once. The card is modal
  like `Overlay::RestartRequired` — `q`/`Esc` and every other key fall through to
  a no-op. The two protection cards additionally answer their own toggle,
  `Shift+A` on `AutoConnect` and `Shift+K` on `KillSwitch`, which is what their
  copy tells the user to press. They call `set_auto_connect` / `set_kill_switch`
  rather than the main screen's `toggle_*` helpers: `toggle_*` opens
  `Overlay::ConfirmDisable` when turning a protection off, which would replace
  the card and then close to `Overlay::None`, stranding the tour. A card about
  one setting is explicit enough to skip that dialog, as `Space c` already is.
  A card that shows a shell command answers `y`, which copies it, and the
  `Profiles` card answers `p`, which imports from the clipboard exactly as the
  Profiles list does. Both are handled client-side in
  `tui_client/handler/key/onboarding.rs` because the clipboard is. The card
  renders `OnboardingStep::command(model.integration_setup)`, which spells a
  setup command exactly as the README does — with the `pacman -S --needed`
  install its package needs, over two lines on a `\` continuation, since a paste
  has to work on a machine that has neither `polkit` nor `nftables`. `y` copies
  `clipboard_command`, the same string folded back into one `&&` line: a `\`
  continuation survives a shell but not every terminal's bracketed paste. The
  copy is derived from what is rendered, so the two cannot diverge, and
  `command` gates both, so a card with no command offers no `y` in its footer.
  The card renders it through `Passage::Command`, which emits one line per
  source line instead of refilling it as prose like the surrounding copy. A card only
  answers a key its own copy tells the user to press: the `Profiles` card asks
  for `p`, so importing must not require dismissing the card first. Unlike `copy_selected`, a failed write is reported through
  `IpcCommand::ClientError`: the command is the step's instruction, and silently
  copying nothing is worse than saying so. The last card answers
  `Space` by opening `Overlay::SettingsMenu(Root)`, since that is what its copy
  tells the user to press; it sets `card_pending` rather than finishing, so the
  card returns once that screen is closed and `Enter` still owns completion.
  `?` works on every card through the intercept in `handle_key`, which restores
  the card via `HelpContext::Onboarding`. `?` shows the ordinary help; the tour has no section of its own. A
  daemon restart mid-tour resumes it on the persisted step via
  `IpcCommand::CheckOnboarding`, which `update/onboarding.rs::resume` answers.
- **The protection cards follow their setup state.** `Shift+A` and `Shift+K` are
  useless until `sudo kvn setup --polkit` / `--killswitch` has run *and* the
  `kvn-tui` group both of them gate on is active. So each card renders its tail
  from `Model::integration_setup`: `SetupState::Missing` → `First, set up …` +
  the copyable command, closing with `Reboot once, then press …` or, when
  `group_active` is already true, just `Then press …` — a session that is already
  in the group needs no reboot, because the sudoers and polkit rules both key on
  it. `PendingReboot` → `… setup is complete. Reboot once to activate it, then
  press …`; `Ready` → just `Press …`. `group_active` lives on `IntegrationSetup`
  rather than in `SetupState` because it is one property of the session shared by
  both cards.
  `doctor::integration_setup` produces the whole struct from the existing signals
  (`polkit_status` + `integration_files::polkit_rule_state` for polkit, the
  installed helper + `outdated_killswitch_files` + `killswitch::integration_group_status`
  for the kill switch and for `group_active`, `omarchy::omakvn_plugin_installed`
  for the Omarchy card), with the pure `polkit_setup_state_from` /
  `killswitch_setup_state_from` mappings owning the rules: an unverifiable polkit
  answer reads as `Missing`, because re-running a setup command is harmless while
  claiming it is done is not, and a polkit denial reads as `PendingReboot` only
  while the group itself is pending activation — with the group already active
  the rule is simply not effective, and a user who is not a member (an install
  done by someone else) has to be added by `setup --polkit`, so both keep the
  setup command instead of being sent to a reboot that cannot help. The state
  reaches clients as `StateSnapshot::integration_setup`.
- **Those cards are re-probed while they are on screen**, because the card tells
  the user to run a command in another terminal and then has to show the result.
  `Effect::CheckIntegrationSetup` → `daemon::connection::check_integration_setup`
  (a thread, like the polkit check) → `Msg::IntegrationSetupChecked`, reduced by
  `update/onboarding.rs::integration_setup_checked`, which stores it and returns
  `Effect::BroadcastState` **only when the value changed** so the poll does not
  push a snapshot per tick. Three triggers: `daemon::probe_integration_setup`
  synchronously at startup while the tour is unfinished (so a card's first paint
  is already right), and `onboarding::probe_visible_card` — called by `advance` /
  `resume` / `open_when_idle` right after they set `Overlay::Onboarding`, and by
  the 250 ms tick. That one function owns both the rule (only `Omarchy`,
  `AutoConnect` and `KillSwitch` report an integration) and the gate: the 2 s
  cadence in `Model::last_integration_check_at`, in the same shape as the
  Clash-API sampler because the probe spawns `pkcheck` / `id`, plus
  `Model::integration_check_pending`, which blocks a second probe while one is
  still out — a probe slower than the interval would otherwise be started twice
  and the two answers could land out of order, restoring a stale state on the
  card. `integration_setup_checked` clears the flag before it compares. Keeping
  both callers on one gate is the point: a card open that did not record the
  timestamp let the very next tick fire a second probe. Ordinary daemons never
  pay for any of it: outside the tour no card is open and the startup probe is
  skipped.
  `doctor::integration_setup` resolves the `kvn-tui` group once per probe and
  threads it into `polkit_status_for` as well as both classifiers, so one `id`
  answer serves the whole struct and the two integrations cannot disagree about
  the session; the classifiers themselves stay separate, since their readiness
  criteria differ.
- **The `Omarchy` card has two texts.** Without the plugin it shows
  `kvn setup --omarchy`; with it (installed during the tour — the card itself
  cannot be dropped mid-process, see above) it confirms `kvn is integrated with
  your Omarchy desktop: the widget is on your bar.` and offers no command, so
  `OnboardingStep::command` returns `None` and the footer loses its `y`.
- **Nothing is marked as already done.** The cards carry no `✓` and no green
  title, and the region and mode pickers pass `active: None` to
  `draw_selection_modal` while the tour awaits them, so no entry is painted with
  `Theme::success()`. In a forward-only tour every card on screen is one the user
  has yet to complete; a "done" marker could only come from state that predates
  the tour, which misleads rather than informs. Those two pickers also drop the
  `q/esc close` action from their footer while awaited, since they refuse it.
- **Handoff and resume.** `update/onboarding.rs` owns six transitions.
  `handoff` opens the real screen through the existing `open_*` helpers (which
  seed the drafts those pages `take()`) and persists nothing, so it cannot fail.
  `advance` and `finish` move the card; `outcome(trigger)` reacts to a real
  screen reporting back; `open_when_idle` runs from the 250 ms tick and shows a
  `card_pending` card only when `model.overlay == Overlay::None`, returning
  `Effect::BroadcastState` so the client actually sees it; `resume` answers
  `IpcCommand::CheckOnboarding`. Triggers are raised at
  each screen's own exit point: `commit_geo_region`, `commit_routing_mode` and
  `Msg::Connected`. Only three steps hand off at all — `Region`, `Routing` and
  `Profiles` — and none of their screens can be abandoned, so there is no
  cancellation path and no `Cancelled` trigger.
- **A TUI attaching mid-tour never cancels a handoff.** `resume` reopens the
  stored card only when nothing is in flight (`awaiting.is_none()`) and the
  screen is free. Clearing `awaiting` instead would strand the tour: a client
  attaching while the first connection is still being made would take the
  `Profiles` step off the wait, and the `Msg::Connected` that follows would then
  find no step to finish. The one case where `resume` does act on a handoff is a
  trigger that has already fired — `tunnel_already_up` — since `Msg::Connected`
  is raised once per connection and cannot repeat. `handoff` checks the same
  thing before opening a screen, so a `Profiles` card reached with the tunnel
  already up (a failed progress write restores it) moves on like an
  informational card instead of waiting for an event that has passed.
- **`Profiles` ends at the first connection, not the first import.**
  `Trigger::TunnelUp` is raised only from `on_connected`; `paste.rs` and the
  subscription result handler deliberately raise nothing. A profile on its own
  proves nothing works, so the step is done once the user has actually
  connected with one — standalone or from a subscription, either way. Since
  there is no way to skip a card, the tour cannot be finished before the app has
  been shown to work once.
- **The region and mode steps cannot be abandoned.** `handle_geo_region` refuses
  `q`/`Esc` while no region is set — unchanged — and additionally while
  `onboarding.awaiting == Some(Region)`; `handle_routing_mode` refuses them while
  `awaiting == Some(Routing)`. Both steps therefore end only by committing, which
  advances the card.
- **`Routing` hands off only where there is a choice.**
  `OnboardingStep::handoff_screen` takes the `Config`: under a country region it
  returns `HandoffScreen::RoutingMode` (the `m` picker, not the full `Space r`
  page), and under `Global` or no region it returns `None`, so `Enter` just moves
  the card on. The card's title and primary action branch the same way. The
  trigger lives in `commit_routing_mode`, placed after its availability check so
  a mode rejected through IPC cannot advance the tour while confirming the
  current mode still does.
- **Failed progress write.** `Effect::PersistOnboarding` carries the whole
  previous `OnboardingProgress` plus an `OnboardingRecovery`, which names what
  has to happen rather than where the transition came from:
  `RestoreCard` (the transition owned the screen) or `WaitForIdle` (a real screen
  is still in front of the user).
  `config_io::restore_onboarding_after_failure` reverts `state`, clears
  `awaiting` (the handed-off screen is gone and will never report again) and
  either restores the card directly or sets `card_pending`;
  `config_io::onboarding_transition` hands the same value to the `SaveConfig`
  error path. It is what makes a failed `finish` recoverable: `finish` may
  navigate to the unescapable region picker, so the card must come back at the
  originating step. Settings the same update already
  committed are never rolled back. On the `SaveConfig` error path in
  `daemon.rs`, the revert is gated on `config_io::onboarding_transition`, read
  before the effect list is filtered — an unrelated failed save must not reopen
  a finished tour.
- **Support prompt.** The 7-day clock starts from `completed_at`, armed by
  `persist_onboarding` right after a successful write; `check_prompt` gates on
  `onboarding.is_complete()`. `support_prompt::load_for_daemon` keeps a
  start-time path only as crash recovery between the two writes.
- **First-install defaults.** `Config::for_first_run` (used only when
  `profiles.json` does not exist) sets `geo_routing.auto_update` to `Every7d`,
  and a pasted subscription gets `Every1d`. Every `Default`/`#[serde(default)]`
  in that chain still means `Off`, so a config that omits the field — or the
  whole `geo_routing` or `settings` section — keeps its current behaviour.

### Disable Confirmation

- Turning auto-connect or the kill switch **off from the main-screen keybinding** (`Shift+A`, the deprecated `a`, `Shift+K`) opens `Overlay::ConfirmDisable(DisableTarget)` instead of applying immediately: both are protections a stray key press should not remove. Turning them **on** applies right away.
- `update/key/confirm_disable.rs` owns both halves: `toggle_auto_connect` / `toggle_kill_switch` open the dialog when the setting is on and no apply is in flight, and `handle_confirm_disable` commits on `y`/`Enter` (delegating to the unchanged `set_auto_connect` / `set_kill_switch` reducers) or closes on `n`/`q`/`Esc`.
- Every other path bypasses the dialog and calls the `set_*` reducers directly: the Connection settings screen (`Space c`, already an explicit two-step edit), IPC `SetAutoConnect` / `SetKillSwitch` (the Omarchy plugin, `kvn disable --killswitch`, `sudo kvn clean --polkit/--killswitch`), and startup reconciliation — headless callers must never block on a TUI dialog.

### Auto-Connect
- `settings.auto_connect` (persisted in `profiles.json`) controls whether the app reconnects to the last used profile on startup.
- `settings.last_connected_profile` stores the UUID of the most recently connected profile. It is updated in `update/connection.rs` on `Msg::Connected` and saved via `Effect::SaveConfig`.
- `Model::new()` calls `resolve_startup_state()` to check `auto_connect` + `last_connected_profile`. If both are set and the profile exists, the model starts in `ConnectionState::Connecting` with that profile pre-selected, and the status bar shows `Auto-connecting to {name}…`.
- The user can toggle `auto_connect` at runtime with the `Shift+A` keybinding (the legacy `a` still works and shows a deprecation toast from `tui_client::handler::key::deprecated_settings_shortcut_message`), the Connection settings overlay, or IPC `SetAutoConnect`. Disabling saves immediately. Enabling first emits `Effect::CheckAutoConnectPolkit` (with `Model::auto_connect_pending` set): the daemon runs `doctor::polkit_readiness()` and replies with `Msg::AutoConnectPolkitChecked`. The flag is flipped and saved only when passwordless polkit is set up and the `kvn-tui` group is active in the daemon's session; otherwise auto-connect stays off and the reason is shown as an error toast and written to the app log.
- `sudo kvn clean --polkit` / `--killswitch` connect to the invoking user's daemon (`/run/user/$SUDO_UID/kvn-tui.sock`) and send `SetAutoConnect`/`SetKillSwitch { enabled: false }`, so the daemon saves the config as the user. If the daemon is not running, startup reconciliation handles it: `reconcile_kill_switch_state` for the kill switch, and `reconcile_auto_connect_state` turns auto-connect off (before the first tick connects) when `doctor::polkit_authorization_denied()`.

---

## Side-effect-free Boundaries

The TEA update function (`app::update::update`) must remain free of I/O, threads, and system calls. Side effects are declared as `Effect` values and executed by the daemon runtime.

Rules of thumb:
- `app::update::update(model, msg) -> Vec<Effect>` must not call functions from `services`, `geo`, `paths`, `atomic_write`, `config::load_config`, `config::subscription`, `singbox::clash_api`, `singbox::runner`, `tui_client::clipboard`, `tui_client::editor`, or perform any file/network/process I/O. **Documented exceptions**: `theme_picker_slugs()` (used by the `C` key handler and `handle_theme_picker`) calls `theme_watch::detect_omarchy_theme()`, which does a single small `fs::read_to_string` of the Omarchy state theme path to decide whether to show the Auto entry; `model::show_omarchy_card()` adds `omarchy::omakvn_plugin_installed()`, one more small read of the plugin manifest, while the model is constructed. Cheap, deterministic, and scoped to a key press; promoted to "OK" because the alternative (caching in `Model`) costs more clarity than it saves. The full `Theme` resolution stays out of `update`: the picker handler only mutates `theme_draft`/`settings.theme`, and the TUI client recomputes `model.theme` via `resolve_active` on snapshot apply.
- `Model::set_status` is pure (mutates only in-memory state). Any message that should also be persisted to the application log must return `Effect::AppendAppLog`.
- `Model::new` is allowed to perform initialization I/O (load config, read `state.json`, etc.).
- `singbox::config::generate_config` is pure: it receives geo file availability (`GeoAvailability`) from the caller and does not touch the file system.
- `ui::widgets::StatusBar::render` reads only `Model` fields; it does not call `GeoManager` or access files.
- The daemon (`daemon::effect::execute_daemon_effect`) is the sole executor of `Effect` values. It dispatches each variant to a `daemon/` submodule. It may perform I/O, spawn threads, and mutate `Model` where appropriate.

If you need to add a new side effect from `update`, add a new `Effect` variant, implement it in the matching `daemon/` submodule, and route it there from `daemon::effect::execute_daemon_effect`.

---

## Configuration Paths

The existing `kvn-tui` paths below are retained for compatibility. Use `kvn`
instead of `kvn-tui` in the path of every new file or directory; do not add new
filesystem resources under the legacy `kvn-tui` namespace.

Keep documentation synchronized with code changes. When a filesystem path,
persistent file, runtime file, command, configuration field, or user-visible
behavior changes, update the corresponding documentation in the same change.

| Resource | Path |
|----------|------|
| Profiles & settings | `~/.config/kvn-tui/profiles.json` |
| Geo rule-sets | `~/.config/kvn-tui/geo/` |
| sing-box logs | `~/.config/kvn-tui/logs/sing-box.log` |
| Temp sing-box config | `$XDG_RUNTIME_DIR/kvn-tui/singbox.json` |
| Runtime state (waybar) | `~/.config/kvn-tui/state.json` |
| IPC socket (daemon ↔ TUI) | `$XDG_RUNTIME_DIR/kvn-tui.sock` |
| First-run tour progress | `$XDG_STATE_HOME/kvn/onboarding.json` |
| Support prompt schedule | `$XDG_STATE_HOME/kvn/support-prompt.json` |
| Applied migration markers | `$XDG_STATE_HOME/kvn/migrations/` |
| Migration backups | `~/.config/kvn-tui/recovery/profiles.json.before-migration-*` |
| Migration lock | `$XDG_RUNTIME_DIR/kvn/migrate.lock` |

---

## Agent Checklist Before Editing

1. Are you preserving atomic file writes for any new config files?
2. Are you using `anyhow::Result` and `tracing` instead of `println!` / `eprintln!`?
3. Are tests added for new public functions, and does `cargo llvm-cov --summary-only` still report **≥ 85 %** region and line coverage?
4. Are you respecting the Arch-only constraint (no support for other distros or BSDs added silently)?
5. Does the sing-box config generation remain valid for sing-box 1.12+?
6. Have you run `cargo fmt` and `cargo clippy --all-targets --all-features` and fixed any warnings?
7. Do all new files and directories use `kvn` rather than the legacy `kvn-tui` path namespace?
8. Does the documentation match every changed path, file, command, configuration field, and user-visible behavior?

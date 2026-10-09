# Agent Guide: kvn

This document contains project-specific context and conventions for AI coding agents. It supplements `README.md` with architectural details, coding styles, and rules of thumb.

Conventions live only in `AGENTS.md` files — there is no `CLAUDE.md`; agents that look for one (Claude Code among them) fall back to `AGENTS.md`. This root file holds the rules for the whole repository. Subsystem details live in nested guides, listed in § Subsystem Guides: read the matching guide before changing that subsystem, wherever the file you edit lives, because not every agent loads nested guides on its own. Put a new convention in the guide of the subsystem it belongs to, or here if it applies everywhere, and never in two places.

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
cargo clippy --all-targets --all-features -- -D warnings  # lint; CI fails on any warning
cargo llvm-cov --summary-only  # coverage report; both region & line totals must stay ≥ 85 % (CI gate)
cargo deny check all           # dependency licenses, advisories, bans, sources (CI gate, deny.toml)
```

CI runs `cargo test --locked` and `cargo llvm-cov --locked`, and the release
workflow builds the published binary with `--locked`, so commit an updated
`Cargo.lock` together with any dependency change.

Every CI job and the release build use one Rust version: `rust-version` in
`Cargo.toml`, installed by `.github/actions/rust-toolchain`. Raise it there,
when Arch ships the new Rust, to move CI and releases together.
`rust-toolchain.toml` keeps local rustup on `stable` with `rustfmt` and `clippy`.

The rules behind each gate live in § Testing Patterns, § Coverage Policy, and § Formatting & Linting.

---

## Module Map

| Module | Path | Responsibility |
|--------|------|----------------|
| `cli` | `src/cli.rs`, `src/cli/clean_all.rs` | CLI argument parsing: `--daemon`, `--waybar-status`, `--version`; `status`/`connect`/`disconnect`/`reconnect`/`toggle` one-shot IPC clients; `enable`/`disable --killswitch`; `disable --support-prompt` (through the daemon's `DismissSupportPrompt` when it runs, otherwise straight into `support-prompt.json`); `doctor`, `migrate [--pending]`, `config {reset,recover}`; `setup` and `clean` for the `--omarchy` / `--polkit` / `--killswitch` integrations (`clean --omarchy` reverts the whole Omarchy integration through `contrib/remove-omarchy.sh`, `clean --omarchy-backups` deletes only its backups); `sudo kvn clean --all` (`clean_all.rs`) stops the invoking user's daemon, runs every integration cleanup, reverts the Omarchy integration as that user through `contrib/remove-omarchy.sh`, and deletes that user's config, state and runtime files |
| `app` | `src/app.rs`, `src/app/model.rs`, `src/app/msg.rs`, `src/app/update.rs`, `src/app/effect.rs` | TEA core: Model, Msg, Update, Effect — pure data, messages, business logic, side-effect declarations |
| `model` | `src/app/model.rs` | Application state (`Model`), overlay + connection state + subscription state, input state — pure data, no side effects |
| `msg` | `src/app/msg.rs` | Message enum (`Msg`) — all external events (keys, ticks, logs, geo, resume, etc.) |
| `update` | `src/app/update.rs` + `src/app/update/` | Pure `update(model, msg) -> Vec<Effect>` — the top-level message dispatcher only; every handler lives in a submodule (see below) |
| `update` submodules | `src/app/update/{status,connection,traffic,config_reload,tick,routing,geo,subscription,paste,onboarding}.rs` | Non-keyboard message handlers, one per concern; see [`src/app/AGENTS.md`](src/app/AGENTS.md) § Module Layout |
| `update::key` | `src/app/update/key.rs` + `src/app/update/key/` | Keyboard input routed by `Model.overlay`, plus `IpcCommand` handling; see [`src/app/AGENTS.md`](src/app/AGENTS.md) § Module Layout |
| `scroll` | `src/app/scroll.rs`, `src/app/scroll/lists.rs` | Pure wheel viewport/selection movement and selectable Profiles/dialog rows |
| `effect` | `src/app/effect.rs` | Effect enum — declarative description of side effects to be executed by runtime |
| `daemon` | `src/daemon.rs` | Headless daemon: owns sing-box process, config, mpsc channel, IPC server, background services; `start` (takes `daemon.lock` before loading the model) / `run` / `run_loop`, `DaemonShared`, `build_snapshot`, and the startup reconciliation of kill-switch and auto-connect state |
| `daemon` submodules | `src/daemon/{effect,connection,geo,config_io,subscription,traffic,profile_test,process_slot}.rs` | Effect execution, one submodule per concern; see [`src/daemon/AGENTS.md`](src/daemon/AGENTS.md) |
| `tui_client` | `src/tui_client.rs` | TUI client orchestration: `run`, the daemon handshake (`connect_to_current_daemon`), terminal setup (`TerminalSession`, OSC colors), snapshot application, and the `run_loop` skeleton that feeds every `Msg` to the handler tree |
| `tui_client` submodules | `src/tui_client/handler.rs` + `src/tui_client/handler/`, `src/tui_client/docs_preview.rs` | `ClientLoop` and its message handlers, client-local keys, docs preview; see [`src/tui_client/AGENTS.md`](src/tui_client/AGENTS.md) |
| `ipc` | `src/ipc.rs` | NDJSON protocol over Unix domain socket for daemon ↔ TUI client communication |
| `migrations` | `src/migrations.rs`, `contrib/migrations/*.sh` | Ordered package migrations: root-owned script discovery, per-user/machine markers written per script, runner lock, `profiles.json` backup, end-of-run daemon restart handoff |
| `test_helpers` | `src/test_helpers.rs` | Shared test utilities (e.g. `model_with_profiles`) |
| `doctor` | `src/doctor.rs` | `kvn doctor` read-only diagnostics, plus the polkit and integration probes the daemon reuses (`polkit_readiness`, `polkit_authorization_denied`, `integration_setup`) |
| `omarchy` | `src/omarchy.rs` | Omarchy detection: active theme name, current-theme paths, Omarchy version, and whether the `yarikov.omakvn` plugin is installed |
| `support_prompt` | `src/support_prompt.rs` | The support prompt's persisted schedule (`support-prompt.json`), outside the versioned config |
| `redaction` | `src/redaction.rs` | Strips credentials, path tokens, queries and fragments from URLs and share links before they are logged |
| `main` | `src/main.rs` | Entry point: legacy-name warning, CLI dispatch, pending-migration gate, logging setup, daemon start (`start_daemon` / `start_current_daemon`) |
| `ui` | `src/ui.rs`, `src/ui/layout.rs`, `src/ui/widgets.rs`, `src/ui/styles.rs`, `src/ui/palette.rs`, `src/ui/icons.rs`, `src/ui/nav.rs`, `src/ui/help.rs` | ratatui rendering (used by TUI client only), layout splits, widget definitions, palette-driven `Theme`, Nerd Font / Unicode icon sets selected by `settings.icons`, navigation helpers, and the Help overlay's rows per `HelpContext` |
| `ui::layout` submodules | `src/ui/layout.rs` + `src/ui/layout/` | Layout facade and one submodule per pane or overlay; see [`src/ui/AGENTS.md`](src/ui/AGENTS.md) § Module Layout |
| `palette` | `src/ui/palette.rs`, `themes/*.toml`, `build.rs` | 22 vendored Omarchy palettes; `build.rs` compiles `themes/*.toml` into a `BUNDLED` static at compile time (no runtime TOML parsing) |
| `config` | `src/config.rs`, `src/config/profile.rs`, `src/config/subscription.rs`, `src/config/json_pointer.rs`, `src/config/merge.rs` + `src/config/merge/resolution.rs`, `src/config/recovery.rs` | JSON config I/O, profile and subscription struct definitions, subscription fetcher; `merge` is the UUID-aware three-way merge and `merge::resolution` builds the `YOUR EDIT` / `CURRENT` conflict document; `recovery` preserves and rotates copies in `recovery/` (three per kind); `json_pointer::JsonIndex` indexes every RFC 6901 pointer of a JSON text in one pass and answers its line (exact, or the nearest present ancestor) |
| `config::profile` submodules | `src/config/profile/{diagnostic,json_schema,protocol,protocol_options,protocol_config,tls,entry,schedule,subscription,routing,settings,schema,migrate,dns}.rs`, `src/config/profile/share_link{.rs,/parse.rs,/encode.rs}` | One persisted type per submodule, behind a `pub use` facade; see [`src/config/AGENTS.md`](src/config/AGENTS.md) § Module Layout |
| `singbox` | `src/singbox.rs`, `src/singbox/config.rs`, `src/singbox/outbound.rs`, `src/singbox/runner.rs`, `src/singbox/clash_api.rs`, `src/singbox/process_handle.rs` | Process lifecycle: allocate a free Clash API port, write temp config, run `sing-box check`, spawn `sing-box run`, retry a lost port race, kill on disconnect; Clash API client for live traffic stats; `Child` wrapper carrying the process's Clash API port |
| `geo` | `src/geo.rs` | Download and cache geoip/geosite rule-sets for sing-box routing |
| `paths` | `src/paths.rs` | Every file path kvn uses: config (`~/.config/kvn-tui/`), state (`$XDG_STATE_HOME/kvn/`) and both runtime namespaces (`$XDG_RUNTIME_DIR/kvn-tui/`, `$XDG_RUNTIME_DIR/kvn/`) |
| `atomic_write` | `src/atomic_write.rs` | Atomic file write helper (write `.tmp` + fsync + rename + parent-dir fsync) |
| `runtime_lock` | `src/runtime_lock.rs` | `flock`-based exclusive lock on a runtime file, optionally waiting for the holder to release it; backs the migration runner lock and the daemon's single-instance lock |
| `net` | `src/net.rs` | Free loopback port allocation (bind `127.0.0.1:0`, read the OS-assigned port, drop the listener) for the Clash API control port and the profile-test SOCKS5 port |
| `pacman` | `src/pacman.rs` | Detects a pacman transaction that has already written a given package file (its ctime is not older than `/var/lib/pacman/db.lck`) and waits for it to settle; used by the migration runner for `kvn-tui`/`sing-box` and by the daemon's connect path for the sing-box binary it launches (`singbox::runner::binary_path`: `SING_BOX_PATH` or the first `sing-box` on `PATH`), whose capabilities hook runs only at PostTransaction |
| `systemd` | `src/systemd.rs` | The daemon's systemd user unit name and its restart helper, so a unit rename lands in one place |
| `onboarding` | `src/onboarding.rs` | First-run tour: the ordered `OnboardingStep` cards, their handoff screen and copyable command, the persisted `OnboardingState` and the session-local `OnboardingProgress`, the `IntegrationSetup` / `SetupState` the protection cards render, plus the grandfathering load for installs that predate the tour |
| `waybar` | `src/services/waybar.rs` | Read/write `state.json` for waybar integration and crash recovery |
| `suspend` | `src/services/suspend.rs` | D-Bus listener for `systemd-logind` `PrepareForSleep` signals (zbus) |
| `integration_files` | `src/integration_files.rs`, `contrib/killswitch.nft`, `contrib/killswitch-helper.sh`, `contrib/kvn-tui-killswitch.{service,sudoers}`, `contrib/49-kvn-tui.rules` | Embedded kill-switch/polkit payloads passed to the setup scripts; detects outdated installed files and SHA-256 stamps in `/var/lib/kvn/integrations/` for `kvn doctor` |
| `killswitch` | `src/services/killswitch.rs` | nftables helper integration: enable/disable systemd unit, flush handshake exceptions left by older daemons, reconcile state on startup |
| `services` | `src/services.rs`, `src/services/log_tailer.rs`, `src/services/waybar.rs`, `src/services/suspend.rs` | Background services: waybar state I/O and the suspend watcher (daemon), the log tailer (TUI client), and the app-log writer |
| `clipboard` | `src/tui_client/clipboard.rs` | System clipboard integration; auto-detects Wayland (`wl-paste` / `wl-copy`) or X11 (`xclip`, falls back to `xsel`); reads the clipboard for `p` and sends it to the daemon as `IpcCommand::Paste` (parsed in `update/paste.rs`), and writes it for `y` |
| `editor` | `src/tui_client/editor.rs`, `src/tui_client/editor/retry.rs` | Launch `$EDITOR` / `$VISUAL` on a private snapshot (`$XDG_RUNTIME_DIR/kvn-tui/profiles-edit-<pid>.json`), temporarily restore the terminal; `retry` is the error screen that offers another pass or cancellation |
| `input` | `src/tui_client/input.rs` | Terminal input: Kitty keyboard protocol for layout-independent shortcuts, mouse capture, unbuffered reads |
| `browser` | `src/tui_client/browser.rs` | Opens the project support page with `xdg-open` |
| `theme_watch` | `src/tui_client/theme_watch.rs` | Resolves `settings.theme` slug to a `Theme` (with `"omarchy"` sentinel falling back to `tokyo-night`); watches Omarchy's XDG state current-theme directory and emits `Msg::ThemeChanged`; no-op when Omarchy isn't installed |

The Omarchy bar widget is not in this repository: it is the standalone [omakvn](https://github.com/yarikov/omakvn) Quickshell plugin (`yarikov.omakvn`), required by the Omarchy integration. `setup --omarchy` requires Omarchy 4+ and installs or updates its Git checkout in `~/.config/omarchy/plugins/yarikov.omakvn/`, aborting and rolling back when the plugin cannot be installed; the main project owns the semantic IPC API.

---

## Subsystem Guides

| Guide | Covers |
|-------|--------|
| [`src/app/AGENTS.md`](src/app/AGENTS.md) | `update` submodules and key handlers; first-run tour, region selection, disable confirmation, auto-connect, wheel scrolling and pointer focus |
| [`src/daemon/AGENTS.md`](src/daemon/AGENTS.md) | Effect execution: what each `daemon/` submodule owns |
| [`src/tui_client/AGENTS.md`](src/tui_client/AGENTS.md) | The client loop, its handlers, and client-local keys |
| [`src/config/AGENTS.md`](src/config/AGENTS.md) | `config::profile` submodules; share-link parsing, the DNS model and its generation, schema migrations, external editor sessions and JSON Schema |
| [`src/singbox/AGENTS.md`](src/singbox/AGENTS.md) | sing-box config generation, the Clash API port, routing modes and service routing |
| [`src/services/AGENTS.md`](src/services/AGENTS.md) | Kill switch, suspend/resume, `state.json` |
| [`src/ui/AGENTS.md`](src/ui/AGENTS.md) | `ui::layout` submodules, rendering snapshots, palettes, theme resolution and pickers, terminal colors |

---

## Build System & Dependencies

- **Rust**: edition 2024, minimum version 1.98 (`rust-version` in `Cargo.toml`); CI builds and releases with exactly that version, so std APIs stabilized later fail CI
- **External binary**: `sing-box` must be installed separately and available on `$PATH` (or via `SING_BOX_PATH` env var)
- **Key crates**: `ratatui` + `crossterm` (TUI), `serde` + `serde_json` (config), `schemars` (config JSON Schema), `zbus` (D-Bus), `ureq` (HTTP), `tracing` (logs), `anyhow` (errors)

Run a release build (no root required when sing-box has capabilities). The
binary is `kvn-tui`; the package installs `kvn` as a symlink to it, and
`kvn-tui <command>` prints a deprecation warning, so prefer `kvn` in docs:

```bash
./target/release/kvn-tui
sudo ./target/release/kvn-tui setup --polkit   # avoids authentication dialogs on connect
```

---

## Release Process

See the `release` skill in `.agents/skills/release/SKILL.md` for the full version-bump and tagging workflow. Supports auto-bump by semver level (major / minor / patch) or explicit version.

---

## Platform Constraints

**Arch Linux.** Both Wayland and X11 sessions are supported. Do not add generic Linux abstractions (other distros, BSDs, …) without explicit user request.

- Clipboard: Wayland and X11 tools are both supported (`clipboard` in § Module Map)
- Power events: listens to `org.freedesktop.login1.Manager.PrepareForSleep` via zbus (display-server-agnostic)
- TUN interface: created by sing-box with `cap_net_admin,cap_net_raw` file capabilities; kvn runs as the regular user

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
- The crate has no `thiserror` dependency; when a structured error is genuinely needed, discuss adding it rather than hand-rolling `Error` impls.
- Prefer `.context("...")` and `.with_context(|| format!("..."))` to add descriptive messages.

### File I/O
- **Atomic writes are mandatory** for config files. Pattern: write to `.tmp`, fsync, `fs::rename`, fsync the parent directory.
- Use `atomic_write::write`, the canonical implementation; `config::save_config_at` and `geo::GeoManager::write_atomic` delegate to it.

### Logging
- In the daemon and the TUI, use `tracing::info!`, `tracing::warn!`, `tracing::error!` — not `println!`. One-shot CLI commands (`cli.rs`, `cli/`, the migration runner) print their user-facing output with `println!` / `eprintln!`.
- The subscriber is initialized in `main.rs` with `EnvFilter` and `fmt::layer().without_time()`.

### Serialization
- All persistent data uses `serde` + `serde_json`.
- Config file: `profiles.json` (top-level `Config` struct with `profiles: Vec<Profile>` and `settings: Settings`).
- `Profile` stores common fields (`id`, `name`, `address`, `port`, and `share_link_params` — every share-link parameter the parser does not map, kept for export) plus `#[serde(flatten)] config: ProtocolConfig`. `ProtocolConfig` is an internally-tagged enum (`#[serde(tag = "protocol", rename_all = "lowercase")]`) with one variant per protocol (11 total). VLESS keeps its TLS fields flat on `VlessConfig` for backward compatibility with configs written before the multi-protocol refactor; its transport uses the shared `TransportConfig` since schema v7.
- Shared TLS/transport types: `TlsCommon` (SNI, ALPN, fingerprint, insecure, `EchSettings`, `RealitySettings`), `TransportConfig` (WebSocket, gRPC, HTTP). ECH and REALITY are mutually exclusive and reported by `ProtocolConfig::diagnostics`.
- `Profile::dedup_key()` returns a stable string key (`"protocol:credential@host:port|…"`) used by the subscription importer to detect duplicate profiles across imports. The endpoint after `@` also carries the SNI (REALITY's when enabled) and the transport's path, host and service name, because panels such as Marzban, 3x-ui and Remnawave give one user the same UUID on every server, and servers behind one CDN address differ only there. `Profile::endpoint_key()` is the same endpoint without the credential. A subscription update keeps a profile's ID when its `dedup_key` matches, or, for what is left, when exactly one old profile and one distinct new server share an `endpoint_key` (a rotated UUID or password); a server whose address, port, SNI or transport changed gets a new ID.
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

- Unit tests are co-located in `#[cfg(test)] mod tests` blocks at the bottom of each source file. `tests/cli_invocation.rs` holds the integration tests that run the built binary.
- `src/test_helpers.rs` provides shared test utilities (e.g., `model_with_profiles`).
- Tests should not depend on external network or the `sing-box` binary unless explicitly marked `#[ignore]` (or guarded by a `command -v sing-box` runtime check).
- Use `tempfile` for file-system tests; use `NamedTempFile` / `tempdir()` for isolation.
- Tests that mutate process environment (`std::env::set_var`) **must** lock `crate::test_helpers::ENV_LOCK` to serialize against other env-touching tests. The same lock is required for tests that *read* the environment non-atomically — spawning a child process (execve/PATH lookup reads the env) or resolving an env-dependent path more than once — otherwise they race the mutating tests and fail intermittently.
- Snapshot tests use [insta](https://insta.rs/). Regenerate with `INSTA_UPDATE=always cargo test`, then review the diffs before committing; with `cargo-insta` installed, `cargo insta review` walks them interactively instead. Pending `.snap.new` files are gitignored.
- The CI `test` job follows `cargo test --locked` with `cargo insta test --check --unreferenced=reject`, so a snapshot orphaned by a deleted or renamed test fails the build. `.config/insta.yaml` applies the same policy to local `cargo insta test` runs.
- Test visual TUI rendering — dimensions, alignment, spacing, styles, and rendered text or row order — only with `insta` snapshots. Do not add granular unit tests that inspect coordinates, widths, styles, helper output, or individual cells in a `ratatui::Buffer`.
- How to write a rendering snapshot (styles, window size, when to add one) is in [`src/ui/AGENTS.md`](src/ui/AGENTS.md) § Rendering Snapshots.
- Use regular unit tests for functional behavior only: input handling, model transitions, persistence, emitted effects, validation, and business logic.
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
5. **Daemon** (`daemon.rs` + `daemon/`) owns the canonical `Model` and executes every `Effect`; **TUI clients** (`tui_client.rs`) render a local copy and forward input. Both, and the IPC protocol between them, are described in § Daemon + TUI Client Architecture.

This separation makes the update tree fully synchronous and trivial to unit-test.

### Background Services
Background work is executed in dedicated threads spawned by the **daemon** (`daemon.rs`):
- **Ticker** — sends `Msg::Tick` every 250 ms to drive connection state machines.
- **Suspend watcher** — `services/suspend.rs` runs a blocking zbus listener that sends `Msg::SystemResumed`; the daemon auto-reconnects on resume even when no TUI is attached.
- **IPC server** — `ipc.rs` accepts Unix socket connections from clients, parses NDJSON commands, and forwards them into the daemon's mpsc channel as `Msg::IpcCommand`, or as `Msg::IpcRequest` when the command carries a request id for a correlated reply.
- **Signal handler** — turns `SIGTERM` / `SIGINT` into `Quit`, so the daemon cleans up sing-box and the socket.
- **Effects** — slow effects run on a short-lived thread that sends its result back through the daemon's channel: `Connect`, the geo effects (`DownloadGeo`, `DownloadGeoIfMissing`, `DownloadServiceRuleSetsIfMissing`, `RetryServiceRuleSets`, `RefreshGeoLastUpdated`), `UpdateSubscription`, `TestProfile`, `FetchTrafficStats`, `ApplyKillSwitch`, `CheckAutoConnectPolkit`, `CheckIntegrationSetup`, and `ReloadConfig`.
- **App log** — app status messages are appended to the app log with an `[app]` prefix (`Effect::AppendAppLog`), next to the sing-box log, so both are visible in the TUI log panel.
- **State I/O** — `services/waybar.rs` writes `state.json` on connect/disconnect for waybar integration.

The **TUI client** (`tui_client.rs`) additionally spawns:
- **Event reader** — reads terminal input (`tui_client/input.rs`) and sends `Msg::Key`, `Msg::Mouse`, `Msg::Paste` and `Msg::Resize` to the local TUI channel. Reading can be paused while `$EDITOR` is open.
- **Ticker** — sends `Msg::Tick` every 250 ms; on each tick `LogTailer` (`services/log_tailer.rs`) reads the new lines of both log files for the log pane.
- **IPC reader** — reads NDJSON state snapshots from the daemon socket and forwards them as `Msg::StateUpdate`; when the connection drops it sends `Msg::DaemonDisconnected`, and the client loop reattaches and applies the fresh snapshot, or, when the daemon is a different version, restarts the TUI from the daemon's executable.

### Daemon + TUI Client Architecture
- **Daemon** (`kvn --daemon`) runs headless. It owns the canonical `Model`, the `mpsc` channel, the sing-box `process_slot`, the config, geo updates, suspend/resume handling, and the background services (§ Background Services). It binds a Unix domain socket for IPC.
- **TUI Client** (`kvn`) connects to the daemon socket, requests a state snapshot (`Attach`), enters the alternate screen, and renders the UI. Keyboard input is forwarded to the daemon as `IpcCommand::Key`, except what needs the terminal, the clipboard or client-local state: `p` / `Ctrl+V` (paste), `y` (copy), `e` (editor), `gg` (sent as `GoFirst`), pane focus, `q` / `Esc` detach, `Ctrl+C`, the log-pane cursor and selection keys, and the onboarding cards' `y` / `p`. Bracketed paste (`\x1b[?2004h`) is enabled so a terminal paste arrives as a single `Msg::Paste` and is sent as `IpcCommand::Paste` instead of being decoded as shortcut keys.
- Pressing `q` (or `Esc`) when no overlay is shown sends `Detach` to the daemon, leaves the alternate screen, disables raw mode, and **exits the TUI process**. The daemon and sing-box keep running. Shell regains the prompt immediately because the foreground TUI process actually exits. If an overlay is open (Help, SettingsMenu, ConfirmDelete, ConfirmDisable, RoutingMode, GeoRegions, DnsSettings, ThemeSettings, ServiceRouting), `q`/`Esc` is forwarded to the daemon as a normal key, which closes the overlay — except where a step refuses it (the region picker without a region, and the pickers the tour is waiting on). `Overlay::Onboarding` is modal and ignores `q`/`Esc` (see `src/app/AGENTS.md` § First-Run Onboarding). `Overlay::RestartRequired` is modal as well: ignores `q`/`Esc`, and takes only `Enter` (exit and `systemctl --user restart kvn-tui.service`) or `Ctrl+C` (stop the daemon).
- Pressing `Ctrl+C` sends `Quit` to the daemon. The daemon stops sing-box, cleans up the Unix socket, and exits. The TUI waits briefly (300 ms) for cleanup to complete before exiting.
- Running `kvn` again connects to the same daemon and re-attaches, restoring the TUI instantly without restarting sing-box.
- **IPC protocol** (`ipc.rs`; the `IpcCommand` enum lives in `app/msg.rs`) uses newline-delimited JSON over a Unix socket.
  - Commands: `Attach`, `AttachSession`, `Detach`, `ClearErrorStatus`, `Key`, `SelectSource`, `MoveSourceSelection` (legacy relative selection), `ScrollViewport` (wheel navigation with a correlated viewport result), `SetMainPaneFocus`, `GoFirst`, `ConnectProfile`, `Disconnect`, `Reconnect`, `Toggle`, `SetRoutingMode`, `SetGeoRegion`, `SetKillSwitch`, `SetAutoConnect`, `CheckOnboarding`, `CheckSupportPrompt`, `ResolveSupportPrompt`, `DismissSupportPrompt`, `Paste`, `Copied`, `ReloadConfig`, `ApplyEditedConfig` (correlated), `RestartRequired`, `Quit`, `ClientError`.
  - Responses: `StateSnapshot` pushed by the daemon after every state change.
  - The semantic commands (`ConnectProfile` through `SetAutoConnect`) exist for non-TUI clients — the Omarchy Quickshell module and the `kvn status/connect/disconnect/reconnect/toggle` CLI subcommands.
  - Overlay commits (routing mode, geo region) are shared between the key handlers and IPC via `commit_routing_mode` / `commit_geo_region` in `update/routing.rs` so both paths run identical logic.
  - The snapshot includes the complete config (`profiles` and `settings`) so the TUI client always renders the current data.
- `handle_ipc_command` unconditionally appends `Effect::BroadcastState` to every IPC command result, ensuring the daemon always pushes state after user interaction.
- `handle_geo_result`, `Msg::ConnectFailed`, and the `handle_tick` idle fallback also append `Effect::BroadcastState` so state mutations that don't produce other broadcast-triggering effects are still visible to the TUI.

---

## Side-effect-free Boundaries

The TEA update function (`app::update::update`) must remain free of I/O, threads, and system calls. Side effects are declared as `Effect` values and executed by the daemon runtime.

Rules of thumb:
- `app::update::update(model, msg) -> Vec<Effect>` must not call functions from `services`, `geo`, `paths`, `atomic_write`, `config::load_config`, `config::subscription`, `singbox::clash_api`, `singbox::runner`, `tui_client::clipboard`, `tui_client::editor`, or perform any file/network/process I/O.
  - **Documented exceptions**: `theme_picker_slugs()` (`update/key/theme.rs`, used by the theme picker and the Interface page's Theme row) calls `omarchy::detect_omarchy_theme()`, which does a single small `fs::read_to_string` of the Omarchy state theme path to decide whether to show the Auto entry; `model::show_omarchy_card()` adds `omarchy::omakvn_plugin_installed()`, one more small read of the plugin manifest, while the model is constructed.
  - Cheap, deterministic, and scoped to a key press; promoted to "OK" because the alternative (caching in `Model`) costs more clarity than it saves.
  - The full `Theme` resolution stays out of `update`: the picker handler only mutates `theme_draft`/`settings.theme`, and the TUI client recomputes `model.theme` via `resolve_active` on snapshot apply.
  - `geo::is_ip_rule_set_tag` (used by the DNS overlay's fake-IP warning) is a pure lookup in the static rule-set asset table and does no I/O.
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
Any added, removed, or changed CLI command, option, or output updates
`docs/cli.md`.

| Resource | Path |
|----------|------|
| Profiles & settings | `~/.config/kvn-tui/profiles.json` |
| Geo rule-sets | `~/.config/kvn-tui/geo/` |
| sing-box log | `~/.config/kvn-tui/logs/sing-box.log` |
| App log | `~/.config/kvn-tui/logs/app.log` |
| Temp sing-box config | `$XDG_RUNTIME_DIR/kvn-tui/singbox.json` |
| Latency-test configs | `$XDG_RUNTIME_DIR/kvn-tui/test-<uuid>.json` |
| Editor snapshot | `$XDG_RUNTIME_DIR/kvn-tui/profiles-edit-<pid>.json` |
| Runtime state (waybar) | `~/.config/kvn-tui/state.json` |
| IPC socket (daemon ↔ TUI) | `$XDG_RUNTIME_DIR/kvn-tui.sock` |
| First-run tour progress | `$XDG_STATE_HOME/kvn/onboarding.json` |
| Support prompt schedule | `$XDG_STATE_HOME/kvn/support-prompt.json` |
| sing-box cache (fake-IP map) | `$XDG_STATE_HOME/kvn/singbox-cache.db` |
| Applied migration markers | `$XDG_STATE_HOME/kvn/migrations/` |
| Recovery copies (3 per kind) | `~/.config/kvn-tui/recovery/profiles.json.{before-migration,invalid,conflict,conflict-invalid}-*` |
| Migration lock | `$XDG_RUNTIME_DIR/kvn/migrate.lock` |
| Daemon instance lock | `$XDG_RUNTIME_DIR/kvn/daemon.lock` |
| Editor JSON Schema | `$XDG_RUNTIME_DIR/kvn/profiles.schema.json` |
| Migration baseline, machine-wide markers | `/var/lib/kvn/migration-baseline`, `/var/lib/kvn/migrations/` |
| Integration stamps | `/var/lib/kvn/integrations/{polkit,killswitch-sudoers}.sha256` |

---

## Agent Checklist Before Editing

1. Are you preserving atomic file writes for any new config files?
2. Are you using `anyhow::Result`, and `tracing` instead of `println!` / `eprintln!` outside CLI output?
3. Are tests added for new public functions, and does `cargo llvm-cov --summary-only` still report **≥ 85 %** region and line coverage?
4. Are you respecting the Arch-only constraint (no support for other distros or BSDs added silently)?
5. Does the sing-box config generation remain valid for sing-box 1.14+?
6. Have you run `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`, and (after a dependency change) `cargo deny check all`, and fixed what they report?
7. Do all new files and directories use `kvn` rather than the legacy `kvn-tui` path namespace?
8. Does the documentation match every changed path, file, command, configuration field, and user-visible behavior?
9. Did you read the subsystem guide (§ Subsystem Guides) for every subsystem you changed, and put any new convention in exactly one guide?

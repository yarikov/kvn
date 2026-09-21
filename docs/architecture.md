# Architecture

## Technology Stack

`kvn` is built with Rust 2024 and requires Rust 1.88 or newer.

| Component | Library / Tool | Purpose |
|-----------|--------------|---------|
| Terminal UI | [ratatui](https://ratatui.rs/) + [crossterm](https://github.com/crossterm-rs/crossterm) | Rendering, keyboard input, and terminal lifecycle |
| VPN backend | [sing-box](https://sing-box.sagernet.org/) 1.12+ | TUN, protocols, DNS, and traffic routing |
| Data formats | [serde](https://serde.rs/), `serde_json`, `toml` | Configuration, IPC messages, and bundled palettes |
| Networking | [ureq](https://github.com/algesten/ureq) with rustls | Subscriptions, rule-sets, and Clash API statistics |
| Linux integration | [zbus](https://docs.rs/zbus/latest/zbus/), `notify`, `signal-hook` | Suspend/resume, theme watching, and Unix signals |
| CLI | [clap](https://docs.rs/clap/) | Commands, setup options, and diagnostics |
| Observability | [tracing](https://github.com/tokio-rs/tracing) | Filtered application and daemon logs |
| Core utilities | `anyhow`, `uuid`, `chrono`, `url`, `base64`, `dirs` | Errors, IDs, timestamps, share links, and XDG paths |

## Highlights

- **Persistent daemon** — owns canonical state, sing-box, and background services; TUI and desktop integrations attach over NDJSON on a Unix socket without interrupting the VPN.
- **TEA-style core** — `Model`, `Msg`, `update`, and declarative `Effect` values separate state transitions from runtime I/O and keep business logic testable.
- **Safe sing-box lifecycle** — generated configuration is validated with `sing-box check` before startup, with immediate-failure detection before a connection is considered active.
- **Least-privilege integration** — TUN uses Linux capabilities instead of a root daemon, while privileged DNS and kill-switch operations are limited to narrowly scoped helpers and permissions.

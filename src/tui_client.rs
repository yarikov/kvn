mod browser;
mod clipboard;
mod docs_preview;
mod editor;
mod handler;
mod input;
pub(crate) mod theme_watch;

pub(crate) use docs_preview::run_docs_preview;

use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::ExecutableCommand;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::model::{ConnectionState, Model};
use crate::app::msg::{IpcCommand, Msg};
use crate::ipc::IpcClient;
use crate::services::LogTailer;
use crate::ui::palette::to_rgb;
use ratatui::style::Color;

/// Format the OSC 11 escape sequence that asks the terminal emulator to
/// repaint its own background (the pixel padding around the character
/// grid that no TUI widget can reach). Most modern emulators
/// (Alacritty, Foot, Kitty, Ghostty, Konsole, xterm, WezTerm…) honor it;
/// the rest silently ignore the unknown OSC and stay as-is.
pub(crate) fn osc_color(slot: u8, color: Color) -> String {
    let (r, g, b) = to_rgb(color);
    format!("\x1b]{slot};#{r:02x}{g:02x}{b:02x}\x1b\\")
}

/// Reset terminal foreground and background to their configured defaults.
pub(crate) const OSC_RESET_COLORS: &str = "\x1b]110\x1b\\\x1b]111\x1b\\";
/// Show a pointing hand over clickable rows; an empty shape list restores the
/// terminal's contextual default (usually an I-beam over terminal text).
pub(crate) const OSC_POINTER_INTERACTIVE: &str = "\x1b]22;pointer\x1b\\";
pub(crate) const OSC_POINTER_TEXT: &str = "\x1b]22;text\x1b\\";
pub(crate) const OSC_POINTER_DEFAULT: &str = "\x1b]22;\x1b\\";

/// Owns the terminal modes enabled by the TUI and restores them on every
/// return path (including an error from the render loop).
struct TerminalSession;

impl TerminalSession {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = stdout.execute(EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        if let Err(error) = input::enable_keyboard_protocol(&mut stdout) {
            let _ = stdout.execute(LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        if let Err(error) = input::enable_mouse_capture(&mut stdout) {
            let _ = input::disable_mouse_capture(&mut stdout);
            let _ = input::disable_keyboard_protocol(&mut stdout);
            let _ = stdout.execute(LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        if let Err(error) = input::enable_bracketed_paste(&mut stdout) {
            let _ = input::disable_bracketed_paste(&mut stdout);
            let _ = input::disable_mouse_capture(&mut stdout);
            let _ = input::disable_keyboard_protocol(&mut stdout);
            let _ = stdout.execute(LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        Ok(Self)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = stdout.write_all(OSC_POINTER_DEFAULT.as_bytes());
        let _ = input::disable_bracketed_paste(&mut stdout);
        let _ = input::disable_mouse_capture(&mut stdout);
        let _ = input::disable_keyboard_protocol(&mut stdout);
        let _ = disable_raw_mode();
        let _ = stdout.execute(LeaveAlternateScreen);
        reset_terminal_colors();
    }
}

/// Write OSC 11 to stdout (no-op when stdout isn't a TTY — pipes, CI,
/// captured output). Errors are swallowed: a terminal that doesn't
/// recognise the sequence is not a failure mode worth surfacing.
fn apply_terminal_colors(foreground: Color, background: Color) {
    let mut stdout = io::stdout();
    if !stdout.is_terminal() {
        return;
    }
    let _ = stdout.write_all(osc_color(10, foreground).as_bytes());
    let _ = stdout.write_all(osc_color(11, background).as_bytes());
    let _ = stdout.flush();
}

/// Counterpart to [`apply_terminal_colors`]: restore terminal defaults.
fn reset_terminal_colors() {
    let mut stdout = io::stdout();
    if !stdout.is_terminal() {
        return;
    }
    let _ = stdout.write_all(OSC_RESET_COLORS.as_bytes());
    let _ = stdout.flush();
}

/// Run the TUI client: connects to daemon, renders UI, forwards input.
pub fn run() -> Result<()> {
    let (mut client, initial_snapshot) = connect_to_current_daemon()?;

    // The daemon snapshot is canonical. Reading profiles.json here would let
    // the client normalize or otherwise rewrite a file concurrently owned by
    // the daemon.
    let config = crate::config::profile::Config {
        profiles: initial_snapshot.profiles.clone(),
        subscriptions: initial_snapshot.subscriptions.clone(),
        settings: initial_snapshot.settings.clone(),
        ..crate::config::profile::Config::default()
    };
    let mut model = Model::from_config(config.clone());
    model.theme = theme_watch::resolve_active(&model.config.settings.theme);

    let (tx, rx) = channel::<Msg>();
    let event_reader_control = Arc::new(input::EventReaderControl::new());
    spawn_ticker(tx.clone());
    theme_watch::spawn_theme_watcher(tx.clone());
    client.spawn_reader(tx.clone())?;
    client.send(&IpcCommand::AttachSession)?;
    if !initial_snapshot.restart_required {
        client.send(&IpcCommand::CheckOnboarding)?;
        client.send(&IpcCommand::CheckSupportPrompt)?;
    }

    let mut log_tailer = LogTailer::new(vec![
        (crate::paths::app_log_path(), "[app]"),
        (crate::paths::singbox_log_path(), "[sb]"),
    ]);
    apply_initial_snapshot(&mut model, initial_snapshot, &mut log_tailer);

    let terminal_session = TerminalSession::enter()?;
    apply_terminal_colors(
        model.theme.palette_foreground(),
        model.theme.palette_background(),
    );
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    input::spawn_event_reader(tx.clone(), event_reader_control.clone());

    let outcome = run_loop(
        &mut terminal,
        &mut model,
        rx,
        &mut client,
        &mut log_tailer,
        event_reader_control,
    )?;
    drop(terminal);
    drop(terminal_session);
    if outcome == TuiExit::RestartDaemon {
        crate::systemd::restart_daemon_unit();
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiExit {
    Normal,
    RestartDaemon,
}

fn reconnect_profile_from_snapshot(value: &serde_json::Value) -> Option<uuid::Uuid> {
    if value.get("connection").and_then(serde_json::Value::as_str) != Some("Connected") {
        return None;
    }
    value
        .get("active_profile_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            value
                .pointer("/settings/last_connected_profile")
                .and_then(serde_json::Value::as_str)
        })
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
}

/// Attach to a daemon built from the same package as this client. Upgrades can
/// leave the previous daemon alive, so inspect the first response as generic
/// JSON before attempting to decode the full (possibly changed) snapshot.
fn connect_to_current_daemon() -> Result<(IpcClient, crate::app::msg::StateSnapshot)> {
    let mut reconnect_profile = None;

    for attempt in 0..=1 {
        let mut client = IpcClient::connect().context("Failed to connect to kvn daemon")?;
        client.send(&IpcCommand::Attach)?;
        let value = client
            .read_snapshot_value(Duration::from_secs(2))
            .context("Daemon did not provide its initial state")?;

        let daemon_version = value
            .get("daemon_version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let ipc_version = value
            .get("ipc_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        let compatible = snapshot_is_compatible(&value);

        if compatible {
            let mut snapshot: crate::app::msg::StateSnapshot = serde_json::from_value(value)
                .context("Malformed state snapshot from the daemon")?;
            if snapshot.connection == ConnectionState::Idle
                && let Some(profile_id) = reconnect_profile
            {
                client.send(&IpcCommand::ConnectProfile { profile_id })?;
                snapshot = client
                    .read_snapshot(Duration::from_secs(2))
                    .context("Restarted daemon did not acknowledge reconnect")?;
            }
            return Ok((client, snapshot));
        }

        anyhow::ensure!(
            attempt == 0,
            "daemon is still incompatible after restart (daemon version {:?}, IPC {}; client version {:?}, IPC {})",
            daemon_version,
            ipc_version,
            env!("CARGO_PKG_VERSION"),
            crate::ipc::IPC_VERSION
        );

        reconnect_profile = reconnect_profile_from_snapshot(&value);

        if io::stderr().is_terminal() {
            eprintln!(
                "kvn: restarting outdated daemon (version {})…",
                if daemon_version.is_empty() {
                    "unknown"
                } else {
                    daemon_version
                }
            );
        }
        client
            .send(&IpcCommand::Quit)
            .context("Failed to ask outdated daemon to stop")?;
        drop(client);
        anyhow::ensure!(
            crate::ipc::wait_for_daemon_exit(Duration::from_secs(5)),
            "outdated daemon did not stop within 5s"
        );
        crate::start_current_daemon().context("Failed to start updated daemon")?;
        anyhow::ensure!(
            crate::ipc::wait_for_daemon(Duration::from_secs(5)),
            "updated daemon did not start within 5s"
        );
    }

    unreachable!("daemon compatibility loop always returns or errors")
}

fn snapshot_is_compatible(value: &serde_json::Value) -> bool {
    value
        .get("daemon_version")
        .and_then(serde_json::Value::as_str)
        == Some(env!("CARGO_PKG_VERSION"))
        && value.get("ipc_version").and_then(serde_json::Value::as_u64)
            == Some(u64::from(crate::ipc::IPC_VERSION))
}

fn apply_initial_snapshot(
    model: &mut Model,
    snapshot: crate::app::msg::StateSnapshot,
    log_tailer: &mut LogTailer,
) {
    if let Some(offsets) = snapshot.log_session_offsets {
        for line in log_tailer.load_history(
            &[offsets.app, offsets.singbox],
            crate::app::model::MAX_LOG_LINES,
        ) {
            model.push_log(line);
        }
    }
    apply_snapshot(model, snapshot);
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    model: &mut Model,
    rx: std::sync::mpsc::Receiver<Msg>,
    client: &mut IpcClient,
    log_tailer: &mut LogTailer,
    event_reader_control: Arc<input::EventReaderControl>,
) -> Result<TuiExit> {
    let mut state =
        handler::ClientLoop::new(terminal, model, client, log_tailer, event_reader_control)?;
    state.initial_draw()?;
    loop {
        match state.handle(rx.recv()?)? {
            handler::Flow::Continue => {}
            handler::Flow::Exit(exit) => return Ok(exit),
        }
    }
}

fn apply_snapshot(model: &mut Model, snapshot: crate::app::msg::StateSnapshot) {
    model.restart_required = snapshot.restart_required;
    model.connection = snapshot.connection;
    model.status =
        crate::app::model::AppStatus::from_snapshot(snapshot.status, snapshot.status_is_error);
    model.status_revision = snapshot.status_revision;
    model.singbox_pid = snapshot.singbox_pid;
    model.active_profile_id = snapshot
        .active_profile_id
        .and_then(|s| uuid::Uuid::parse_str(&s).ok());
    model.selected = snapshot.selected;
    model.main_pane_focus = snapshot.main_pane_focus;
    model.routing_selected = snapshot.routing_selected;
    model.geo_region_selected = snapshot.geo_region_selected;
    model.dns_selected = snapshot.dns_selected;
    model.dns_preset_draft = snapshot.dns_preset_draft;
    model.dns_strategy_draft = snapshot.dns_strategy_draft;
    model.dns_fakeip_draft = snapshot.dns_fakeip_draft;
    model.theme_selected = snapshot.theme_selected;
    model.theme_draft = snapshot.theme_draft.clone();
    model.service_routing_selected = snapshot.service_routing_selected;
    model.service_routing_draft = snapshot.service_routing_draft;
    model.onboarding.awaiting = snapshot.onboarding_awaiting;
    model.onboarding.include_omarchy_card = snapshot.onboarding_omarchy;
    model.integration_setup = snapshot.integration_setup;
    model.settings_menu_return = snapshot.settings_menu_return;
    model.settings_menu_selected = snapshot.settings_menu_selected;
    model.routing_settings_draft = snapshot.routing_settings_draft;
    model.connection_settings_draft = snapshot.connection_settings_draft;
    model.interface_settings_draft = snapshot.interface_settings_draft;
    model.geo_updating = snapshot.geo_updating;
    model.geo_last_updated = snapshot.geo_last_updated;
    model.geo_last_checked_at = snapshot.geo_last_checked_at;
    model.service_checked_at = snapshot.service_checked_at;
    model.overlay = snapshot.overlay;
    model.config.profiles = snapshot.profiles;
    model.config.subscriptions = snapshot.subscriptions;
    model.config.settings = snapshot.settings;
    model.traffic = snapshot.traffic;
    model.profile_latencies = snapshot
        .profile_latencies
        .into_iter()
        .filter_map(|(s, ms)| uuid::Uuid::parse_str(&s).ok().map(|id| (id, ms)))
        .collect();

    model.testing_profiles = snapshot
        .testing_profiles
        .into_iter()
        .filter_map(|s| uuid::Uuid::parse_str(&s).ok())
        .collect();
    // Resolve the effective theme: live-preview draft wins while the
    // picker is open, otherwise honor the committed `Settings.theme`.
    let effective_slug = model
        .theme_draft
        .as_deref()
        .unwrap_or(&model.config.settings.theme);
    let previous = (
        model.theme.palette_foreground(),
        model.theme.palette_background(),
    );
    model.theme = theme_watch::resolve_active(effective_slug);
    let current = (
        model.theme.palette_foreground(),
        model.theme.palette_background(),
    );
    if current != previous {
        apply_terminal_colors(current.0, current.1);
    }
}

fn spawn_ticker(tx: Sender<Msg>) {
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(250));
            if tx.send(Msg::Tick).is_err() {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn interface_drafts_preview_theme_and_icons_until_closed() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let mut daemon_model = crate::test_helpers::model_with_profiles(vec![]);
        daemon_model.config.settings.theme = "tokyo-night".into();
        let mut client_model = crate::test_helpers::model_with_profiles(vec![]);
        let sync = |daemon_model: &Model, client_model: &mut Model| {
            let snapshot = crate::daemon::build_snapshot(
                daemon_model,
                crate::app::msg::LogSessionOffsets::default(),
                1,
                None,
                None,
            );
            apply_snapshot(client_model, snapshot);
        };
        let saved_background = theme_watch::resolve_active("tokyo-night").palette_background();

        for key in [' ', 'i', 'l', 'j', 'l'] {
            crate::app::update::update(
                &mut daemon_model,
                Msg::Key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)),
            );
        }
        sync(&daemon_model, &mut client_model);

        let draft = daemon_model.theme_draft.clone().unwrap();
        assert_ne!(draft, "tokyo-night");
        assert_eq!(
            client_model.theme.palette_background(),
            theme_watch::resolve_active(&draft).palette_background()
        );
        assert_eq!(
            client_model.icon_set(),
            crate::config::profile::IconSet::Unicode
        );

        crate::app::update::update(
            &mut daemon_model,
            Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        );
        sync(&daemon_model, &mut client_model);

        assert_eq!(client_model.theme.palette_background(), saved_background);
        assert_eq!(
            client_model.icon_set(),
            crate::config::profile::IconSet::Nerd
        );
    }

    #[test]
    fn snapshot_compatibility_requires_matching_binary_and_ipc_versions() {
        let current = serde_json::json!({
            "daemon_version": env!("CARGO_PKG_VERSION"),
            "ipc_version": crate::ipc::IPC_VERSION,
        });
        assert!(snapshot_is_compatible(&current));
        assert!(!snapshot_is_compatible(&serde_json::json!({})));
        assert!(!snapshot_is_compatible(&serde_json::json!({
            "daemon_version": "0.0.0",
            "ipc_version": crate::ipc::IPC_VERSION,
        })));
        assert!(!snapshot_is_compatible(&serde_json::json!({
            "daemon_version": env!("CARGO_PKG_VERSION"),
            "ipc_version": crate::ipc::IPC_VERSION + 1,
        })));
    }

    #[test]
    fn restart_reconnects_only_the_profile_from_a_connected_snapshot() {
        let id = uuid::Uuid::new_v4();
        let connected = serde_json::json!({
            "connection": "Connected",
            "active_profile_id": id.to_string(),
        });
        assert_eq!(reconnect_profile_from_snapshot(&connected), Some(id));

        let idle = serde_json::json!({
            "connection": "Idle",
            "active_profile_id": id.to_string(),
        });
        assert_eq!(reconnect_profile_from_snapshot(&idle), None);
    }

    #[test]
    fn restart_reconnect_profile_falls_back_to_persisted_last_profile() {
        let id = uuid::Uuid::new_v4();
        let snapshot = serde_json::json!({
            "connection": "Connected",
            "active_profile_id": null,
            "settings": { "last_connected_profile": id.to_string() },
        });
        assert_eq!(reconnect_profile_from_snapshot(&snapshot), Some(id));
    }

    #[test]
    fn osc_color_formats_rgb_color_as_hex_triplet() {
        // Tokyo Night accent — typical RGB value.
        assert_eq!(
            osc_color(11, Color::Rgb(0x7a, 0xa2, 0xf7)),
            "\x1b]11;#7aa2f7\x1b\\"
        );
    }

    #[test]
    fn osc_color_formats_named_ansi_colors_via_to_rgb_table() {
        // Color::Black maps to (0, 0, 0) in palette::to_rgb.
        assert_eq!(osc_color(10, Color::Black), "\x1b]10;#000000\x1b\\");
    }

    #[test]
    fn osc_reset_colors_resets_foreground_and_background() {
        assert_eq!(OSC_RESET_COLORS, "\x1b]110\x1b\\\x1b]111\x1b\\");
    }
}

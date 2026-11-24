use std::time::Instant;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::msg::IpcCommand;
use crate::tui_client::{DAEMON_EXIT_TIMEOUT, TuiExit};

use super::super::{ClientLoop, Flow};

pub(super) fn handle_unsupported_size(state: &mut ClientLoop, key: KeyEvent) -> Result<Flow> {
    if is_interrupt(&key) {
        return Ok(stop_daemon(state));
    }
    if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
        return detach(state);
    }
    Ok(Flow::Continue)
}

pub(super) fn detach(state: &mut ClientLoop) -> Result<Flow> {
    let _ = state.client.send(&IpcCommand::Detach);
    Ok(Flow::Exit(TuiExit::Normal))
}

pub(super) fn stop_daemon(state: &mut ClientLoop) -> Flow {
    let deadline = Instant::now() + DAEMON_EXIT_TIMEOUT;
    let _ = state.client.send_quit(deadline);
    Flow::WaitForDaemonExit { deadline }
}

pub(super) fn is_interrupt(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

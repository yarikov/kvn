use std::thread;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::msg::IpcCommand;
use crate::tui_client::TuiExit;

use super::super::{ClientLoop, Flow};

pub(super) fn handle_restart_required(state: &mut ClientLoop, key: KeyEvent) -> Result<Flow> {
    if key.code == KeyCode::Enter {
        let _ = state.client.send(&IpcCommand::Detach);
        return Ok(Flow::Exit(TuiExit::RestartDaemon));
    }
    if is_interrupt(&key) {
        return Ok(stop_daemon(state));
    }
    Ok(Flow::Continue)
}

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
    state.client.send(&IpcCommand::Detach)?;
    Ok(Flow::Exit(TuiExit::Normal))
}

pub(super) fn stop_daemon(state: &mut ClientLoop) -> Flow {
    let _ = state.client.send(&IpcCommand::Quit);
    thread::sleep(Duration::from_millis(300));
    Flow::Exit(TuiExit::Normal)
}

pub(super) fn is_interrupt(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

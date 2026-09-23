use std::io::Write;

use anyhow::Result;
use crossterm::ExecutableCommand;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::app::msg::IpcCommand;
use crate::tui_client::{OSC_POINTER_DEFAULT, editor, input};

use super::super::ClientLoop;
use super::super::pointer::PointerShape;

pub(super) fn open(state: &mut ClientLoop) -> Result<()> {
    state.clear_log_selection();
    anyhow::ensure!(
        state.event_reader_control.pause(),
        "Timed out while pausing terminal input for external editor"
    );
    state
        .terminal
        .backend_mut()
        .write_all(OSC_POINTER_DEFAULT.as_bytes())?;
    input::disable_bracketed_paste(state.terminal.backend_mut())?;
    input::disable_mouse_capture(state.terminal.backend_mut())?;
    input::disable_keyboard_protocol(state.terminal.backend_mut())?;
    disable_raw_mode()?;
    state.terminal.backend_mut().execute(LeaveAlternateScreen)?;
    let target = state.model.selected_row().map(editor::EditorTarget::from);
    let result = editor::open_profiles_editor(target, state.model.config.clone());
    enable_raw_mode()?;
    state.terminal.backend_mut().execute(EnterAlternateScreen)?;
    input::enable_keyboard_protocol(state.terminal.backend_mut())?;
    input::enable_mouse_capture(state.terminal.backend_mut())?;
    input::enable_bracketed_paste(state.terminal.backend_mut())?;
    state.pointer_shape = PointerShape::Default;
    state.terminal.clear()?;
    input::discard_pending_input();
    state.event_reader_control.resume();
    state.refresh_pointer_shape()?;
    match result {
        Ok(edit) => {
            state.client.send(&IpcCommand::ApplyEditedConfig {
                base: Box::new(edit.base),
                edited: Box::new(edit.edited),
            })?;
        }
        Err(e) => {
            // The live config was never opened by the
            // editor. Route the snapshot error through
            // the daemon so it survives the next state
            // broadcast and appears in app.log.
            let message = format!("Edit rejected: {e:#}");
            state.client.send(&IpcCommand::ClientError { message })?;
        }
    }
    state.needs_redraw = true;
    Ok(())
}

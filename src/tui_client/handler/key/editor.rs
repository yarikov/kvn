use std::io::Write;
use std::time::Instant;

use anyhow::Result;
use crossterm::ExecutableCommand;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::app::model::AppStatus;
use crate::tui_client::{OSC_POINTER_DEFAULT, editor, input};

use super::super::ClientLoop;
use super::super::pointer::PointerShape;

pub(super) fn open(state: &mut ClientLoop) -> Result<()> {
    state.clear_log_selection();
    if !state.event_reader_control.pause() {
        state.event_reader_control.resume();
        anyhow::bail!("Timed out while pausing terminal input for external editor");
    }
    let result = leave_terminal(state).and_then(|()| {
        let target = state.model.selected_row().map(editor::EditorTarget::from);
        editor::open_profiles_editor(target, state.model.config.clone())
    });
    let restored = restore_terminal(state);
    input::discard_pending_input();
    state.event_reader_control.resume();
    let status = editor_status(result);
    let reported = if let Some(AppStatus::Error(message)) = &status {
        // The live config was never opened by the
        // editor. Route the snapshot error through
        // the daemon so it survives the next state
        // broadcast and appears in app.log.
        state.report_error(message.clone())
    } else {
        if let Some(AppStatus::Info(message)) = &status {
            crate::services::log_tailer::append_app_log("INFO", message);
            state.toast.show_info(message, Instant::now());
        }
        Ok(())
    };
    finish_terminal_return(restored, reported, status.as_ref())?;
    state.refresh_pointer_shape()?;
    state.needs_redraw = true;
    Ok(())
}

fn editor_status(result: Result<editor::EditorOutcome>) -> Option<AppStatus> {
    match result {
        Ok(editor::EditorOutcome::Saved) => None,
        Ok(editor::EditorOutcome::Cancelled { recovery }) => {
            Some(AppStatus::Info(match recovery {
                Some(path) => format!(
                    "Configuration edit cancelled; edited version saved to {}",
                    path.display()
                ),
                None => "Configuration edit cancelled".into(),
            }))
        }
        Err(error) => Some(AppStatus::Error(format!(
            "Configuration edit failed: {error:#}"
        ))),
    }
}

fn finish_terminal_return(
    restored: Result<()>,
    reported: Result<()>,
    status: Option<&AppStatus>,
) -> Result<()> {
    let mut failures = Vec::new();
    if let Err(error) = restored {
        failures.push(format!("Failed to restore terminal: {error:#}"));
    }
    if let Err(error) = reported {
        failures.push(format!("Failed to report editor result: {error:#}"));
    }
    if failures.is_empty() {
        return Ok(());
    }
    if let Some(AppStatus::Info(message) | AppStatus::Error(message)) = status {
        failures.push(message.clone());
    }
    anyhow::bail!("{}", failures.join("; "))
}

fn leave_terminal(state: &mut ClientLoop) -> Result<()> {
    input::discard_pending_input();
    state
        .terminal
        .backend_mut()
        .write_all(OSC_POINTER_DEFAULT.as_bytes())?;
    input::disable_bracketed_paste(state.terminal.backend_mut())?;
    input::disable_mouse_capture(state.terminal.backend_mut())?;
    input::disable_keyboard_protocol(state.terminal.backend_mut())?;
    disable_raw_mode()?;
    state.terminal.backend_mut().execute(LeaveAlternateScreen)?;
    Ok(())
}

fn restore_terminal(state: &mut ClientLoop) -> Result<()> {
    let result = enable_raw_mode().map_err(anyhow::Error::from);
    let result = result.and(
        state
            .terminal
            .backend_mut()
            .execute(EnterAlternateScreen)
            .map(|_| ())
            .map_err(anyhow::Error::from),
    );
    let result = result.and(
        input::enable_keyboard_protocol(state.terminal.backend_mut()).map_err(anyhow::Error::from),
    );
    let result = result.and(
        input::enable_mouse_capture(state.terminal.backend_mut()).map_err(anyhow::Error::from),
    );
    let result = result.and(
        input::enable_bracketed_paste(state.terminal.backend_mut()).map_err(anyhow::Error::from),
    );
    state.pointer_shape = PointerShape::Default;
    result.and(state.terminal.clear().map_err(anyhow::Error::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_cancellation_uses_an_informational_status() {
        for recovery in [None, Some(std::path::PathBuf::from("/tmp/recovery.json"))] {
            let expected_path = recovery.clone();
            let status = editor_status(Ok(editor::EditorOutcome::Cancelled { recovery }));
            let Some(AppStatus::Info(message)) = status else {
                panic!("expected informational cancellation")
            };
            assert!(message.contains("cancelled"));
            if let Some(path) = expected_path {
                assert!(message.contains(path.to_str().unwrap()));
            }
        }
    }

    #[test]
    fn restoration_and_reporting_failures_keep_the_editor_error_and_recovery_path() {
        let status = editor_status(Err(anyhow::anyhow!(
            "Edited version saved to /tmp/recovery.json: editor failed"
        )));
        let error = finish_terminal_return(
            Err(anyhow::anyhow!("raw mode unavailable")),
            Err(anyhow::anyhow!("socket closed")),
            status.as_ref(),
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("raw mode unavailable"));
        assert!(message.contains("socket closed"));
        assert!(message.contains("editor failed"));
        assert!(message.contains("/tmp/recovery.json"));
    }

    #[test]
    fn restoration_failure_keeps_cancellation_recovery_path() {
        let status = editor_status(Ok(editor::EditorOutcome::Cancelled {
            recovery: Some("/tmp/recovery.json".into()),
        }));
        let error = finish_terminal_return(
            Err(anyhow::anyhow!("terminal closed")),
            Ok(()),
            status.as_ref(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
        assert!(error.to_string().contains("/tmp/recovery.json"));
    }
}

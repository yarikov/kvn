use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::msg::{CopiedTarget, IpcCommand};
use crate::onboarding::OnboardingStep;
use crate::tui_client::clipboard;

use super::super::{ClientLoop, Flow};

pub(super) fn handle(
    state: &mut ClientLoop,
    step: OnboardingStep,
    key: KeyEvent,
    completes_go_first: bool,
) -> Result<Flow> {
    if key.code == KeyCode::Char('y')
        && let Some(command) = step.clipboard_command(state.model.integration_setup)
    {
        copy_command(state, &command)?;
        return Ok(Flow::Continue);
    }
    if key.code == KeyCode::Char('p') && step == OnboardingStep::Profiles {
        super::clipboard::paste(state)?;
        return Ok(Flow::Continue);
    }
    super::overlay(state, key, completes_go_first)
}

fn copy_command(state: &mut ClientLoop, command: &str) -> Result<()> {
    match clipboard::write_clipboard_text(command) {
        Ok(()) => state.client.send(&IpcCommand::Copied {
            target: CopiedTarget::Command,
        })?,
        Err(error) => state.client.send(&IpcCommand::ClientError {
            message: format!("Command copy failed: {error:#}"),
        })?,
    }
    Ok(())
}

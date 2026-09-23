use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::msg::{IpcCommand, SupportPromptResolution};
use crate::tui_client::browser;

use super::super::{ClientLoop, Flow};

pub(super) fn handle(
    state: &mut ClientLoop,
    key: KeyEvent,
    completes_go_first: bool,
) -> Result<Flow> {
    match key.code {
        KeyCode::Char('g') => {
            if completes_go_first {
                state.model.support_selected = 0;
            }
            state.needs_redraw = true;
        }
        KeyCode::Char('q') | KeyCode::Esc => {
            state.client.send(&IpcCommand::ResolveSupportPrompt {
                resolution: SupportPromptResolution::RemindLater,
            })?
        }
        KeyCode::Char('j') | KeyCode::Down => {
            crate::ui::nav::select_next(&mut state.model.support_selected, 3);
            state.needs_redraw = true;
        }
        KeyCode::Char('k') | KeyCode::Up => {
            crate::ui::nav::select_prev(&mut state.model.support_selected);
            state.needs_redraw = true;
        }
        KeyCode::Char('G') => {
            crate::ui::nav::select_last(&mut state.model.support_selected, 3);
            state.needs_redraw = true;
        }
        KeyCode::Enter => resolve_selected(state)?,
        _ => state.forward_key(key)?,
    }
    Ok(Flow::Continue)
}

fn resolve_selected(state: &mut ClientLoop) -> Result<()> {
    match state.model.support_selected {
        0 => match browser::open_support_page() {
            Ok(()) => state.client.send(&IpcCommand::ResolveSupportPrompt {
                resolution: SupportPromptResolution::Supported,
            })?,
            Err(error) => state.client.send(&IpcCommand::ClientError {
                message: format!("Failed to open support page: {error:#}"),
            })?,
        },
        1 => state.client.send(&IpcCommand::ResolveSupportPrompt {
            resolution: SupportPromptResolution::RemindLater,
        })?,
        _ => state.client.send(&IpcCommand::ResolveSupportPrompt {
            resolution: SupportPromptResolution::Dismiss,
        })?,
    }
    Ok(())
}

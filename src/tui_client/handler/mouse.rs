use std::time::Instant;

use anyhow::Result;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::app::model::{MainPaneFocus, Overlay, SourceRow};
use crate::app::msg::{CopiedTarget, IpcCommand};

use super::pointer::PointerShape;
use super::{ClientLoop, Flow};

pub(super) fn handle(state: &mut ClientLoop, mouse: MouseEvent) -> Result<Flow> {
    if state.model.overlay == Overlay::RestartRequired {
        return Ok(Flow::Continue);
    }
    state.mouse_position = Some((mouse.column, mouse.row));
    let hit = state.refresh_pointer_shape()?;
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => press(state, mouse, hit)?,
        MouseEventKind::Moved => {
            if state.log_dragging
                && let Some(selection) = &mut state.log_selection
            {
                selection.update(mouse.column, mouse.row);
                state.needs_redraw = true;
            }
        }
        MouseEventKind::Up(MouseButton::Left) => release(state, mouse)?,
        _ => {}
    }
    Ok(Flow::Continue)
}

fn press(state: &mut ClientLoop, mouse: MouseEvent, hit: Option<usize>) -> Result<()> {
    state.clear_log_selection();
    let area = state.terminal_area()?;
    if let Some(selection) = crate::ui::layout::log_viewport_with_navigation(
        state.model,
        area,
        Some(&state.log_navigation),
    )
    .and_then(|viewport| crate::ui::layout::LogSelection::start(viewport, mouse.column, mouse.row))
    {
        state.focus_pane(MainPaneFocus::Logs)?;
        state.log_navigation.clear();
        state.click_tracker.reset();
        state.log_selection = Some(selection);
        state.log_dragging = true;
    } else if let Some(index) = hit {
        state.focus_pane(MainPaneFocus::Sources)?;
        state.client.send(&IpcCommand::SelectSource { index })?;
        state.model.selected = index;
        let profile_id = match state.model.source_rows()[index] {
            SourceRow::StandaloneProfile(profile_idx)
            | SourceRow::SubscriptionProfile { profile_idx, .. } => {
                Some(state.model.config.profiles[profile_idx].id)
            }
            SourceRow::SubscriptionHeader(_) => None,
        };
        if let Some(profile_id) = profile_id
            && state
                .click_tracker
                .profile_pressed(profile_id, Instant::now())
        {
            state
                .client
                .send(&IpcCommand::ConnectProfile { profile_id })?;
        } else if profile_id.is_none() {
            state.click_tracker.reset();
        }
    } else if state.pointer_shape == PointerShape::Logs {
        state.focus_pane(MainPaneFocus::Logs)?;
        state.click_tracker.reset();
    } else {
        state.click_tracker.reset();
    }
    state.needs_redraw = true;
    Ok(())
}

fn release(state: &mut ClientLoop, mouse: MouseEvent) -> Result<()> {
    if !state.log_dragging {
        return Ok(());
    }
    let Some(selection) = &mut state.log_selection else {
        return Ok(());
    };
    state.log_dragging = false;
    selection.update(mouse.column, mouse.row);
    let copy_result = (!selection.is_empty())
        .then(|| crate::tui_client::clipboard::write_clipboard_text(&selection.text()));
    state.log_selection = None;
    if let Some(result) = copy_result {
        match result {
            Ok(()) => state.client.send(&IpcCommand::Copied {
                target: CopiedTarget::Logs,
            })?,
            Err(error) => state.client.send(&IpcCommand::ClientError {
                message: format!("Log copy failed: {error:#}"),
            })?,
        }
    }
    state.needs_redraw = true;
    Ok(())
}

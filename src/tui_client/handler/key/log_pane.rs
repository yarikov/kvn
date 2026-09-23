use std::time::Instant;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::msg::IpcCommand;
use crate::tui_client::clipboard;

use super::super::{ClientLoop, Flow};

pub(super) fn handle(state: &mut ClientLoop, key: KeyEvent) -> Result<Flow> {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => move_cursor(state, 1)?,
        KeyCode::Char('k') | KeyCode::Up => move_cursor(state, -1)?,
        KeyCode::Char('G') => {
            state
                .log_navigation
                .select_buffer_edge(state.model.logs.len(), false, Instant::now());
            state.needs_redraw = true;
        }
        KeyCode::Char('V') => enter_visual(state)?,
        KeyCode::Char('y') => copy_selection(state)?,
        _ => state.forward_key(key)?,
    }
    Ok(Flow::Continue)
}

pub(super) fn select_oldest(state: &mut ClientLoop) {
    state
        .log_navigation
        .select_buffer_edge(state.model.logs.len(), true, Instant::now());
}

fn move_cursor(state: &mut ClientLoop, delta: isize) -> Result<()> {
    let area = state.terminal_area()?;
    let now = Instant::now();
    if state.log_navigation.cursor().is_none() {
        if let Some(viewport) = crate::ui::layout::log_viewport_with_navigation(
            state.model,
            area,
            Some(&state.log_navigation),
        ) {
            state.log_navigation.select_edge(&viewport, delta > 0, now);
        }
    } else {
        crate::ui::layout::sync_log_scroll(state.model, area, &mut state.log_navigation);
        state
            .log_navigation
            .move_by(delta, state.model.logs.len(), now);
    }
    state.needs_redraw = true;
    Ok(())
}

fn enter_visual(state: &mut ClientLoop) -> Result<()> {
    let now = Instant::now();
    if state.log_navigation.cursor().is_none() {
        let area = state.terminal_area()?;
        if let Some(viewport) = crate::ui::layout::log_viewport_with_navigation(
            state.model,
            area,
            Some(&state.log_navigation),
        ) {
            state.log_navigation.select_edge(&viewport, true, now);
        }
    }
    state.log_navigation.enter_visual(now);
    state.needs_redraw = true;
    Ok(())
}

fn copy_selection(state: &mut ClientLoop) -> Result<()> {
    if let Some((text, count)) = state.log_navigation.selected_text(state.model) {
        match clipboard::write_clipboard_text(&text) {
            Ok(()) => {
                state.log_navigation.copied(Instant::now());
                state.client.send(&IpcCommand::Copied {
                    name: "log".into(),
                    count,
                })?;
            }
            Err(error) => state.client.send(&IpcCommand::ClientError {
                message: format!("Failed to copy log text: {error:#}"),
            })?,
        }
    }
    state.needs_redraw = true;
    Ok(())
}

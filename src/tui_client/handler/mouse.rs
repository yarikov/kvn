use std::time::Instant;

use anyhow::Result;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::app::model::{MainPaneFocus, Overlay, SourceRow};
use crate::app::msg::{CopiedTarget, IpcCommand};

use super::pointer::PointerShape;
use super::wheel::WheelDirection;
use super::{ClientLoop, Flow};

pub(super) fn handle(state: &mut ClientLoop, mouse: MouseEvent) -> Result<Flow> {
    if state.model.overlay == Overlay::RestartRequired {
        return Ok(Flow::Continue);
    }
    state.mouse_position = Some((mouse.column, mouse.row));
    let hit = state.refresh_pointer_shape()?;
    if let Some(focus) = hover_focus(
        state.model,
        state.terminal_area()?,
        state.pane_focus,
        mouse,
        state.log_dragging,
    ) {
        state.focus_pane(focus)?;
    }
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
        MouseEventKind::ScrollUp => return scroll(state, mouse, WheelDirection::Up),
        MouseEventKind::ScrollDown => return scroll(state, mouse, WheelDirection::Down),
        _ => {}
    }
    Ok(Flow::Continue)
}

fn scroll(state: &mut ClientLoop, mouse: MouseEvent, direction: WheelDirection) -> Result<Flow> {
    let area = state.terminal_area()?;
    if state.log_dragging || !crate::ui::layout::terminal_size_supported(area) {
        return Ok(Flow::Continue);
    }
    let now = Instant::now();
    let step = state.wheel.step(direction, now);
    let delta = match direction {
        WheelDirection::Up => -(step as isize),
        WheelDirection::Down => step as isize,
    };
    if state.model.overlay != Overlay::None {
        super::scroll::enqueue(state, delta)?;
        return Ok(Flow::Continue);
    }
    match crate::ui::layout::main_pane_at(state.model, area, mouse.column, mouse.row) {
        Some(MainPaneFocus::Sources) => super::scroll::enqueue(state, delta)?,
        Some(MainPaneFocus::Logs) => {
            let delta_rows = match direction {
                WheelDirection::Up => -(step as isize),
                WheelDirection::Down => step as isize,
            };
            if crate::ui::layout::scroll_logs(
                state.model,
                area,
                &mut state.log_navigation,
                delta_rows,
                now,
            ) {
                state.needs_redraw = true;
            }
        }
        None => {}
    }
    Ok(Flow::Continue)
}

fn press(state: &mut ClientLoop, mouse: MouseEvent, hit: Option<usize>) -> Result<()> {
    state.scroll_queue.cancel();
    state.clear_log_selection();
    let area = state.terminal_area()?;
    if let Some(selection) = crate::ui::layout::log_viewport_with_navigation(
        state.model,
        area,
        Some(&state.log_navigation),
    )
    .and_then(|viewport| {
        state.log_navigation.select_at(
            state.model,
            &viewport,
            mouse.column,
            mouse.row,
            Instant::now(),
        );
        crate::ui::layout::LogSelection::start(viewport, mouse.column, mouse.row)
    }) {
        state.focus_pane(MainPaneFocus::Logs)?;
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
            Err(error) => state.report_error(format!("Log copy failed: {error:#}"))?,
        }
    }
    state.needs_redraw = true;
    Ok(())
}

fn hover_focus(
    model: &crate::app::model::Model,
    area: ratatui::layout::Rect,
    current: MainPaneFocus,
    mouse: MouseEvent,
    dragging: bool,
) -> Option<MainPaneFocus> {
    if dragging
        || !matches!(
            mouse.kind,
            MouseEventKind::Moved
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollDown
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight
        )
    {
        return None;
    }
    crate::ui::layout::main_pane_at(model, area, mouse.column, mouse.row)
        .filter(|focus| *focus != current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{APP_WINDOW_COLS, APP_WINDOW_ROWS, model_with_subscription};
    use crossterm::event::KeyModifiers;
    use ratatui::layout::Rect;

    fn movement(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn hover_changes_focus_only_when_entering_another_visible_pane() {
        let model = model_with_subscription();
        let area = Rect::new(0, 0, APP_WINDOW_COLS, APP_WINDOW_ROWS);
        let logs = movement(MouseEventKind::Moved, 80, 10);
        assert_eq!(
            hover_focus(&model, area, MainPaneFocus::Sources, logs, false),
            Some(MainPaneFocus::Logs)
        );
        assert_eq!(
            hover_focus(&model, area, MainPaneFocus::Logs, logs, false),
            None
        );
        assert_eq!(
            hover_focus(
                &model,
                area,
                MainPaneFocus::Logs,
                movement(MouseEventKind::ScrollDown, 2, 10),
                false
            ),
            Some(MainPaneFocus::Sources)
        );
        for (column, row) in [(80, 1), (80, APP_WINDOW_ROWS - 1)] {
            assert_eq!(
                hover_focus(
                    &model,
                    area,
                    MainPaneFocus::Sources,
                    movement(MouseEventKind::Moved, column, row),
                    false
                ),
                None
            );
        }
    }

    #[test]
    fn hover_ignores_overlays_dragging_hidden_logs_and_button_release() {
        let mut model = model_with_subscription();
        let area = Rect::new(0, 0, APP_WINDOW_COLS, APP_WINDOW_ROWS);
        let logs = movement(MouseEventKind::Moved, 80, 10);
        assert_eq!(
            hover_focus(&model, area, MainPaneFocus::Sources, logs, true),
            None
        );
        assert_eq!(
            hover_focus(
                &model,
                Rect::new(0, 0, 89, APP_WINDOW_ROWS),
                MainPaneFocus::Sources,
                logs,
                false
            ),
            None
        );
        assert_eq!(
            hover_focus(
                &model,
                area,
                MainPaneFocus::Sources,
                movement(MouseEventKind::Up(MouseButton::Left), 80, 10),
                false
            ),
            None
        );
        model.overlay = Overlay::Help(Default::default());
        assert_eq!(
            hover_focus(&model, area, MainPaneFocus::Sources, logs, false),
            None
        );
    }

    #[test]
    fn hover_focuses_empty_space_and_borders() {
        let model = model_with_subscription();
        let area = Rect::new(0, 0, APP_WINDOW_COLS, APP_WINDOW_ROWS);
        for (column, row, previous, expected) in [
            (
                APP_WINDOW_COLS - 1,
                10,
                MainPaneFocus::Sources,
                MainPaneFocus::Logs,
            ),
            (80, 20, MainPaneFocus::Sources, MainPaneFocus::Logs),
            (0, 10, MainPaneFocus::Logs, MainPaneFocus::Sources),
            (2, 20, MainPaneFocus::Logs, MainPaneFocus::Sources),
        ] {
            assert_eq!(
                hover_focus(
                    &model,
                    area,
                    previous,
                    movement(MouseEventKind::Moved, column, row),
                    false
                ),
                Some(expected)
            );
        }
    }
}

mod clipboard;
mod editor;
mod log_pane;
mod onboarding;
mod quit;
mod support;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::model::{MainPaneFocus, Overlay};
use crate::app::msg::IpcCommand;

use super::{ClientLoop, Flow};

pub(super) fn handle(state: &mut ClientLoop, key: KeyEvent) -> Result<Flow> {
    if !crate::ui::layout::terminal_size_supported(state.terminal_area()?) {
        return quit::handle_unsupported_size(state, key);
    }
    let completes_go_first = state.go_first_sequence.feed(&key.code);
    if quit::is_interrupt(&key) {
        return Ok(quit::stop_daemon(state));
    }
    match state.model.overlay {
        Overlay::Onboarding(step) => onboarding::handle(state, step, key, completes_go_first),
        Overlay::Support => support::handle(state, key, completes_go_first),
        Overlay::None => main_screen(state, key, completes_go_first),
        _ => overlay(state, key, completes_go_first),
    }
}

fn main_screen(state: &mut ClientLoop, key: KeyEvent, completes_go_first: bool) -> Result<Flow> {
    match key.code {
        KeyCode::Char('g') => {
            if completes_go_first {
                if state.pane_focus == MainPaneFocus::Logs {
                    log_pane::select_oldest(state);
                } else {
                    state.client.send(&IpcCommand::GoFirst)?;
                }
            }
            state.needs_redraw = true;
            Ok(Flow::Continue)
        }
        KeyCode::Esc
            if state.pane_focus == MainPaneFocus::Logs && state.log_navigation.is_visual() =>
        {
            state.log_navigation.cancel_visual();
            state.needs_redraw = true;
            Ok(Flow::Continue)
        }
        KeyCode::Char('q') | KeyCode::Esc => quit::detach(state),
        _ => match pane_focus_shortcut(&key) {
            Some(focus) => focus_shortcut(state, focus),
            None => match state.pane_focus {
                MainPaneFocus::Logs => log_pane::handle(state, key),
                MainPaneFocus::Sources => sources(state, key),
            },
        },
    }
}

fn overlay(state: &mut ClientLoop, key: KeyEvent, completes_go_first: bool) -> Result<Flow> {
    if key.code == KeyCode::Char('g') {
        if completes_go_first {
            state.client.send(&IpcCommand::GoFirst)?;
        }
        state.needs_redraw = true;
        return Ok(Flow::Continue);
    }
    state.forward_key(key)?;
    Ok(Flow::Continue)
}

fn sources(state: &mut ClientLoop, key: KeyEvent) -> Result<Flow> {
    match key.code {
        KeyCode::Char('p') => clipboard::paste(state)?,
        KeyCode::Char('v')
            if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
        {
            clipboard::paste(state)?
        }
        KeyCode::Char('y') => clipboard::copy_selected(state)?,
        KeyCode::Char('e') => editor::open(state)?,
        _ => state.forward_key(key)?,
    }
    Ok(Flow::Continue)
}

fn focus_shortcut(state: &mut ClientLoop, focus: MainPaneFocus) -> Result<Flow> {
    match focus {
        MainPaneFocus::Sources => state.focus_pane(MainPaneFocus::Sources)?,
        MainPaneFocus::Logs => {
            if crate::ui::layout::logs_visible(state.terminal_area()?) {
                state.focus_pane(MainPaneFocus::Logs)?;
            }
        }
    }
    state.needs_redraw = true;
    Ok(Flow::Continue)
}

#[derive(Debug, Default)]
pub(super) struct GoFirstSequence {
    pending: bool,
}

impl GoFirstSequence {
    fn feed(&mut self, code: &crossterm::event::KeyCode) -> bool {
        if *code != crossterm::event::KeyCode::Char('g') {
            self.pending = false;
            return false;
        }
        if self.pending {
            self.pending = false;
            true
        } else {
            self.pending = true;
            false
        }
    }
}

fn pane_focus_shortcut(
    key: &crossterm::event::KeyEvent,
) -> Option<crate::app::model::MainPaneFocus> {
    use crate::app::model::MainPaneFocus;
    use crossterm::event::{KeyCode, KeyModifiers};

    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    match key.code {
        KeyCode::Left | KeyCode::Char('h') => Some(MainPaneFocus::Sources),
        KeyCode::Right | KeyCode::Char('l') => Some(MainPaneFocus::Logs),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn pane_focus_shortcuts_require_ctrl() {
        use crate::app::model::MainPaneFocus;

        for (code, focus) in [
            (KeyCode::Char('h'), MainPaneFocus::Sources),
            (KeyCode::Left, MainPaneFocus::Sources),
            (KeyCode::Char('l'), MainPaneFocus::Logs),
            (KeyCode::Right, MainPaneFocus::Logs),
        ] {
            assert_eq!(
                pane_focus_shortcut(&KeyEvent::new(code, KeyModifiers::CONTROL)),
                Some(focus)
            );
            assert_eq!(
                pane_focus_shortcut(&KeyEvent::new(code, KeyModifiers::NONE)),
                None
            );
        }
    }

    #[test]
    fn go_first_sequence_requires_two_consecutive_g_keys() {
        use crossterm::event::KeyCode;

        let mut sequence = GoFirstSequence::default();
        assert!(!sequence.feed(&KeyCode::Char('g')));
        assert!(sequence.feed(&KeyCode::Char('g')));
        assert!(!sequence.feed(&KeyCode::Char('g')));
    }

    #[test]
    fn go_first_sequence_is_cancelled_by_another_key() {
        use crossterm::event::KeyCode;

        let mut sequence = GoFirstSequence::default();
        assert!(!sequence.feed(&KeyCode::Char('g')));
        assert!(!sequence.feed(&KeyCode::Char('j')));
        assert!(!sequence.feed(&KeyCode::Char('g')));
        assert!(sequence.feed(&KeyCode::Char('g')));
    }
}

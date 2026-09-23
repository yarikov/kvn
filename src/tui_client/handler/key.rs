mod clipboard;
mod editor;
mod log_pane;
mod quit;
mod support;

use std::time::Instant;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::model::{MainPaneFocus, Overlay};
use crate::app::msg::IpcCommand;

use super::{ClientLoop, Flow};

pub(super) fn handle(state: &mut ClientLoop, key: KeyEvent) -> Result<Flow> {
    if state.model.overlay == Overlay::RestartRequired {
        return quit::handle_restart_required(state, key);
    }
    if !crate::ui::layout::terminal_size_supported(state.terminal_area()?) {
        return quit::handle_unsupported_size(state, key);
    }
    let completes_go_first = state.go_first_sequence.feed(&key.code);
    if let Some(message) = deprecated_settings_shortcut_message(&key, state.model.overlay) {
        state.toast.show_info(message, Instant::now());
        state.needs_redraw = true;
    }
    if quit::is_interrupt(&key) {
        return Ok(quit::stop_daemon(state));
    }
    match state.model.overlay {
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
            Some(shortcut) => focus_shortcut(state, shortcut),
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

fn focus_shortcut(state: &mut ClientLoop, shortcut: PaneFocusShortcut) -> Result<Flow> {
    if let Some(message) = shortcut.deprecation {
        state.toast.show_info(message, Instant::now());
    }
    match shortcut.focus {
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

pub(super) const DEPRECATED_PANE_FOCUS_MESSAGE: &str =
    "h/l pane switching is deprecated; use Ctrl+h/Ctrl+l";
const DEPRECATED_ARROW_PANE_FOCUS_MESSAGE: &str =
    "←/→ pane switching is deprecated; use Ctrl+h/Ctrl+l";

#[derive(Debug, Default)]
pub(super) struct GoFirstSequence {
    pending: bool,
}

impl GoFirstSequence {
    /// Consume one key and report whether it completes a consecutive `gg`.
    /// Any non-g key cancels a pending prefix before continuing normally.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PaneFocusShortcut {
    focus: crate::app::model::MainPaneFocus,
    deprecation: Option<&'static str>,
}

fn pane_focus_shortcut(key: &crossterm::event::KeyEvent) -> Option<PaneFocusShortcut> {
    use crate::app::model::MainPaneFocus;
    use crossterm::event::{KeyCode, KeyModifiers};

    let (focus, deprecation) = match key.code {
        KeyCode::Left => (
            MainPaneFocus::Sources,
            Some(DEPRECATED_ARROW_PANE_FOCUS_MESSAGE),
        ),
        KeyCode::Right => (
            MainPaneFocus::Logs,
            Some(DEPRECATED_ARROW_PANE_FOCUS_MESSAGE),
        ),
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            (MainPaneFocus::Sources, None)
        }
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            (MainPaneFocus::Logs, None)
        }
        KeyCode::Char('h') if key.modifiers == KeyModifiers::NONE => {
            (MainPaneFocus::Sources, Some(DEPRECATED_PANE_FOCUS_MESSAGE))
        }
        KeyCode::Char('l') if key.modifiers == KeyModifiers::NONE => {
            (MainPaneFocus::Logs, Some(DEPRECATED_PANE_FOCUS_MESSAGE))
        }
        _ => return None,
    };
    Some(PaneFocusShortcut { focus, deprecation })
}

fn deprecated_settings_shortcut_message(
    key: &crossterm::event::KeyEvent,
    overlay: crate::app::model::Overlay,
) -> Option<&'static str> {
    use crossterm::event::KeyCode;

    if overlay != crate::app::model::Overlay::None {
        return None;
    }
    match key.code {
        KeyCode::Char('m') => Some("m is deprecated; use Space r"),
        KeyCode::Char('o') => Some("o is deprecated; use Space r"),
        KeyCode::Char('D') => Some("D is deprecated; use Space d"),
        KeyCode::Char('S') => Some("S is deprecated; use Space r"),
        KeyCode::Char('C') => Some("C is deprecated; use Space i"),
        KeyCode::Char('a') => Some("a is deprecated; use Shift+A"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn pane_focus_shortcuts_deprecate_plain_h_and_l() {
        use crate::app::model::MainPaneFocus;

        assert_eq!(
            pane_focus_shortcut(&KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)),
            Some(PaneFocusShortcut {
                focus: MainPaneFocus::Sources,
                deprecation: None,
            })
        );
        assert_eq!(
            pane_focus_shortcut(&KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)),
            Some(PaneFocusShortcut {
                focus: MainPaneFocus::Logs,
                deprecation: None,
            })
        );
        assert_eq!(
            pane_focus_shortcut(&KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
            Some(PaneFocusShortcut {
                focus: MainPaneFocus::Sources,
                deprecation: Some(DEPRECATED_PANE_FOCUS_MESSAGE),
            })
        );
        assert_eq!(
            pane_focus_shortcut(&KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE)),
            Some(PaneFocusShortcut {
                focus: MainPaneFocus::Logs,
                deprecation: Some(DEPRECATED_PANE_FOCUS_MESSAGE),
            })
        );
        assert_eq!(
            pane_focus_shortcut(&KeyEvent::new(KeyCode::Char('h'), KeyModifiers::ALT)),
            None
        );
    }

    #[test]
    fn arrow_keys_focus_main_panes_with_deprecation_notice() {
        use crate::app::model::MainPaneFocus;

        assert_eq!(
            pane_focus_shortcut(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            Some(PaneFocusShortcut {
                focus: MainPaneFocus::Sources,
                deprecation: Some(DEPRECATED_ARROW_PANE_FOCUS_MESSAGE),
            })
        );
        assert_eq!(
            pane_focus_shortcut(&KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            Some(PaneFocusShortcut {
                focus: MainPaneFocus::Logs,
                deprecation: Some(DEPRECATED_ARROW_PANE_FOCUS_MESSAGE),
            })
        );
    }

    #[test]
    fn legacy_settings_shortcuts_point_to_space_menu_only_on_main_screen() {
        use crate::app::model::{Overlay, SettingsMenuPage};

        for (key, expected) in [
            ('m', "m is deprecated; use Space r"),
            ('o', "o is deprecated; use Space r"),
            ('D', "D is deprecated; use Space d"),
            ('S', "S is deprecated; use Space r"),
            ('C', "C is deprecated; use Space i"),
            ('a', "a is deprecated; use Shift+A"),
        ] {
            let event = KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE);
            assert_eq!(
                deprecated_settings_shortcut_message(&event, Overlay::None),
                Some(expected)
            );
            assert_eq!(
                deprecated_settings_shortcut_message(
                    &event,
                    Overlay::SettingsMenu(SettingsMenuPage::Root)
                ),
                None
            );
        }

        for key in ['x', 'A', 'K'] {
            let event = KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE);
            assert_eq!(
                deprecated_settings_shortcut_message(&event, Overlay::None),
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

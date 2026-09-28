use std::io::{self, Write};

use anyhow::{Context, Result};
use crossterm::ExecutableCommand;
use crossterm::cursor::Show;
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::tui_client::input::{self, InputEvent, PausedInputReader};

pub(super) fn retry_edit(message: &str) -> Result<bool> {
    let mut output = io::stdout().lock();
    enable_raw_mode().context("Failed to enable retry prompt input")?;
    input::discard_pending_input();
    let mut keyboard_enabled = false;
    let result = (|| {
        output.execute(EnterAlternateScreen)?;
        input::enable_keyboard_protocol(&mut output)?;
        keyboard_enabled = true;
        input::enable_bracketed_paste(&mut output)?;
        let mut terminal = Terminal::new(CrosstermBackend::new(&mut output))?;
        terminal.clear()?;
        let mut reader = PausedInputReader::new()?;
        let mut prompt = RetryPrompt::default();
        loop {
            terminal.draw(|frame| prompt.draw(frame, message))?;
            let Some(events) = reader.read_events()? else {
                return Ok(false);
            };
            if let Some(answer) = events.iter().find_map(|event| prompt.handle(event)) {
                return Ok(answer);
            }
        }
    })();
    let mut failures = Vec::new();
    let mut record = |result: io::Result<()>| {
        if let Err(error) = result {
            failures.push(error.to_string());
        }
    };
    record(input::disable_bracketed_paste(&mut output));
    if keyboard_enabled {
        record(input::disable_keyboard_protocol(&mut output));
    }
    record(output.execute(Show).map(|_| ()));
    record(output.execute(LeaveAlternateScreen).map(|_| ()));
    record(output.flush());
    input::discard_pending_input();
    record(disable_raw_mode());
    finish_prompt(result, failures)
        .with_context(|| format!("While showing editor issue: {message}"))
}

fn finish_prompt(result: Result<bool>, failures: Vec<String>) -> Result<bool> {
    if failures.is_empty() {
        return result;
    }
    let message = format!(
        "Failed to restore terminal after retry prompt: {}",
        failures.join("; ")
    );
    match result {
        Err(error) => Err(error).context(message),
        Ok(_) => anyhow::bail!(message),
    }
}

#[derive(Default)]
struct RetryPrompt {
    scroll: u16,
    scroll_limit: u16,
}

impl RetryPrompt {
    fn handle(&mut self, event: &InputEvent) -> Option<bool> {
        let InputEvent::Key(key) = event else {
            return None;
        };
        if key.kind != KeyEventKind::Press || !(key.modifiers - KeyModifiers::SHIFT).is_empty() {
            return None;
        }
        match key.code {
            KeyCode::Char('G') => self.scroll = self.scroll_limit,
            _ if !key.modifiers.is_empty() => return None,
            KeyCode::Enter => return Some(true),
            KeyCode::Char('q') | KeyCode::Esc => return Some(false),
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll = self.scroll.saturating_add(1).min(self.scroll_limit);
            }
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char('g') => self.scroll = 0,
            _ => {}
        }
        None
    }

    fn draw(&mut self, frame: &mut Frame, message: &str) {
        let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(2)])
            .margin(1)
            .areas(frame.area());
        let paragraph = Paragraph::new(message).wrap(Wrap { trim: false });
        self.scroll_limit = u16::try_from(
            paragraph
                .line_count(body.width)
                .saturating_sub(usize::from(body.height.max(1))),
        )
        .unwrap_or(u16::MAX);
        self.scroll = self.scroll.min(self.scroll_limit);
        frame.render_widget(paragraph.scroll((self.scroll, 0)), body);
        let mut hint = "Enter — Continue editing · q/Esc — Cancel".to_string();
        if self.scroll_limit > 0 {
            hint.push_str("\nj/k ↑/↓ — Scroll · g/G — Start/End");
        }
        frame.render_widget(Paragraph::new(hint).wrap(Wrap { trim: false }), footer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, MouseEvent, MouseEventKind};
    use ratatui::backend::TestBackend;

    use crate::test_helpers::{APP_WINDOW_COLS, APP_WINDOW_ROWS, buffer_to_styled_string};

    #[test]
    fn retry_keys_act_without_confirmation() {
        for (code, expected) in [
            (KeyCode::Enter, Some(true)),
            (KeyCode::Char('q'), Some(false)),
            (KeyCode::Esc, Some(false)),
            (KeyCode::Char('y'), None),
            (KeyCode::Char('n'), None),
            (KeyCode::Char('Q'), None),
            (KeyCode::Up, None),
        ] {
            assert_eq!(
                RetryPrompt::default().handle(&InputEvent::Key(KeyEvent::from(code))),
                expected
            );
        }
    }

    #[test]
    fn retry_ignores_modified_keys_releases_repeats_paste_and_mouse() {
        for code in [KeyCode::Enter, KeyCode::Char('q'), KeyCode::Esc] {
            for kind in [KeyEventKind::Release, KeyEventKind::Repeat] {
                assert_eq!(
                    RetryPrompt::default().handle(&InputEvent::Key(KeyEvent::new_with_kind(
                        code,
                        KeyModifiers::NONE,
                        kind,
                    ))),
                    None
                );
            }
            assert_eq!(
                RetryPrompt::default()
                    .handle(&InputEvent::Key(KeyEvent::new(code, KeyModifiers::CONTROL))),
                None
            );
        }
        assert_eq!(
            RetryPrompt::default().handle(&InputEvent::Paste("q\n".into())),
            None
        );
        assert_eq!(
            RetryPrompt::default().handle(&InputEvent::Mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            })),
            None
        );
    }

    #[test]
    fn restoration_failure_keeps_prompt_error() {
        let error = finish_prompt(
            Err(anyhow::anyhow!("input failed")),
            vec!["restore failed".into()],
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("input failed"));
        assert!(message.contains("restore failed"));
        assert!(finish_prompt(Ok(true), vec!["restore failed".into()]).is_err());
        assert!(!finish_prompt(Ok(false), vec![]).unwrap());
    }

    #[test]
    fn scrolling_keys_move_within_message_bounds() {
        let mut prompt = RetryPrompt {
            scroll: 0,
            scroll_limit: 25,
        };
        for (code, expected) in [
            (KeyCode::Char('k'), 0),
            (KeyCode::Char('j'), 1),
            (KeyCode::Down, 2),
            (KeyCode::PageDown, 2),
            (KeyCode::PageUp, 2),
            (KeyCode::Char('G'), 25),
            (KeyCode::Down, 25),
            (KeyCode::Up, 24),
            (KeyCode::Char('g'), 0),
        ] {
            assert_eq!(prompt.handle(&InputEvent::Key(KeyEvent::from(code))), None);
            assert_eq!(prompt.scroll, expected);
        }
        prompt.handle(&InputEvent::Key(KeyEvent::new(
            KeyCode::Char('G'),
            KeyModifiers::SHIFT,
        )));
        assert_eq!(prompt.scroll, 25);
    }

    #[test]
    fn long_message_scrolls_in_small_terminal() {
        let paths = (1..=40)
            .map(|index| format!("profiles[profile-{index}].name"))
            .collect::<Vec<_>>()
            .join(", ");
        let message = format!(
            "Conflicts at {paths}. Keep the desired values and remove the conflict markers."
        );
        let mut terminal = Terminal::new(TestBackend::new(64, 12)).unwrap();
        let mut prompt = RetryPrompt::default();
        terminal.draw(|frame| prompt.draw(frame, &message)).unwrap();
        let top = buffer_to_styled_string(terminal.backend().buffer());
        prompt.handle(&InputEvent::Key(KeyEvent::from(KeyCode::Char('G'))));
        terminal.draw(|frame| prompt.draw(frame, &message)).unwrap();
        let bottom = buffer_to_styled_string(terminal.backend().buffer());
        insta::assert_snapshot!(format!("TOP\n{top}\nBOTTOM\n{bottom}"));
    }

    #[test]
    fn retry_prompt_shows_conflict_instructions() {
        let mut terminal =
            Terminal::new(TestBackend::new(APP_WINDOW_COLS, APP_WINDOW_ROWS)).unwrap();
        terminal
            .draw(|frame| {
                RetryPrompt::default().draw(frame,
            "Conflicts at settings.theme. Keep the desired values and remove the conflict markers."
        )
            })
            .unwrap();
        insta::assert_snapshot!(buffer_to_styled_string(terminal.backend().buffer()));
    }
}

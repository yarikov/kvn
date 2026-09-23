mod key;
mod mouse;
mod pointer;
mod toast;

use std::io;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;

use crate::app::model::{AppStatus, MainPaneFocus, Model, Overlay};
use crate::app::msg::{IpcCommand, Msg, StateSnapshot};
use crate::ipc::IpcClient;
use crate::services::LogTailer;
use crate::ui::layout::{LogNavigation, LogSelection};
use crate::ui::styles::Theme;

use super::{TuiExit, apply_snapshot, apply_terminal_colors, input, theme_watch};
use key::GoFirstSequence;
use pointer::{ClickTracker, PointerShape, update_pointer_shape};
use toast::ToastState;

pub(super) enum Flow {
    Continue,
    Exit(TuiExit),
}

pub(super) struct ClientLoop<'a> {
    terminal: &'a mut Terminal<CrosstermBackend<io::Stdout>>,
    model: &'a mut Model,
    client: &'a mut IpcClient,
    log_tailer: &'a mut LogTailer,
    event_reader_control: Arc<input::EventReaderControl>,
    pane_focus: MainPaneFocus,
    log_navigation: LogNavigation,
    log_selection: Option<LogSelection>,
    log_dragging: bool,
    go_first_sequence: GoFirstSequence,
    toast: ToastState,
    pointer_shape: PointerShape,
    mouse_position: Option<(u16, u16)>,
    click_tracker: ClickTracker,
    pending_error_status_clear: Option<u64>,
    needs_redraw: bool,
}

impl<'a> ClientLoop<'a> {
    pub(super) fn new(
        terminal: &'a mut Terminal<CrosstermBackend<io::Stdout>>,
        model: &'a mut Model,
        client: &'a mut IpcClient,
        log_tailer: &'a mut LogTailer,
        event_reader_control: Arc<input::EventReaderControl>,
    ) -> Result<Self> {
        let pane_focus = model.main_pane_focus;
        let toast = ToastState::new(model.status_revision);
        let mut state = Self {
            terminal,
            model,
            client,
            log_tailer,
            event_reader_control,
            pane_focus,
            log_navigation: LogNavigation::default(),
            log_selection: None,
            log_dragging: false,
            go_first_sequence: GoFirstSequence::default(),
            toast,
            pointer_shape: PointerShape::Default,
            mouse_position: None,
            click_tracker: ClickTracker::default(),
            pending_error_status_clear: None,
            needs_redraw: false,
        };
        if !crate::ui::layout::logs_visible(state.terminal_area()?)
            && state.pane_focus == MainPaneFocus::Logs
        {
            state.focus_pane(MainPaneFocus::Sources)?;
        }
        Ok(state)
    }

    pub(super) fn initial_draw(&mut self) -> Result<()> {
        let clear_initial_error = self
            .toast
            .show_initial_error(self.model.status.clone(), Instant::now());
        self.draw()?;
        if clear_initial_error {
            self.client.send(&IpcCommand::ClearErrorStatus {
                status_revision: self.model.status_revision,
            })?;
        }
        Ok(())
    }

    pub(super) fn handle(&mut self, msg: Msg) -> Result<Flow> {
        self.needs_redraw = false;
        let flow = match msg {
            Msg::Mouse(mouse) => mouse::handle(self, mouse)?,
            Msg::Paste(text) => self.paste(text)?,
            Msg::Key(key) => key::handle(self, key)?,
            Msg::StateUpdate { snapshot, .. } => self.apply_state_update(*snapshot)?,
            Msg::IpcReadFailed { message, .. } => anyhow::bail!(message),
            Msg::Tick => self.tick(),
            Msg::Resize => self.resize()?,
            Msg::ThemeChanged(theme)
                if self.model.config.settings.theme == theme_watch::OMARCHY_SENTINEL =>
            {
                self.change_theme(theme)
            }
            _ => Flow::Continue,
        };
        if matches!(flow, Flow::Continue) && self.needs_redraw {
            self.draw()?;
            if let Some(status_revision) = self.pending_error_status_clear.take() {
                self.client
                    .send(&IpcCommand::ClearErrorStatus { status_revision })?;
            }
        }
        Ok(flow)
    }

    fn paste(&mut self, text: String) -> Result<Flow> {
        if self.model.overlay == Overlay::None && self.pane_focus == MainPaneFocus::Sources {
            self.client.send(&IpcCommand::Paste { text })?;
        }
        Ok(Flow::Continue)
    }

    fn apply_state_update(&mut self, snapshot: StateSnapshot) -> Result<Flow> {
        self.pane_focus = snapshot.main_pane_focus;
        let toast_status = if snapshot.status_is_error {
            AppStatus::Error(snapshot.status.clone())
        } else {
            AppStatus::Info(snapshot.status.clone())
        };
        self.pending_error_status_clear =
            self.toast
                .observe(snapshot.status_revision, toast_status, Instant::now());
        apply_snapshot(self.model, snapshot);
        if self.model.restart_required {
            self.model.overlay = Overlay::RestartRequired;
        }
        self.focus_sources_when_logs_hidden()?;
        if self.model.overlay != Overlay::None {
            self.clear_log_selection();
        }
        self.refresh_pointer_shape()?;
        self.needs_redraw = true;
        Ok(Flow::Continue)
    }

    fn tick(&mut self) -> Flow {
        let now = Instant::now();
        self.log_navigation.expire_if_idle(now);
        self.toast.expire(now);
        let new_lines = self.log_tailer.tail();
        if !new_lines.is_empty() && !self.log_dragging {
            self.log_selection = None;
        }
        for line in new_lines {
            if self.model.logs.len() == crate::app::model::MAX_LOG_LINES {
                self.log_navigation.oldest_log_evicted();
            }
            self.model.push_log(line);
        }
        self.needs_redraw = true;
        Flow::Continue
    }

    fn resize(&mut self) -> Result<Flow> {
        self.clear_log_selection();
        self.focus_sources_when_logs_hidden()?;
        self.refresh_pointer_shape()?;
        self.needs_redraw = true;
        Ok(Flow::Continue)
    }

    fn change_theme(&mut self, theme: Theme) -> Flow {
        let previous = (
            self.model.theme.palette_foreground(),
            self.model.theme.palette_background(),
        );
        self.model.theme = theme;
        let current = (
            self.model.theme.palette_foreground(),
            self.model.theme.palette_background(),
        );
        if current != previous {
            apply_terminal_colors(current.0, current.1);
        }
        self.needs_redraw = true;
        Flow::Continue
    }

    fn draw(&mut self) -> Result<()> {
        let model = &*self.model;
        let pane_focus = self.pane_focus;
        let log_navigation = &self.log_navigation;
        let log_selection = self.log_selection.as_ref();
        let toast = self.toast.current();
        self.terminal.draw(|frame| {
            crate::ui::layout::draw_with_toast(
                frame,
                model,
                pane_focus,
                Some(log_navigation),
                log_selection,
                toast,
            )
        })?;
        Ok(())
    }

    fn terminal_area(&self) -> Result<Rect> {
        Ok(self.terminal.size()?.into())
    }

    fn focus_pane(&mut self, focus: MainPaneFocus) -> Result<()> {
        self.pane_focus = focus;
        self.client.send(&IpcCommand::SetMainPaneFocus { focus })
    }

    fn focus_sources_when_logs_hidden(&mut self) -> Result<()> {
        if crate::ui::layout::logs_visible(self.terminal_area()?)
            || self.pane_focus != MainPaneFocus::Logs
        {
            return Ok(());
        }
        self.pane_focus = MainPaneFocus::Sources;
        if !self.model.restart_required {
            self.client.send(&IpcCommand::SetMainPaneFocus {
                focus: MainPaneFocus::Sources,
            })?;
        }
        Ok(())
    }

    fn clear_log_selection(&mut self) {
        self.log_selection = None;
        self.log_dragging = false;
    }

    fn refresh_pointer_shape(&mut self) -> Result<Option<usize>> {
        update_pointer_shape(
            self.terminal,
            self.model,
            self.mouse_position,
            &mut self.pointer_shape,
        )
    }

    fn forward_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        use crossterm::event::{KeyCode, KeyModifiers};

        let (code, char) = match key.code {
            KeyCode::Char(c) => ("Char".to_string(), Some(c)),
            other => (format!("{:?}", other), None),
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        self.client.send(&IpcCommand::Key { code, char, ctrl })
    }
}

use std::collections::VecDeque;
use std::time::Instant;

use anyhow::Result;
use uuid::Uuid;

use crate::app::model::Overlay;
use crate::app::msg::IpcCommand;
use crate::app::scroll::{ScrollPosition, context};

use super::ClientLoop;

#[derive(Default)]
pub(super) struct ScrollQueue {
    pending: Option<PendingScroll>,
    events: VecDeque<(Overlay, isize)>,
}

struct PendingScroll {
    id: Uuid,
    context: Overlay,
    sent_at: Instant,
}

impl ScrollQueue {
    pub(super) fn cancel(&mut self) {
        self.events.clear();
        self.pending = None;
    }

    pub(super) fn expire(&mut self, now: Instant) -> bool {
        if self.pending.as_ref().is_some_and(|pending| {
            now.saturating_duration_since(pending.sent_at) >= super::IPC_INTERACTION_TIMEOUT
        }) {
            self.cancel();
            true
        } else {
            false
        }
    }

    pub(super) fn acknowledge(&mut self, response: Option<Uuid>, overlay: Overlay) -> bool {
        if !self
            .pending
            .as_ref()
            .is_some_and(|pending| Some(pending.id) == response)
        {
            return false;
        }
        let pending = self.pending.take().unwrap();
        pending.context == context(overlay)
    }
}

pub(super) fn enqueue(state: &mut ClientLoop, delta: isize) -> Result<()> {
    if state.model.overlay == Overlay::Support {
        if let Some((rows, selected)) = crate::app::scroll::list(state.model) {
            let position = crate::app::scroll::scroll(
                &rows,
                rows.len(),
                ScrollPosition { start: 0, selected },
                delta,
            );
            crate::app::scroll::select(state.model, position.selected);
            state.needs_redraw = true;
        }
        return Ok(());
    }
    state
        .scroll_queue
        .events
        .push_back((context(state.model.overlay), delta));
    send_next(state)
}

pub(super) fn send_next(state: &mut ClientLoop) -> Result<()> {
    if state.scroll_queue.pending.is_some() {
        return Ok(());
    }
    while let Some((target, delta)) = state.scroll_queue.events.pop_front() {
        if target != context(state.model.overlay) {
            continue;
        }
        let Some((start, visible)) =
            crate::ui::layout::wheel_window(state.model, state.terminal_area()?)
        else {
            continue;
        };
        let id = state.client.send_request(&IpcCommand::ScrollViewport {
            context: target,
            start,
            visible,
            delta,
        })?;
        state.scroll_queue.pending = Some(PendingScroll {
            id,
            context: target,
            sent_at: Instant::now(),
        });
        break;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_matching_reply_releases_the_pending_scroll() {
        let id = Uuid::new_v4();
        let mut queue = ScrollQueue {
            pending: Some(PendingScroll {
                id,
                context: Overlay::None,
                sent_at: Instant::now(),
            }),
            events: VecDeque::from([(Overlay::None, 1), (Overlay::None, -1)]),
        };
        assert!(!queue.acknowledge(Some(Uuid::new_v4()), Overlay::None));
        assert!(queue.pending.is_some());
        assert!(queue.acknowledge(Some(id), Overlay::None));
        assert!(queue.pending.is_none());
        assert_eq!(queue.events.pop_front(), Some((Overlay::None, 1)));
        assert_eq!(queue.events.pop_front(), Some((Overlay::None, -1)));
    }

    #[test]
    fn cancellation_discards_old_events_and_does_not_apply_inflight_result() {
        let id = Uuid::new_v4();
        let mut queue = ScrollQueue {
            pending: Some(PendingScroll {
                id,
                context: Overlay::None,
                sent_at: Instant::now(),
            }),
            ..Default::default()
        };
        queue.events.push_back((Overlay::None, 3));
        queue.cancel();
        assert!(queue.pending.is_none());
        assert!(queue.events.is_empty());
        assert!(!queue.acknowledge(Some(id), Overlay::None));
        assert!(queue.pending.is_none());
    }

    #[test]
    fn missing_reply_expires_without_replaying_uncertain_scroll() {
        let now = Instant::now();
        let id = Uuid::new_v4();
        let mut queue = ScrollQueue {
            pending: Some(PendingScroll {
                id,
                context: Overlay::None,
                sent_at: now,
            }),
            events: VecDeque::from([(Overlay::None, 7)]),
        };
        assert!(!queue.expire(
            now + super::super::IPC_INTERACTION_TIMEOUT - std::time::Duration::from_millis(1)
        ));
        assert!(queue.expire(now + super::super::IPC_INTERACTION_TIMEOUT));
        assert!(queue.pending.is_none());
        assert!(queue.events.is_empty());
        let next_id = Uuid::new_v4();
        queue.pending = Some(PendingScroll {
            id: next_id,
            context: Overlay::None,
            sent_at: now + super::super::IPC_INTERACTION_TIMEOUT,
        });
        assert!(!queue.acknowledge(Some(id), Overlay::None));
        assert!(queue.pending.is_some());
        assert!(queue.acknowledge(Some(next_id), Overlay::None));
    }

    #[test]
    fn reply_for_closed_context_is_not_applied() {
        let id = Uuid::new_v4();
        let mut queue = ScrollQueue {
            pending: Some(PendingScroll {
                id,
                context: Overlay::ThemeSettings,
                sent_at: Instant::now(),
            }),
            ..Default::default()
        };
        assert!(!queue.acknowledge(Some(id), Overlay::None));
    }
}

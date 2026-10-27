use anyhow::Result;
use crossterm::event::KeyEvent;

use crate::app::msg::RoutingEditResult;
use crate::tui_client::editor::submit_routing_rules;

use super::super::ClientLoop;
use super::editor::{self, EditedFile};

pub(super) fn apply(state: &mut ClientLoop, key: KeyEvent) -> Result<()> {
    let Some(edit) = state.model.routing_rules_edit() else {
        return state.forward_key(key);
    };
    let saved = match submit_routing_rules(&edit) {
        Ok(RoutingEditResult::Saved) => true,
        Ok(RoutingEditResult::Conflict { current, paths }) => editor::open(
            state,
            EditedFile::RoutingRulesConflict {
                edit,
                current,
                paths,
            },
        )?,
        Ok(RoutingEditResult::Failed { message }) => {
            state.report_error(not_saved(&message))?;
            false
        }
        Err(error) => {
            state.report_error(not_saved(&format!("{error:#}")))?;
            false
        }
    };
    if saved {
        state.forward_key(key)?;
    }
    state.needs_redraw = true;
    Ok(())
}

fn not_saved(reason: &str) -> String {
    format!("Routing rules were not saved: {reason}")
}

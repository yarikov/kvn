use anyhow::Result;

use crate::app::msg::IpcCommand;
use crate::tui_client::clipboard;

use super::super::ClientLoop;

pub(super) fn paste(state: &mut ClientLoop) -> Result<()> {
    match clipboard::read_clipboard_text() {
        Ok(text) => state.client.send(&IpcCommand::Paste { text })?,
        Err(error) => state.client.send(&IpcCommand::ClientError {
            message: format!("Failed to read clipboard: {error:#}"),
        })?,
    }
    Ok(())
}

pub(super) fn copy_selected(state: &mut ClientLoop) -> Result<()> {
    if let Some(profile) = state.model.selected_profile() {
        if let Ok(link) = crate::config::profile::encode_share_link(profile)
            && clipboard::write_clipboard_text(&link).is_ok()
        {
            let name = profile.name.clone();
            state.client.send(&IpcCommand::Copied { name, count: 1 })?;
        }
    } else if let Some(subscription) = state.model.selected_subscription()
        && clipboard::write_clipboard_text(&subscription.url).is_ok()
    {
        let name = subscription.name.clone();
        state.client.send(&IpcCommand::Copied { name, count: 1 })?;
    }
    Ok(())
}

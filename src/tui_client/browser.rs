use std::process::{Command, Stdio};

use anyhow::{Context, Result};

pub(super) fn open_support_page() -> Result<()> {
    let mut child = Command::new("xdg-open")
        .arg(crate::support_prompt::SUPPORT_URL)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("could not start xdg-open")?;
    // Reap the short-lived launcher without blocking the TUI. The browser
    // itself is detached by xdg-open.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

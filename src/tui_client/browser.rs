use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

pub(super) fn open_support_page() -> Result<()> {
    let mut command = Command::new("xdg-open");
    command
        .arg(crate::support_prompt::SUPPORT_URL)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command.spawn().context("could not start xdg-open")?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

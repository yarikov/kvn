use std::process::Command;

/// The systemd user unit that runs the daemon. Every call site resolves it
/// through this constant so a rename lands in one place.
pub(crate) const DAEMON_UNIT: &str = "kvn-tui.service";

pub(crate) fn restart_daemon_unit() {
    match Command::new("systemctl")
        .args(["--user", "restart", DAEMON_UNIT])
        .status()
    {
        Ok(status) if status.success() => {
            println!("Restarted {DAEMON_UNIT}.");
        }
        Ok(status) => print_restart_hint(&format!("systemctl exited with {status}")),
        Err(error) => print_restart_hint(&format!("systemctl could not be started ({error})")),
    }
}

fn print_restart_hint(reason: &str) {
    eprintln!("Could not restart the kvn daemon automatically: {reason}.");
    eprintln!("Run: systemctl --user restart {DAEMON_UNIT}");
}

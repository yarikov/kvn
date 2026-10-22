/// The systemd user unit that runs the daemon. Every call site resolves it
/// through this constant so a rename lands in one place.
pub(crate) const DAEMON_UNIT: &str = "kvn-tui.service";

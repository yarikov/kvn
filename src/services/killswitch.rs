//! Network kill switch: a system-level firewall that blocks all non-VPN egress
//! when enabled. The actual firewall rules are loaded by a systemd unit
//! (`kvn-tui-killswitch.service`) that runs a wrapped `nft -f` at boot. This
//! module shells out to a helper script installed at
//! `/usr/lib/kvn-tui/killswitch-helper.sh` (via `sudo -n`, NOPASSWD for the
//! dedicated `kvn-tui` group) to enable/disable the unit and add/remove handshake
//! exceptions while sing-box is establishing the VPN tunnel.
//!
//! See `contrib/setup-killswitch.sh` for the one-time setup that the user
//! runs as `sudo kvn setup --killswitch`.

use anyhow::{Context, Result, bail};
use std::net::{SocketAddr, ToSocketAddrs};
use std::process::Command;

const HELPER: &str = "/usr/lib/kvn-tui/killswitch-helper.sh";
const UNIT: &str = "kvn-tui-killswitch.service";
const INTEGRATION_GROUP: &str = "kvn-tui";

fn helper_present() -> bool {
    std::path::Path::new(HELPER).is_file()
}

fn run_helper(args: &[&str]) -> Result<()> {
    if !helper_present() {
        bail!(
            "kill switch helper not installed at {} — run `sudo kvn setup --killswitch`",
            HELPER
        );
    }
    let output = Command::new("sudo")
        .arg("-n")
        .arg(HELPER)
        .args(args)
        .output()
        .with_context(|| format!("failed to spawn sudo {}", HELPER))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "{} {:?} failed ({}): {}{}",
            HELPER,
            args,
            output.status,
            stderr.trim(),
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!(" / {}", stdout.trim())
            }
        );
    }
    Ok(())
}

/// Enable or disable the kill switch systemd unit (also starts/stops it).
pub fn apply(enabled: bool) -> Result<()> {
    if enabled && helper_present() {
        ensure_integration_group_ready(integration_group_status())?;
    }
    if !enabled && !helper_present() && !helper_needed_to_disable(is_active().ok()) {
        return Ok(());
    }
    run_helper(&[if enabled { "enable" } else { "disable" }])
}

fn helper_needed_to_disable(active: Option<bool>) -> bool {
    active != Some(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntegrationGroup {
    Active,
    PendingActivation,
    NotMember,
}

fn ensure_integration_group_ready(status: Option<IntegrationGroup>) -> Result<()> {
    match status {
        Some(IntegrationGroup::PendingActivation) => {
            bail!("reboot to activate the `kvn-tui` group")
        }
        Some(IntegrationGroup::NotMember) => bail!(
            "the current user is not in the `kvn-tui` group; run `sudo kvn setup --killswitch`, then reboot"
        ),
        Some(IntegrationGroup::Active) | None => Ok(()),
    }
}

pub(crate) fn integration_group_status() -> Option<IntegrationGroup> {
    let active = id_output(&["-Gn"])?;
    if group_list_includes_integration_group(&active) {
        return Some(IntegrationGroup::Active);
    }
    let username = id_output(&["-un"])?;
    let username = std::str::from_utf8(&username).ok()?.trim();
    let configured_groups = id_output(&["-Gn", username])?;
    Some(
        if group_list_includes_integration_group(&configured_groups) {
            IntegrationGroup::PendingActivation
        } else {
            IntegrationGroup::NotMember
        },
    )
}

pub(crate) fn integration_group_pending_activation() -> Option<bool> {
    integration_group_status().map(|status| status == IntegrationGroup::PendingActivation)
}

fn id_output(args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("id").args(args).output().ok()?;
    output.status.success().then_some(output.stdout)
}

fn group_list_includes_integration_group(output: &[u8]) -> bool {
    String::from_utf8_lossy(output)
        .split_whitespace()
        .any(|group| group == INTEGRATION_GROUP)
}

/// Add a temporary exception so sing-box can reach the given endpoint during
/// the TLS/REALITY handshake (before the tun interface is up). Idempotent at
/// the nft layer (set elements dedupe).
pub fn allow_endpoint(addr: &SocketAddr, proto: &str) -> Result<()> {
    let ip = addr.ip().to_string();
    let port = addr.port().to_string();
    run_helper(&["allow", &ip, proto, &port])
}

/// Flush the dynamic handshake set. Called on disconnect.
pub fn revoke() -> Result<()> {
    run_helper(&["revoke"])
}

/// Resolve a `host:port` to one or more socket addresses (blocking).
pub fn resolve_endpoints(host: &str, port: u16) -> Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("DNS lookup failed for {}:{}", host, port))?
        .collect();
    if addrs.is_empty() {
        bail!("no addresses resolved for {}:{}", host, port);
    }
    Ok(addrs)
}

/// Query systemd for the actual unit state. Used at daemon startup to
/// reconcile `settings.kill_switch` (which may have been edited externally or
/// set on a host where the helper isn't installed).
pub fn is_active() -> Result<bool> {
    let output = Command::new("systemctl")
        .arg("is-active")
        .arg("--quiet")
        .arg(UNIT)
        .output()
        .context("failed to spawn systemctl")?;
    classify_active_exit_code(output.status.code()).with_context(|| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        format!(
            "failed to query {} state ({}): {}",
            UNIT,
            output.status,
            stderr.trim()
        )
    })
}

fn classify_active_exit_code(code: Option<i32>) -> Result<bool> {
    match code {
        Some(0) => Ok(true),
        Some(3 | 4) => Ok(false),
        Some(code) => bail!("systemctl is-active exited with code {code}"),
        None => bail!("systemctl is-active terminated by signal"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        IntegrationGroup, classify_active_exit_code, ensure_integration_group_ready,
        group_list_includes_integration_group, helper_needed_to_disable,
    };

    #[test]
    fn disabling_an_inactive_unit_does_not_need_the_helper() {
        assert!(!helper_needed_to_disable(Some(false)));
        assert!(helper_needed_to_disable(Some(true)));
        assert!(helper_needed_to_disable(None));
    }

    #[test]
    fn active_exit_code_distinguishes_inactive_from_errors() {
        assert!(classify_active_exit_code(Some(0)).unwrap());
        assert!(!classify_active_exit_code(Some(3)).unwrap());
        assert!(classify_active_exit_code(Some(1)).is_err());
        assert!(!classify_active_exit_code(Some(4)).unwrap());
        assert!(classify_active_exit_code(None).is_err());
    }

    #[test]
    fn integration_group_requires_an_exact_group_name() {
        assert!(group_list_includes_integration_group(
            b"users wheel kvn-tui\n"
        ));
        assert!(!group_list_includes_integration_group(
            b"users wheel kvn-tui-old\n"
        ));
    }

    #[test]
    fn inactive_integration_group_has_actionable_error() {
        assert!(ensure_integration_group_ready(Some(IntegrationGroup::Active)).is_ok());
        assert!(ensure_integration_group_ready(None).is_ok());

        let pending = ensure_integration_group_ready(Some(IntegrationGroup::PendingActivation))
            .unwrap_err()
            .to_string();
        assert_eq!(pending, "reboot to activate the `kvn-tui` group");

        let missing = ensure_integration_group_ready(Some(IntegrationGroup::NotMember))
            .unwrap_err()
            .to_string();
        assert!(missing.contains("not in the `kvn-tui` group"));
        assert!(missing.contains("sudo kvn setup --killswitch"));
    }
}

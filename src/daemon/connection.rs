use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::app::model::{AppStatus, ConnectionState, Model, Overlay, TrafficStats};
use crate::app::msg::{IpcError, Msg};
use crate::config::profile::{DnsConfig, Profile, Protocol, Settings};

use super::DaemonShared;
use super::process_slot::{is_current_attempt, lock_process_slot};

const LOG_PRUNE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

pub(super) fn connect(
    tx: &Sender<Msg>,
    model: &mut Model,
    shared: &DaemonShared,
    profile: Profile,
    settings: Settings,
    attempt_id: u64,
) {
    let previous = {
        let mut slot = lock_process_slot(&shared.process_slot);
        slot.attempt_id = attempt_id;
        slot.handle.take()
    };
    if let Some(mut handle) = previous
        && let Err(e) = handle.kill_and_wait()
    {
        tracing::warn!("Failed to stop sing-box process: {}", e);
    }
    model.connection = ConnectionState::ConnectPending;
    let tx = tx.clone();
    let slot = shared.process_slot.clone();
    let coordinator = shared.connect_coordinator.clone();
    let log_pruned_at = shared.singbox_log_pruned_at.clone();
    let kill_switch = model.config.settings.kill_switch;
    let dns = settings.dns.clone();
    thread::spawn(move || {
        let _coordinator = coordinator.lock().unwrap_or_else(|p| p.into_inner());
        if !is_current_attempt(&slot, attempt_id) {
            return;
        }
        if kill_switch {
            if let Err(e) = crate::services::killswitch::revoke() {
                let err = IpcError::from(e.context("failed to clear stale kill switch exceptions"));
                let _ = tx.send(Msg::ConnectFailed {
                    attempt_id,
                    error: err,
                });
                return;
            }
            if !is_current_attempt(&slot, attempt_id) {
                return;
            }
            if let Err(e) = open_handshake_window(&profile, &dns) {
                if let Err(cleanup_err) = crate::services::killswitch::revoke() {
                    tracing::warn!(
                        "Failed to clean up kill switch exceptions after handshake error: {}",
                        cleanup_err
                    );
                }
                let err = IpcError::from(e.context("kill switch handshake setup failed"));
                let _ = tx.send(Msg::ConnectFailed {
                    attempt_id,
                    error: err,
                });
                return;
            }
        }
        if !is_current_attempt(&slot, attempt_id) {
            return;
        }
        let now = Instant::now();
        let mut last_pruned = log_pruned_at.lock().unwrap_or_else(|p| p.into_inner());
        if log_prune_due(*last_pruned, now) {
            let log_path = crate::paths::singbox_log_path();
            match crate::services::log_tailer::prune_log_to_lines(
                &log_path,
                settings.logs.line_retention.singbox,
            ) {
                Ok(()) => *last_pruned = Some(now),
                Err(error) => tracing::warn!(
                    "Failed to enforce log line limit for {:?}: {}",
                    log_path,
                    error
                ),
            }
        }
        drop(last_pruned);
        match crate::singbox::runner::start(&profile, &settings) {
            Ok(handle) => {
                let pid = handle.pid;
                let stale_handle = {
                    let mut slot = lock_process_slot(&slot);
                    if slot.attempt_id == attempt_id {
                        slot.handle = Some(handle);
                        None
                    } else {
                        Some(handle)
                    }
                };
                if let Some(mut handle) = stale_handle {
                    let _ = handle.kill_and_wait();
                    return;
                }
                let _ = tx.send(Msg::Connected {
                    pid,
                    profile_id: profile.id,
                    attempt_id,
                });
            }
            Err(e) => {
                if kill_switch && let Err(cleanup_err) = crate::services::killswitch::revoke() {
                    tracing::warn!(
                        "Failed to clean up kill switch exceptions after connect failure: {}",
                        cleanup_err
                    );
                }
                let _ = tx.send(Msg::ConnectFailed {
                    attempt_id,
                    error: IpcError::from(e),
                });
            }
        }
    });
}

pub(super) fn disconnect(model: &mut Model, shared: &DaemonShared) {
    model.connect_attempt_id = model.connect_attempt_id.wrapping_add(1);
    {
        let mut slot = lock_process_slot(&shared.process_slot);
        slot.attempt_id = model.connect_attempt_id;
    }
    // Wait for an in-flight setup to observe invalidation and stop
    // before flushing its temporary kill-switch exceptions.
    let _coordinator = shared
        .connect_coordinator
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let previous = {
        let mut slot = lock_process_slot(&shared.process_slot);
        slot.handle.take()
    };
    if let Some(mut handle) = previous
        && let Err(e) = handle.kill_and_wait()
    {
        tracing::warn!("Failed to stop sing-box process: {}", e);
    }
    model.connection = ConnectionState::Idle;
    model.active_profile_id = None;
    model.connecting_profile_id = None;
    model.singbox_pid = None;
    model.traffic = TrafficStats::default();
    model.last_traffic_sample_at_ms = 0;
    model.traffic_request_id = 0;
    model.last_traffic_response_id = 0;
    model.last_traffic_fetch_at = None;
    model.set_status(AppStatus::Info("Disconnected".into()));
    model.overlay = Overlay::None;
    crate::services::waybar::write_state(model);
    if model.config.settings.kill_switch
        && let Err(e) = crate::services::killswitch::revoke()
    {
        tracing::warn!("Failed to flush kill switch handshake set: {}", e);
    }
}

pub(super) fn revoke_kill_switch_exceptions(model: &Model) {
    if model.config.settings.kill_switch
        && let Err(e) = crate::services::killswitch::revoke()
    {
        tracing::warn!(
            "Failed to flush kill switch handshake set after sing-box exit: {}",
            e
        );
    }
}

pub(super) fn apply_kill_switch(tx: &Sender<Msg>, enabled: bool) {
    let tx = tx.clone();
    thread::spawn(move || {
        let error = crate::services::killswitch::apply(enabled)
            .err()
            .map(IpcError::from);
        let _ = tx.send(Msg::KillSwitchApplied { enabled, error });
    });
}

pub(super) fn check_integration_setup(tx: &Sender<Msg>) {
    let tx = tx.clone();
    thread::spawn(move || {
        let setup = crate::doctor::integration_setup(std::path::Path::new("/"));
        let _ = tx.send(Msg::IntegrationSetupChecked(setup));
    });
}

pub(super) fn check_auto_connect_polkit(tx: &Sender<Msg>) {
    let tx = tx.clone();
    thread::spawn(move || {
        let error = crate::doctor::polkit_readiness().err().map(IpcError::from);
        let _ = tx.send(Msg::AutoConnectPolkitChecked { error });
    });
}

/// Pre-resolve the VPN endpoint and open a temporary nft exception so the
/// initial handshake can pass through the kill switch. Also allowlists every
/// non-`local`, non-`fakeip` DNS upstream the user has configured so sing-box
/// can resolve the VPN server hostname (see `src/singbox/config.rs`).
///
/// Set elements are deduplicated by nftables and remain until disconnect, so
/// repeated calls are idempotent and safe across reconnects.
fn open_handshake_window(profile: &Profile, dns: &DnsConfig) -> Result<()> {
    let endpoints = crate::services::killswitch::resolve_endpoints(&profile.address, profile.port)?;
    for addr in &endpoints {
        for protocol in handshake_protocols(profile.protocol()) {
            crate::services::killswitch::allow_endpoint(addr, protocol)?;
        }
    }
    for (host, port, proto) in dns_bootstrap_endpoints(dns) {
        match crate::services::killswitch::resolve_endpoints(&host, port) {
            Ok(addrs) => {
                for addr in &addrs {
                    crate::services::killswitch::allow_endpoint(addr, proto)?;
                }
            }
            Err(e) => {
                tracing::warn!("DNS upstream {host}:{port} resolution failed: {e}");
            }
        }
    }
    Ok(())
}

/// Network protocols that must be allowed to reach a VPN endpoint before the
/// tunnel is established. QUIC-based outbounds use UDP, while SOCKS and
/// Shadowsocks may carry traffic over either transport.
fn handshake_protocols(protocol: Protocol) -> &'static [&'static str] {
    match protocol {
        Protocol::Hysteria2 | Protocol::Tuic => &["udp"],
        Protocol::Shadowsocks | Protocol::Socks => &["tcp", "udp"],
        Protocol::Vless
        | Protocol::Vmess
        | Protocol::Trojan
        | Protocol::Shadowtls
        | Protocol::Anytls
        | Protocol::Http
        | Protocol::Ssh => &["tcp"],
    }
}

/// Return `(host, port, proto)` triples for every DNS server that needs an
/// outbound network allowlist before the tun interface is up. `local` and
/// `fakeip` servers are skipped — they never leave the host.
fn dns_bootstrap_endpoints(dns: &DnsConfig) -> Vec<(String, u16, &'static str)> {
    use crate::config::profile::DnsServer;
    dns.servers
        .iter()
        .filter_map(|s| match s {
            DnsServer::Local { .. } | DnsServer::FakeIp { .. } => None,
            DnsServer::Udp {
                server,
                server_port,
                ..
            } => Some((server.clone(), server_port.unwrap_or(53), "udp")),
            DnsServer::Tcp {
                server,
                server_port,
                ..
            } => Some((server.clone(), server_port.unwrap_or(53), "tcp")),
            DnsServer::Tls {
                server,
                server_port,
                ..
            } => Some((server.clone(), server_port.unwrap_or(853), "tcp")),
            DnsServer::Https {
                server,
                server_port,
                ..
            } => Some((server.clone(), server_port.unwrap_or(443), "tcp")),
            DnsServer::Quic {
                server,
                server_port,
                ..
            } => Some((server.clone(), server_port.unwrap_or(853), "udp")),
        })
        .collect()
}

fn log_prune_due(last_pruned: Option<Instant>, now: Instant) -> bool {
    last_pruned.is_none_or(|last| now.duration_since(last) >= LOG_PRUNE_INTERVAL)
}

#[cfg(test)]
mod tests {
    use super::{handshake_protocols, log_prune_due};
    use crate::config::profile::Protocol;
    use std::time::{Duration, Instant};

    #[test]
    fn log_prune_is_due_initially_and_after_twenty_four_hours() {
        let start = Instant::now();
        assert!(log_prune_due(None, start));
        assert!(!log_prune_due(
            Some(start),
            start + Duration::from_secs(24 * 60 * 60 - 1)
        ));
        assert!(log_prune_due(
            Some(start),
            start + Duration::from_secs(24 * 60 * 60)
        ));
    }

    #[test]
    fn handshake_protocols_match_outbound_transports() {
        for protocol in [Protocol::Hysteria2, Protocol::Tuic] {
            assert_eq!(handshake_protocols(protocol), &["udp"]);
        }

        for protocol in [Protocol::Shadowsocks, Protocol::Socks] {
            assert_eq!(handshake_protocols(protocol), &["tcp", "udp"]);
        }

        for protocol in [
            Protocol::Vless,
            Protocol::Vmess,
            Protocol::Trojan,
            Protocol::Shadowtls,
            Protocol::Anytls,
            Protocol::Http,
            Protocol::Ssh,
        ] {
            assert_eq!(handshake_protocols(protocol), &["tcp"]);
        }
    }
}

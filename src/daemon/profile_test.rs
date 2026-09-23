use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use uuid::Uuid;

use crate::app::model::Model;
use crate::app::msg::Msg;
use crate::config::profile::Profile;

pub(super) fn start(tx: &Sender<Msg>, model: &Model, id: Uuid) {
    let profile = model.config.profiles.iter().find(|p| p.id == id).cloned();
    let probe = model.config.settings.connectivity_probe.clone();
    let tx = tx.clone();
    thread::spawn(move || {
        let latency_ms = profile.and_then(|p| {
            if !probe.enabled {
                return None;
            }
            let probe_url = probe.url.as_deref()?;
            match run_test(&p, id, probe_url) {
                Ok(ms) => Some(ms),
                Err(e) => {
                    tracing::warn!("profile test failed: {e:#}");
                    None
                }
            }
        });
        let _ = tx.send(Msg::TestResult { id, latency_ms });
    });
}

/// Test a profile's reachability using a temporary sing-box instance.
///
/// Allocates a free loopback port, writes a minimal SOCKS5-inbound config,
/// spawns sing-box, waits for the port to open, then performs a SOCKS5
/// CONNECT to the configured HTTP(S) endpoint through the proxy and returns
/// the end-to-end request latency in milliseconds.
fn run_test(profile: &Profile, id: uuid::Uuid, probe_url: &str) -> anyhow::Result<u64> {
    use std::process::{Command, Stdio};

    let probe = crate::config::profile::parse_connectivity_probe_url(probe_url)?;

    let socks_port = crate::net::allocate_loopback_port()?;

    let config_path = write_test_config(profile, id, socks_port)?;

    let singbox_bin = std::env::var("SING_BOX_PATH").unwrap_or_else(|_| "sing-box".to_string());
    let mut child = Command::new(&singbox_bin)
        .args(["run", "-c", config_path.to_string_lossy().as_ref()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to spawn {singbox_bin}"))?;

    let cleanup = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&config_path);
    };

    // Wait up to 3 s for sing-box to open the SOCKS5 port.
    let addr = format!("127.0.0.1:{socks_port}");
    let deadline = Instant::now() + Duration::from_secs(3);
    let ready = loop {
        if Instant::now() >= deadline {
            break false;
        }
        if TcpStream::connect(&addr).is_ok() {
            break true;
        }
        thread::sleep(Duration::from_millis(80));
    };

    if !ready {
        cleanup(&mut child);
        anyhow::bail!("sing-box did not open SOCKS5 port within 3 s");
    }

    let result = socks5_connect_latency(&addr, &probe);
    cleanup(&mut child);
    result
}

fn write_test_config(
    profile: &Profile,
    id: uuid::Uuid,
    socks_port: u16,
) -> anyhow::Result<std::path::PathBuf> {
    let config = crate::singbox::config::generate_test_config(profile, socks_port)?;
    crate::paths::ensure_runtime_dir()?;
    let path = crate::paths::temp_test_config_path(&id)?;
    crate::atomic_write::write(&path, serde_json::to_string(&config)?.as_bytes())?;
    Ok(path)
}

/// Tunnel through the SOCKS5 proxy at `addr`, send a minimal HTTP(S) GET, and
/// return the time from request/TLS start to the first response byte.
///
/// sing-box replies to SOCKS5 CONNECT before the outbound tunnel is open, so
/// measuring CONNECT RTT gives ~0 ms. The HTTP round-trip through the actual
/// VPN tunnel is the meaningful latency number.
fn socks5_connect_latency(addr: &str, probe: &url::Url) -> anyhow::Result<u64> {
    let host = probe
        .host_str()
        .context("connectivity probe URL is missing a host")?;
    let host_bytes = host.as_bytes();
    anyhow::ensure!(
        host_bytes.len() <= u8::MAX as usize,
        "connectivity probe host is too long"
    );
    let port = probe
        .port_or_known_default()
        .context("connectivity probe URL has no port")?;
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;

    // SOCKS5 greeting: no-auth method selection.
    stream.write_all(&[0x05, 0x01, 0x00])?;
    let mut resp = [0u8; 2];
    stream.read_exact(&mut resp)?;
    anyhow::ensure!(resp == [0x05, 0x00], "SOCKS5 auth negotiation failed");

    // SOCKS5 CONNECT to target host (domain ATYP 0x03).
    let mut req = vec![0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8];
    req.extend_from_slice(host_bytes);
    req.push((port >> 8) as u8);
    req.push((port & 0xff) as u8);
    stream.write_all(&req)?;

    // Read and discard CONNECT reply — sing-box answers before the tunnel is
    // actually open, so this RTT is not meaningful.
    let mut hdr = [0u8; 4];
    stream.read_exact(&mut hdr)?;
    anyhow::ensure!(hdr[0] == 0x05 && hdr[1] == 0x00, "SOCKS5 CONNECT rejected");
    match hdr[3] {
        0x01 => {
            let mut b = [0u8; 6];
            stream.read_exact(&mut b)?;
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len)?;
            let mut b = vec![0u8; len[0] as usize + 2];
            stream.read_exact(&mut b)?;
        }
        0x04 => {
            let mut b = [0u8; 18];
            stream.read_exact(&mut b)?;
        }
        _ => anyhow::bail!("unknown SOCKS5 address type"),
    }

    let request_target = match probe.query() {
        Some(query) => format!("{}?{query}", probe.path()),
        None => probe.path().to_string(),
    };
    let host_header = match probe.host().expect("validated probe host") {
        url::Host::Ipv6(address) => format!("[{address}]"),
        host => host.to_string(),
    };
    let host_header = probe
        .port()
        .map_or(host_header.clone(), |port| format!("{host_header}:{port}"));
    let request = format!(
        "GET {request_target} HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\n\r\n"
    );

    // sing-box acknowledges SOCKS CONNECT before the outbound connection is
    // ready. For HTTPS, starting here includes TLS setup in the user-visible
    // end-to-end latency, matching ordinary web traffic more closely.
    let start = Instant::now();
    let mut buf = [0u8; 16];
    if probe.scheme() == "https" {
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let server_name = rustls::pki_types::ServerName::try_from(host.to_owned())
            .context("invalid connectivity probe TLS server name")?;
        let connection = rustls::ClientConnection::new(std::sync::Arc::new(config), server_name)
            .context("failed to initialize connectivity probe TLS")?;
        let mut tls = rustls::StreamOwned::new(connection, stream);
        tls.write_all(request.as_bytes())?;
        tls.read_exact(&mut buf)?;
    } else {
        stream.write_all(request.as_bytes())?;
        stream.read_exact(&mut buf)?;
    }
    Ok(start.elapsed().as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::write_test_config;
    use crate::config::profile::Profile;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn latency_test_config_is_private() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", runtime.path());
        let profile =
            Profile::new_vless("Test".into(), "1.2.3.4".into(), 443, "secret-uuid".into());
        let path = write_test_config(&profile, uuid::Uuid::new_v4(), 1080).unwrap();

        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

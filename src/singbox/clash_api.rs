//! Minimal HTTP client for sing-box's Clash-compatible experimental API.
//!
//! Only `GET /connections` is used today — it returns cumulative byte
//! counters and the list of active connections, enough to drive the live
//! traffic readout in the status bar. The endpoint is enabled via
//! `experimental.clash_api.external_controller` in the generated sing-box
//! config (see `src/singbox/config.rs`); its port is allocated per sing-box
//! start and passed in by the daemon.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

pub fn connections_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/connections")
}

/// Snapshot of cumulative byte counters and connection count parsed from
/// the Clash `/connections` endpoint.
#[derive(Debug, Clone, Default)]
pub struct ConnectionsSnapshot {
    pub up_total: u64,
    pub down_total: u64,
    pub conn_count: usize,
}

/// Subset of `/connections` we care about. Sing-box may add or rename fields
/// across versions — `#[serde(default)]` keeps deserialization forgiving.
#[derive(Debug, Deserialize, Default)]
struct ConnectionsResponse {
    #[serde(default, rename = "uploadTotal")]
    upload_total: u64,
    #[serde(default, rename = "downloadTotal")]
    download_total: u64,
    #[serde(default)]
    connections: Vec<serde_json::Value>,
}

/// Fetch `/connections` from sing-box's Clash API. Times out aggressively so
/// a stuck endpoint can't pile up daemon threads at 1 Hz.
pub fn fetch_connections(port: u16) -> Result<ConnectionsSnapshot> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .build();
    let agent: ureq::Agent = config.into();
    let resp = agent
        .get(&connections_url(port))
        .call()
        .context("GET /connections failed")?;
    if resp.status() != 200 {
        anyhow::bail!("clash_api returned HTTP {}", resp.status());
    }
    let raw = resp
        .into_body()
        .read_to_string()
        .context("Failed to read /connections body")?;
    let body: ConnectionsResponse =
        serde_json::from_str(&raw).context("Failed to parse /connections JSON")?;
    Ok(ConnectionsSnapshot {
        up_total: body.upload_total,
        down_total: body.download_total,
        conn_count: body.connections.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connections_url_uses_the_given_port() {
        assert_eq!(connections_url(41390), "http://127.0.0.1:41390/connections");
    }
}

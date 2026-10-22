use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

pub fn connections_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/connections")
}

#[derive(Debug, Clone, Default)]
pub struct ConnectionsSnapshot {
    pub up_total: u64,
    pub down_total: u64,
    pub conn_count: usize,
}

#[derive(Debug, Deserialize, Default)]
struct ConnectionsResponse {
    #[serde(default, rename = "uploadTotal")]
    upload_total: u64,
    #[serde(default, rename = "downloadTotal")]
    download_total: u64,
    #[serde(default)]
    connections: Vec<serde_json::Value>,
}

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

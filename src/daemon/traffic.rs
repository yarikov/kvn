use std::sync::mpsc::Sender;
use std::thread;

use crate::app::msg::Msg;

use super::DaemonShared;
use super::process_slot::lock_process_slot;

pub(super) fn fetch(tx: &Sender<Msg>, shared: &DaemonShared, attempt_id: u64, request_id: u64) {
    let clash_api_port = lock_process_slot(&shared.process_slot)
        .handle
        .as_ref()
        .map(|handle| handle.clash_api_port);
    let Some(clash_api_port) = clash_api_port else {
        return;
    };
    let tx = tx.clone();
    thread::spawn(move || {
        match crate::singbox::clash_api::fetch_connections(clash_api_port) {
            Ok(snap) => {
                let sampled_at_ms = unix_now_ms();
                let _ = tx.send(Msg::TrafficStatsUpdated {
                    attempt_id,
                    request_id,
                    up_total: snap.up_total,
                    down_total: snap.down_total,
                    conn_count: snap.conn_count,
                    sampled_at_ms,
                });
            }
            Err(e) => {
                // The endpoint may legitimately be unreachable for a few
                // hundred ms after sing-box spawns; don't surface this to
                // the user.
                tracing::debug!("clash_api fetch failed: {e}");
            }
        }
    });
}

/// Wall-clock time in milliseconds since the Unix epoch.
fn unix_now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

use crate::app::effect::Effect;
use crate::app::model::{ConnectionState, Model, TrafficStats};
use std::time::Duration;

/// Compute a per-second byte rate from two cumulative samples. Returns 0 when
/// no time has passed, when the counter went backwards (e.g. sing-box restart
/// reset its totals), or when there's no prior sample yet (`prev_at_ms == 0`).
/// Minimum interval between Clash-API scrapes. The daemon ticker fires every
/// 250 ms; we only emit `Effect::FetchTrafficStats` once per second.
pub(in crate::app::update) const TRAFFIC_POLL_INTERVAL: Duration = Duration::from_secs(1);

fn compute_rate(prev_total: u64, curr_total: u64, elapsed_ms: u64) -> u64 {
    if elapsed_ms == 0 {
        return 0;
    }
    let delta = curr_total.saturating_sub(prev_total);
    // (delta bytes / elapsed ms) * 1000 — done in u128 to avoid overflow.
    ((delta as u128 * 1000) / elapsed_ms as u128) as u64
}

pub(in crate::app::update) fn handle_traffic_stats_updated(
    model: &mut Model,
    attempt_id: u64,
    request_id: u64,
    up_total: u64,
    down_total: u64,
    conn_count: usize,
    sampled_at_ms: u64,
) -> Vec<Effect> {
    if model.connection != ConnectionState::Connected
        || attempt_id != model.connect_attempt_id
        || request_id <= model.last_traffic_response_id
    {
        return vec![];
    }
    let prev_at_ms = model.last_traffic_sample_at_ms;
    // No prior sample yet — record totals but leave rates at zero. The next
    // tick produces the first instantaneous reading against this baseline.
    let (up_rate, down_rate) = if prev_at_ms == 0 {
        (0, 0)
    } else {
        let elapsed = sampled_at_ms.saturating_sub(prev_at_ms);
        (
            compute_rate(model.traffic.up_total, up_total, elapsed),
            compute_rate(model.traffic.down_total, down_total, elapsed),
        )
    };
    model.traffic = TrafficStats {
        up_rate_bps: up_rate,
        down_rate_bps: down_rate,
        up_total,
        down_total,
        conn_count,
    };
    model.last_traffic_sample_at_ms = sampled_at_ms;
    model.last_traffic_response_id = request_id;
    vec![Effect::BroadcastState]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::update::tick::handle_tick;
    use crate::test_helpers::*;

    #[test]
    fn compute_rate_zero_elapsed_returns_zero() {
        assert_eq!(compute_rate(0, 1_000_000, 0), 0);
        assert_eq!(compute_rate(100, 1_000, 0), 0);
    }

    #[test]
    fn compute_rate_counter_rollback_saturates_to_zero() {
        // sing-box restarted mid-session — totals reset; saturating_sub.
        assert_eq!(compute_rate(10_000, 500, 1_000), 0);
    }

    #[test]
    fn compute_rate_basic() {
        // 2000 bytes delta over 1000 ms = 2000 B/s
        assert_eq!(compute_rate(1_000, 3_000, 1_000), 2_000);
        // 1500 bytes delta over 500 ms = 3000 B/s
        assert_eq!(compute_rate(0, 1_500, 500), 3_000);
    }

    #[test]
    fn compute_rate_fractional_second() {
        // 100 bytes over 250 ms = 400 B/s
        assert_eq!(compute_rate(0, 100, 250), 400);
    }

    #[test]
    fn traffic_stats_updated_first_sample_records_zero_rate() {
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Connected;
        let effects = handle_traffic_stats_updated(&mut model, 0, 1, 10_000, 50_000, 3, 1_000);
        assert_eq!(model.traffic.up_total, 10_000);
        assert_eq!(model.traffic.down_total, 50_000);
        assert_eq!(model.traffic.conn_count, 3);
        // First sample → no prior baseline → rates remain zero.
        assert_eq!(model.traffic.up_rate_bps, 0);
        assert_eq!(model.traffic.down_rate_bps, 0);
        assert_eq!(model.last_traffic_sample_at_ms, 1_000);
        assert_eq!(model.last_traffic_response_id, 1);
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn traffic_stats_updated_second_sample_computes_rate() {
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Connected;
        // Prime with a first sample.
        handle_traffic_stats_updated(&mut model, 0, 1, 0, 0, 0, 1_000);
        // 5000 ↑ + 12000 ↓ over 1 second.
        let effects = handle_traffic_stats_updated(&mut model, 0, 2, 5_000, 12_000, 7, 2_000);
        assert_eq!(model.traffic.up_rate_bps, 5_000);
        assert_eq!(model.traffic.down_rate_bps, 12_000);
        assert_eq!(model.traffic.conn_count, 7);
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn traffic_stats_updated_handles_singbox_restart() {
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Connected;
        handle_traffic_stats_updated(&mut model, 0, 1, 10_000, 50_000, 5, 1_000);
        // sing-box restarts → counters reset, but we still get a sample.
        handle_traffic_stats_updated(&mut model, 0, 2, 100, 200, 1, 2_000);
        // Rate saturates to 0 for this transition sample.
        assert_eq!(model.traffic.up_rate_bps, 0);
        assert_eq!(model.traffic.down_rate_bps, 0);
        // Totals reflect the new (lower) baseline.
        assert_eq!(model.traffic.up_total, 100);
        assert_eq!(model.traffic.down_total, 200);
    }

    #[test]
    fn handle_tick_emits_fetch_when_connected() {
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Connected;
        model.last_traffic_fetch_at = None;
        let effects = handle_tick(&mut model);
        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::FetchTrafficStats {
                    attempt_id: 0,
                    request_id: 1,
                }
            )),
            "expected Effect::FetchTrafficStats, got {:?}",
            effects
        );
        assert!(model.last_traffic_fetch_at.is_some());
        assert_eq!(model.traffic_request_id, 1);
    }

    #[test]
    fn traffic_stats_ignore_previous_connection_attempt() {
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Connected;
        model.connect_attempt_id = 2;

        let effects = handle_traffic_stats_updated(&mut model, 1, 1, 10, 20, 3, 1_000);

        assert!(effects.is_empty());
        assert_eq!(model.traffic, TrafficStats::default());
        assert_eq!(model.last_traffic_response_id, 0);
    }

    #[test]
    fn traffic_stats_ignore_response_when_not_connected() {
        let mut model = model_with_profiles(vec![]);

        let effects = handle_traffic_stats_updated(&mut model, 0, 1, 10, 20, 3, 1_000);

        assert!(effects.is_empty());
        assert_eq!(model.traffic, TrafficStats::default());
    }

    #[test]
    fn traffic_stats_do_not_roll_back_on_out_of_order_response() {
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Connected;
        handle_traffic_stats_updated(&mut model, 0, 2, 200, 400, 4, 2_000);

        let effects = handle_traffic_stats_updated(&mut model, 0, 1, 100, 200, 2, 1_000);

        assert!(effects.is_empty());
        assert_eq!(model.traffic.up_total, 200);
        assert_eq!(model.traffic.down_total, 400);
        assert_eq!(model.last_traffic_sample_at_ms, 2_000);
        assert_eq!(model.last_traffic_response_id, 2);
    }

    #[test]
    fn handle_tick_skips_fetch_when_idle() {
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Idle;
        let effects = handle_tick(&mut model);
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::FetchTrafficStats { .. })),
            "Idle connection must not poll Clash API"
        );
    }

    #[test]
    fn handle_tick_throttles_within_one_second() {
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Connected;
        // First tick → fetch emitted.
        let _ = handle_tick(&mut model);
        let first_at = model.last_traffic_fetch_at;
        // Second tick almost immediately → no additional fetch.
        let effects = handle_tick(&mut model);
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::FetchTrafficStats { .. })),
            "second tick within 1s must not emit FetchTrafficStats"
        );
        assert_eq!(model.last_traffic_fetch_at, first_at);
    }
}

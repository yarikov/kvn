mod config_reload;
mod connection;
mod geo;
mod key;
mod paste;
mod routing;
mod status;
mod subscription;
mod tick;
mod traffic;

use crate::app::effect::Effect;
use crate::app::model::Model;
use crate::app::msg::Msg;
use connection::{
    handle_auto_connect_polkit_checked, handle_kill_switch_applied, on_connect_failed,
    on_connected, on_singbox_exited, on_system_resumed,
};
use geo::handle_geo_result;
use key::handle_key;
use key::ipc::handle_ipc_command;
use routing::on_service_rule_sets_ready;
use subscription::handle_subscription_result;
use tick::handle_tick;
use traffic::handle_traffic_stats_updated;

pub(crate) use config_reload::handle_config_reloaded;
pub use key::theme::{shorten_theme_name, theme_picker_labels, theme_picker_slugs};

/// Pure function: Model + Msg → updated Model + list of Effects.
/// No I/O, no threads, no system calls.
pub fn update(model: &mut Model, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Key(key) => handle_key(model, key),
        Msg::Tick => handle_tick(model),
        Msg::GeoUpdated(result) => handle_geo_result(model, result),
        Msg::GeoMetadataRefreshed {
            last_updated,
            last_checked_at,
            retry_state,
            service_retry_states,
            service_checked_at,
            next_update,
            service_next_updates,
        } => {
            model.geo_last_updated = last_updated;
            model.geo_last_checked_at = last_checked_at;
            model.geo_last_attempt_at = None;
            model.geo_retry_state = retry_state;
            model.service_retry_states = service_retry_states;
            model.service_checked_at = service_checked_at;
            model.geo_next_update = next_update;
            model.service_next_updates = service_next_updates;
            vec![Effect::BroadcastState]
        }
        Msg::SystemResumed => on_system_resumed(model),
        Msg::Connected {
            pid,
            profile_id,
            attempt_id,
        } => on_connected(model, pid, profile_id, attempt_id),
        Msg::SubscriptionFetched { id, result } => {
            let mut effects = handle_subscription_result(model, id, result);
            effects.push(Effect::BroadcastState);
            effects
        }
        Msg::ServiceRuleSetsReady {
            retry_states,
            checked_at,
            next_updates,
            updated_parts,
            errors,
        } => on_service_rule_sets_ready(
            model,
            retry_states,
            checked_at,
            next_updates,
            updated_parts,
            errors,
        ),
        Msg::ConnectFailed { attempt_id, error } => on_connect_failed(model, attempt_id, error),
        Msg::SingBoxExited {
            attempt_id,
            code,
            signal,
        } => on_singbox_exited(model, attempt_id, code, signal),
        Msg::Resize => {
            model.needs_redraw = true;
            vec![]
        }
        Msg::IpcCommand(cmd) | Msg::IpcRequest { command: cmd, .. } => {
            handle_ipc_command(model, cmd)
        }
        Msg::StateUpdate { .. } | Msg::IpcReadFailed { .. } => vec![],
        Msg::ConfigReloaded(result) => handle_config_reloaded(model, *result),
        Msg::KillSwitchApplied { enabled, error } => {
            handle_kill_switch_applied(model, enabled, error)
        }
        Msg::AutoConnectPolkitChecked { error } => handle_auto_connect_polkit_checked(model, error),
        Msg::TrafficStatsUpdated {
            attempt_id,
            request_id,
            up_total,
            down_total,
            conn_count,
            sampled_at_ms,
        } => handle_traffic_stats_updated(
            model,
            attempt_id,
            request_id,
            up_total,
            down_total,
            conn_count,
            sampled_at_ms,
        ),
        Msg::ThemeChanged(theme) => {
            // Manual picker override wins: ignore Omarchy watcher events
            // unless the user has explicitly opted into auto-follow.
            if model.config.settings.theme != "omarchy" {
                return vec![];
            }
            model.theme = theme;
            model.needs_redraw = true;
            vec![]
        }
        Msg::TestResult { id, latency_ms } => {
            model.testing_profiles.remove(&id);
            model.profile_latencies.insert(id, latency_ms);
            vec![Effect::BroadcastState]
        }
        Msg::Mouse(_) | Msg::Paste(_) => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::Overlay;
    use crate::test_helpers::*;

    #[test]
    fn handle_event_non_key_is_noop() {
        let mut model = model_with_profiles(vec![]);
        let effects = update(&mut model, Msg::Resize);
        assert!(effects.is_empty());
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn terminal_paste_message_is_ignored_by_update() {
        let mut model = model_with_profiles(vec![]);
        assert!(update(&mut model, Msg::Paste("vless://x".into())).is_empty());
        assert!(model.config.profiles.is_empty());
    }

    /// `Msg::ThemeChanged` from the Omarchy watcher is a no-op whenever
    /// the user has picked an explicit non-`"omarchy"` theme.
    #[test]
    fn theme_changed_msg_ignored_when_manual_override_set() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.theme = "gruvbox".into();
        let original = model.theme;
        let _ = update(
            &mut model,
            Msg::ThemeChanged(crate::ui::styles::Theme::resolve("nord")),
        );
        assert_eq!(model.theme, original, "manual override blocks watcher");
    }

    /// `Msg::ThemeChanged` applies when the user is in Auto-follow mode.
    #[test]
    fn theme_changed_msg_applies_in_omarchy_mode() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.theme = "omarchy".into();
        let new_theme = crate::ui::styles::Theme::resolve("nord");
        let _ = update(&mut model, Msg::ThemeChanged(new_theme));
        assert_eq!(model.theme, new_theme);
    }

    #[test]
    fn test_result_ok_stores_latency_and_clears_testing() {
        let profiles = sample_profiles();
        let id = profiles[0].id;
        let mut model = model_with_profiles(profiles);
        model.testing_profiles.insert(id);
        let effects = update(
            &mut model,
            Msg::TestResult {
                id,
                latency_ms: Some(42),
            },
        );
        assert!(!model.testing_profiles.contains(&id));
        assert_eq!(model.profile_latencies.get(&id), Some(&Some(42)));
        assert!(effects.iter().any(|e| matches!(e, Effect::BroadcastState)));
    }

    #[test]
    fn test_result_err_stores_none_and_clears_testing() {
        let profiles = sample_profiles();
        let id = profiles[0].id;
        let mut model = model_with_profiles(profiles);
        model.profile_latencies.insert(id, Some(100));
        model.testing_profiles.insert(id);
        let effects = update(
            &mut model,
            Msg::TestResult {
                id,
                latency_ms: None,
            },
        );
        assert!(!model.testing_profiles.contains(&id));
        assert_eq!(
            model.profile_latencies.get(&id),
            Some(&None),
            "failure stores None (shown as err in UI)"
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::BroadcastState)));
    }
}

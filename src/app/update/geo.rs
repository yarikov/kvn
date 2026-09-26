use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model};
use crate::app::msg::GeoResult;

use crate::app::update::connection::queue_connect;
use crate::app::update::status::{DownloadKind, append_download_hint, push_status};

pub(in crate::app::update) fn handle_geo_result(
    model: &mut Model,
    result: GeoResult,
) -> Vec<Effect> {
    let pending_geo_reconnect = std::mem::take(&mut model.pending_geo_reconnect);
    model.geo_updating = pending_geo_reconnect && model.pending_service_reconnect;
    model.geo_automatic_update = false;
    let mut effects = match result {
        GeoResult::Updated {
            parts,
            last_updated: _,
            checked_at,
            retry_state,
            service_retry_states,
            service_checked_at,
            next_update,
            service_next_updates,
            warnings,
        } => {
            model.geo_retry_state = retry_state;
            model.service_retry_states = service_retry_states;
            model.service_checked_at = service_checked_at;
            model.geo_next_update = next_update;
            model.service_next_updates = service_next_updates;
            model.geo_last_updated = Some(checked_at.format("%d %b %H:%M").to_string());
            model.geo_last_checked_at = Some(checked_at);
            let mut log_effects = Vec::new();
            for part in &parts {
                let text = format!("Updated: {}", part);
                log_effects.push(Effect::AppendAppLog {
                    level: "INFO".to_string(),
                    message: text.clone(),
                });
                model.logs.push_back(text);
            }
            for warning in &warnings {
                log_effects.push(Effect::AppendAppLog {
                    level: "ERROR".to_string(),
                    message: format!("Geo batch partial failure: {warning}"),
                });
            }
            if !warnings.is_empty() {
                append_download_hint(&mut log_effects, model, DownloadKind::Geo);
            }
            let status = if warnings.is_empty() {
                AppStatus::Info("Geo databases updated".into())
            } else {
                AppStatus::Error(format!(
                    "Geo update partially failed: {}",
                    warnings.join("; ")
                ))
            };
            push_status(&mut log_effects, model, status);
            if !pending_geo_reconnect
                && model.connection == ConnectionState::Connected
                && let Some(active_id) = model.active_profile_id
                && queue_connect(model, active_id)
            {
                model
                    .logs
                    .push_back("Reconnecting to apply new geo databases".into());
            }
            log_effects
        }
        GeoResult::UpToDate {
            checked_at,
            retry_state,
            service_retry_states,
            service_checked_at,
            next_update,
            service_next_updates,
            warnings,
        } => {
            model.geo_retry_state = retry_state;
            model.service_retry_states = service_retry_states;
            model.service_checked_at = service_checked_at;
            model.geo_next_update = next_update;
            model.service_next_updates = service_next_updates;
            model.geo_last_checked_at = checked_at;
            if let Some(checked_at) = checked_at {
                model.geo_last_updated = Some(checked_at.format("%d %b %H:%M").to_string());
            }
            let mut effects = Vec::new();
            let status = if warnings.is_empty() {
                AppStatus::Info("Geo databases up to date".into())
            } else {
                AppStatus::Error(format!(
                    "Service rule-set update failed: {}",
                    warnings.join("; ")
                ))
            };
            push_status(&mut effects, model, status);
            if !warnings.is_empty() {
                append_download_hint(&mut effects, model, DownloadKind::Geo);
            }
            effects
        }
        GeoResult::Error {
            message,
            retry_state,
            service_retry_states,
            service_checked_at,
            next_update,
            service_next_updates,
            updated_parts,
        } => {
            model.geo_retry_state = retry_state;
            model.service_retry_states = service_retry_states;
            model.service_checked_at = service_checked_at;
            model.geo_next_update = next_update;
            model.service_next_updates = service_next_updates;
            let mut effects = Vec::new();
            let has_updates = !updated_parts.is_empty();
            push_status(
                &mut effects,
                model,
                crate::app::model::AppStatus::Error(format!("Geo update failed: {message}")),
            );
            append_download_hint(&mut effects, model, DownloadKind::Geo);
            for part in updated_parts {
                effects.push(Effect::AppendAppLog {
                    level: "INFO".to_string(),
                    message: format!("Updated: {part}"),
                });
            }
            if !pending_geo_reconnect
                && has_updates
                && model.connection == ConnectionState::Connected
                && let Some(active_id) = model.active_profile_id
            {
                queue_connect(model, active_id);
            }
            effects
        }
    };
    if pending_geo_reconnect
        && !model.pending_service_reconnect
        && model.connection == ConnectionState::Connected
        && let Some(active_id) = model.active_profile_id
    {
        queue_connect(model, active_id);
    }
    effects.push(Effect::BroadcastState);
    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::msg::Msg;
    use crate::app::update::update;
    use crate::config::profile::Profile;
    use crate::test_helpers::*;
    use chrono::Local;

    #[test]
    fn geo_last_updated_message_updates_model() {
        let mut model = model_with_profiles(vec![]);
        model.geo_last_updated = None;
        let effects = update(
            &mut model,
            Msg::GeoMetadataRefreshed {
                last_updated: Some("2026-06-15 08:00".to_string()),
                last_checked_at: None,
                retry_state: None,
                service_retry_states: Default::default(),
                service_checked_at: Default::default(),
                next_update: None,
                service_next_updates: Default::default(),
            },
        );
        assert_eq!(model.geo_last_updated, Some("2026-06-15 08:00".to_string()));
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn geo_result_updated_broadcasts_state() {
        let mut model = model_with_profiles(vec![]);
        model.geo_updating = true;
        let effects = update(
            &mut model,
            Msg::GeoUpdated(GeoResult::Updated {
                parts: vec!["geoip".into()],
                last_updated: Some("2026-05-31 13:41".to_string()),
                checked_at: Local::now(),
                retry_state: None,
                service_retry_states: Default::default(),
                service_checked_at: Default::default(),
                next_update: None,
                service_next_updates: Default::default(),
                warnings: Vec::new(),
            }),
        );
        assert!(!model.geo_updating);
        assert_eq!(
            effects,
            vec![
                app_log_info("Updated: geoip"),
                app_log_info("Geo databases updated"),
                Effect::BroadcastState
            ]
        );
    }

    #[test]
    fn geo_update_queues_active_profile_not_cursor() {
        let a = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let b = Profile::new_vless("B".into(), "2.2.2.2".into(), 443, "u2".into());
        let active_id = a.id;
        let mut model = model_with_profiles(vec![a, b]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(active_id);
        model.select_next();

        update(
            &mut model,
            Msg::GeoUpdated(GeoResult::Updated {
                parts: vec!["geoip".into()],
                last_updated: None,
                checked_at: Local::now(),
                retry_state: None,
                service_retry_states: Default::default(),
                service_checked_at: Default::default(),
                next_update: None,
                service_next_updates: Default::default(),
                warnings: Vec::new(),
            }),
        );

        assert_eq!(model.connecting_profile_id, Some(active_id));
    }

    #[test]
    fn geo_result_up_to_date_broadcasts_state() {
        let mut model = model_with_profiles(vec![]);
        model.geo_updating = true;
        let effects = update(
            &mut model,
            Msg::GeoUpdated(GeoResult::UpToDate {
                checked_at: Some(Local::now()),
                retry_state: None,
                service_retry_states: Default::default(),
                service_checked_at: Default::default(),
                next_update: None,
                service_next_updates: Default::default(),
                warnings: Vec::new(),
            }),
        );
        assert!(!model.geo_updating);
        assert_eq!(
            effects,
            vec![
                app_log_info("Geo databases up to date"),
                Effect::BroadcastState
            ]
        );
    }

    #[test]
    fn geo_result_error_broadcasts_state() {
        let mut model = model_with_profiles(vec![]);
        model.geo_updating = true;
        let retry_state = crate::geo::GeoRetryState {
            consecutive_failures: 1,
            retry_at: Local::now() + chrono::Duration::minutes(1),
            attempt_date: None,
        };
        let effects = update(
            &mut model,
            Msg::GeoUpdated(GeoResult::Error {
                message: "net fail".into(),
                retry_state: Some(retry_state),
                service_retry_states: Default::default(),
                service_checked_at: Default::default(),
                next_update: None,
                service_next_updates: Default::default(),
                updated_parts: Vec::new(),
            }),
        );
        assert!(!model.geo_updating);
        assert_eq!(model.geo_retry_state, Some(retry_state));
        assert_eq!(
            effects,
            vec![
                app_log_error("Geo update failed: net fail"),
                Effect::AppendAppLog {
                    level: "WARN".into(),
                    message:
                        "VPN is disconnected. Try connecting to VPN and retrying the geo download."
                            .into(),
                },
                Effect::BroadcastState,
            ]
        );
    }
}

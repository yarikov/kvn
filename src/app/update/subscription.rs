use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model};
use crate::config::profile::{Profile, SubscriptionAutoUpdate};
use chrono::Local;
use uuid::Uuid;

use crate::app::update::config_reload::profile_runtime_changed;
use crate::app::update::connection::queue_connect;
use crate::app::update::status::{DownloadKind, append_download_hint, push_status};

pub(in crate::app::update) fn handle_subscription_result(
    model: &mut Model,
    id: Uuid,
    result: Result<Vec<Profile>, crate::app::msg::IpcError>,
) -> Vec<Effect> {
    handle_subscription_result_at(model, id, result, Local::now())
}

fn handle_subscription_result_at(
    model: &mut Model,
    id: Uuid,
    result: Result<Vec<Profile>, crate::app::msg::IpcError>,
    now: chrono::DateTime<Local>,
) -> Vec<Effect> {
    let managed = !id.is_nil();
    let automatic = model.automatic_subscription_updates.remove(&id);
    model.subscription_updates.remove(&id);
    if model.subscription_updates.is_empty() {
        model.subscription_fetching = false;
    }

    // Remember where the cursor is so we can restore it to the subscription
    // header after import (add_profile moves selection on every call).
    let saved_selected = model.selected;
    let old_active_profile = model
        .active_profile_id
        .and_then(|id| model.config.profiles.iter().find(|p| p.id == id).cloned());
    let old_connecting_profile = model
        .connecting_profile_id
        .and_then(|id| model.config.profiles.iter().find(|p| p.id == id).cloned());

    // Capture old dedup_key → UUID mapping before removing subscription profiles,
    // so we can reuse UUIDs for servers that survive the update.
    let old_sub_ids: std::collections::HashMap<String, Uuid> = model
        .config
        .profiles
        .iter()
        .filter(|p| p.subscription_id == Some(id))
        .map(|p| (p.dedup_key(), p.id))
        .collect();

    let mut effects = match result {
        Ok(profiles) => {
            if managed {
                if let Some(sub) = model.config.subscriptions.iter_mut().find(|s| s.id == id) {
                    sub.last_updated = Some(now);
                    if automatic {
                        sub.schedule_next_after_success(now);
                    }
                }
                // Only replace the previous snapshot after the fetch and parse
                // succeeded. A transient subscription error must leave the
                // user's working profiles and update timestamp untouched.
                model
                    .config
                    .profiles
                    .retain(|p| p.subscription_id != Some(id));
            }
            let mut imported = 0;
            for mut profile in profiles {
                let key = profile.dedup_key();
                if let Some(&old_id) = old_sub_ids.get(&key) {
                    // Same server was in this subscription before — reuse its UUID so
                    // active_profile_id stays valid across updates.
                    profile.id = old_id;
                    if managed {
                        profile.subscription_id = Some(id);
                    }
                    model.add_profile(profile);
                    imported += 1;
                } else if let Some(idx) = model
                    .config
                    .profiles
                    .iter()
                    .position(|p| p.dedup_key() == key)
                {
                    let existing = &model.config.profiles[idx];
                    if existing.subscription_id.is_none() {
                        // Update the standalone profile in place and link it to
                        // the subscription, preserving its identity.
                        profile.id = existing.id;
                        if managed {
                            profile.subscription_id = Some(id);
                        }
                        model.config.profiles[idx] = profile;
                        imported += 1;
                    }
                    // Profiles belonging to other subscriptions are skipped.
                } else {
                    if managed {
                        profile.subscription_id = Some(id);
                    }
                    model.add_profile(profile);
                    imported += 1;
                }
            }

            let mut effects = Vec::new();
            if imported > 0 || managed {
                effects.push(Effect::SaveConfig);
            }
            if imported > 0 {
                push_status(
                    &mut effects,
                    model,
                    crate::app::model::AppStatus::Info(format!(
                        "Imported {} profile(s) from subscription",
                        imported
                    )),
                );
            } else {
                push_status(
                    &mut effects,
                    model,
                    crate::app::model::AppStatus::Info("No new profiles in subscription".into()),
                );
            }
            effects
        }
        Err(err) => {
            let mut effects = if managed {
                if let Some(sub) = model.config.subscriptions.iter_mut().find(|s| s.id == id)
                    && automatic
                    && sub.auto_update != SubscriptionAutoUpdate::Off
                {
                    sub.record_fetch_failure(now);
                    vec![Effect::SaveConfig]
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            push_status(
                &mut effects,
                model,
                crate::app::model::AppStatus::Error(format!("Subscription failed: {}", err)),
            );
            append_download_hint(&mut effects, model, DownloadKind::Subscription);
            effects
        }
    };

    // If either the active or in-flight profile was removed from the
    // subscription, invalidate the attempt and stop any process it spawned.
    let active_missing = model
        .active_profile_id
        .is_some_and(|id| !model.config.profiles.iter().any(|p| p.id == id));
    let connecting_missing = matches!(
        model.connection,
        ConnectionState::Connecting | ConnectionState::ConnectPending
    ) && model
        .connecting_profile_id
        .is_some_and(|id| !model.config.profiles.iter().any(|p| p.id == id));
    if active_missing || connecting_missing {
        effects.push(Effect::Disconnect);
    } else {
        let active_changed = old_active_profile.as_ref().is_some_and(|old| {
            model
                .config
                .profiles
                .iter()
                .find(|p| p.id == old.id)
                .is_some_and(|new| profile_runtime_changed(old, new))
        });
        let connecting_changed = old_connecting_profile.as_ref().is_some_and(|old| {
            model
                .config
                .profiles
                .iter()
                .find(|p| p.id == old.id)
                .is_some_and(|new| profile_runtime_changed(old, new))
        });
        let reconnect_id = match model.connection {
            ConnectionState::Connected if active_changed => model.active_profile_id,
            ConnectionState::ConnectPending if connecting_changed => model.connecting_profile_id,
            _ => None,
        };
        if let Some(id) = reconnect_id
            && queue_connect(model, id)
        {
            push_status(
                &mut effects,
                model,
                AppStatus::Info("Subscription changed — reconnecting".into()),
            );
        }
    }

    // Restore cursor to the subscription header (add_profile moves it on every
    // call, so without this the focus would land on the last imported profile).
    if managed {
        if let Some(sub_idx) = model.config.subscriptions.iter().position(|s| s.id == id) {
            model.selected = crate::app::model::row_for_subscription_header(&model.config, sub_idx);
        }
    } else {
        model.selected = saved_selected;
    }

    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::msg::Msg;
    use crate::app::update::update;
    use crate::config::profile::Subscription;
    use crate::test_helpers::*;

    #[test]
    fn subscription_fetched_adds_profiles_and_saves() {
        let mut model = model_with_profiles(vec![]);
        let profiles = vec![
            Profile::new_vless(
                "Sub1".to_string(),
                "1.1.1.1".to_string(),
                443,
                "u1".to_string(),
            ),
            Profile::new_vless(
                "Sub2".to_string(),
                "2.2.2.2".to_string(),
                443,
                "u2".to_string(),
            ),
        ];

        let effects = handle_subscription_result(&mut model, Uuid::nil(), Ok(profiles));

        assert!(!model.subscription_fetching);
        assert_eq!(model.config.profiles.len(), 2);
        assert_eq!(
            effects,
            vec![
                Effect::SaveConfig,
                app_log_info("Imported 2 profile(s) from subscription")
            ]
        );
    }

    #[test]
    fn subscription_fetched_updates_standalone_duplicate() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Existing".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        let profiles = vec![
            Profile::new_vless(
                "Existing".to_string(),
                "1.1.1.1".to_string(),
                443,
                "u1".to_string(),
            ),
            Profile::new_vless(
                "New".to_string(),
                "2.2.2.2".to_string(),
                443,
                "u2".to_string(),
            ),
        ];

        let effects = handle_subscription_result(&mut model, Uuid::nil(), Ok(profiles));

        assert_eq!(model.config.profiles.len(), 2);
        assert_eq!(
            effects,
            vec![
                Effect::SaveConfig,
                app_log_info("Imported 2 profile(s) from subscription")
            ]
        );
    }

    #[test]
    fn subscription_fetched_skips_duplicate_from_other_subscription() {
        let other_sub_id = Uuid::new_v4();
        let mut existing = Profile::new_vless(
            "Existing".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        );
        existing.subscription_id = Some(other_sub_id);
        let mut model = model_with_profiles(vec![existing]);
        model.config.subscriptions.push(Subscription {
            id: other_sub_id,
            name: "Other".to_string(),
            url: "http://example.com/other".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });

        let new_sub_id = Uuid::new_v4();
        model.config.subscriptions.push(Subscription {
            id: new_sub_id,
            name: "New".to_string(),
            url: "http://example.com/new".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });

        let fetched = Profile::new_vless(
            "Existing".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        );

        let effects = handle_subscription_result(&mut model, new_sub_id, Ok(vec![fetched]));

        assert_eq!(model.config.profiles.len(), 1);
        assert_eq!(model.config.profiles[0].subscription_id, Some(other_sub_id));
        assert_eq!(
            effects,
            vec![
                Effect::SaveConfig,
                app_log_info("No new profiles in subscription")
            ]
        );
    }

    #[test]
    fn subscription_fetched_attaches_standalone_duplicate() {
        let sub_id = Uuid::new_v4();
        let standalone = Profile::new_vless(
            "OldName".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        );
        let standalone_id = standalone.id;
        let mut model = model_with_profiles(vec![standalone]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Sub".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });

        let mut fetched = Profile::new_vless(
            "NewName".to_string(),
            "2.2.2.2".to_string(),
            443,
            "u1".to_string(),
        );
        // Different id from parse, same uuid as the standalone profile.
        fetched.id = Uuid::new_v4();

        let effects = handle_subscription_result(&mut model, sub_id, Ok(vec![fetched]));

        assert_eq!(model.config.profiles.len(), 1);
        assert_eq!(model.config.profiles[0].id, standalone_id);
        assert_eq!(model.config.profiles[0].name, "NewName");
        assert_eq!(model.config.profiles[0].address, "2.2.2.2");
        assert_eq!(model.config.profiles[0].subscription_id, Some(sub_id));
        assert_eq!(
            effects,
            vec![
                Effect::SaveConfig,
                app_log_info("Imported 1 profile(s) from subscription")
            ]
        );
    }

    #[test]
    fn subscription_fetched_empty_logs_no_new_profiles() {
        let mut model = model_with_profiles(vec![]);

        let effects = handle_subscription_result(&mut model, Uuid::nil(), Ok(vec![]));

        assert_eq!(model.config.profiles.len(), 0);
        assert_eq!(
            effects,
            vec![app_log_info("No new profiles in subscription")]
        );
    }

    #[test]
    fn subscription_fetched_error_logs_failure() {
        let sub_id = Uuid::new_v4();
        let mut existing = Profile::new_vless(
            "Existing".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        );
        existing.subscription_id = Some(sub_id);
        let existing_id = existing.id;
        let last_updated = Local::now() - chrono::Duration::try_hours(2).unwrap();
        let mut model = model_with_profiles(vec![existing]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Sub".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: Some(last_updated),
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        model.automatic_subscription_updates.insert(sub_id);

        let failed_at = Local::now();
        let effects = handle_subscription_result_at(
            &mut model,
            sub_id,
            Err(crate::app::msg::IpcError::new("network down")),
            failed_at,
        );

        assert!(!model.subscription_fetching);
        assert_eq!(model.config.profiles.len(), 1);
        assert_eq!(model.config.profiles[0].id, existing_id);
        assert_eq!(
            model.config.subscriptions[0].last_updated,
            Some(last_updated)
        );
        assert_eq!(
            model.config.subscriptions[0].retry_state,
            Some(crate::config::profile::SubscriptionRetryState {
                consecutive_failures: 1,
                retry_at: failed_at + chrono::Duration::minutes(1),
                attempt_date: Some(failed_at.date_naive()),
            })
        );
        assert_eq!(
            effects,
            vec![
                Effect::SaveConfig,
                app_log_error("Subscription failed: network down"),
                Effect::AppendAppLog {
                    level: "WARN".into(),
                    message: "VPN is disconnected. Try connecting to VPN and retrying the subscription update."
                        .into(),
                },
            ]
        );
    }

    #[test]
    fn subscription_fetched_error_broadcasts_state_without_other_effects() {
        let mut model = model_with_profiles(vec![]);
        let effects = update(
            &mut model,
            Msg::SubscriptionFetched {
                id: Uuid::nil(),
                result: Err(crate::app::msg::IpcError::new("network down")),
            },
        );

        assert!(effects.contains(&Effect::BroadcastState));
        assert!(matches!(model.status, Some(AppStatus::Error(_))));
    }

    #[test]
    fn subscription_fetched_managed_replaces_profiles_and_updates_last_updated() {
        let sub_id = Uuid::new_v4();
        let mut existing = Profile::new_vless(
            "Old".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        );
        existing.subscription_id = Some(sub_id);
        let mut model = model_with_profiles(vec![existing]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Sub".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        model.config.subscriptions[0].record_fetch_failure(Local::now());
        model.automatic_subscription_updates.insert(sub_id);

        let new_profiles = vec![Profile::new_vless(
            "New".to_string(),
            "2.2.2.2".to_string(),
            443,
            "u2".to_string(),
        )];

        let effects = handle_subscription_result(&mut model, sub_id, Ok(new_profiles));

        assert_eq!(model.config.profiles.len(), 1);
        assert_eq!(model.config.profiles[0].name, "New");
        assert_eq!(model.config.profiles[0].subscription_id, Some(sub_id));
        assert!(
            model
                .config
                .subscriptions
                .iter()
                .find(|s| s.id == sub_id)
                .unwrap()
                .last_updated
                .is_some()
        );
        assert!(model.config.subscriptions[0].retry_state.is_none());
        assert_eq!(
            effects,
            vec![
                Effect::SaveConfig,
                app_log_info("Imported 1 profile(s) from subscription")
            ]
        );
    }

    #[test]
    fn subscription_fetched_restores_selection_to_subscription_header() {
        let sub_id = Uuid::new_v4();
        let mut existing = Profile::new_vless(
            "Old".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        );
        existing.subscription_id = Some(sub_id);
        let mut model = model_with_profiles(vec![existing]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Sub".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        // Start with cursor on the subscription header.
        model.selected = crate::app::model::row_for_subscription_header(&model.config, 0);
        let header_row = model.selected;

        let new_profiles = vec![
            Profile::new_vless(
                "A".to_string(),
                "2.2.2.2".to_string(),
                443,
                "u2".to_string(),
            ),
            Profile::new_vless(
                "B".to_string(),
                "3.3.3.3".to_string(),
                443,
                "u3".to_string(),
            ),
        ];
        handle_subscription_result(&mut model, sub_id, Ok(new_profiles));

        assert_eq!(
            model.selected, header_row,
            "cursor should stay on the subscription header"
        );
    }

    #[test]
    fn subscription_fetched_preserves_active_profile_id_for_same_server() {
        let sub_id = Uuid::new_v4();
        let mut existing = Profile::new_vless(
            "Server".to_string(),
            "1.1.1.1".to_string(),
            443,
            "same-uuid".to_string(),
        );
        existing.subscription_id = Some(sub_id);
        let old_profile_id = existing.id;
        let mut model = model_with_profiles(vec![existing]);
        model.active_profile_id = Some(old_profile_id);

        // Same server (same dedup key) comes back in the updated subscription.
        let updated = Profile::new_vless(
            "Server Renamed".to_string(),
            "1.1.1.1".to_string(),
            443,
            "same-uuid".to_string(),
        );
        handle_subscription_result(&mut model, sub_id, Ok(vec![updated]));

        assert_eq!(model.config.profiles.len(), 1);
        assert_eq!(model.config.profiles[0].id, old_profile_id);
        assert_eq!(model.active_profile_id, Some(old_profile_id));
    }

    #[test]
    fn subscription_fetched_reconnects_changed_active_profile() {
        let sub_id = Uuid::new_v4();
        let mut existing = Profile::new_vless(
            "Server".to_string(),
            "1.1.1.1".to_string(),
            443,
            "same-uuid".to_string(),
        );
        existing.subscription_id = Some(sub_id);
        let old_profile_id = existing.id;
        let mut model = model_with_profiles(vec![existing]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Sub".into(),
            url: "http://example.com/sub".into(),
            auto_update: SubscriptionAutoUpdate::Off,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(old_profile_id);
        let mut updated = Profile::new_vless(
            "Server".to_string(),
            "1.1.1.1".to_string(),
            443,
            "same-uuid".to_string(),
        );
        if let crate::config::profile::ProtocolConfig::Vless(config) = &mut updated.config {
            config.flow = Some(crate::config::profile::Flow::XtlsRprxVision);
        }

        let effects = handle_subscription_result(&mut model, sub_id, Ok(vec![updated]));

        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(old_profile_id));
        assert!(effects.contains(&app_log_info("Subscription changed — reconnecting")));
    }

    #[test]
    fn subscription_update_disconnects_when_connecting_profile_is_removed() {
        let sub_id = Uuid::new_v4();
        let mut existing = Profile::new_vless(
            "Server".to_string(),
            "1.1.1.1".to_string(),
            443,
            "old-uuid".to_string(),
        );
        existing.subscription_id = Some(sub_id);
        let profile_id = existing.id;
        let mut model = model_with_profiles(vec![existing]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Sub".into(),
            url: "http://example.com/sub".into(),
            auto_update: SubscriptionAutoUpdate::Off,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        assert!(queue_connect(&mut model, profile_id));
        model.connection = ConnectionState::ConnectPending;

        let effects = handle_subscription_result(&mut model, sub_id, Ok(vec![]));

        assert!(effects.contains(&Effect::Disconnect));
    }

    #[test]
    fn subscription_fetched_clears_active_profile_id_when_profile_removed() {
        let sub_id = Uuid::new_v4();
        let mut existing = Profile::new_vless(
            "Old Server".to_string(),
            "1.1.1.1".to_string(),
            443,
            "old-uuid".to_string(),
        );
        existing.subscription_id = Some(sub_id);
        let old_profile_id = existing.id;
        let mut model = model_with_profiles(vec![existing]);
        model.active_profile_id = Some(old_profile_id);

        // Updated subscription has a different server; old one is gone.
        let new_server = Profile::new_vless(
            "New Server".to_string(),
            "2.2.2.2".to_string(),
            443,
            "new-uuid".to_string(),
        );
        let effects = handle_subscription_result(&mut model, sub_id, Ok(vec![new_server]));

        assert_eq!(model.config.profiles.len(), 1);
        // Disconnect is emitted; active_profile_id is cleared when the effect runs.
        assert!(effects.contains(&Effect::Disconnect));
    }

    #[test]
    fn manual_subscription_result_does_not_change_automatic_schedule() {
        let now = Local::now();
        let id = Uuid::new_v4();
        let mut model = model_with_profiles(vec![]);
        let retry = crate::config::profile::SubscriptionRetryState {
            consecutive_failures: 2,
            retry_at: now + chrono::Duration::minutes(5),
            attempt_date: Some(now.date_naive()),
        };
        model.config.subscriptions.push(Subscription {
            id,
            name: "Sub".into(),
            url: "https://example.com/sub".into(),
            auto_update: SubscriptionAutoUpdate::Every3d,
            last_updated: None,
            next_auto_update: Some(now.date_naive()),
            retry_state: Some(retry.clone()),
            send_hwid: false,
            hwid: None,
        });

        handle_subscription_result_at(&mut model, id, Ok(Vec::new()), now);

        assert_eq!(model.config.subscriptions[0].retry_state, Some(retry));
        assert_eq!(
            model.config.subscriptions[0].next_auto_update,
            Some(now.date_naive())
        );
    }
}

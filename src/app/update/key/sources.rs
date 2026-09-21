use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model, Overlay};
use crate::config::profile::{GeoRegion, SubscriptionAutoUpdate};
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::update::connection::queue_connect;
use crate::app::update::key::settings_menu::{
    open_dns_settings, open_geo_region, open_routing_mode, open_service_routing,
    open_theme_settings, toggle_auto_connect, toggle_kill_switch,
};
use crate::app::update::status::{
    DownloadKind, download_allowed, push_download_blocked, push_status,
};

pub(in crate::app::update) fn handle_sources(model: &mut Model, key: KeyEvent) -> Vec<Effect> {
    let mut effects = Vec::new();
    match key.code {
        // Navigation
        KeyCode::Char('j') | KeyCode::Down => model.select_next(),
        KeyCode::Char('k') | KeyCode::Up => model.select_prev(),
        KeyCode::Char('G') => model.select_last(),

        // Actions. Note: 'p' (paste) and 'e' (edit profiles) are handled in
        // the TUI client directly, not routed through the daemon, so they
        // don't appear here.
        KeyCode::Enter => return handle_enter_on_sources(model),
        KeyCode::Char('d')
            if model.selected_profile().is_some() || model.selected_subscription().is_some() =>
        {
            if is_connection_affected(model) {
                let mut effects = vec![];
                push_status(
                    &mut effects,
                    model,
                    AppStatus::Error("Disconnect before deleting".into()),
                );
                return effects;
            }
            model.overlay = Overlay::ConfirmDelete;
        }
        KeyCode::Char('m') => {
            open_routing_mode(model, None);
        }
        KeyCode::Char('u') => return handle_update_key(model),
        KeyCode::Char('I') => {
            let schedule = model.config.settings.geo_routing.auto_update.next();
            model.config.settings.geo_routing.auto_update = schedule;
            let mut effects = vec![Effect::SaveConfig, Effect::ResetGeoUpdateSchedules];
            if let Some(region) = model.config.settings.geo_routing.current_region {
                model.geo_retry_state = None;
                effects.push(Effect::ClearGeoRetryState { region });
            }
            push_status(
                &mut effects,
                model,
                AppStatus::Info(format!(
                    "Rule sets {} {}",
                    crate::ui::icons::icons(model.config.settings.icons).refresh,
                    schedule.label()
                )),
            );
            return effects;
        }
        KeyCode::Char('i') => {
            if let Some(idx) = model.selected_subscription_index() {
                let (name, label) = if let Some(sub) = model.config.subscriptions.get_mut(idx) {
                    sub.auto_update = sub.auto_update.next();
                    sub.clear_retry_state();
                    sub.next_auto_update =
                        (sub.auto_update != SubscriptionAutoUpdate::Off).then(|| {
                            crate::config::profile::next_update_window_date(chrono::Local::now())
                        });
                    (sub.name.clone(), sub.auto_update.label())
                } else {
                    return effects;
                };
                let mut effects = vec![Effect::SaveConfig];
                push_status(
                    &mut effects,
                    model,
                    crate::app::model::AppStatus::Info(format!(
                        "Subscription '{}' {} {}",
                        name,
                        crate::ui::icons::icons(model.config.settings.icons).refresh,
                        label
                    )),
                );
                return effects;
            }
        }
        KeyCode::Char('o') => {
            open_geo_region(model, None);
        }
        KeyCode::Char('r') if model.connection == ConnectionState::Connected => {
            if let Some(profile) = model.active_profile_id.and_then(|id| {
                model
                    .config
                    .profiles
                    .iter()
                    .find(|profile| profile.id == id)
            }) {
                let profile_id = profile.id;
                let profile_name = profile.name.clone();
                push_status(
                    &mut effects,
                    model,
                    crate::app::model::AppStatus::Info(format!(
                        "Reconnecting to {}…",
                        profile_name
                    )),
                );
                queue_connect(model, profile_id);
            }
        }
        KeyCode::Char('s') if model.connection == ConnectionState::Connected => {
            return vec![Effect::Disconnect];
        }
        KeyCode::Char('a') => {
            return toggle_auto_connect(model);
        }
        KeyCode::Char('K') => {
            return toggle_kill_switch(model);
        }
        KeyCode::Char('D') => {
            open_dns_settings(model, None);
        }
        KeyCode::Char('S') => {
            open_service_routing(model, None);
        }
        KeyCode::Char('t') => {
            if !model.config.settings.connectivity_probe.enabled {
                push_status(
                    &mut effects,
                    model,
                    crate::app::model::AppStatus::Info(
                        "Profile latency testing is disabled in profiles.json".into(),
                    ),
                );
                return effects;
            }
            if let Some(p) = model.selected_profile() {
                let id = p.id;
                if !model.testing_profiles.contains(&id) && !model.pending_tests.contains(&id) {
                    model.pending_tests.push_back(id);
                }
            }
        }
        KeyCode::Char('T') => {
            if !model.config.settings.connectivity_probe.enabled {
                push_status(
                    &mut effects,
                    model,
                    crate::app::model::AppStatus::Info(
                        "Profile latency testing is disabled in profiles.json".into(),
                    ),
                );
                return effects;
            }
            for p in &model.config.profiles {
                if !model.testing_profiles.contains(&p.id) && !model.pending_tests.contains(&p.id) {
                    model.pending_tests.push_back(p.id);
                }
            }
        }
        KeyCode::Char('C') => {
            open_theme_settings(model, None);
        }

        _ => {}
    }
    effects
}

fn handle_enter_on_sources(model: &mut Model) -> Vec<Effect> {
    let mut effects = Vec::new();
    if let Some(profile) = model.selected_profile() {
        let profile_id = profile.id;
        let profile_name = profile.name.clone();
        if model.active_profile_id == Some(profile_id) {
            return effects;
        }
        push_status(
            &mut effects,
            model,
            crate::app::model::AppStatus::Info(format!("Connecting to {}…", profile_name)),
        );
        queue_connect(model, profile_id);
    } else if let Some(sub) = model.selected_subscription() {
        let id = sub.id;
        let name = sub.name.clone();
        if !model
            .config
            .profiles
            .iter()
            .any(|p| p.subscription_id == Some(id))
        {
            if !download_allowed(model) {
                let mut result = vec![Effect::SaveConfig];
                push_download_blocked(&mut result, model, DownloadKind::Subscription);
                return result;
            }
            model.subscription_fetching = true;
            model.subscription_updates.insert(id);
            let mut result = vec![Effect::SaveConfig, Effect::UpdateSubscription { id }];
            push_status(
                &mut result,
                model,
                crate::app::model::AppStatus::Info(format!("Updating subscription '{}'…", name)),
            );
            return result;
        }
    } else {
        push_status(
            &mut effects,
            model,
            crate::app::model::AppStatus::Info("No sources. Press p to paste or e to edit.".into()),
        );
    }
    effects
}

fn handle_update_key(model: &mut Model) -> Vec<Effect> {
    let mut effects = Vec::new();
    if let Some(idx) = model.selected_subscription_index() {
        if let Some(sub) = model.config.subscriptions.get(idx) {
            let id = sub.id;
            let name = sub.name.clone();
            if !download_allowed(model) {
                push_download_blocked(&mut effects, model, DownloadKind::Subscription);
                return effects;
            }
            model.subscription_fetching = true;
            model.subscription_updates.insert(id);
            let mut result = vec![Effect::SaveConfig, Effect::UpdateSubscription { id }];
            push_status(
                &mut result,
                model,
                crate::app::model::AppStatus::Info(format!("Updating subscription '{}'…", name)),
            );
            return result;
        }
    } else if !model.geo_updating {
        if model.config.settings.geo_routing.current_region == Some(GeoRegion::Global) {
            let services = model.config.settings.geo_routing.enabled_services();
            if services.is_empty() {
                push_status(
                    &mut effects,
                    model,
                    crate::app::model::AppStatus::Info(
                        "No enabled service rule-sets to update".to_string(),
                    ),
                );
                return effects;
            }
            if !download_allowed(model) {
                push_download_blocked(&mut effects, model, DownloadKind::Geo);
                return effects;
            }
            model.geo_updating = true;
            model.geo_automatic_update = false;
            push_status(
                &mut effects,
                model,
                crate::app::model::AppStatus::Info("Checking service rule-sets...".to_string()),
            );
            effects.push(Effect::RetryServiceRuleSets { services });
            return effects;
        }
        if !download_allowed(model) {
            push_download_blocked(&mut effects, model, DownloadKind::Geo);
            return effects;
        }
        model.geo_updating = true;
        model.geo_automatic_update = false;
        model.geo_last_attempt_at = Some(chrono::Local::now());
        push_status(
            &mut effects,
            model,
            crate::app::model::AppStatus::Info("Checking for geo updates...".to_string()),
        );
        effects.push(Effect::DownloadGeo);
        return effects;
    }
    effects
}

pub(in crate::app::update) fn is_connection_affected(model: &Model) -> bool {
    let affected =
        |id| model.active_profile_id == Some(id) || model.connecting_profile_id == Some(id);
    if model.active_profile_id.is_none() && model.connecting_profile_id.is_none() {
        return false;
    }
    use crate::app::model::SourceRow;
    match model.selected_row() {
        Some(SourceRow::StandaloneProfile(_)) | Some(SourceRow::SubscriptionProfile { .. }) => {
            model
                .selected_profile()
                .map(|p| affected(p.id))
                .unwrap_or(false)
        }
        Some(SourceRow::SubscriptionHeader(_)) => model
            .selected_subscription()
            .map(|sub| {
                model
                    .config
                    .profiles
                    .iter()
                    .any(|p| p.subscription_id == Some(sub.id) && affected(p.id))
            })
            .unwrap_or(false),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::msg::Msg;
    use crate::app::update::key::handle_key;
    use crate::app::update::key::ipc::handle_ipc_command;
    use crate::app::update::tick::handle_tick;
    use crate::app::update::update;
    use crate::config::profile::{
        DnsPreset, DnsStrategy, GeoAutoUpdate, Profile, RoutedService, RoutingMode, Subscription,
    };
    use crate::test_helpers::*;
    use chrono::Local;
    use uuid::Uuid;

    #[test]
    fn normal_mode_navigates() {
        let mut model = model_with_profiles(vec![
            Profile::new_vless(
                "A".to_string(),
                "1.1.1.1".to_string(),
                443,
                "u1".to_string(),
            ),
            Profile::new_vless(
                "B".to_string(),
                "2.2.2.2".to_string(),
                443,
                "u2".to_string(),
            ),
        ]);
        assert_eq!(model.selected, 0);
        let _ = handle_sources(&mut model, key('j'));
        assert_eq!(model.selected, 1);
        let _ = handle_sources(&mut model, key('k'));
        assert_eq!(model.selected, 0);
        let _ = handle_sources(&mut model, key('G'));
        assert_eq!(model.selected, 1);
        let _ = handle_sources(&mut model, key('g'));
        assert_eq!(model.selected, 1); // a lone g is only a client-side prefix
        let _ = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::GoFirst);
        assert_eq!(model.selected, 0);
    }

    #[test]
    fn normal_mode_enter_connects() {
        let a = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let b = Profile::new_vless("B".into(), "2.2.2.2".into(), 443, "u2".into());
        let a_id = a.id;
        let mut model = model_with_profiles(vec![a, b]);
        let effects = handle_sources(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(a_id));
        assert_eq!(effects, vec![app_log_info("Connecting to A…")]);

        // Moving the cursor before the daemon tick must not retarget the attempt.
        model.select_next();
        let effects = handle_tick(&mut model);
        assert!(
            effects.iter().any(
                |effect| matches!(effect, Effect::Connect { profile, .. } if profile.id == a_id)
            )
        );
    }

    #[test]
    fn normal_mode_enter_no_profile() {
        let mut model = model_with_profiles(vec![]);
        let effects = handle_sources(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(model.overlay, Overlay::None);
        assert_eq!(
            effects,
            vec![app_log_info("No sources. Press p to paste or e to edit.")]
        );
    }

    #[test]
    fn normal_mode_enter_on_subscription_header_does_nothing() {
        use crate::config::profile::Subscription;
        use uuid::Uuid;

        let sub_id = Uuid::new_v4();
        let mut profile = Profile::new_vless(
            "SubProfile".to_string(),
            "2.2.2.2".to_string(),
            443,
            "u2".to_string(),
        );
        profile.subscription_id = Some(sub_id);
        let mut model = model_with_profiles(vec![profile]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Sub".to_string(),
            url: "http://example.com".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        model.selected = 0; // subscription header
        let effects = handle_sources(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(model.connection, ConnectionState::Idle);
        assert!(effects.is_empty());
    }

    #[test]
    fn normal_mode_enter_on_empty_subscription_updates_it() {
        use crate::config::profile::Subscription;
        use uuid::Uuid;

        let sub_id = Uuid::new_v4();
        let mut model = model_with_profiles(vec![]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Sub".to_string(),
            url: "http://example.com".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        model.selected = 0; // subscription header
        let effects = handle_sources(&mut model, KeyEvent::from(KeyCode::Enter));
        assert!(model.subscription_fetching);
        assert!(effects.contains(&Effect::UpdateSubscription { id: sub_id }));
        assert!(effects.contains(&Effect::SaveConfig));
    }

    #[test]
    fn normal_mode_d_confirms_delete() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        let effects = handle_sources(&mut model, key('d'));
        assert_eq!(model.overlay, Overlay::ConfirmDelete);
        assert!(effects.is_empty());
    }

    #[test]
    fn normal_mode_m_opens_routing() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model
            .config
            .settings
            .geo_routing
            .set_mode(RoutingMode::Bypass(GeoRegion::Ru));
        let effects = handle_sources(&mut model, key('m'));
        assert_eq!(model.overlay, Overlay::RoutingMode);
        assert_eq!(model.routing_selected, 1);
        assert!(effects.is_empty());
    }

    #[test]
    fn normal_mode_u_in_global_updates_enabled_services() {
        let mut model = model_with_profiles(vec![]);
        model
            .config
            .settings
            .geo_routing
            .set_region(GeoRegion::Global);
        model.config.settings.geo_routing.service_routes.insert(
            RoutedService::Telegram,
            crate::config::profile::ServiceRoute::Proxy,
        );
        model.connection = ConnectionState::Connected;

        let effects = handle_sources(&mut model, key('u'));
        assert!(model.geo_updating);
        assert!(!effects.contains(&Effect::DownloadGeo));
        assert!(effects.contains(&Effect::RetryServiceRuleSets {
            services: vec![RoutedService::Telegram],
        }));
    }

    #[test]
    fn normal_mode_u_in_global_without_services_is_noop() {
        let mut model = model_with_profiles(vec![]);
        model
            .config
            .settings
            .geo_routing
            .set_region(GeoRegion::Global);

        let effects = handle_sources(&mut model, key('u'));
        assert!(!model.geo_updating);
        assert!(!effects.contains(&Effect::DownloadGeo));
        assert!(model.status.text().contains("No enabled service"));
    }

    #[test]
    fn shift_i_cycles_geo_auto_update_and_saves() {
        let mut model = model_with_profiles(vec![]);
        for expected in [
            GeoAutoUpdate::Every1d,
            GeoAutoUpdate::Every3d,
            GeoAutoUpdate::Every7d,
            GeoAutoUpdate::Off,
        ] {
            let effects = handle_sources(&mut model, key('I'));
            assert_eq!(model.config.settings.geo_routing.auto_update, expected);
            assert!(effects.contains(&Effect::SaveConfig));
            assert!(model.status.text().contains(&expected.label()));
        }
    }

    #[test]
    fn connected_mode_s_disconnects() {
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Connected;
        model.overlay = Overlay::None;
        let effects = handle_key(&mut model, key('s'));
        assert_eq!(effects, vec![Effect::Disconnect]);
    }

    #[test]
    fn connected_mode_navigates() {
        let mut model = model_with_profiles(vec![
            Profile::new_vless(
                "A".to_string(),
                "1.1.1.1".to_string(),
                443,
                "u1".to_string(),
            ),
            Profile::new_vless(
                "B".to_string(),
                "2.2.2.2".to_string(),
                443,
                "u2".to_string(),
            ),
        ]);
        model.connection = ConnectionState::Connected;
        model.overlay = Overlay::None;
        assert_eq!(model.selected, 0);
        let _ = handle_key(&mut model, key('j'));
        assert_eq!(model.selected, 1);
        let _ = handle_key(&mut model, key('k'));
        assert_eq!(model.selected, 0);
        let _ = handle_key(&mut model, key('G'));
        assert_eq!(model.selected, 1);
        let _ = handle_key(&mut model, key('g'));
        assert_eq!(model.selected, 1);
        let _ = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::GoFirst);
        assert_eq!(model.selected, 0);
    }

    #[test]
    fn connected_mode_enter_connects() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        )]);
        model.connection = ConnectionState::Connected;
        model.overlay = Overlay::None;
        let effects = handle_key(&mut model, KeyEvent::from(KeyCode::Enter));
        assert_eq!(effects, vec![app_log_info("Connecting to A…")]);
        assert_eq!(model.connection, ConnectionState::Connecting);
    }

    #[test]
    fn enter_on_active_profile_is_noop() {
        let profile = Profile::new_vless(
            "A".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        );
        let profile_id = profile.id;
        let mut model = model_with_profiles(vec![profile]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile_id);
        model.overlay = Overlay::None;
        let effects = handle_key(&mut model, KeyEvent::from(KeyCode::Enter));
        assert!(effects.is_empty());
        assert_eq!(model.connection, ConnectionState::Connected);
    }

    #[test]
    fn connected_mode_r_reconnects() {
        let a = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
        let b = Profile::new_vless("B".into(), "2.2.2.2".into(), 443, "u2".into());
        let a_id = a.id;
        let mut model = model_with_profiles(vec![a, b]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(a_id);
        model.overlay = Overlay::None;
        model.select_next();
        let effects = handle_key(&mut model, key('r'));
        assert_eq!(effects, vec![app_log_info("Reconnecting to A…")]);
        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(model.connecting_profile_id, Some(a_id));
    }

    #[test]
    fn connected_mode_help() {
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Connected;
        model.overlay = Overlay::None;
        let effects = handle_key(&mut model, key('?'));
        assert!(effects.is_empty());
        assert!(matches!(model.overlay, Overlay::Help(_)));
    }

    #[test]
    fn subscriptions_update_triggers_fetch() {
        let mut model = model_with_profiles(vec![]);
        let sub_id = Uuid::new_v4();
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
        model.selected = 0;

        let effects = handle_sources(&mut model, key('u'));

        assert!(model.subscription_fetching);
        assert!(model.subscription_updates.contains(&sub_id));
        assert_eq!(
            effects,
            vec![
                Effect::SaveConfig,
                Effect::UpdateSubscription { id: sub_id },
                app_log_info("Updating subscription 'Sub'…"),
            ]
        );
    }

    #[test]
    fn subscription_interval_shows_status() {
        use crate::config::profile::Subscription;
        use uuid::Uuid;

        let sub_id = Uuid::new_v4();
        let mut model = model_with_profiles(vec![]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Sub".to_string(),
            url: "http://example.com".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        model.selected = 0;
        model.config.subscriptions[0].record_fetch_failure(Local::now());
        let effects = handle_sources(&mut model, KeyEvent::from(KeyCode::Char('i')));
        assert_eq!(
            model.config.subscriptions[0].auto_update,
            SubscriptionAutoUpdate::Every3d
        );
        assert!(effects.contains(&Effect::SaveConfig));
        assert!(effects.contains(&app_log_info("Subscription 'Sub'  (3d)")));
        assert!(model.config.subscriptions[0].retry_state.is_none());
    }

    #[test]
    fn subscription_interval_status_uses_configured_icon_set() {
        use crate::config::profile::{IconSet, Subscription};

        let mut model = model_with_profiles(vec![]);
        model.config.settings.icons = IconSet::Unicode;
        model.config.subscriptions.push(Subscription {
            id: uuid::Uuid::new_v4(),
            name: "Sub".to_string(),
            url: "https://example.com".to_string(),
            auto_update: SubscriptionAutoUpdate::Every1d,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        model.selected = 0;

        let effects = handle_sources(&mut model, KeyEvent::from(KeyCode::Char('i')));

        assert!(effects.contains(&app_log_info("Subscription 'Sub' ↻ (3d)")));
    }

    #[test]
    fn subscriptions_interval_cycles() {
        let mut model = model_with_profiles(vec![]);
        model.config.subscriptions.push(Subscription {
            id: Uuid::new_v4(),
            name: "Sub".to_string(),
            url: "http://example.com/sub".to_string(),
            auto_update: SubscriptionAutoUpdate::Off,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        model.selected = 0;

        let _ = handle_sources(&mut model, KeyEvent::from(KeyCode::Char('i')));
        assert_eq!(
            model.config.subscriptions[0].auto_update,
            SubscriptionAutoUpdate::Every1d
        );
        let _ = handle_sources(&mut model, KeyEvent::from(KeyCode::Char('i')));
        assert_eq!(
            model.config.subscriptions[0].auto_update,
            SubscriptionAutoUpdate::Every3d
        );
        let _ = handle_sources(&mut model, KeyEvent::from(KeyCode::Char('i')));
        assert_eq!(
            model.config.subscriptions[0].auto_update,
            SubscriptionAutoUpdate::Every7d
        );
        let _ = handle_sources(&mut model, KeyEvent::from(KeyCode::Char('i')));
        assert_eq!(
            model.config.subscriptions[0].auto_update,
            SubscriptionAutoUpdate::Off
        );
    }

    // ── Profile testing ──────────────────────────────────────────────────────

    #[test]
    fn t_key_with_no_profile_does_nothing() {
        let mut model = model_with_profiles(vec![]);
        let key = KeyEvent::new(KeyCode::Char('t'), crossterm::event::KeyModifiers::NONE);
        let _ = update(&mut model, Msg::Key(key));
        assert!(model.pending_tests.is_empty());
        assert!(model.testing_profiles.is_empty());
    }

    #[test]
    fn t_key_adds_selected_profile_to_pending() {
        let profiles = sample_profiles();
        let id = profiles[0].id;
        let mut model = model_with_profiles(profiles);
        model.selected = 0;
        let key = KeyEvent::new(KeyCode::Char('t'), crossterm::event::KeyModifiers::NONE);
        let _ = update(&mut model, Msg::Key(key));
        assert_eq!(model.pending_tests.len(), 1);
        assert_eq!(model.pending_tests[0], id);
    }

    #[test]
    fn profile_tests_report_when_connectivity_probe_is_disabled() {
        let mut model = model_with_profiles(sample_profiles());
        model.config.settings.connectivity_probe.enabled = false;

        for code in [KeyCode::Char('t'), KeyCode::Char('T')] {
            let effects = update(
                &mut model,
                Msg::Key(KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)),
            );
            assert!(model.pending_tests.is_empty());
            assert!(effects.iter().any(|effect| matches!(
                effect,
                Effect::AppendAppLog { message, .. } if message.contains("latency testing is disabled")
            )));
        }
    }

    #[test]
    fn t_key_does_not_duplicate_already_queued_profile() {
        let profiles = sample_profiles();
        let id = profiles[0].id;
        let mut model = model_with_profiles(profiles);
        model.selected = 0;
        let key = KeyEvent::new(KeyCode::Char('t'), crossterm::event::KeyModifiers::NONE);
        let _ = update(&mut model, Msg::Key(key));
        let _ = update(&mut model, Msg::Key(key));
        assert_eq!(model.pending_tests.len(), 1, "no duplicate enqueue");
        assert_eq!(model.pending_tests[0], id);
    }

    #[test]
    fn shift_t_adds_all_profiles_to_pending() {
        let profiles = sample_profiles();
        let count = profiles.len();
        let mut model = model_with_profiles(profiles);
        let key = KeyEvent::new(KeyCode::Char('T'), crossterm::event::KeyModifiers::NONE);
        let _ = update(&mut model, Msg::Key(key));
        assert_eq!(model.pending_tests.len(), count);
    }

    #[test]
    fn shift_t_does_not_duplicate_already_queued_profiles() {
        let profiles = sample_profiles();
        let count = profiles.len();
        let mut model = model_with_profiles(profiles);
        let key = KeyEvent::new(KeyCode::Char('T'), crossterm::event::KeyModifiers::NONE);
        let _ = update(&mut model, Msg::Key(key));
        let _ = update(&mut model, Msg::Key(key));
        assert_eq!(
            model.pending_tests.len(),
            count,
            "second T adds no duplicates"
        );
    }

    #[test]
    fn manual_geo_update_is_blocked_only_by_disconnected_kill_switch() {
        let mut direct = model_with_profiles(vec![]);
        direct.config.settings.geo_routing.set_region(GeoRegion::Ru);
        let effects = update(&mut direct, Msg::Key(key('u')));
        assert!(effects.contains(&Effect::DownloadGeo));

        let mut blocked = model_with_profiles(vec![]);
        blocked
            .config
            .settings
            .geo_routing
            .set_region(GeoRegion::Ru);
        blocked.config.settings.kill_switch = true;
        let effects = update(&mut blocked, Msg::Key(key('u')));
        assert!(!effects.contains(&Effect::DownloadGeo));
        assert!(!blocked.geo_updating);
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::AppendAppLog { message, .. }
                if message.contains("blocked by the kill switch")
        )));

        blocked.connection = ConnectionState::Connected;
        let effects = update(&mut blocked, Msg::Key(key('u')));
        assert!(effects.contains(&Effect::DownloadGeo));
    }

    // ---- handle_sources gaps ----

    #[test]
    fn sources_d_blocked_when_active_profile_selected() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            "u".into(),
        )]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(model.config.profiles[0].id);
        model.selected = 0;
        let effects = handle_sources(&mut model, key('d'));
        assert_eq!(model.overlay, Overlay::None);
        assert!(model.status.is_error());
        assert!(model.status.text().contains("Disconnect before"));
        // Effect carries the AppendAppLog only (no SaveConfig, no opening overlay).
        assert!(!effects.iter().any(|e| matches!(e, Effect::SaveConfig)));
    }

    #[test]
    fn sources_d_blocked_when_active_profile_under_subscription_header() {
        // Create a subscription with one profile that's currently active.
        let sub_id = Uuid::new_v4();
        let mut profile = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u".into());
        profile.subscription_id = Some(sub_id);
        let mut model = model_with_profiles(vec![profile]);
        model.config.subscriptions.push(Subscription {
            id: sub_id,
            name: "Sub".into(),
            url: "http://s".into(),
            auto_update: SubscriptionAutoUpdate::Off,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(model.config.profiles[0].id);
        // Select the subscription header (first row now that the standalone group is empty).
        model.selected = crate::app::model::row_for_subscription_header(&model.config, 0);
        let _ = handle_sources(&mut model, key('d'));
        assert_eq!(model.overlay, Overlay::None);
        assert!(model.status.is_error());
    }

    #[test]
    fn sources_d_blocked_when_connecting_profile_selected() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            "u".into(),
        )]);
        model.connection = ConnectionState::ConnectPending;
        model.connecting_profile_id = Some(model.config.profiles[0].id);

        let effects = handle_sources(&mut model, key('d'));

        assert_eq!(model.overlay, Overlay::None);
        assert!(model.status.text().contains("Disconnect before"));
        assert!(!effects.contains(&Effect::SaveConfig));
    }

    #[test]
    fn sources_o_opens_geo_regions_preselecting_current() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Cn);
        let _ = handle_sources(&mut model, key('o'));
        assert_eq!(model.overlay, Overlay::GeoRegions);
        assert_eq!(model.geo_region_selected, 1);
    }

    #[test]
    fn sources_o_preselects_ir_and_global_and_falls_back_to_zero() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ir);
        let _ = handle_sources(&mut model, key('o'));
        assert_eq!(model.geo_region_selected, 2);

        let mut model = model_with_profiles(vec![]);
        model
            .config
            .settings
            .geo_routing
            .set_region(GeoRegion::Global);
        let _ = handle_sources(&mut model, key('o'));
        assert_eq!(model.geo_region_selected, 3);

        // No region set → defaults to 0.
        let mut model = model_with_profiles(vec![]);
        let _ = handle_sources(&mut model, key('o'));
        assert_eq!(model.geo_region_selected, 0);
    }

    #[test]
    fn sources_capital_d_opens_dns_settings_and_resets_draft() {
        let mut model = model_with_profiles(vec![]);
        model.dns_selected = 4;
        model.dns_preset_draft = Some(DnsPreset::GoogleDot);
        model.dns_strategy_draft = Some(DnsStrategy::OnlyIpv6);
        model.dns_fakeip_draft = Some(true);
        let effects = handle_sources(&mut model, key('D'));
        assert!(effects.is_empty());
        assert_eq!(model.overlay, Overlay::DnsSettings);
        assert_eq!(model.dns_selected, 0);
        assert!(model.dns_preset_draft.is_none());
        assert!(model.dns_strategy_draft.is_none());
        assert!(model.dns_fakeip_draft.is_none());
    }

    #[test]
    fn sources_unknown_key_is_noop() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            "u".into(),
        )]);
        let before_selected = model.selected;
        let before_overlay = model.overlay;
        let effects = handle_sources(&mut model, key('z'));
        assert!(effects.is_empty());
        assert_eq!(model.selected, before_selected);
        assert_eq!(model.overlay, before_overlay);
    }

    #[test]
    fn sources_r_ignored_when_not_connected() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            "u".into(),
        )]);
        // Idle by default.
        let effects = handle_sources(&mut model, key('r'));
        assert!(effects.is_empty());
        assert_eq!(model.connection, ConnectionState::Idle);
    }

    #[test]
    fn sources_i_on_non_subscription_is_noop() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            "u".into(),
        )]);
        // Cursor on standalone profile, not on a subscription header.
        let effects = handle_sources(&mut model, key('i'));
        assert!(effects.is_empty());
    }

    // ---- handle_enter_on_sources gaps ----

    #[test]
    fn enter_on_empty_sources_lists_a_message() {
        let mut model = model_with_profiles(vec![]);
        let effects = handle_enter_on_sources(&mut model);
        // No profile, no subscription → status message, no effects beyond log.
        assert!(model.status.text().contains("No sources"));
        assert!(!effects.iter().any(|e| matches!(e, Effect::Connect { .. })));
    }
}

use crate::app::effect::Effect;
use crate::app::model::{ConnectionState, Model, Overlay};
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::update::key::settings_menu::{finish_settings_overlay, return_to_settings_menu};
use crate::app::update::status::{
    DownloadKind, download_allowed, push_download_blocked, push_status,
};

/// Handle keys inside the service routing overlay. `h`/`l` cycle the
/// highlighted service's draft route; Enter commits the whole draft (and
/// reconnects if the active tunnel is affected); Esc discards it.
pub(in crate::app::update) fn handle_service_routing(
    model: &mut Model,
    key: KeyEvent,
) -> Vec<Effect> {
    use crate::config::profile::{RoutedService, ServiceRoute};

    let len = RoutedService::ALL.len();
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            crate::ui::nav::select_next(&mut model.service_routing_selected, len);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            crate::ui::nav::select_prev(&mut model.service_routing_selected);
        }
        KeyCode::Char('G') => crate::ui::nav::select_last(&mut model.service_routing_selected, len),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Char('h') | KeyCode::Left => {
            let Some(service) = RoutedService::ALL
                .get(model.service_routing_selected)
                .copied()
            else {
                return vec![];
            };
            // The draft is created when the overlay opens; a missing one
            // means inconsistent overlay state — do nothing.
            let Some(draft) = model.service_routing_draft.as_mut() else {
                return vec![];
            };
            let current = draft.get(&service).copied().unwrap_or_default();
            let forward = matches!(key.code, KeyCode::Char('l') | KeyCode::Right);
            let new = if forward {
                current.next()
            } else {
                current.prev()
            };
            // Absent = Disabled everywhere, so a route cycled back to
            // Disabled is removed rather than stored: a full cycle leaves
            // the draft identical to the committed map and Enter stays a
            // no-op instead of saving + reconnecting for nothing.
            if new == ServiceRoute::Disabled {
                draft.remove(&service);
            } else {
                draft.insert(service, new);
            }
        }
        KeyCode::Enter => {
            let Some(draft) = model.service_routing_draft.take() else {
                finish_settings_overlay(model);
                return vec![];
            };
            finish_settings_overlay(model);
            let changed = draft != model.config.settings.geo_routing.service_routes;
            if !changed {
                return vec![];
            }
            model.config.settings.geo_routing.service_routes = draft;
            let mut effects = vec![Effect::SaveConfig, Effect::BroadcastState];
            match model.connection {
                ConnectionState::Connected => {
                    // Don't reconnect yet: a first-enabled service's
                    // rule-sets may not be on disk, and the new sing-box
                    // would come up without its rules (inert until a second
                    // manual reconnect). Fetch them through the STILL-ACTIVE
                    // tunnel first; `Msg::ServiceRuleSetsReady` then
                    // reconnects the active profile (missing files degrade
                    // gracefully — the reconnect happens regardless).
                    model.pending_service_reconnect = true;
                    push_status(
                        &mut effects,
                        model,
                        crate::app::model::AppStatus::Info(
                            "Service routing changed — updating rule-sets".into(),
                        ),
                    );
                    effects.push(Effect::DownloadServiceRuleSetsIfMissing);
                }
                ConnectionState::Connecting | ConnectionState::ConnectPending => {
                    // A connect is already in flight with a settings snapshot
                    // taken before this commit — be honest that the change is
                    // not active yet.
                    push_status(
                        &mut effects,
                        model,
                        crate::app::model::AppStatus::Info(
                            "Service routing saved — takes effect on next reconnect".into(),
                        ),
                    );
                }
                _ => {
                    if model
                        .config
                        .settings
                        .geo_routing
                        .enabled_services()
                        .is_empty()
                    {
                        push_status(
                            &mut effects,
                            model,
                            crate::app::model::AppStatus::Info("Service routing updated".into()),
                        );
                    } else if download_allowed(model) {
                        push_status(
                            &mut effects,
                            model,
                            crate::app::model::AppStatus::Info(
                                "Service routing changed — updating rule-sets".into(),
                            ),
                        );
                        effects.push(Effect::DownloadServiceRuleSetsIfMissing);
                    } else {
                        push_download_blocked(&mut effects, model, DownloadKind::Geo);
                    }
                }
            }
            return effects;
        }
        KeyCode::Char('q') | KeyCode::Esc => {
            model.settings_menu_return = None;
            model.overlay = Overlay::None;
            model.service_routing_draft = None;
        }
        KeyCode::Backspace if model.settings_menu_return.is_some() => {
            model.service_routing_draft = None;
            return_to_settings_menu(model);
        }
        _ => {}
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::update::key::ipc::handle_go_first;
    use crate::app::update::key::sources::handle_sources;
    use crate::config::profile::Profile;
    use crate::test_helpers::*;

    fn with_service_routing_overlay() -> Model {
        let mut model = model_with_profiles(vec![]);
        handle_sources(&mut model, key('S'));
        assert_eq!(model.overlay, Overlay::ServiceRouting);
        model
    }

    #[test]
    fn service_routing_opens_with_draft_copy_of_committed_routes() {
        use crate::config::profile::{RoutedService, ServiceRoute};
        let mut model = model_with_profiles(vec![]);
        model
            .config
            .settings
            .geo_routing
            .service_routes
            .insert(RoutedService::Steam, ServiceRoute::Direct);
        handle_sources(&mut model, key('S'));
        assert_eq!(model.overlay, Overlay::ServiceRouting);
        assert_eq!(model.service_routing_selected, 0);
        assert_eq!(
            model.service_routing_draft,
            Some(model.config.settings.geo_routing.service_routes.clone())
        );
    }

    #[test]
    fn service_routing_navigation_and_bounds() {
        use crate::config::profile::RoutedService;
        let mut model = with_service_routing_overlay();
        assert_eq!(model.service_routing_selected, 0);
        handle_service_routing(&mut model, key('j'));
        assert_eq!(model.service_routing_selected, 1);
        handle_service_routing(&mut model, key('k'));
        assert_eq!(model.service_routing_selected, 0);
        handle_service_routing(&mut model, key('G'));
        assert_eq!(model.service_routing_selected, RoutedService::ALL.len() - 1);
        handle_service_routing(&mut model, key('g'));
        assert_eq!(model.service_routing_selected, RoutedService::ALL.len() - 1);
        handle_go_first(&mut model);
        assert_eq!(model.service_routing_selected, 0);
    }

    #[test]
    fn service_routing_l_and_h_cycle_draft_route() {
        use crate::config::profile::{RoutedService, ServiceRoute};
        let mut model = with_service_routing_overlay();
        let route_of = |m: &Model, s| {
            m.service_routing_draft
                .as_ref()
                .unwrap()
                .get(&s)
                .copied()
                .unwrap_or_default()
        };
        assert_eq!(
            route_of(&model, RoutedService::Steam),
            ServiceRoute::Disabled
        );
        handle_service_routing(&mut model, key('l'));
        assert_eq!(route_of(&model, RoutedService::Steam), ServiceRoute::Proxy);
        handle_service_routing(&mut model, key('l'));
        assert_eq!(route_of(&model, RoutedService::Steam), ServiceRoute::Direct);
        handle_service_routing(&mut model, key('h'));
        assert_eq!(route_of(&model, RoutedService::Steam), ServiceRoute::Proxy);
        // Only the highlighted service's draft changes; committed is untouched.
        assert_eq!(
            route_of(&model, RoutedService::Telegram),
            ServiceRoute::Disabled
        );
        assert!(model.config.settings.geo_routing.service_routes.is_empty());
    }

    #[test]
    fn service_routing_enter_commits_draft_and_saves() {
        use crate::config::profile::{RoutedService, ServiceRoute};
        let mut model = with_service_routing_overlay();
        handle_service_routing(&mut model, key('l')); // Steam → Proxy
        let effects = handle_service_routing(&mut model, enter());
        assert_eq!(model.overlay, Overlay::None);
        assert!(model.service_routing_draft.is_none());
        assert_eq!(
            model
                .config
                .settings
                .geo_routing
                .service_route(RoutedService::Steam),
            ServiceRoute::Proxy
        );
        assert!(effects.contains(&Effect::SaveConfig));
        assert!(effects.contains(&Effect::DownloadServiceRuleSetsIfMissing));
        // Not connected — no reconnect.
        assert_eq!(model.connection, ConnectionState::Idle);
    }

    #[test]
    fn service_routing_enter_without_changes_is_noop() {
        let mut model = with_service_routing_overlay();
        let effects = handle_service_routing(&mut model, enter());
        assert_eq!(model.overlay, Overlay::None);
        assert!(model.service_routing_draft.is_none());
        assert!(!effects.contains(&Effect::SaveConfig));
    }

    #[test]
    fn service_routing_esc_discards_draft() {
        let mut model = with_service_routing_overlay();
        handle_service_routing(&mut model, key('l'));
        handle_service_routing(&mut model, esc());
        assert_eq!(model.overlay, Overlay::None);
        assert!(model.service_routing_draft.is_none());
        assert!(model.config.settings.geo_routing.service_routes.is_empty());
    }

    #[test]
    fn service_routing_commit_while_connected_downloads_before_reconnect() {
        // First-enable ordering: the commit must NOT reconnect immediately —
        // a first-enabled service's rule-sets may not be on disk yet, and
        // the new sing-box would come up without its rules. Instead it
        // downloads using the still-active tunnel and defers the reconnect
        // to Msg::ServiceRuleSetsReady (covered in update.rs tests).
        let a = Profile::new_vless("A".into(), "e".into(), 1, "u".into());
        let active_id = a.id;
        let mut model = model_with_profiles(vec![a]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(active_id);
        handle_sources(&mut model, key('S'));
        handle_service_routing(&mut model, key('l'));
        let effects = handle_service_routing(&mut model, enter());
        assert!(effects.contains(&Effect::SaveConfig));
        assert!(effects.contains(&Effect::DownloadServiceRuleSetsIfMissing));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Connect { .. })),
            "reconnect must wait for the rule-set download to finish"
        );
        // The old tunnel keeps running while the download is in flight.
        assert_eq!(model.connection, ConnectionState::Connected);
        assert!(model.pending_service_reconnect);
    }

    #[test]
    fn service_routing_full_cycle_back_to_disabled_commits_as_noop() {
        // Disabled → Proxy → Direct → Disabled must land on a draft
        // identical to the (empty) committed map: absent = Disabled, so the
        // entry is removed rather than stored, and Enter neither saves nor
        // reconnects.
        let profile = Profile::new_vless("A".into(), "e".into(), 1, "u".into());
        let mut model = model_with_profiles(vec![profile.clone()]);
        model.connection = ConnectionState::Connected;
        model.active_profile_id = Some(profile.id);
        handle_sources(&mut model, key('S'));
        handle_service_routing(&mut model, key('l')); // Proxy
        handle_service_routing(&mut model, key('l')); // Direct
        handle_service_routing(&mut model, key('l')); // Disabled
        assert!(
            model.service_routing_draft.as_ref().unwrap().is_empty(),
            "cycling back to Disabled must remove the entry, not store it"
        );
        let effects = handle_service_routing(&mut model, enter());
        assert_eq!(model.overlay, Overlay::None);
        assert!(!effects.contains(&Effect::SaveConfig));
        assert!(!effects.contains(&Effect::DownloadServiceRuleSetsIfMissing));
        assert!(!model.pending_service_reconnect);
        assert_eq!(model.connection, ConnectionState::Connected);
    }

    #[test]
    fn service_routing_commit_during_pending_connect_saves_without_reconnect() {
        let profile = Profile::new_vless("A".into(), "e".into(), 1, "u".into());
        let mut model = model_with_profiles(vec![profile]);
        model.connection = ConnectionState::ConnectPending;
        handle_sources(&mut model, key('S'));
        handle_service_routing(&mut model, key('l'));
        let effects = handle_service_routing(&mut model, enter());
        assert!(effects.contains(&Effect::SaveConfig));
        assert!(!effects.iter().any(|e| matches!(e, Effect::Connect { .. })));
        assert_eq!(model.connection, ConnectionState::ConnectPending);
        assert_eq!(
            model.status.text(),
            "Service routing saved — takes effect on next reconnect"
        );
    }
}

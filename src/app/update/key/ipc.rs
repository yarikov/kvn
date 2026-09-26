mod semantic;
mod support;

use crate::app::effect::Effect;
use crate::app::model::{AppStatus, Model, Overlay};

use crate::app::update::key::handle_key;
use crate::app::update::key::settings_menu::{set_auto_connect, set_kill_switch};
use crate::app::update::key::theme::theme_picker_slugs;
use crate::app::update::onboarding;
use crate::app::update::paste::handle_clipboard_text;
use crate::app::update::routing::{commit_geo_region, commit_routing_mode};
use crate::app::update::status::{handle_copied_status, push_status};

pub(in crate::app::update) fn handle_ipc_command(
    model: &mut Model,
    cmd: crate::app::msg::IpcCommand,
) -> Vec<Effect> {
    use crate::app::msg::IpcCommand;
    match cmd {
        IpcCommand::ClearErrorStatus { status_revision } => {
            model.clear_error_status(status_revision);
            return finish_ipc_effects(vec![]);
        }
        IpcCommand::RestartRequired => {
            model.restart_required = true;
            model.overlay = Overlay::RestartRequired;
            return finish_ipc_effects(vec![]);
        }
        IpcCommand::Quit => return finish_ipc_effects(vec![Effect::Quit]),
        _ if model.restart_required => {
            return finish_ipc_effects(vec![]);
        }
        _ => {}
    }
    let effects = match cmd {
        IpcCommand::Attach | IpcCommand::AttachSession | IpcCommand::Detach => vec![],
        IpcCommand::CheckOnboarding => onboarding::resume(model),
        IpcCommand::CheckSupportPrompt => support::check_prompt(model),
        IpcCommand::ResolveSupportPrompt { resolution } => {
            support::resolve_prompt(model, resolution)
        }
        IpcCommand::Key { code, char, ctrl } => match rebuild_key_event(&code, char, ctrl) {
            Some(key) => handle_key(model, key),
            None => vec![],
        },
        IpcCommand::SelectSource { index } => {
            if index < model.source_rows().len() {
                model.selected = index;
            }
            vec![]
        }
        IpcCommand::SetMainPaneFocus { focus } => {
            model.main_pane_focus = focus;
            vec![]
        }
        IpcCommand::GoFirst => handle_go_first(model),
        IpcCommand::ConnectProfile { profile_id } => semantic::connect_profile(model, profile_id),
        IpcCommand::Disconnect => semantic::disconnect(model),
        IpcCommand::Reconnect => semantic::reconnect(model),
        IpcCommand::Toggle => semantic::toggle(model),
        IpcCommand::SetRoutingMode { mode } => commit_routing_mode(model, mode),
        IpcCommand::SetGeoRegion { region } => commit_geo_region(model, region),
        IpcCommand::SetKillSwitch { enabled } => set_kill_switch(model, enabled),
        IpcCommand::SetAutoConnect { enabled } => set_auto_connect(model, enabled),
        IpcCommand::Paste { text } => handle_clipboard_text(model, &text),
        IpcCommand::Copied { target } => handle_copied_status(model, target),
        IpcCommand::ReloadConfig => vec![Effect::ReloadConfig],
        IpcCommand::ApplyEditedConfig { base, edited } => apply_edited_config(model, base, edited),
        IpcCommand::ClientError { message } => {
            let mut effects = Vec::new();
            push_status(&mut effects, model, AppStatus::Error(message));
            effects
        }
        IpcCommand::ClearErrorStatus { .. } | IpcCommand::RestartRequired | IpcCommand::Quit => {
            unreachable!("handled above")
        }
    };
    finish_ipc_effects(effects)
}

fn apply_edited_config(
    model: &mut Model,
    base: Box<crate::config::profile::Config>,
    edited: Box<crate::config::profile::Config>,
) -> Vec<Effect> {
    if !model.config_persistence_blocked {
        return vec![Effect::CommitEditedConfig { base, edited }];
    }
    let conflicts = vec!["persisted config failed to load".to_string()];
    let mut effects = vec![Effect::SaveConfigConflict { edited, conflicts }];
    push_status(
        &mut effects,
        model,
        AppStatus::Error("Configuration edit failed: profiles.json is unreadable".into()),
    );
    effects
}

fn finish_ipc_effects(mut effects: Vec<Effect>) -> Vec<Effect> {
    effects.push(Effect::BroadcastState);
    effects
}

pub(in crate::app::update) fn handle_go_first(model: &mut Model) -> Vec<Effect> {
    match model.overlay {
        Overlay::None => model.select_first(),
        Overlay::SettingsMenu(_) => {}
        Overlay::RoutingMode => crate::ui::nav::select_first(&mut model.routing_selected),
        Overlay::GeoRegions => crate::ui::nav::select_first(&mut model.geo_region_selected),
        Overlay::DnsSettings => crate::ui::nav::select_first(&mut model.dns_selected),
        Overlay::ThemeSettings => {
            crate::ui::nav::select_first(&mut model.theme_selected);
            model.theme_draft = theme_picker_slugs().first().cloned();
        }
        Overlay::ServiceRouting => {
            crate::ui::nav::select_first(&mut model.service_routing_selected);
        }
        Overlay::Support => crate::ui::nav::select_first(&mut model.support_selected),
        Overlay::Onboarding(_) => {}
        Overlay::Help(mut state) => {
            state.selected = crate::ui::help::first_command(&crate::ui::help::rows(state.context));
            model.overlay = Overlay::Help(state);
        }
        Overlay::ConfirmDelete => {}
        Overlay::ConfirmDisable(_) => {}
        Overlay::RestartRequired => {}
    }
    vec![]
}

fn rebuild_key_event(
    code: &str,
    ch: Option<char>,
    ctrl: bool,
) -> Option<crossterm::event::KeyEvent> {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let key_code = match code {
        "Enter" => KeyCode::Enter,
        "Esc" => KeyCode::Esc,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Tab" => KeyCode::Tab,
        "BackTab" => KeyCode::BackTab,
        "Backspace" => KeyCode::Backspace,
        "Char" => KeyCode::Char(ch.unwrap_or(' ')),
        _ => return None,
    };
    let mut modifiers = KeyModifiers::empty();
    if ctrl {
        modifiers |= KeyModifiers::CONTROL;
    }
    Some(KeyEvent::new(key_code, modifiers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::ConnectionState;
    use crate::app::msg::Msg;
    use crate::app::update::key::dns::DnsSettingsItem;
    use crate::app::update::update;
    use crate::config::profile::{GeoRegion, Profile, RoutingMode};
    use crate::test_helpers::*;

    #[test]
    fn ipc_command_attach_broadcasts_state() {
        let mut model = model_with_profiles(vec![]);
        let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::Attach);
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn restart_required_opens_the_overlay_and_freezes_further_commands() {
        let mut model = model_with_profiles(vec![]);

        let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::RestartRequired);

        assert!(model.restart_required);
        assert_eq!(model.overlay, Overlay::RestartRequired);
        assert_eq!(effects, vec![Effect::BroadcastState]);

        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::SetAutoConnect { enabled: true },
        );
        assert!(!model.config.settings.auto_connect);
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn a_frozen_daemon_still_shuts_down_on_quit() {
        let mut model = model_with_profiles(vec![]);
        handle_ipc_command(&mut model, crate::app::msg::IpcCommand::RestartRequired);

        let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::Quit);

        assert!(effects.contains(&Effect::Quit));
    }

    #[test]
    fn ipc_error_status_clear_is_revision_safe() {
        let mut model = model_with_profiles(vec![]);
        model.set_status(AppStatus::Error("first".into()));
        let first_revision = model.status_revision;

        handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::ClearErrorStatus {
                status_revision: first_revision.wrapping_sub(1),
            },
        );
        assert_eq!(model.status, Some(AppStatus::Error("first".into())));

        handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::ClearErrorStatus {
                status_revision: first_revision,
            },
        );
        assert!(model.status.is_none());
        assert_eq!(model.status_revision, first_revision);

        model.set_status(AppStatus::Error("first".into()));
        assert_eq!(model.status_revision, first_revision + 1);
        assert_eq!(model.status, Some(AppStatus::Error("first".into())));
    }

    #[test]
    fn ipc_command_client_error_sets_status_and_logs() {
        let mut model = model_with_profiles(vec![]);
        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::ClientError {
                message: "Edit rejected: bad UUID".into(),
            },
        );
        assert_eq!(
            effects,
            vec![
                app_log_error("Edit rejected: bad UUID"),
                Effect::BroadcastState,
            ]
        );
        assert!(model.status_is_error(), "status: {:?}", model.status);
        assert_eq!(model.status_text(), "Edit rejected: bad UUID");
        // set_status also pushes into the in-memory log panel so the message
        // survives a later status overwrite.
        assert!(model.logs.iter().any(|l| l.contains("Edit rejected")));
    }

    #[test]
    fn ipc_command_reload_config_returns_effect() {
        let mut model = model_with_profiles(vec![]);
        let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::ReloadConfig);
        assert_eq!(effects, vec![Effect::ReloadConfig, Effect::BroadcastState]);
    }

    #[test]
    fn ipc_apply_edited_config_merges_concurrent_setting_change() {
        let mut model = model_with_profiles(vec![]);
        let base = model.config.clone();
        model.config.settings.auto_connect = true;
        let mut edited = base.clone();
        edited.settings.theme = "nord".into();

        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::ApplyEditedConfig {
                base: Box::new(base),
                edited: Box::new(edited),
            },
        );

        assert!(model.config.settings.auto_connect);
        assert_eq!(model.config.settings.theme, "tokyo-night");
        assert!(matches!(
            &effects[0],
            Effect::CommitEditedConfig { base: effect_base, edited: effect_edited }
                if effect_base.settings.theme == "tokyo-night" && effect_edited.settings.theme == "nord"
        ));
    }

    #[test]
    fn ipc_apply_edited_config_preserves_current_on_conflict() {
        let mut model = model_with_profiles(vec![]);
        let base = model.config.clone();
        model.config.settings.theme = "nord".into();
        let mut edited = base.clone();
        edited.settings.theme = "catppuccin".into();

        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::ApplyEditedConfig {
                base: Box::new(base),
                edited: Box::new(edited),
            },
        );

        assert_eq!(model.config.settings.theme, "nord");
        assert!(matches!(effects[0], Effect::CommitEditedConfig { .. }));
    }

    #[test]
    fn ipc_command_set_routing_mode_reaches_the_commit_and_broadcasts() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::SetRoutingMode {
                mode: RoutingMode::Bypass(GeoRegion::Ru),
            },
        );
        assert_eq!(
            model.config.settings.geo_routing.mode(),
            RoutingMode::Bypass(GeoRegion::Ru)
        );
        assert_eq!(effects.last(), Some(&Effect::BroadcastState));
    }

    #[test]
    fn ipc_command_set_routing_mode_reconnects_when_connected() {
        let (mut model, _) = connected_model();
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::SetRoutingMode {
                mode: RoutingMode::Bypass(GeoRegion::Ru),
            },
        );
        assert_eq!(model.connection, ConnectionState::Connecting);
        assert_eq!(
            effects,
            vec![
                Effect::SaveConfig,
                app_log_info("Routing mode changed: Bypass RU — reconnecting"),
                Effect::BroadcastState,
            ]
        );
    }

    #[test]
    fn ipc_command_set_routing_mode_rejects_unavailable_mode() {
        let mut model = model_with_profiles(vec![]);
        // No region selected (or Global) → only RoutingMode::Global is valid.
        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::SetRoutingMode {
                mode: RoutingMode::Only(GeoRegion::Ru),
            },
        );
        assert_eq!(
            model.config.settings.geo_routing.mode(),
            RoutingMode::Global
        );
        assert!(!effects.contains(&Effect::SaveConfig));
        assert_eq!(
            effects,
            vec![
                app_log_error(
                    "Routing mode change failed: Only RU is unavailable for region GLOBAL"
                ),
                Effect::BroadcastState,
            ]
        );
    }

    #[test]
    fn ipc_command_set_geo_region_reaches_the_commit_and_broadcasts() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);

        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::SetGeoRegion {
                region: GeoRegion::Cn,
            },
        );

        assert_eq!(
            model.config.settings.geo_routing.current_region,
            Some(GeoRegion::Cn)
        );
        assert_eq!(effects.last(), Some(&Effect::BroadcastState));
    }

    #[test]
    fn ipc_command_set_geo_region_same_region_only_saves() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::SetGeoRegion {
                region: GeoRegion::Ru,
            },
        );
        assert!(!model.geo_updating);
        assert!(!effects.contains(&Effect::DownloadGeoIfMissing));
        assert!(!effects.contains(&Effect::RefreshGeoLastUpdated));
        assert!(effects.contains(&Effect::SaveConfig));
    }

    #[test]
    fn ipc_command_set_kill_switch_applies_and_saves_via_result() {
        let mut model = model_with_profiles(vec![]);
        assert!(!model.config.settings.kill_switch);
        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::SetKillSwitch { enabled: true },
        );
        assert_eq!(model.kill_switch_pending, Some(true));
        assert_eq!(
            effects,
            vec![
                app_log_info("Enabling kill switch…"),
                Effect::ApplyKillSwitch { enabled: true },
                Effect::BroadcastState,
            ]
        );

        // While the first toggle is still in flight, a second is ignored.
        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::SetKillSwitch { enabled: false },
        );
        assert_eq!(effects, vec![Effect::BroadcastState]);
        assert_eq!(model.kill_switch_pending, Some(true));
    }

    #[test]
    fn ipc_command_set_kill_switch_noop_when_already_set() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.kill_switch = true;
        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::SetKillSwitch { enabled: true },
        );
        assert_eq!(effects, vec![Effect::BroadcastState]);
        assert_eq!(model.kill_switch_pending, None);
    }

    #[test]
    fn ipc_command_set_auto_connect_flips_and_saves() {
        let mut model = model_with_profiles(vec![]);
        assert!(!model.config.settings.auto_connect);
        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::SetAutoConnect { enabled: true },
        );
        assert!(!model.config.settings.auto_connect);
        assert_eq!(
            effects,
            vec![Effect::CheckAutoConnectPolkit, Effect::BroadcastState]
        );
        let effects = update(&mut model, Msg::AutoConnectPolkitChecked { error: None });
        assert!(model.config.settings.auto_connect);
        assert_eq!(
            effects,
            vec![
                app_log_info("Auto-connect enabled"),
                Effect::SaveConfig,
                Effect::BroadcastState,
            ]
        );

        // Setting the same value again is a no-op (no redundant save).
        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::SetAutoConnect { enabled: true },
        );
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn ipc_command_key_navigates() {
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
        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::Key {
                code: "Char".into(),
                char: Some('j'),
                ctrl: false,
            },
        );
        assert_eq!(effects, vec![Effect::BroadcastState]);
        assert_eq!(model.selected, 1);
    }

    #[test]
    fn rebuild_key_event_handles_tab_and_backtab() {
        use crossterm::event::{KeyCode, KeyModifiers};

        let tab = rebuild_key_event("Tab", None, false).unwrap();
        assert_eq!(tab.code, KeyCode::Tab);
        assert_eq!(tab.modifiers, KeyModifiers::empty());

        let backtab = rebuild_key_event("BackTab", None, false).unwrap();
        assert_eq!(backtab.code, KeyCode::BackTab);
        assert_eq!(backtab.modifiers, KeyModifiers::empty());

        let backspace = rebuild_key_event("Backspace", None, false).unwrap();
        assert_eq!(backspace.code, KeyCode::Backspace);
        assert_eq!(backspace.modifiers, KeyModifiers::empty());
    }

    // ---- rebuild_key_event ----

    #[test]
    fn rebuild_key_event_named_codes() {
        use crossterm::event::{KeyCode, KeyModifiers};
        assert_eq!(
            rebuild_key_event("Enter", None, false).unwrap().code,
            KeyCode::Enter
        );
        assert_eq!(
            rebuild_key_event("Esc", None, false).unwrap().code,
            KeyCode::Esc
        );
        assert_eq!(
            rebuild_key_event("Up", None, false).unwrap().code,
            KeyCode::Up
        );
        assert_eq!(
            rebuild_key_event("Down", None, false).unwrap().code,
            KeyCode::Down
        );
        assert_eq!(
            rebuild_key_event("Left", None, false).unwrap().code,
            KeyCode::Left
        );
        assert_eq!(
            rebuild_key_event("Right", None, false).unwrap().code,
            KeyCode::Right
        );
        let ctrl_c = rebuild_key_event("Char", Some('c'), true).unwrap();
        assert_eq!(ctrl_c.code, KeyCode::Char('c'));
        assert_eq!(ctrl_c.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn rebuild_key_event_char_without_value_uses_space() {
        let event = rebuild_key_event("Char", None, false).unwrap();
        assert_eq!(event.code, crossterm::event::KeyCode::Char(' '));
    }

    #[test]
    fn rebuild_key_event_unknown_code_returns_none() {
        assert!(rebuild_key_event("F1", None, false).is_none());
        assert!(rebuild_key_event("", None, false).is_none());
    }

    // ---- handle_ipc_command gaps ----

    #[test]
    fn ipc_command_detach_emits_broadcast_only() {
        let mut model = model_with_profiles(vec![]);
        let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::Detach);
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn ipc_command_updates_session_pane_focus() {
        use crate::app::model::MainPaneFocus;

        let mut model = model_with_profiles(vec![]);
        assert_eq!(model.main_pane_focus, MainPaneFocus::Sources);

        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::SetMainPaneFocus {
                focus: MainPaneFocus::Logs,
            },
        );
        assert_eq!(model.main_pane_focus, MainPaneFocus::Logs);
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn ipc_command_quit_emits_quit_then_broadcast() {
        let mut model = model_with_profiles(vec![]);
        let effects = handle_ipc_command(&mut model, crate::app::msg::IpcCommand::Quit);
        assert_eq!(effects, vec![Effect::Quit, Effect::BroadcastState]);
    }

    #[test]
    fn ipc_copy_routes_to_the_status_handler_and_broadcasts() {
        let mut model = model_with_profiles(vec![]);
        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::Copied {
                target: crate::app::msg::CopiedTarget::Profile {
                    name: "Alpha".into(),
                },
            },
        );
        assert!(model.status.is_some());
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::AppendAppLog { .. }))
        );
        assert_eq!(effects.last(), Some(&Effect::BroadcastState));
    }

    #[test]
    fn ipc_command_key_with_unknown_code_only_broadcasts() {
        let mut model = model_with_profiles(vec![]);
        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::Key {
                code: "F1".into(),
                char: None,
                ctrl: false,
            },
        );
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn ipc_go_first_dispatches_to_sources_and_selectable_overlays() {
        let mut model = model_with_profiles(vec![
            crate::config::profile::Profile::new_vless(
                "A".into(),
                "1.1.1.1".into(),
                443,
                "u1".into(),
            ),
            crate::config::profile::Profile::new_vless(
                "B".into(),
                "2.2.2.2".into(),
                443,
                "u2".into(),
            ),
        ]);
        model.selected = 1;
        handle_ipc_command(&mut model, crate::app::msg::IpcCommand::GoFirst);
        assert_eq!(model.selected, 0);

        model.overlay = Overlay::RoutingMode;
        model.routing_selected = 2;
        handle_ipc_command(&mut model, crate::app::msg::IpcCommand::GoFirst);
        assert_eq!(model.routing_selected, 0);

        model.overlay = Overlay::GeoRegions;
        model.geo_region_selected = 3;
        handle_ipc_command(&mut model, crate::app::msg::IpcCommand::GoFirst);
        assert_eq!(model.geo_region_selected, 0);

        model.overlay = Overlay::DnsSettings;
        model.dns_selected = DnsSettingsItem::ALL.len() - 1;
        handle_ipc_command(&mut model, crate::app::msg::IpcCommand::GoFirst);
        assert_eq!(model.dns_selected, 0);

        model.overlay = Overlay::ThemeSettings;
        model.theme_selected = theme_picker_slugs().len().saturating_sub(1);
        handle_ipc_command(&mut model, crate::app::msg::IpcCommand::GoFirst);
        assert_eq!(model.theme_selected, 0);
        assert_eq!(model.theme_draft, theme_picker_slugs().first().cloned());

        model.overlay = Overlay::ServiceRouting;
        model.service_routing_selected = 1;
        handle_ipc_command(&mut model, crate::app::msg::IpcCommand::GoFirst);
        assert_eq!(model.service_routing_selected, 0);
    }

    #[test]
    fn ipc_go_first_is_noop_for_non_selectable_overlays() {
        let mut model = model_with_profiles(vec![]);
        model.routing_selected = 2;
        for overlay in [
            Overlay::Help(crate::app::model::HelpState::default()),
            Overlay::ConfirmDelete,
        ] {
            model.overlay = overlay;
            handle_ipc_command(&mut model, crate::app::msg::IpcCommand::GoFirst);
            assert_eq!(model.overlay, overlay);
            assert_eq!(model.routing_selected, 2);
        }
    }

    #[test]
    fn ipc_mouse_selection_validates_the_source_index() {
        let profile = Profile::new_vless("A".into(), "e".into(), 1, "u".into());
        let mut model = model_with_profiles(vec![profile]);
        model.selected = 0;
        handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::SelectSource { index: 99 },
        );
        assert_eq!(model.selected, 0);
    }
}

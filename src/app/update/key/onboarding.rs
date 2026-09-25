use crossterm::event::{KeyCode, KeyEvent};

use crate::app::effect::Effect;
use crate::app::model::{Model, Overlay, SettingsMenuPage};
use crate::app::update::key::settings_menu::{set_auto_connect, set_kill_switch};
use crate::app::update::onboarding;
use crate::onboarding::OnboardingStep;

pub(in crate::app::update) fn handle_onboarding(
    model: &mut Model,
    step: OnboardingStep,
    key: KeyEvent,
) -> Vec<Effect> {
    match key.code {
        KeyCode::Enter => primary_action(model, step),
        // NOTE: these call `set_*` rather than the `toggle_*` helpers the main
        // screen uses. Turning either protection off through `toggle_*` opens
        // `Overlay::ConfirmDisable`, which would replace the card, and closing
        // that dialog returns to `Overlay::None` — leaving the tour with no way
        // back. A card dedicated to one setting is an explicit enough context to
        // skip the dialog, as the `Space c` screen already does.
        KeyCode::Char('A') if step == OnboardingStep::AutoConnect => {
            set_auto_connect(model, !model.config.settings.auto_connect)
        }
        KeyCode::Char('K') if step == OnboardingStep::KillSwitch => {
            set_kill_switch(model, !model.config.settings.kill_switch)
        }
        // The last card tells the user Space opens settings, so it does. The
        // card comes back through `card_pending` once that screen is closed —
        // the tour is not finished until Enter.
        KeyCode::Char(' ') if step == OnboardingStep::Finish => {
            model.onboarding.card_pending = true;
            model.settings_menu_return = None;
            model.settings_menu_selected = 0;
            model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
            vec![]
        }
        // The tour only moves forward, one step at a time: there is no skip, no
        // way back and no early exit, and the card is modal so q/Esc cannot
        // dismiss it either.
        _ => vec![],
    }
}

// The handoff is never skipped because a step already looks satisfied: a
// routing mode always has a value, so gating Enter on that would make the
// Routing card's real screen unreachable.
fn primary_action(model: &mut Model, step: OnboardingStep) -> Vec<Effect> {
    if step.handoff_screen(&model.config).is_some() {
        return onboarding::handoff(model, step);
    }
    if step == OnboardingStep::Finish {
        return onboarding::finish(model);
    }
    onboarding::advance(model, step.next(model.onboarding.include_omarchy_card))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::Overlay;
    use crate::app::update::key::handle_key;
    use crate::config::profile::GeoRegion;
    use crate::test_helpers::{enter, esc, key, model_with_profiles};

    fn tour_at(step: OnboardingStep) -> Model {
        let mut model = model_with_profiles(vec![]);
        model.onboarding.state.step = step;
        model.overlay = Overlay::Onboarding(step);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model
    }

    #[test]
    fn the_tour_only_moves_forward_and_only_through_enter() {
        // No skip and no way back: every navigation key a user might reach for
        // leaves the card exactly where it is.
        for code in [
            KeyCode::Char('n'),
            KeyCode::Char('l'),
            KeyCode::Right,
            KeyCode::Backspace,
            KeyCode::Char('h'),
            KeyCode::Left,
            KeyCode::Char('j'),
            KeyCode::Char('k'),
        ] {
            let mut model = tour_at(OnboardingStep::Doctor);
            assert!(handle_key(&mut model, KeyEvent::from(code)).is_empty());
            assert_eq!(
                model.overlay,
                Overlay::Onboarding(OnboardingStep::Doctor),
                "{code:?} moved the tour"
            );
            assert_eq!(model.onboarding.state.step, OnboardingStep::Doctor);
        }

        let mut model = tour_at(OnboardingStep::Doctor);
        handle_key(&mut model, enter());
        assert_eq!(
            model.overlay,
            Overlay::Onboarding(OnboardingStep::AutoConnect)
        );
    }

    #[test]
    fn enter_hands_off_even_when_the_step_already_looks_done() {
        let mut model = tour_at(OnboardingStep::Routing);
        assert!(handle_key(&mut model, enter()).is_empty());
        assert_eq!(model.overlay, Overlay::RoutingMode);
        assert_eq!(model.onboarding.awaiting, Some(OnboardingStep::Routing));
    }

    #[test]
    fn the_routing_card_only_hands_off_for_a_country_region() {
        let mut model = tour_at(OnboardingStep::Routing);
        model
            .config
            .settings
            .geo_routing
            .set_region(GeoRegion::Global);

        handle_key(&mut model, enter());

        // Nothing to choose under Global, so the card simply moves on.
        assert_eq!(model.overlay, Overlay::Onboarding(OnboardingStep::Profiles));
        assert_eq!(model.onboarding.awaiting, None);
    }

    #[test]
    fn the_mode_picker_cannot_be_left_without_choosing_one() {
        let mut model = tour_at(OnboardingStep::Routing);
        handle_key(&mut model, enter());
        assert_eq!(model.overlay, Overlay::RoutingMode);

        for code in [KeyCode::Esc, KeyCode::Char('q')] {
            assert!(handle_key(&mut model, KeyEvent::from(code)).is_empty());
            assert_eq!(model.overlay, Overlay::RoutingMode, "{code:?} closed it");
            assert_eq!(model.onboarding.awaiting, Some(OnboardingStep::Routing));
        }

        // Outside the tour it closes as before.
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.overlay = Overlay::RoutingMode;
        handle_key(&mut model, esc());
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn enter_advances_an_informational_card_and_finishes_the_last_one() {
        let mut model = tour_at(OnboardingStep::Doctor);
        handle_key(&mut model, enter());
        assert_eq!(
            model.overlay,
            Overlay::Onboarding(OnboardingStep::AutoConnect)
        );

        let mut model = tour_at(OnboardingStep::Finish);
        handle_key(&mut model, enter());
        assert!(model.onboarding.is_complete());
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn each_protection_card_toggles_its_own_setting() {
        use crate::app::model::Overlay;

        let mut model = tour_at(OnboardingStep::AutoConnect);
        let effects = handle_key(&mut model, key('A'));
        assert_eq!(effects, vec![Effect::CheckAutoConnectPolkit]);
        assert!(model.auto_connect_pending);
        assert_eq!(
            model.overlay,
            Overlay::Onboarding(OnboardingStep::AutoConnect),
            "the card must stay up"
        );

        let mut model = tour_at(OnboardingStep::KillSwitch);
        let effects = handle_key(&mut model, key('K'));
        assert!(effects.contains(&Effect::ApplyKillSwitch { enabled: true }));
        assert_eq!(
            model.overlay,
            Overlay::Onboarding(OnboardingStep::KillSwitch)
        );
    }

    #[test]
    fn turning_a_protection_off_on_its_card_skips_the_confirm_dialog() {
        // `Overlay::ConfirmDisable` would replace the card and close to the
        // main screen, stranding the tour.
        let mut model = tour_at(OnboardingStep::AutoConnect);
        model.config.settings.auto_connect = true;
        handle_key(&mut model, key('A'));
        assert!(!model.config.settings.auto_connect);
        assert_eq!(
            model.overlay,
            Overlay::Onboarding(OnboardingStep::AutoConnect)
        );

        let mut model = tour_at(OnboardingStep::KillSwitch);
        model.config.settings.kill_switch = true;
        let effects = handle_key(&mut model, key('K'));
        assert!(effects.contains(&Effect::ApplyKillSwitch { enabled: false }));
        assert_eq!(
            model.overlay,
            Overlay::Onboarding(OnboardingStep::KillSwitch)
        );
    }

    #[test]
    fn a_protection_key_does_nothing_on_another_card() {
        for (step, code) in [
            (OnboardingStep::KillSwitch, 'A'),
            (OnboardingStep::AutoConnect, 'K'),
            (OnboardingStep::Doctor, 'A'),
            (OnboardingStep::Doctor, 'K'),
        ] {
            let mut model = tour_at(step);
            assert!(
                handle_key(&mut model, key(code)).is_empty(),
                "{code} on {step:?}"
            );
            assert!(!model.auto_connect_pending);
            assert_eq!(model.kill_switch_pending, None);
        }
    }

    #[test]
    fn space_opens_settings_from_the_last_card_and_the_card_returns() {
        use crate::app::update::tick::handle_tick;

        let mut model = tour_at(OnboardingStep::Finish);
        handle_key(&mut model, key(' '));

        assert_eq!(model.overlay, Overlay::SettingsMenu(SettingsMenuPage::Root));
        assert!(
            !model.onboarding.is_complete(),
            "Space must not finish the tour"
        );

        // The card waits rather than replacing the screen the user just opened.
        handle_tick(&mut model);
        assert_eq!(model.overlay, Overlay::SettingsMenu(SettingsMenuPage::Root));

        handle_key(&mut model, key('q'));
        handle_tick(&mut model);
        assert_eq!(model.overlay, Overlay::Onboarding(OnboardingStep::Finish));

        handle_key(&mut model, enter());
        assert!(model.onboarding.is_complete());
    }

    #[test]
    fn space_does_nothing_on_the_other_cards() {
        for step in [OnboardingStep::Welcome, OnboardingStep::Doctor] {
            let mut model = tour_at(step);
            assert!(handle_key(&mut model, key(' ')).is_empty(), "{step:?}");
            assert_eq!(model.overlay, Overlay::Onboarding(step));
        }
    }

    #[test]
    fn a_card_ignores_dismissal_and_navigation_keys() {
        for code in [
            KeyCode::Esc,
            KeyCode::Char('q'),
            KeyCode::Char('s'),
            KeyCode::Char('x'),
            KeyCode::Char(' '),
        ] {
            let mut model = tour_at(OnboardingStep::AutoConnect);
            assert!(handle_key(&mut model, KeyEvent::from(code)).is_empty());
            assert_eq!(
                model.overlay,
                Overlay::Onboarding(OnboardingStep::AutoConnect),
                "{code:?} moved or closed the card"
            );
            assert!(!model.onboarding.is_complete());
        }

        let mut model = tour_at(OnboardingStep::AutoConnect);
        handle_key(&mut model, enter());
        assert_eq!(
            model.overlay,
            Overlay::Onboarding(OnboardingStep::KillSwitch)
        );
    }

    #[test]
    fn a_region_commit_returns_the_card_on_the_next_tick() {
        use crate::app::effect::Effect;
        use crate::app::update::tick::handle_tick;

        let mut model = tour_at(OnboardingStep::Region);
        model.config.settings.geo_routing.current_region = None;

        handle_key(&mut model, enter());
        assert_eq!(model.overlay, Overlay::GeoRegions);

        handle_key(&mut model, enter());
        assert_eq!(model.onboarding.awaiting, None);
        assert_eq!(model.onboarding.state.step, OnboardingStep::Routing);
        assert_eq!(model.overlay, Overlay::None);

        let effects = handle_tick(&mut model);
        assert_eq!(model.overlay, Overlay::Onboarding(OnboardingStep::Routing));
        assert!(effects.contains(&Effect::BroadcastState));
    }

    #[test]
    fn the_region_picker_cannot_be_left_without_choosing_one() {
        // Even with a region already stored, the tour's own step ends only by
        // committing one.
        for region in [None, Some(GeoRegion::Ru)] {
            let mut model = tour_at(OnboardingStep::Region);
            model.config.settings.geo_routing.current_region = region;

            handle_key(&mut model, enter());
            assert_eq!(model.overlay, Overlay::GeoRegions);

            for code in [KeyCode::Esc, KeyCode::Char('q')] {
                assert!(handle_key(&mut model, KeyEvent::from(code)).is_empty());
                assert_eq!(model.overlay, Overlay::GeoRegions, "{code:?} closed it");
                assert_eq!(model.onboarding.awaiting, Some(OnboardingStep::Region));
            }
        }

        // Outside the tour the picker closes again once a region is set.
        let mut model = model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.overlay = Overlay::GeoRegions;
        handle_key(&mut model, esc());
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn choosing_a_mode_advances_the_card_on_the_next_tick() {
        use crate::app::update::tick::handle_tick;

        let mut model = tour_at(OnboardingStep::Routing);
        handle_key(&mut model, enter());
        handle_key(&mut model, enter());

        assert_eq!(model.onboarding.state.step, OnboardingStep::Profiles);
        assert_eq!(model.overlay, Overlay::None);

        handle_tick(&mut model);
        assert_eq!(model.overlay, Overlay::Onboarding(OnboardingStep::Profiles));
    }

    #[test]
    fn a_rejected_mode_does_not_advance_the_tour() {
        use crate::app::update::routing::commit_routing_mode;
        use crate::config::profile::RoutingMode;

        let mut model = tour_at(OnboardingStep::Routing);
        handle_key(&mut model, enter());

        // Only reachable through IPC, which does not filter by availability.
        commit_routing_mode(&mut model, RoutingMode::Bypass(GeoRegion::Cn));

        assert!(model.status_is_error());
        assert_eq!(model.onboarding.state.step, OnboardingStep::Routing);
        assert_eq!(model.onboarding.awaiting, Some(OnboardingStep::Routing));
    }

    #[test]
    fn an_unrelated_settings_visit_does_not_cancel_the_profiles_step() {
        let mut model = tour_at(OnboardingStep::Profiles);
        handle_key(&mut model, enter());
        assert_eq!(model.overlay, Overlay::None);
        assert_eq!(model.onboarding.awaiting, Some(OnboardingStep::Profiles));

        handle_key(&mut model, key(' '));
        handle_key(&mut model, key('c'));
        handle_key(&mut model, esc());

        assert_eq!(model.onboarding.awaiting, Some(OnboardingStep::Profiles));
        assert!(!model.onboarding.card_pending);
    }

    #[test]
    fn only_a_connection_advances_past_the_profiles_step() {
        use crate::app::model::ConnectionState;
        use crate::app::msg::{IpcCommand, Msg};
        use crate::app::update::update;
        use crate::config::profile::{Profile, Subscription, SubscriptionAutoUpdate};

        let sub = Subscription {
            id: uuid::Uuid::new_v4(),
            name: "S".into(),
            url: "https://example.com/s".into(),
            auto_update: SubscriptionAutoUpdate::Off,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        };
        let id = sub.id;

        let mut model = tour_at(OnboardingStep::Profiles);
        model.config.subscriptions.push(sub);
        handle_key(&mut model, enter());
        assert_eq!(model.onboarding.awaiting, Some(OnboardingStep::Profiles));

        // Pasting a standalone profile is not enough.
        update(
            &mut model,
            Msg::IpcCommand(IpcCommand::Paste {
                text: "vless://671c62c7-6768-4b98-ac6b-572c9c707be0@203.0.113.42:443#A".into(),
            }),
        );
        assert_eq!(model.config.profiles.len(), 1);
        assert_eq!(model.onboarding.state.step, OnboardingStep::Profiles);

        // Neither is a subscription that imports profiles.
        update(
            &mut model,
            Msg::SubscriptionFetched {
                id,
                result: Ok(vec![Profile::new_vless(
                    "B".into(),
                    "1.1.1.1".into(),
                    443,
                    "u1".into(),
                )]),
            },
        );
        assert_eq!(model.onboarding.state.step, OnboardingStep::Profiles);
        assert_eq!(model.onboarding.awaiting, Some(OnboardingStep::Profiles));

        // Connecting with one of them is.
        let profile_id = model.config.profiles[0].id;
        let attempt_id = model.connect_attempt_id;
        model.connection = ConnectionState::Connecting;
        update(
            &mut model,
            Msg::Connected {
                pid: 1,
                profile_id,
                attempt_id,
            },
        );
        assert_eq!(model.onboarding.awaiting, None);
        assert_eq!(model.onboarding.state.step, OnboardingStep::Connected);
    }

    #[test]
    fn a_pending_card_never_replaces_the_help_popup() {
        use crate::app::update::tick::handle_tick;

        let mut model = tour_at(OnboardingStep::Profiles);
        model.onboarding.card_pending = true;
        model.overlay = Overlay::None;
        handle_key(&mut model, key('?'));

        handle_tick(&mut model);
        assert!(matches!(model.overlay, Overlay::Help(_)));
        assert!(model.onboarding.card_pending);

        handle_key(&mut model, key('?'));
        handle_tick(&mut model);
        assert_eq!(model.overlay, Overlay::Onboarding(OnboardingStep::Profiles));
    }

    #[test]
    fn help_opens_from_a_card_and_restores_it() {
        let mut model = tour_at(OnboardingStep::Finish);
        handle_key(&mut model, key('?'));
        assert!(matches!(
            model.overlay,
            Overlay::Help(crate::app::model::HelpState {
                context: crate::app::model::HelpContext::Onboarding(OnboardingStep::Finish),
                ..
            })
        ));

        handle_key(&mut model, key('?'));
        assert_eq!(model.overlay, Overlay::Onboarding(OnboardingStep::Finish));
    }
}

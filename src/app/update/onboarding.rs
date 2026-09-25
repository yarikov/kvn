use crate::app::effect::Effect;
use crate::app::model::{ConnectionState, Model, Overlay};
use crate::onboarding::{HandoffScreen, IntegrationSetup, OnboardingRecovery, OnboardingStep};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app::update) enum Trigger {
    RegionCommitted,
    RoutingCommitted,
    /// Raised by `Msg::Connected`. Adding a profile is deliberately not enough:
    /// the step is done once the user has actually connected with one, from a
    /// subscription or standalone alike.
    TunnelUp,
}

impl Trigger {
    fn matches(self, awaiting: OnboardingStep) -> bool {
        match self {
            Self::RegionCommitted => awaiting == OnboardingStep::Region,
            Self::RoutingCommitted => awaiting == OnboardingStep::Routing,
            Self::TunnelUp => awaiting == OnboardingStep::Profiles,
        }
    }
}

/// `Msg::Connected` is raised once per connection, so a `Profiles` step taken
/// while the tunnel is already up would wait for an event that has already
/// happened. A failed progress write restores the card, and a client can attach
/// long after the connection came up, so both ends allow for it.
fn tunnel_already_up(model: &Model, step: OnboardingStep) -> bool {
    step == OnboardingStep::Profiles && model.connection == ConnectionState::Connected
}

/// Open the real screen a card points at. Nothing durable changes, so this
/// cannot fail and emits no effect: a restart mid-handoff legitimately forgets
/// the pending screen and reopens the same card. The one exception is a step
/// whose outcome is already in: it has nothing to wait for, so it moves the card
/// on like an informational one.
pub(in crate::app::update) fn handoff(model: &mut Model, step: OnboardingStep) -> Vec<Effect> {
    let Some(screen) = step.handoff_screen(&model.config) else {
        return vec![];
    };
    if tunnel_already_up(model, step) {
        return advance(model, step.next(model.onboarding.include_omarchy_card));
    }
    model.onboarding.awaiting = Some(step);
    model.onboarding.card_pending = false;
    match screen {
        HandoffScreen::GeoRegions => {
            crate::app::update::key::settings_menu::open_geo_region(model, None);
        }
        HandoffScreen::RoutingMode => {
            crate::app::update::key::settings_menu::open_routing_mode(model, None);
        }
        HandoffScreen::MainScreen => {
            model.settings_menu_return = None;
            model.overlay = Overlay::None;
        }
    }
    vec![]
}

/// A TUI attaching mid-tour. A handoff in flight is left alone — the screen it
/// opened is the one the user has to act on — unless what it waits for has
/// already happened, which would otherwise strand the tour behind an event that
/// cannot repeat.
pub(in crate::app::update) fn resume(model: &mut Model) -> Vec<Effect> {
    if model.onboarding.is_complete() {
        return vec![];
    }
    if let Some(awaiting) = model.onboarding.awaiting {
        if tunnel_already_up(model, awaiting) {
            return outcome(model, Trigger::TunnelUp);
        }
        return vec![];
    }
    if model.overlay != Overlay::None {
        return vec![];
    }
    model.onboarding.card_pending = false;
    model.overlay = Overlay::Onboarding(model.onboarding.step());
    probe_visible_card(model)
}

const PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

fn card_reports_integration(step: OnboardingStep) -> bool {
    matches!(
        step,
        OnboardingStep::Omarchy | OnboardingStep::AutoConnect | OnboardingStep::KillSwitch
    )
}

/// Re-probe the integration the card on screen reports, so a setup command run
/// in another terminal shows up on it. Called both as a card opens and from the
/// 250 ms tick, with one gate for both: the probe spawns `pkcheck` / `id`, so it
/// runs at most every `PROBE_INTERVAL`, and never while one is still out — two
/// overlapping probes could report back out of order and restore a stale state.
pub(in crate::app::update) fn probe_visible_card(model: &mut Model) -> Vec<Effect> {
    let Overlay::Onboarding(step) = model.overlay else {
        return vec![];
    };
    if !card_reports_integration(step) || model.integration_check_pending {
        return vec![];
    }
    let now = std::time::Instant::now();
    let due = match model.last_integration_check_at {
        None => true,
        Some(prev) => now.duration_since(prev) >= PROBE_INTERVAL,
    };
    if !due {
        return vec![];
    }
    model.last_integration_check_at = Some(now);
    model.integration_check_pending = true;
    vec![Effect::CheckIntegrationSetup]
}

pub(in crate::app::update) fn integration_setup_checked(
    model: &mut Model,
    setup: IntegrationSetup,
) -> Vec<Effect> {
    model.integration_check_pending = false;
    if model.integration_setup == setup {
        return vec![];
    }
    model.integration_setup = setup;
    vec![Effect::BroadcastState]
}

pub(in crate::app::update) fn advance(model: &mut Model, step: OnboardingStep) -> Vec<Effect> {
    let previous = model.onboarding;
    model.onboarding.state.step = step;
    model.onboarding.awaiting = None;
    model.onboarding.card_pending = false;
    model.overlay = Overlay::Onboarding(step);
    let mut effects = vec![Effect::PersistOnboarding {
        previous,
        recovery: OnboardingRecovery::RestoreCard,
    }];
    effects.extend(probe_visible_card(model));
    effects
}

pub(in crate::app::update) fn finish(model: &mut Model) -> Vec<Effect> {
    let previous = model.onboarding;
    model.onboarding.state.complete(chrono::Utc::now());
    model.onboarding.awaiting = None;
    model.onboarding.card_pending = false;
    if model.config.settings.geo_routing.current_region.is_none() {
        crate::app::update::key::settings_menu::open_geo_region(model, None);
    } else {
        model.settings_menu_return = None;
        model.overlay = Overlay::None;
    }
    vec![Effect::PersistOnboarding {
        previous,
        recovery: OnboardingRecovery::RestoreCard,
    }]
}

pub(in crate::app::update) fn outcome(model: &mut Model, trigger: Trigger) -> Vec<Effect> {
    let Some(awaiting) = model.onboarding.awaiting else {
        return vec![];
    };
    if !trigger.matches(awaiting) {
        return vec![];
    }
    let previous = model.onboarding;
    model.onboarding.awaiting = None;
    model.onboarding.card_pending = true;
    model.onboarding.state.step = awaiting.next(model.onboarding.include_omarchy_card);
    vec![Effect::PersistOnboarding {
        previous,
        recovery: OnboardingRecovery::WaitForIdle,
    }]
}

pub(in crate::app::update) fn open_when_idle(model: &mut Model) -> Vec<Effect> {
    if !model.onboarding.card_pending
        || model.onboarding.is_complete()
        || model.overlay != Overlay::None
    {
        return vec![];
    }
    model.onboarding.card_pending = false;
    model.overlay = Overlay::Onboarding(model.onboarding.step());
    let mut effects = vec![Effect::BroadcastState];
    effects.extend(probe_visible_card(model));
    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::GeoRegion;
    use crate::test_helpers::model_with_profiles;

    fn tour_at(step: OnboardingStep) -> Model {
        let mut model = model_with_profiles(vec![]);
        model.onboarding.state.step = step;
        model.overlay = Overlay::Onboarding(step);
        model
    }

    #[test]
    fn the_cards_that_report_an_integration_re_probe_it_as_they_open() {
        let mut model = tour_at(OnboardingStep::Doctor);
        assert!(
            advance(&mut model, OnboardingStep::AutoConnect)
                .contains(&Effect::CheckIntegrationSetup)
        );

        let mut model = tour_at(OnboardingStep::Welcome);
        assert!(
            !advance(&mut model, OnboardingStep::Region).contains(&Effect::CheckIntegrationSetup)
        );

        let mut model = tour_at(OnboardingStep::KillSwitch);
        model.onboarding.card_pending = true;
        model.overlay = Overlay::None;
        assert!(open_when_idle(&mut model).contains(&Effect::CheckIntegrationSetup));

        let mut model = tour_at(OnboardingStep::Omarchy);
        model.overlay = Overlay::None;
        assert!(resume(&mut model).contains(&Effect::CheckIntegrationSetup));
    }

    #[test]
    fn one_throttle_covers_both_the_card_and_the_tick() {
        use crate::app::update::tick::handle_tick;

        // Opening the card probes, and the tick right after must not repeat it.
        let mut model = tour_at(OnboardingStep::Doctor);
        assert!(
            advance(&mut model, OnboardingStep::AutoConnect)
                .contains(&Effect::CheckIntegrationSetup)
        );
        let opened_at = model.last_integration_check_at;
        assert!(opened_at.is_some());
        assert!(!handle_tick(&mut model).contains(&Effect::CheckIntegrationSetup));
        assert_eq!(model.last_integration_check_at, opened_at);

        // The interval has passed, but the first probe is still out.
        model.last_integration_check_at = opened_at.and_then(|at| at.checked_sub(PROBE_INTERVAL));
        assert!(probe_visible_card(&mut model).is_empty());

        let reported = model.integration_setup;
        integration_setup_checked(&mut model, reported);
        assert!(!model.integration_check_pending);
        assert_eq!(
            probe_visible_card(&mut model),
            vec![Effect::CheckIntegrationSetup]
        );
    }

    #[test]
    fn only_a_card_that_reports_an_integration_is_probed() {
        for overlay in [
            Overlay::Onboarding(OnboardingStep::Doctor),
            Overlay::DnsSettings,
            Overlay::None,
        ] {
            let mut model = model_with_profiles(vec![]);
            model.overlay = overlay;
            assert!(
                probe_visible_card(&mut model).is_empty(),
                "{overlay:?} probed the integrations"
            );
            assert_eq!(model.last_integration_check_at, None);
        }
    }

    #[test]
    fn a_probe_result_broadcasts_only_when_it_changed() {
        let mut model = model_with_profiles(vec![]);
        let setup = IntegrationSetup {
            polkit: crate::onboarding::SetupState::Ready,
            ..IntegrationSetup::default()
        };

        assert_eq!(
            integration_setup_checked(&mut model, setup),
            vec![Effect::BroadcastState]
        );
        assert_eq!(model.integration_setup, setup);
        assert!(integration_setup_checked(&mut model, setup).is_empty());
    }

    #[test]
    fn resume_opens_the_stored_card_only_on_a_free_screen() {
        let mut model = tour_at(OnboardingStep::Profiles);
        model.overlay = Overlay::DnsSettings;

        let effects = crate::app::update::key::ipc::handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::CheckOnboarding,
        );
        assert_eq!(model.overlay, Overlay::DnsSettings);
        assert!(effects.contains(&Effect::BroadcastState));

        model.overlay = Overlay::None;
        crate::app::update::key::ipc::handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::CheckOnboarding,
        );
        assert_eq!(model.overlay, Overlay::Onboarding(OnboardingStep::Profiles));
    }

    #[test]
    fn resume_ignores_a_finished_tour() {
        let mut model = model_with_profiles(vec![]);
        model.onboarding.state.complete(chrono::Utc::now());

        assert!(resume(&mut model).is_empty());
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn resume_keeps_a_handoff_in_flight() {
        let mut model = tour_at(OnboardingStep::Profiles);
        assert!(handoff(&mut model, OnboardingStep::Profiles).is_empty());
        model.connection = ConnectionState::Connecting;

        assert!(resume(&mut model).is_empty());
        assert_eq!(model.overlay, Overlay::None);
        assert_eq!(model.onboarding.awaiting, Some(OnboardingStep::Profiles));
    }

    #[test]
    fn resume_finishes_a_profiles_step_whose_tunnel_is_already_up() {
        let mut model = tour_at(OnboardingStep::Profiles);
        handoff(&mut model, OnboardingStep::Profiles);
        model.connection = ConnectionState::Connected;

        assert!(matches!(
            resume(&mut model).as_slice(),
            [Effect::PersistOnboarding { .. }]
        ));
        assert_eq!(model.onboarding.awaiting, None);
        assert_eq!(model.onboarding.state.step, OnboardingStep::Connected);
        assert!(model.onboarding.card_pending);
    }

    #[test]
    fn the_profiles_card_moves_on_when_the_tunnel_is_already_up() {
        let mut model = tour_at(OnboardingStep::Profiles);
        model.connection = ConnectionState::Connected;

        let effects = handoff(&mut model, OnboardingStep::Profiles);
        assert!(matches!(
            effects.as_slice(),
            [Effect::PersistOnboarding {
                recovery: OnboardingRecovery::RestoreCard,
                ..
            }]
        ));
        assert_eq!(model.onboarding.awaiting, None);
        assert_eq!(
            model.overlay,
            Overlay::Onboarding(OnboardingStep::Connected)
        );
    }

    #[test]
    fn handoff_opens_the_real_screen_without_persisting() {
        let mut model = tour_at(OnboardingStep::Region);
        assert!(handoff(&mut model, OnboardingStep::Region).is_empty());
        assert_eq!(model.overlay, Overlay::GeoRegions);
        assert_eq!(model.onboarding.awaiting, Some(OnboardingStep::Region));
        assert_eq!(model.onboarding.state.step, OnboardingStep::Region);
    }

    #[test]
    fn outcome_only_fires_for_the_awaited_step() {
        let mut model = tour_at(OnboardingStep::Region);
        assert!(outcome(&mut model, Trigger::RegionCommitted).is_empty());

        model.onboarding.awaiting = Some(OnboardingStep::Region);
        assert!(outcome(&mut model, Trigger::RoutingCommitted).is_empty());
        assert_eq!(model.onboarding.awaiting, Some(OnboardingStep::Region));

        let effects = outcome(&mut model, Trigger::RegionCommitted);
        assert_eq!(
            effects,
            vec![Effect::PersistOnboarding {
                previous: crate::onboarding::OnboardingProgress {
                    state: crate::onboarding::OnboardingState {
                        step: OnboardingStep::Region,
                        completed_at: None,
                    },
                    awaiting: Some(OnboardingStep::Region),
                    card_pending: false,
                    include_omarchy_card: false,
                },
                recovery: OnboardingRecovery::WaitForIdle,
            }]
        );
        assert_eq!(model.onboarding.state.step, OnboardingStep::Routing);
        assert!(model.onboarding.card_pending);
        assert_eq!(model.onboarding.awaiting, None);
    }

    #[test]
    fn open_when_idle_waits_for_a_free_screen_and_broadcasts() {
        let mut model = tour_at(OnboardingStep::Profiles);
        model.onboarding.card_pending = true;
        model.overlay = Overlay::Help(crate::app::model::HelpState::default());
        assert!(open_when_idle(&mut model).is_empty());
        assert!(model.onboarding.card_pending);

        model.overlay = Overlay::None;
        assert_eq!(open_when_idle(&mut model), vec![Effect::BroadcastState]);
        assert_eq!(model.overlay, Overlay::Onboarding(OnboardingStep::Profiles));
        assert!(!model.onboarding.card_pending);
    }

    #[test]
    fn open_when_idle_ignores_a_finished_tour() {
        let mut model = model_with_profiles(vec![]);
        model.onboarding.card_pending = true;
        model.onboarding.state.complete(chrono::Utc::now());
        assert!(open_when_idle(&mut model).is_empty());
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn finish_forces_the_region_picker_when_no_region_is_set() {
        let mut model = tour_at(OnboardingStep::Finish);
        let effects = finish(&mut model);
        assert!(model.onboarding.is_complete());
        assert_eq!(model.overlay, Overlay::GeoRegions);
        assert!(matches!(
            effects.as_slice(),
            [Effect::PersistOnboarding {
                recovery: OnboardingRecovery::RestoreCard,
                ..
            }]
        ));

        let mut model = tour_at(OnboardingStep::Finish);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        finish(&mut model);
        assert_eq!(model.overlay, Overlay::None);
    }
}

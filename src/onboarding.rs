use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::profile::{Config, GeoRegion};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingStep {
    #[default]
    Welcome,
    Region,
    Routing,
    Profiles,
    Connected,
    Doctor,
    Omarchy,
    AutoConnect,
    KillSwitch,
    Finish,
}

/// How far the system setup a protection card depends on has got. Produced by
/// `doctor::integration_setup`, so the card can tell the user what is still
/// missing instead of always repeating the setup command.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupState {
    #[default]
    Missing,
    PendingReboot,
    Ready,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationSetup {
    #[serde(default)]
    pub polkit: SetupState,
    #[serde(default)]
    pub kill_switch: SetupState,
    /// The `kvn-tui` group both integrations gate on is already active in this
    /// session, so a fresh setup takes effect without a reboot.
    #[serde(default)]
    pub group_active: bool,
    #[serde(default)]
    pub omarchy_plugin: bool,
}

/// What a failed progress write has to put back on screen. The tour's own
/// screen is gone by then, so the recovery says where the card belongs rather
/// than where the transition came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingRecovery {
    /// Show the card again right away: the transition owned the screen.
    RestoreCard,
    /// Wait for a free screen: a real screen is still in front of the user.
    WaitForIdle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffScreen {
    GeoRegions,
    RoutingMode,
    MainScreen,
}

impl OnboardingStep {
    // NOTE: `Omarchy` comes before the two protection cards because
    // `kvn setup --omarchy` clones the plugin from GitHub, and an enabled kill
    // switch leaves the machine without internet until the reboot its pending
    // group membership needs. Do not move it back after `KillSwitch`.
    pub const ORDER: [Self; 10] = [
        Self::Welcome,
        Self::Region,
        Self::Routing,
        Self::Profiles,
        Self::Connected,
        Self::Doctor,
        Self::Omarchy,
        Self::AutoConnect,
        Self::KillSwitch,
        Self::Finish,
    ];

    /// The cards actually shown on this machine. `Omarchy` is dropped when
    /// Omarchy is not installed, so the counter, `next` and what is rendered
    /// all agree.
    pub fn sequence(include_omarchy_card: bool) -> impl Iterator<Item = Self> {
        Self::ORDER
            .into_iter()
            .filter(move |step| include_omarchy_card || *step != Self::Omarchy)
    }

    pub fn index(self, include_omarchy_card: bool) -> usize {
        Self::sequence(include_omarchy_card)
            .position(|step| step == self)
            .unwrap_or(0)
    }

    pub fn total(include_omarchy_card: bool) -> usize {
        Self::sequence(include_omarchy_card).count()
    }

    pub fn next(self, include_omarchy_card: bool) -> Self {
        Self::sequence(include_omarchy_card)
            .skip_while(|step| *step != self)
            .nth(1)
            .unwrap_or(Self::Finish)
    }

    /// The shell command this card tells the user to run, if any. The card
    /// renders it and `y` copies it, so the two cannot drift apart. The two
    /// protection cards offer theirs only while their integration is still
    /// missing — an install that already has it needs the toggle, not the setup.
    pub fn command(self, setup: IntegrationSetup) -> Option<&'static str> {
        match self {
            Self::Doctor => Some("kvn doctor"),
            Self::Omarchy => (!setup.omarchy_plugin).then_some("kvn setup --omarchy"),
            Self::AutoConnect => {
                (setup.polkit == SetupState::Missing).then_some("sudo kvn setup --polkit")
            }
            Self::KillSwitch => {
                (setup.kill_switch == SetupState::Missing).then_some("sudo kvn setup --killswitch")
            }
            Self::Welcome
            | Self::Region
            | Self::Routing
            | Self::Profiles
            | Self::Connected
            | Self::Finish => None,
        }
    }

    /// The real screen this step hands off to, if any. `Routing` has one only
    /// for a country region: under `Global` there are no modes to choose, so
    /// the card is informational and `Enter` simply moves on.
    pub fn handoff_screen(self, config: &Config) -> Option<HandoffScreen> {
        match self {
            Self::Region => Some(HandoffScreen::GeoRegions),
            Self::Routing => match config.settings.geo_routing.current_region {
                Some(GeoRegion::Global) | None => None,
                Some(_) => Some(HandoffScreen::RoutingMode),
            },
            Self::Profiles => Some(HandoffScreen::MainScreen),
            Self::Welcome
            | Self::Connected
            | Self::Doctor
            | Self::AutoConnect
            | Self::KillSwitch
            | Self::Omarchy
            | Self::Finish => None,
        }
    }
}

/// Small, non-config UX state. Keeping it outside profiles.json avoids making a
/// one-off first-run flow part of the versioned VPN configuration schema.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnboardingState {
    #[serde(default)]
    pub step: OnboardingStep,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
}

impl OnboardingState {
    pub fn is_complete(&self) -> bool {
        self.completed_at.is_some()
    }

    pub fn complete(&mut self, now: DateTime<Utc>) {
        self.completed_at = Some(now);
    }
}

/// Persisted progress plus the session-local bookkeeping of an in-flight tour.
/// Only `state` reaches the disk; `awaiting` and `card_pending` describe a
/// handoff that a restart legitimately forgets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OnboardingProgress {
    pub state: OnboardingState,
    pub awaiting: Option<OnboardingStep>,
    pub card_pending: bool,
    /// Whether the tour includes the `Omarchy` card: an Omarchy desktop that
    /// does not have the plugin yet. Decided once when the model is built rather
    /// than re-read while rendering. It is an environment fact, not progress, and
    /// so is never persisted.
    pub include_omarchy_card: bool,
}

impl OnboardingProgress {
    pub fn new(state: OnboardingState, include_omarchy_card: bool) -> Self {
        let step = match (state.step, include_omarchy_card) {
            // A step recorded on an Omarchy machine, resumed on one without it.
            (OnboardingStep::Omarchy, false) => OnboardingStep::AutoConnect,
            (step, _) => step,
        };
        Self {
            state: OnboardingState { step, ..state },
            awaiting: None,
            card_pending: false,
            include_omarchy_card,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.state.is_complete()
    }

    pub fn step(&self) -> OnboardingStep {
        self.state.step
    }
}

pub fn load_at(path: &Path) -> Result<Option<OnboardingState>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("Failed to read {path:?}")),
    };
    serde_json::from_str(&contents)
        .map(Some)
        .with_context(|| format!("Failed to parse {path:?}"))
}

pub fn save_at(path: &Path, state: &OnboardingState) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("Onboarding path {path:?} has no parent"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create onboarding directory {parent:?}"))?;
    let json = serde_json::to_string_pretty(state)?;
    crate::atomic_write::write(path, json.as_bytes())
}

pub fn load_for_daemon(region_configured: bool, now: DateTime<Utc>) -> OnboardingState {
    let Some(path) = crate::paths::onboarding_path() else {
        tracing::warn!("Failed to determine onboarding state path");
        return initial_state(region_configured, now);
    };
    let stored = match load_at(&path) {
        Ok(stored) => stored,
        Err(error) => {
            tracing::warn!("Resetting invalid onboarding state: {error:#}");
            None
        }
    };
    if let Some(state) = stored {
        return state;
    }
    let state = initial_state(region_configured, now);
    if state.is_complete()
        && let Err(error) = save_at(&path, &state)
    {
        tracing::warn!("Failed to persist onboarding state: {error:#}");
    }
    state
}

// NOTE: an install that already has a geo region predates the tour, so it is
// recorded as complete rather than shown a first-run flow it does not need.
// This is also what arms the support prompt for those users (see
// `support_prompt::load_for_daemon`).
fn initial_state(region_configured: bool, now: DateTime<Utc>) -> OnboardingState {
    let mut state = OnboardingState::default();
    if region_configured {
        state.step = OnboardingStep::Finish;
        state.complete(now);
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 10, 12, 0, 0).unwrap()
    }

    #[test]
    fn steps_walk_the_documented_order_and_saturate() {
        assert_eq!(OnboardingStep::default(), OnboardingStep::Welcome);

        for include_card in [true, false] {
            let expected: Vec<_> = OnboardingStep::sequence(include_card).collect();
            assert_eq!(OnboardingStep::total(include_card), expected.len());
            assert_eq!(OnboardingStep::Welcome.index(include_card), 0);
            assert_eq!(
                OnboardingStep::Finish.index(include_card),
                expected.len() - 1,
                "include_card={include_card}"
            );

            let mut step = OnboardingStep::Welcome;
            let mut visited = vec![step];
            for _ in 1..OnboardingStep::total(include_card) {
                step = step.next(include_card);
                visited.push(step);
            }
            assert_eq!(visited, expected, "include_card={include_card}");
            assert_eq!(
                OnboardingStep::Finish.next(include_card),
                OnboardingStep::Finish
            );
        }
    }

    #[test]
    fn the_omarchy_card_is_shown_only_on_an_omarchy_desktop() {
        assert_eq!(OnboardingStep::total(true), 10);
        assert_eq!(OnboardingStep::total(false), 9);
        assert!(OnboardingStep::sequence(true).any(|s| s == OnboardingStep::Omarchy));
        assert!(!OnboardingStep::sequence(false).any(|s| s == OnboardingStep::Omarchy));

        // Without it the diagnostics card leads straight to auto-connect.
        assert_eq!(
            OnboardingStep::Doctor.next(false),
            OnboardingStep::AutoConnect
        );
        assert_eq!(OnboardingStep::Doctor.next(true), OnboardingStep::Omarchy);
    }

    #[test]
    fn a_step_recorded_on_omarchy_is_normalised_when_resumed_without_it() {
        let state = OnboardingState {
            step: OnboardingStep::Omarchy,
            completed_at: None,
        };
        assert_eq!(
            OnboardingProgress::new(state, false).state.step,
            OnboardingStep::AutoConnect
        );
        assert_eq!(
            OnboardingProgress::new(state, true).state.step,
            OnboardingStep::Omarchy
        );
    }

    #[test]
    fn only_the_cards_that_show_a_command_offer_one_to_copy() {
        // Asserted over ORDER so a new card cannot quietly skip the decision.
        let with_commands = |setup| -> Vec<_> {
            OnboardingStep::ORDER
                .into_iter()
                .filter_map(|step| step.command(setup).map(|command| (step, command)))
                .collect()
        };
        assert_eq!(
            with_commands(IntegrationSetup::default()),
            vec![
                (OnboardingStep::Doctor, "kvn doctor"),
                (OnboardingStep::Omarchy, "kvn setup --omarchy"),
                (OnboardingStep::AutoConnect, "sudo kvn setup --polkit"),
                (OnboardingStep::KillSwitch, "sudo kvn setup --killswitch"),
            ]
        );
    }

    #[test]
    fn the_omarchy_card_drops_its_command_once_the_plugin_is_installed() {
        let setup = IntegrationSetup {
            omarchy_plugin: true,
            ..IntegrationSetup::default()
        };
        assert_eq!(OnboardingStep::Omarchy.command(setup), None);
    }

    #[test]
    fn a_protection_card_drops_its_setup_command_once_the_integration_is_in_place() {
        for state in [SetupState::PendingReboot, SetupState::Ready] {
            let setup = IntegrationSetup {
                polkit: state,
                kill_switch: state,
                ..IntegrationSetup::default()
            };
            assert_eq!(
                OnboardingStep::AutoConnect.command(setup),
                None,
                "{state:?}"
            );
            assert_eq!(OnboardingStep::KillSwitch.command(setup), None, "{state:?}");
            assert_eq!(
                OnboardingStep::Doctor.command(setup),
                Some("kvn doctor"),
                "{state:?}"
            );
        }
    }

    #[test]
    fn only_the_three_acting_steps_hand_off() {
        use HandoffScreen::*;

        let mut config = Config::default();
        config.settings.geo_routing.set_region(GeoRegion::Ru);

        assert_eq!(
            OnboardingStep::Region.handoff_screen(&config),
            Some(GeoRegions)
        );
        assert_eq!(
            OnboardingStep::Routing.handoff_screen(&config),
            Some(RoutingMode)
        );
        assert_eq!(
            OnboardingStep::Profiles.handoff_screen(&config),
            Some(MainScreen)
        );

        for step in [
            OnboardingStep::Welcome,
            OnboardingStep::Connected,
            OnboardingStep::Doctor,
            OnboardingStep::AutoConnect,
            OnboardingStep::KillSwitch,
            OnboardingStep::Omarchy,
            OnboardingStep::Finish,
        ] {
            assert_eq!(step.handoff_screen(&config), None, "{step:?}");
        }
    }

    #[test]
    fn routing_hands_off_only_where_there_are_modes_to_choose() {
        let mut config = Config::default();
        assert_eq!(OnboardingStep::Routing.handoff_screen(&config), None);

        config
            .settings
            .geo_routing
            .set_region(crate::config::profile::GeoRegion::Global);
        assert_eq!(OnboardingStep::Routing.handoff_screen(&config), None);

        for region in [GeoRegion::Ru, GeoRegion::Cn, GeoRegion::Ir] {
            config.settings.geo_routing.set_region(region);
            assert_eq!(
                OnboardingStep::Routing.handoff_screen(&config),
                Some(HandoffScreen::RoutingMode),
                "{region:?}"
            );
        }
    }

    #[test]
    fn completion_records_when_the_tour_ended() {
        let mut state = OnboardingState::default();
        assert!(!state.is_complete());

        state.complete(now());
        assert!(state.is_complete());
        assert_eq!(state.completed_at, Some(now()));
    }

    #[test]
    fn state_roundtrips_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/onboarding.json");
        let state = OnboardingState {
            step: OnboardingStep::Profiles,
            ..OnboardingState::default()
        };
        save_at(&path, &state).unwrap();
        assert_eq!(load_at(&path).unwrap(), Some(state));
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn state_io_reports_unreadable_paths() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_at(dir.path()).is_err());

        let parent_file = dir.path().join("not-a-directory");
        fs::write(&parent_file, "file").unwrap();
        assert!(
            save_at(
                &parent_file.join("onboarding.json"),
                &OnboardingState::default()
            )
            .is_err()
        );
    }

    #[test]
    fn a_fresh_install_starts_the_tour_without_writing_state() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", dir.path());
        let path = crate::paths::onboarding_path().unwrap();

        let state = load_for_daemon(false, now());
        assert_eq!(state, OnboardingState::default());
        assert!(!state.is_complete());
        assert!(!path.exists());
    }

    #[test]
    fn an_existing_install_is_grandfathered_and_persisted() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", dir.path());
        let path = crate::paths::onboarding_path().unwrap();

        let state = load_for_daemon(true, now());
        assert_eq!(state.completed_at, Some(now()));
        assert_eq!(state.step, OnboardingStep::Finish);
        assert_eq!(load_at(&path).unwrap(), Some(state));
    }

    #[test]
    fn corrupt_state_is_replaced_without_re_running_a_finished_tour() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        for (region_configured, expect_complete) in [(true, true), (false, false)] {
            let dir = tempfile::tempdir().unwrap();
            let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", dir.path());
            let path = crate::paths::onboarding_path().unwrap();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "not json").unwrap();

            assert_eq!(
                load_for_daemon(region_configured, now()).is_complete(),
                expect_complete,
                "region_configured={region_configured}"
            );
        }
    }

    #[test]
    fn stored_progress_wins_over_grandfathering() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", dir.path());
        let path = crate::paths::onboarding_path().unwrap();
        let stored = OnboardingState {
            step: OnboardingStep::KillSwitch,
            ..OnboardingState::default()
        };
        save_at(&path, &stored).unwrap();

        assert_eq!(load_for_daemon(true, now()), stored);
    }
}

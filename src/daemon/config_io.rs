use std::sync::mpsc::Sender;
use std::thread;

use anyhow::{Context, Result};

use crate::app::model::{AppStatus, Model, Overlay};
use crate::app::msg::{IpcError, Msg};
use crate::config::profile::Config;
use crate::onboarding::{OnboardingProgress, OnboardingRecovery};
use crate::support_prompt::SupportPromptState;

use super::DaemonShared;
use super::effect::execute_daemon_effect;

pub(super) fn persist_config_unless_frozen(
    model: &Model,
    base: &Config,
    edited: &Config,
) -> anyhow::Result<Config> {
    if model.restart_required {
        return Ok(edited.clone());
    }
    commit_config_change(model, base, edited)
}

pub(super) fn commit_config_change(
    model: &Model,
    base: &Config,
    edited: &Config,
) -> anyhow::Result<Config> {
    anyhow::ensure!(
        !model.restart_required,
        "configuration is frozen until the kvn daemon is restarted"
    );
    anyhow::ensure!(
        !model.config_persistence_blocked,
        "persisted config previously failed to load"
    );
    let path = crate::paths::profiles_path().context("Failed to determine profiles path")?;
    let expected = match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("Failed to read config revision"),
    };
    let current = match expected.as_deref() {
        Some(bytes) => crate::config::load_config_bytes_read_only(bytes, &path)?,
        None => Config::default(),
    };
    let mut merged = crate::config::merge::merge_configs(base, &current, edited)
        .map_err(|conflicts| anyhow::anyhow!("config conflicts at {}", conflicts.join(", ")))?;
    merged.settings.kill_switch = edited.settings.kill_switch;
    crate::config::save_config_at_revision(&path, &merged, expected.as_deref())?;
    Ok(merged)
}

pub(super) fn report_uncommitted_save(model: &mut Model) {
    // The main loop must consume this marker through
    // `commit_config_change` before executing effects. Never fall
    // back to an unconditional write here.
    model.set_status(AppStatus::Error(
        "Configuration save failed: uncommitted SaveConfig effect".into(),
    ));
}

pub(super) fn persist_support_prompt(model: &mut Model, previous: SupportPromptState) {
    let result = crate::paths::support_prompt_path()
        .context("Failed to determine support prompt state path")
        .and_then(|path| crate::support_prompt::save_at(&path, &model.support_prompt));
    if let Err(error) = result {
        model.support_prompt = previous;
        if model.support_prompt.is_due(chrono::Utc::now()) {
            model.overlay = Overlay::Support;
        }
        let message = format!("Support prompt save failed: {error:#}");
        model.set_status(AppStatus::Error(message.clone()));
        crate::services::log_tailer::append_app_log("ERROR", &message);
    }
}

pub(super) fn persist_onboarding(
    model: &mut Model,
    previous: OnboardingProgress,
    recovery: OnboardingRecovery,
) {
    let result = crate::paths::onboarding_path()
        .context("Failed to determine onboarding state path")
        .and_then(|path| crate::onboarding::save_at(&path, &model.onboarding.state));
    match result {
        Ok(()) => arm_support_prompt_after_onboarding(model),
        Err(error) => {
            restore_onboarding_after_failure(model, previous, recovery);
            let message = format!("Onboarding progress save failed: {error:#}");
            model.set_status(AppStatus::Error(message.clone()));
            crate::services::log_tailer::append_app_log("ERROR", &message);
        }
    }
}

/// Revert a failed progress write so the user can repeat the action. Settings
/// the same update already committed are deliberately left alone.
///
/// `awaiting` is cleared rather than restored: the handed-off screen has closed
/// by now and will never report again, so keeping it would strand the tour.
/// The `recovery` says where the card belongs, which the post-transition
/// `model.overlay` can no longer tell us — without it a `finish` that opened the
/// mandatory region picker would leave the user on a screen they cannot leave.
pub(super) fn restore_onboarding_after_failure(
    model: &mut Model,
    previous: OnboardingProgress,
    recovery: OnboardingRecovery,
) {
    model.onboarding.state = previous.state;
    model.onboarding.awaiting = None;
    match recovery {
        OnboardingRecovery::RestoreCard => {
            model.overlay = Overlay::Onboarding(model.onboarding.step());
            model.onboarding.card_pending = false;
        }
        OnboardingRecovery::WaitForIdle => model.onboarding.card_pending = true,
    }
}

/// How this update would have to be undone, when it moved the tour at all. Read
/// before the error path filters the effect list: only a tour transition may
/// revert tour progress, or an unrelated failed save would reopen a finished
/// tour.
pub(super) fn onboarding_transition(
    effects: &[crate::app::effect::Effect],
) -> Option<OnboardingRecovery> {
    effects.iter().find_map(|effect| match effect {
        crate::app::effect::Effect::PersistOnboarding { recovery, .. } => Some(*recovery),
        _ => None,
    })
}

fn arm_support_prompt_after_onboarding(model: &mut Model) {
    let Some(completed_at) = model.onboarding.state.completed_at else {
        return;
    };
    let previous = model.support_prompt.clone();
    if model.support_prompt.schedule_initial(completed_at) {
        persist_support_prompt(model, previous);
    }
}

pub(super) fn save_conflict(model: &mut Model, edited: Box<Config>, conflicts: Vec<String>) {
    match crate::config::save_conflict_config(&edited) {
        Ok(path) => model.set_status(AppStatus::Error(format!(
            "Configuration edit failed: conflicts at {}; edited version saved to {}",
            conflicts.join(", "),
            path.display()
        ))),
        Err(error) => model.set_status(AppStatus::Error(format!(
            "Configuration edit failed: conflicts at {}; edited version save failed: {error:#}",
            conflicts.join(", ")
        ))),
    }
}

pub(super) fn commit_edited(
    tx: &Sender<Msg>,
    model: &mut Model,
    shared: &DaemonShared,
    base: Box<Config>,
    edited: Box<Config>,
) -> Result<()> {
    let mut edited_for_commit = (*edited).clone();
    edited_for_commit.settings.kill_switch = model.config.settings.kill_switch;
    let result = commit_config_change(model, &base, &edited_for_commit);
    match result {
        Ok(config) => {
            for nested in crate::app::update::handle_config_reloaded(model, Ok(config)) {
                execute_daemon_effect(nested, tx, model, shared)?;
            }
        }
        Err(error) => {
            let message = match crate::config::save_conflict_config(&edited) {
                Ok(path) => format!(
                    "Configuration edit failed: {error:#}; edited version saved to {}",
                    path.display()
                ),
                Err(save_error) => format!(
                    "Configuration edit failed: {error:#}; edited version preservation failed: {save_error:#}"
                ),
            };
            model.set_status(AppStatus::Error(message.clone()));
            crate::services::log_tailer::append_app_log("ERROR", &message);
        }
    }
    Ok(())
}

pub(super) fn reload(tx: &Sender<Msg>) {
    let tx = tx.clone();
    thread::spawn(move || {
        let result = crate::config::load_config()
            .and_then(|c| c.validate().map(|_| c))
            .map_err(IpcError::from);
        let _ = tx.send(Msg::ConfigReloaded(Box::new(result)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::effect::Effect;
    use crate::app::model::{Model, Overlay, SettingsMenuPage};
    use crate::app::msg::Msg;
    use crate::app::update::update;
    use crate::config::profile::{Config, GeoRegion};
    use crate::onboarding::{
        OnboardingProgress, OnboardingRecovery, OnboardingState, OnboardingStep,
    };
    use crate::test_helpers::{ENV_LOCK, EnvVarGuard, enter, model_with_profiles};
    use std::path::Path;

    /// A state home whose `kvn` directory is occupied by a regular file, so
    /// every progress write fails without touching the real one.
    fn unwritable_state_home(dir: &Path) -> EnvVarGuard {
        let guard = EnvVarGuard::set("XDG_STATE_HOME", dir);
        std::fs::write(dir.join("kvn"), "not a directory").unwrap();
        guard
    }

    fn apply_onboarding_writes(model: &mut Model, effects: Vec<Effect>) {
        for effect in effects {
            if let Effect::PersistOnboarding { previous, recovery } = effect {
                persist_onboarding(model, previous, recovery);
            }
        }
    }

    fn tour_at(step: OnboardingStep) -> Model {
        let mut model = model_with_profiles(vec![]);
        model.onboarding.state.step = step;
        model.overlay = Overlay::Onboarding(step);
        model
    }

    #[test]
    fn a_failed_card_write_restores_the_card_so_the_step_can_be_repeated() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _state_home = unwritable_state_home(dir.path());

        let mut model = tour_at(OnboardingStep::Doctor);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        let effects = update(&mut model, Msg::Key(enter()));
        assert_eq!(
            model.overlay,
            Overlay::Onboarding(OnboardingStep::AutoConnect)
        );

        apply_onboarding_writes(&mut model, effects);

        assert_eq!(model.onboarding.state.step, OnboardingStep::Doctor);
        assert_eq!(model.overlay, Overlay::Onboarding(OnboardingStep::Doctor));
        assert!(!model.onboarding.card_pending);
        assert_eq!(model.onboarding.awaiting, None);
        assert!(model.status_is_error());

        // With a writable path the very same key press now works.
        let writable = tempfile::tempdir().unwrap();
        let _writable_home = EnvVarGuard::set("XDG_STATE_HOME", writable.path());
        let effects = update(&mut model, Msg::Key(enter()));
        apply_onboarding_writes(&mut model, effects);
        assert_eq!(model.onboarding.state.step, OnboardingStep::AutoConnect);
        assert_eq!(
            crate::onboarding::load_at(&crate::paths::onboarding_path().unwrap())
                .unwrap()
                .map(|state| state.step),
            Some(OnboardingStep::AutoConnect)
        );
    }

    #[test]
    fn a_failed_screen_write_keeps_the_committed_setting_and_reopens_the_card() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _state_home = unwritable_state_home(dir.path());

        let mut model = tour_at(OnboardingStep::Routing);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        update(&mut model, Msg::Key(enter()));
        assert_eq!(model.overlay, Overlay::RoutingMode);

        // Confirm the current mode, which is also the retry path.
        let effects = update(&mut model, Msg::Key(enter()));
        assert!(matches!(
            effects.first(),
            Some(Effect::PersistOnboarding {
                recovery: OnboardingRecovery::WaitForIdle,
                ..
            })
        ));
        let committed = model.config.settings.geo_routing.mode();
        apply_onboarding_writes(&mut model, effects);

        assert_eq!(model.onboarding.state.step, OnboardingStep::Routing);
        assert_eq!(model.onboarding.awaiting, None);
        assert!(model.onboarding.card_pending);
        // The screen owned the transition, so the card waits for the tick
        // rather than being restored in place.
        assert_eq!(model.overlay, Overlay::None);
        assert_eq!(model.config.settings.geo_routing.mode(), committed);
        assert_eq!(
            model.config.settings.geo_routing.current_region,
            Some(GeoRegion::Ru)
        );
    }

    #[test]
    fn a_failed_finish_never_strands_the_user_on_the_region_picker() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _state_home = unwritable_state_home(dir.path());

        // Finishing without a region hands off to the picker, which refuses
        // Esc while none is set. A failed write must not leave the user there.
        let mut model = tour_at(OnboardingStep::Finish);
        model.config.settings.geo_routing.current_region = None;
        let effects = update(&mut model, Msg::Key(enter()));
        assert_eq!(model.overlay, Overlay::GeoRegions);
        apply_onboarding_writes(&mut model, effects);

        assert_eq!(model.overlay, Overlay::Onboarding(OnboardingStep::Finish));
        assert!(!model.onboarding.is_complete());
    }

    #[test]
    fn a_successful_finish_arms_the_support_prompt_from_the_completion_time() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _state_home = EnvVarGuard::set("XDG_STATE_HOME", dir.path());

        let mut model = tour_at(OnboardingStep::Finish);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        let effects = update(&mut model, Msg::Key(enter()));
        let completed_at = model.onboarding.state.completed_at.unwrap();
        apply_onboarding_writes(&mut model, effects);

        assert_eq!(
            model.support_prompt.next_show_at,
            Some(completed_at + chrono::Duration::days(crate::support_prompt::INITIAL_DELAY_DAYS))
        );
        assert_eq!(
            crate::support_prompt::load_at(&crate::paths::support_prompt_path().unwrap()).unwrap(),
            Some(model.support_prompt.clone())
        );
    }

    #[test]
    fn only_a_tour_transition_may_revert_tour_progress() {
        assert_eq!(onboarding_transition(&[Effect::SaveConfig]), None);
        assert_eq!(
            onboarding_transition(&[
                Effect::SaveConfig,
                Effect::PersistOnboarding {
                    previous: OnboardingProgress::default(),
                    recovery: OnboardingRecovery::RestoreCard,
                },
            ]),
            Some(OnboardingRecovery::RestoreCard)
        );
    }

    #[test]
    fn restoring_after_a_failure_repairs_the_screen_the_recovery_names() {
        let mut completed = OnboardingState::default();
        completed.complete(chrono::Utc::now());
        let previous = OnboardingProgress {
            state: OnboardingState {
                step: OnboardingStep::Region,
                ..OnboardingState::default()
            },
            awaiting: Some(OnboardingStep::Region),
            card_pending: false,
            include_omarchy_card: false,
        };

        let mut model = tour_at(OnboardingStep::Routing);
        restore_onboarding_after_failure(&mut model, previous, OnboardingRecovery::RestoreCard);
        assert_eq!(model.overlay, Overlay::Onboarding(OnboardingStep::Region));
        assert!(!model.onboarding.card_pending);
        assert_eq!(model.onboarding.awaiting, None);

        let mut model = model_with_profiles(vec![]);
        model.overlay = Overlay::SettingsMenu(SettingsMenuPage::Root);
        restore_onboarding_after_failure(&mut model, previous, OnboardingRecovery::WaitForIdle);
        assert_eq!(model.overlay, Overlay::SettingsMenu(SettingsMenuPage::Root));
        assert!(model.onboarding.card_pending);
    }

    use super::commit_config_change;

    #[test]
    fn config_commit_merges_external_and_model_changes() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _config_home = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let base = Config::default();
        crate::config::save_config(&base).unwrap();
        let mut external = base.clone();
        external.settings.theme = "nord".into();
        crate::config::save_config(&external).unwrap();
        let mut edited = base.clone();
        edited.settings.auto_connect = true;
        let model = crate::app::model::Model::test_new(edited.clone());

        let merged = commit_config_change(&model, &base, &edited).unwrap();
        assert_eq!(merged.settings.theme, "nord");
        assert!(merged.settings.auto_connect);
        assert_eq!(
            crate::config::load_config_at_read_only(&crate::paths::profiles_path().unwrap())
                .unwrap(),
            merged
        );
    }
}

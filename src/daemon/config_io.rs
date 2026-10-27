use std::sync::mpsc::Sender;
use std::thread;

use anyhow::Context;

use crate::app::effect::Effect;
use crate::app::model::{AppStatus, Model, Overlay, StatusKey};
use crate::app::msg::{ConfigEditResult, IpcError, Msg, RoutingEditResult};
use crate::config::profile::Config;
use crate::onboarding::{OnboardingProgress, OnboardingRecovery};
use crate::support_prompt::SupportPromptState;

use super::DaemonShared;
use super::effect::execute_daemon_effect;

#[derive(Debug)]
struct ConfigConflict {
    current: Box<Config>,
    paths: Vec<String>,
}

impl std::fmt::Display for ConfigConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "config conflicts at {}", self.paths.join(", "))
    }
}

impl std::error::Error for ConfigConflict {}

pub(super) struct UpdateBaseline {
    config: Config,
    support_prompt: SupportPromptState,
    onboarding: OnboardingProgress,
}

impl UpdateBaseline {
    pub(super) fn capture(model: &Model) -> Self {
        Self {
            config: model.config.clone(),
            support_prompt: model.support_prompt.clone(),
            onboarding: model.onboarding,
        }
    }
}

pub(super) fn persist_config_changes(
    model: &mut Model,
    before: UpdateBaseline,
    effects: &mut Vec<Effect>,
) {
    if effects.contains(&Effect::SaveConfig) {
        persist_requested_change(model, before, effects);
    } else if effects.contains(&Effect::PersistConfirmedState) {
        persist_confirmed_state(model, before.config, effects);
    }
}

fn persist_requested_change(model: &mut Model, before: UpdateBaseline, effects: &mut Vec<Effect>) {
    let onboarding_transition = onboarding_transition(effects);
    let edited = model.config.clone();
    match commit_model_config(model, &before.config) {
        Ok(()) => effects.retain(|effect| *effect != Effect::SaveConfig),
        Err(error) => {
            model.replace_config_preserving_selection(before.config);
            model.support_prompt = before.support_prompt;
            if let Some(recovery) = onboarding_transition {
                restore_onboarding_after_failure(model, before.onboarding, recovery);
            }
            let message = match crate::config::save_conflict_config(&edited) {
                Ok(path) => format!(
                    "Configuration save failed: {error:#}; unsaved version preserved at {}",
                    path.display()
                ),
                Err(save_error) => format!(
                    "Configuration save failed: {error:#}; unsaved version preservation failed: {save_error:#}"
                ),
            };
            report_save_failure(model, &message);
            effects.retain(|effect| {
                matches!(effect, Effect::BroadcastState | Effect::AppendAppLog { .. })
            });
            ensure_broadcast(effects);
        }
    }
}

fn persist_confirmed_state(model: &mut Model, before: Config, effects: &mut Vec<Effect>) {
    if let Err(error) = commit_model_config(model, &before) {
        model.unsaved_state_merge_base.get_or_insert(before);
        report_save_failure(model, &format!("Configuration save failed: {error:#}"));
        ensure_broadcast(effects);
    }
    effects.retain(|effect| *effect != Effect::PersistConfirmedState);
}

fn commit_model_config(model: &mut Model, before: &Config) -> anyhow::Result<()> {
    let base = model.unsaved_state_merge_base.as_ref().unwrap_or(before);
    let config = commit_config_change(model, base, &model.config)?;
    model.replace_config_preserving_selection(config);
    model.unsaved_state_merge_base = None;
    Ok(())
}

fn report_save_failure(model: &mut Model, message: &str) {
    model.set_status(StatusKey::Config, AppStatus::Error(message.to_string()));
    crate::services::log_tailer::append_app_log("ERROR", message);
}

fn ensure_broadcast(effects: &mut Vec<Effect>) {
    if !effects.contains(&Effect::BroadcastState) {
        effects.push(Effect::BroadcastState);
    }
}

pub(super) fn commit_config_change(
    model: &Model,
    base: &Config,
    edited: &Config,
) -> anyhow::Result<Config> {
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
    let mut merged = crate::config::merge::merge_configs(base, &current, edited).map_err(
        |error| match error {
            crate::config::merge::MergeError::Conflicts(paths) => {
                anyhow::Error::new(ConfigConflict {
                    current: Box::new(current.clone()),
                    paths,
                })
            }
            error => anyhow::Error::new(error),
        },
    )?;
    merged.settings.kill_switch = edited.settings.kill_switch;
    merged.normalize();
    crate::config::save_config_at_revision(&path, &merged, expected.as_deref())?;
    Ok(merged)
}

pub(super) fn report_uncommitted_save(model: &mut Model) {
    model.set_status(
        StatusKey::Config,
        AppStatus::Error("Configuration save failed: uncommitted SaveConfig effect".into()),
    );
}

pub(super) fn persist_support_prompt(
    model: &mut Model,
    previous: SupportPromptState,
    reopen_prompt: bool,
) {
    let result = crate::paths::support_prompt_path()
        .context("Failed to determine support prompt state path")
        .and_then(|path| crate::support_prompt::save_at(&path, &model.support_prompt));
    if let Err(error) = result {
        model.support_prompt = previous;
        if reopen_prompt && model.support_prompt.is_due(chrono::Utc::now()) {
            model.overlay = Overlay::Support;
        }
        let message = format!("Support prompt save failed: {error:#}");
        model.set_status(StatusKey::Config, AppStatus::Error(message.clone()));
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
            model.set_status(StatusKey::Config, AppStatus::Error(message.clone()));
            crate::services::log_tailer::append_app_log("ERROR", &message);
        }
    }
}

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

fn onboarding_transition(effects: &[Effect]) -> Option<OnboardingRecovery> {
    effects.iter().find_map(|effect| match effect {
        Effect::PersistOnboarding { recovery, .. } => Some(*recovery),
        _ => None,
    })
}

fn arm_support_prompt_after_onboarding(model: &mut Model) {
    let Some(completed_at) = model.onboarding.state.completed_at else {
        return;
    };
    let previous = model.support_prompt.clone();
    if model.support_prompt.schedule_initial(completed_at) {
        persist_support_prompt(model, previous, false);
    }
}

pub(super) fn save_conflict(model: &mut Model, edited: Box<Config>, conflicts: Vec<String>) {
    match crate::config::save_conflict_config(&edited) {
        Ok(path) => model.set_status(
            StatusKey::Config,
            AppStatus::Error(format!(
                "Configuration edit failed: conflicts at {}; edited version saved to {}",
                conflicts.join(", "),
                path.display()
            )),
        ),
        Err(error) => model.set_status(
            StatusKey::Config,
            AppStatus::Error(format!(
                "Configuration edit failed: conflicts at {}; edited version save failed: {error:#}",
                conflicts.join(", ")
            )),
        ),
    }
}

pub(super) fn commit_edited(
    tx: &Sender<Msg>,
    model: &mut Model,
    shared: &DaemonShared,
    base: Box<Config>,
    edited: Box<Config>,
    reply_requested: bool,
) -> ConfigEditResult {
    let mut edited_for_commit = (*edited).clone();
    edited_for_commit.settings.kill_switch = model.config.settings.kill_switch;
    let result = commit_config_change(model, &base, &edited_for_commit);
    match result {
        Ok(config) => {
            for nested in crate::app::update::handle_config_reloaded(model, Ok(config)) {
                execute_daemon_effect(nested, tx, model, shared, false);
            }
            ConfigEditResult::Saved
        }
        Err(error) => {
            if reply_requested {
                return match error.downcast::<ConfigConflict>() {
                    Ok(conflict) => ConfigEditResult::Conflict {
                        current: conflict.current,
                        paths: conflict.paths,
                    },
                    Err(error) => ConfigEditResult::Failed {
                        message: format!("{error:#}"),
                    },
                };
            }
            let message = match crate::config::save_conflict_config(&edited) {
                Ok(path) => format!(
                    "Configuration edit failed: {error:#}; edited version saved to {}",
                    path.display()
                ),
                Err(save_error) => format!(
                    "Configuration edit failed: {error:#}; edited version preservation failed: {save_error:#}"
                ),
            };
            model.set_status(StatusKey::Config, AppStatus::Error(message.clone()));
            crate::services::log_tailer::append_app_log("ERROR", &message);
            ConfigEditResult::Failed { message }
        }
    }
}

pub(super) fn commit_edited_routing(
    tx: &Sender<Msg>,
    model: &mut Model,
    shared: &DaemonShared,
    base: crate::config::routing::RoutingEditBase,
    edited: Box<crate::config::routing::RoutingConfig>,
) -> RoutingEditResult {
    let commit = crate::paths::routing_path()
        .context("Failed to determine routing config path")
        .and_then(|path| crate::config::routing::commit_edit_at(&path, &base, &edited));
    match commit {
        Ok(crate::config::routing::RoutingCommit::Saved(routing)) => {
            for nested in crate::app::update::handle_routing_edit_saved(model, routing) {
                execute_daemon_effect(nested, tx, model, shared, false);
            }
            RoutingEditResult::Saved
        }
        Ok(crate::config::routing::RoutingCommit::Conflict { current, paths }) => {
            RoutingEditResult::Conflict { current, paths }
        }
        Err(error) => RoutingEditResult::Failed {
            message: format!("{error:#}"),
        },
    }
}

pub(super) fn add_region_modes(tx: &Sender<Msg>, region: crate::config::profile::GeoRegion) {
    let tx = tx.clone();
    thread::spawn(move || {
        let result = crate::paths::routing_path()
            .context("Failed to determine routing config path")
            .and_then(|path| {
                crate::config::routing::update_routing_at(&path, |routing| {
                    routing.add_region_modes(region)
                })
            })
            .map_err(IpcError::from);
        let _ = tx.send(Msg::RegionModesSaved {
            region,
            result: Box::new(result),
        });
    });
}

pub(super) fn reload(tx: &Sender<Msg>) {
    let tx = tx.clone();
    thread::spawn(move || {
        let mut result = crate::config::load_config().and_then(|c| c.validate().map(|_| c));
        let routing = match &result {
            Ok(config) => crate::config::load_routing(&config.settings.geo_routing, true),
            Err(_) => crate::config::load_routing(&Default::default(), false),
        }
        .map_err(IpcError::from);
        if let Ok(config) = &mut result {
            config.settings.geo_routing.forget_legacy_regions();
        }
        let _ = tx.send(Msg::ConfigReloaded(Box::new(
            result.map_err(IpcError::from),
        )));
        let _ = tx.send(Msg::RoutingReloaded(Box::new(routing)));
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
        model.routing.add_region_modes(GeoRegion::Ru);
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
        model.routing.add_region_modes(GeoRegion::Ru);
        update(&mut model, Msg::Key(enter()));
        assert_eq!(model.overlay, Overlay::RoutingMode);

        let effects = update(&mut model, Msg::Key(enter()));
        assert!(matches!(
            effects.first(),
            Some(Effect::PersistOnboarding {
                recovery: OnboardingRecovery::WaitForIdle,
                ..
            })
        ));
        let committed = model
            .config
            .settings
            .geo_routing
            .mode(&model.routing)
            .to_string();
        apply_onboarding_writes(&mut model, effects);

        assert_eq!(model.onboarding.state.step, OnboardingStep::Routing);
        assert_eq!(model.onboarding.awaiting, None);
        assert!(model.onboarding.card_pending);
        assert_eq!(model.overlay, Overlay::None);
        assert_eq!(
            model.config.settings.geo_routing.mode(&model.routing),
            committed
        );
    }

    #[test]
    fn a_successful_finish_arms_the_support_prompt_from_the_completion_time() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _state_home = EnvVarGuard::set("XDG_STATE_HOME", dir.path());

        let mut model = tour_at(OnboardingStep::Finish);
        model.routing.add_region_modes(GeoRegion::Ru);
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

    #[test]
    fn config_commit_hands_back_profiles_in_source_order() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _config_home = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let base = Config::default();
        crate::config::save_config(&base).unwrap();
        let subscription = crate::test_helpers::subscription_with_hwid(false, None);
        let profile = |name: &str| {
            crate::config::profile::Profile::new_vless(
                name.into(),
                "h".into(),
                1,
                uuid::Uuid::new_v4().to_string(),
            )
        };
        let mut owned = profile("Owned");
        owned.subscription_id = Some(subscription.id);
        let standalone = profile("Standalone");
        let mut edited = base.clone();
        edited.subscriptions.push(subscription);
        edited.profiles = vec![owned, standalone];
        let model = crate::app::model::Model::test_new(edited.clone());

        let merged = commit_config_change(&model, &base, &edited).unwrap();

        let names: Vec<&str> = merged.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Standalone", "Owned"]);
    }

    fn update_with_unwritable_config(model: &mut Model, msg: Msg) -> Vec<Effect> {
        model.config_persistence_blocked = true;
        let before = UpdateBaseline::capture(model);
        let mut effects = update(model, msg);
        persist_config_changes(model, before, &mut effects);
        effects
    }

    #[test]
    fn a_failed_save_keeps_the_kill_switch_state_the_helper_applied() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _config_home = EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());

        for enabled in [true, false] {
            let mut model = model_with_profiles(vec![]);
            model.config.settings.kill_switch = !enabled;
            model.kill_switch_pending = Some(enabled);

            let effects = update_with_unwritable_config(
                &mut model,
                Msg::KillSwitchApplied {
                    enabled,
                    error: None,
                },
            );

            assert_eq!(model.config.settings.kill_switch, enabled);
            assert!(model.status_text().contains("Configuration save failed"));
            assert!(effects.contains(&Effect::BroadcastState));
            assert!(!effects.contains(&Effect::PersistConfirmedState));
        }
    }

    #[test]
    fn a_failed_save_after_connecting_still_writes_the_connection_state() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _config_home = EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let mut model = model_with_profiles(vec![]);
        let profile_id = uuid::Uuid::new_v4();

        let effects = update_with_unwritable_config(
            &mut model,
            Msg::Connected {
                pid: 42,
                profile_id,
                attempt_id: 0,
                routing_fallback: None,
                skipped_rule_sets: Vec::new(),
            },
        );

        assert!(effects.contains(&Effect::WriteState));
        assert_eq!(
            model.config.settings.last_connected_profile,
            Some(profile_id)
        );
        assert!(model.status_text().contains("Configuration save failed"));
    }

    #[test]
    fn the_next_save_keeps_confirmed_state_an_earlier_save_lost() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _config_home = EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let mut model = model_with_profiles(crate::test_helpers::sample_profiles());
        crate::config::save_config(&model.config).unwrap();
        let profile_id = model.config.profiles[0].id;
        update_with_unwritable_config(
            &mut model,
            Msg::Connected {
                pid: 42,
                profile_id,
                attempt_id: 0,
                routing_fallback: None,
                skipped_rule_sets: Vec::new(),
            },
        );

        model.config_persistence_blocked = false;
        let before = UpdateBaseline::capture(&model);
        model.config.settings.theme = "nord".into();
        let mut effects = vec![Effect::SaveConfig];
        persist_config_changes(&mut model, before, &mut effects);

        let saved =
            crate::config::load_config_at_read_only(&crate::paths::profiles_path().unwrap())
                .unwrap();
        assert_eq!(saved.settings.theme, "nord");
        assert_eq!(saved.settings.last_connected_profile, Some(profile_id));
        assert_eq!(model.unsaved_state_merge_base, None);
    }

    #[test]
    fn confirmed_state_is_saved_without_reaching_the_effect_executor() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _config_home = EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let mut model = model_with_profiles(vec![]);
        model.kill_switch_pending = Some(true);

        let before = UpdateBaseline::capture(&model);
        let mut effects = update(
            &mut model,
            Msg::KillSwitchApplied {
                enabled: true,
                error: None,
            },
        );
        persist_config_changes(&mut model, before, &mut effects);

        assert!(!effects.contains(&Effect::PersistConfirmedState));
        assert!(
            crate::config::load_config_at_read_only(&crate::paths::profiles_path().unwrap())
                .unwrap()
                .settings
                .kill_switch
        );
    }
}

use std::sync::mpsc::Sender;
use std::thread;

use anyhow::{Context, Result};

use crate::app::model::{AppStatus, Model, Overlay};
use crate::app::msg::{IpcError, Msg};
use crate::config::profile::Config;
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
        "Internal error: uncommitted SaveConfig effect".into(),
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
        let message = format!("Failed to save support prompt state: {error:#}");
        model.set_status(AppStatus::Error(message.clone()));
        crate::services::log_tailer::append_app_log("ERROR", &message);
    }
}

pub(super) fn save_conflict(model: &mut Model, edited: Box<Config>, conflicts: Vec<String>) {
    match crate::config::save_conflict_config(&edited) {
        Ok(path) => model.set_status(AppStatus::Error(format!(
            "Edit conflicts at {}; edited version saved to {}",
            conflicts.join(", "),
            path.display()
        ))),
        Err(error) => model.set_status(AppStatus::Error(format!(
            "Edit conflicts at {}; failed to save edited version: {error:#}",
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
                    "Failed to apply edited config: {error:#}; edited version saved to {}",
                    path.display()
                ),
                Err(save_error) => format!(
                    "Failed to apply edited config: {error:#}; failed to preserve edited version: {save_error:#}"
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
    use super::commit_config_change;
    use crate::config::profile::Config;

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

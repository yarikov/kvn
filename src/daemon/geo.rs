use std::sync::mpsc::Sender;
use std::thread;

use crate::app::model::{AppStatus, Model, StatusKey};
use crate::app::msg::Msg;
use crate::config::profile::GeoAutoUpdate;
use crate::geo::{RuleSet, RuleSetOutcome, RuleSetReport, ScheduleWrite};

pub(super) fn download_rule_sets(
    tx: &Sender<Msg>,
    model: &Model,
    rule_sets: Vec<RuleSet>,
    only_missing: bool,
    record_schedule: bool,
) {
    let tx = tx.clone();
    let epoch = model.geo_schedule_epoch;
    let schedule = record_schedule.then(|| ScheduleWrite {
        interval_days: interval_days(model),
        epoch,
    });
    thread::spawn(move || {
        let msg = match crate::geo::GeoManager::new() {
            Ok(gm) => Msg::RuleSetsUpdated {
                reports: gm.update_rule_sets(&rule_sets, only_missing, schedule),
                states: gm.states(&rule_sets),
                epoch,
            },
            Err(e) => Msg::RuleSetsUpdated {
                reports: rule_sets
                    .iter()
                    .map(|rule_set| RuleSetReport {
                        key: rule_set.key(),
                        tag: rule_set.tag.to_string(),
                        outcome: RuleSetOutcome::Failed(format!("{e:#}")),
                    })
                    .collect(),
                states: Default::default(),
                epoch,
            },
        };
        let _ = tx.send(msg);
    });
}

pub(super) fn refresh_rule_set_states(tx: &Sender<Msg>, model: &Model) {
    let tx = tx.clone();
    let selectable = model.selectable_rule_sets();
    let epoch = model.geo_schedule_epoch;
    thread::spawn(move || {
        let states = crate::geo::GeoManager::new()
            .map(|g| g.states(&selectable))
            .unwrap_or_default();
        let _ = tx.send(Msg::RuleSetStatesRefreshed { states, epoch });
    });
}

pub(super) fn reset_update_schedules(model: &mut Model) {
    let Ok(manager) = crate::geo::GeoManager::new() else {
        return;
    };
    let selectable = model.selectable_rule_sets();
    let enabled = model.config.settings.geo_routing.auto_update != GeoAutoUpdate::Off;
    match manager.reset_update_schedules(&selectable, enabled) {
        Ok(schedule_epoch) => {
            model.geo_schedule_epoch = schedule_epoch;
            model.apply_rule_set_states(manager.states(&selectable), schedule_epoch);
        }
        Err(error) => {
            let message = format!("Rule-set update schedule reset failed: {error:#}");
            model.set_status(StatusKey::Geo, AppStatus::Error(message.clone()));
            crate::services::log_tailer::append_app_log("ERROR", &message);
        }
    }
}

fn interval_days(model: &Model) -> i64 {
    (model
        .config
        .settings
        .geo_routing
        .auto_update
        .interval_minutes()
        / 1_440) as i64
}

#[cfg(test)]
mod tests {
    use crate::config::profile::{GeoAutoUpdate, GeoRegion, RetryState};

    #[test]
    fn schedule_reset_clears_the_model_retry_state() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let mut model = crate::test_helpers::model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Off;
        for rule_set in model.selectable_rule_sets() {
            model
                .rule_set_states
                .entry(rule_set.key())
                .or_default()
                .retry = Some(RetryState {
                consecutive_failures: 2,
                retry_at: chrono::Local::now(),
                attempt_date: None,
            });
        }

        super::reset_update_schedules(&mut model);

        assert!(
            model
                .rule_set_states
                .values()
                .all(|state| state.retry.is_none())
        );
    }

    #[test]
    fn schedule_reset_advances_the_model_schedule_epoch() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let mut model = crate::test_helpers::model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        let before = model.geo_schedule_epoch;

        super::reset_update_schedules(&mut model);

        assert_ne!(model.geo_schedule_epoch, before);
        assert_eq!(
            model.geo_schedule_epoch,
            crate::geo::GeoManager::new().unwrap().schedule_epoch()
        );
    }

    #[test]
    fn schedule_reset_write_failure_is_reported_as_status() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let geo_dir = crate::paths::geo_dir();
        std::fs::create_dir_all(&geo_dir).unwrap();
        std::fs::set_permissions(&geo_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let mut model = crate::test_helpers::model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);

        super::reset_update_schedules(&mut model);

        std::fs::set_permissions(&geo_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            model
                .status_text()
                .starts_with("Rule-set update schedule reset failed"),
            "{}",
            model.status_text()
        );
        assert!(model.status_is_error());
    }
}

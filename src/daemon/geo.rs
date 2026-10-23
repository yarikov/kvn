use std::sync::mpsc::Sender;
use std::thread;

use crate::app::model::{AppStatus, Model, StatusKey};
use crate::app::msg::{GeoResult, Msg};
use crate::config::profile::{GeoAutoUpdate, GeoRegion};
use crate::geo::RegionUpdate;

pub(super) fn download(tx: &Sender<Msg>, model: &mut Model) {
    model.geo_updating = true;
    let tx = tx.clone();
    let region = current_region(model);
    let automatic = model.geo_automatic_update;
    let interval_days = interval_days(model);
    let schedule_epoch = model.geo_schedule_epoch;
    thread::spawn(move || {
        let gm = match crate::geo::GeoManager::new() {
            Ok(gm) => gm,
            Err(e) => {
                let _ = tx.send(Msg::GeoUpdated {
                    result: GeoResult::Error {
                        message: e.to_string(),
                        schedule: None,
                    },
                    region_ready: false,
                });
                return;
            }
        };
        let regional = gm.update_if_needed(region);
        let result = finalize_geo_result(
            &gm,
            region,
            regional,
            ScheduleWrite {
                retry_enabled: automatic,
                interval_days,
                epoch: schedule_epoch,
            },
        );
        let _ = tx.send(Msg::GeoUpdated {
            result,
            region_ready: gm.has_databases(region),
        });
    });
}

pub(super) fn refresh_last_updated(tx: &Sender<Msg>, model: &Model) {
    let tx = tx.clone();
    let region = current_region(model);
    let schedule_epoch = model.geo_schedule_epoch;
    thread::spawn(move || {
        let manager = crate::geo::GeoManager::new().ok();
        let _ = tx.send(Msg::GeoMetadataRefreshed {
            last_updated: manager.as_ref().and_then(|g| g.last_updated(region)),
            last_checked_at: manager.as_ref().and_then(|g| g.last_checked_at(region)),
            schedule: manager.map(|g| g.schedule(region, schedule_epoch)),
        });
    });
}

pub(super) fn clear_retry_state(region: GeoRegion) {
    if let Ok(manager) = crate::geo::GeoManager::new()
        && let Err(e) = manager.clear_retry_state(region)
    {
        tracing::warn!("Failed to clear geo retry state: {e}");
    }
}

pub(super) fn reset_update_schedules(model: &mut Model) {
    let Ok(manager) = crate::geo::GeoManager::new() else {
        return;
    };
    let region = current_region(model);
    let enabled = model.config.settings.geo_routing.auto_update != GeoAutoUpdate::Off;
    match manager.reset_update_schedules(region, enabled) {
        Ok(schedule_epoch) => {
            model.geo_schedule_epoch = schedule_epoch;
            model.apply_geo_schedule(manager.schedule(region, schedule_epoch));
        }
        Err(error) => {
            let message = format!("Rule-set update schedule reset failed: {error:#}");
            model.set_status(StatusKey::Geo, AppStatus::Error(message.clone()));
            crate::services::log_tailer::append_app_log("ERROR", &message);
        }
    }
}

pub(super) fn download_if_missing(tx: &Sender<Msg>, model: &mut Model) {
    model.geo_updating = true;
    let tx = tx.clone();
    let region = current_region(model);
    let retry_enabled = model.config.settings.geo_routing.auto_update != GeoAutoUpdate::Off;
    let interval_days = interval_days(model);
    let schedule_epoch = model.geo_schedule_epoch;
    thread::spawn(move || {
        let (result, region_ready) = match crate::geo::GeoManager::new() {
            Ok(gm) => {
                let result = if gm.has_databases(region) {
                    let _ = gm.clear_retry_state(region);
                    GeoResult::UpToDate {
                        checked_at: gm.last_checked_at(region),
                        schedule: Some(gm.schedule(region, schedule_epoch)),
                    }
                } else {
                    finalize_geo_result(
                        &gm,
                        region,
                        gm.update_if_needed(region),
                        ScheduleWrite {
                            retry_enabled,
                            interval_days,
                            epoch: schedule_epoch,
                        },
                    )
                };
                (result, gm.has_databases(region))
            }
            Err(e) => (
                GeoResult::Error {
                    message: e.to_string(),
                    schedule: None,
                },
                false,
            ),
        };
        let _ = tx.send(Msg::GeoUpdated {
            result,
            region_ready,
        });
    });
}

fn current_region(model: &Model) -> GeoRegion {
    model
        .config
        .settings
        .geo_routing
        .current_region
        .unwrap_or(GeoRegion::Global)
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

struct ScheduleWrite {
    retry_enabled: bool,
    interval_days: i64,
    epoch: u64,
}

fn finalize_geo_result(
    manager: &crate::geo::GeoManager,
    region: GeoRegion,
    regional: anyhow::Result<RegionUpdate>,
    write: ScheduleWrite,
) -> GeoResult {
    if write.retry_enabled {
        if regional.is_err() {
            let _ = manager.record_update_failure(region, write.epoch);
        } else if let Err(e) =
            manager.record_region_schedule_success(region, write.interval_days, write.epoch)
        {
            tracing::warn!("Failed to schedule geo update: {e}");
        }
    }
    let schedule = Some(manager.schedule(region, write.epoch));

    match regional {
        Ok(RegionUpdate::Updated { parts, checked_at }) => GeoResult::Updated {
            parts,
            checked_at,
            schedule,
        },
        Ok(RegionUpdate::UpToDate { checked_at }) => GeoResult::UpToDate {
            checked_at,
            schedule,
        },
        Err(e) => GeoResult::Error {
            message: e.to_string(),
            schedule,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{ScheduleWrite, finalize_geo_result};
    use crate::app::msg::GeoResult;
    use crate::config::profile::{GeoAutoUpdate, GeoRegion};

    fn automatic_schedule(epoch: u64) -> ScheduleWrite {
        ScheduleWrite {
            retry_enabled: true,
            interval_days: 1,
            epoch,
        }
    }

    #[test]
    fn geo_batch_started_before_a_reset_reports_the_reset_schedule() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let manager = crate::geo::GeoManager::new().unwrap();
        let stale = manager.schedule_epoch();
        manager.reset_update_schedules(GeoRegion::Ru, true).unwrap();
        let reset_next = manager.region_next_update(GeoRegion::Ru);

        let result = finalize_geo_result(
            &manager,
            GeoRegion::Ru,
            Err(anyhow::anyhow!("regional unavailable")),
            automatic_schedule(stale),
        );

        let GeoResult::Error {
            schedule: Some(schedule),
            ..
        } = result
        else {
            panic!("expected an error result");
        };
        assert!(schedule.retry_state.is_none());
        assert_eq!(schedule.next_update, reset_next);
        assert!(manager.retry_state(GeoRegion::Ru).is_none());
    }

    #[test]
    fn schedule_reset_clears_the_model_retry_state() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let manager = crate::geo::GeoManager::new().unwrap();
        let mut model = crate::test_helpers::model_with_profiles(vec![]);
        model.config.settings.geo_routing.set_region(GeoRegion::Ru);
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Off;
        let epoch = manager.schedule_epoch();
        model.geo_retry_state = manager.record_update_failure(GeoRegion::Ru, epoch).unwrap();

        super::reset_update_schedules(&mut model);

        assert!(model.geo_retry_state.is_none());
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

    #[test]
    fn geo_batch_schedules_a_retry_when_the_region_fails() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let manager = crate::geo::GeoManager::new().unwrap();

        let result = finalize_geo_result(
            &manager,
            GeoRegion::Ru,
            Err(anyhow::anyhow!("regional unavailable")),
            automatic_schedule(0),
        );

        let GeoResult::Error {
            schedule: Some(schedule),
            ..
        } = result
        else {
            panic!("expected an error result");
        };
        assert_eq!(schedule.retry_state.unwrap().consecutive_failures, 1);
    }
}

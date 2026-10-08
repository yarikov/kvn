use std::sync::mpsc::Sender;
use std::thread;

use crate::app::model::{AppStatus, Model};
use crate::app::msg::{GeoResult, Msg};
use crate::config::profile::{GeoAutoUpdate, GeoRegion, RoutedService};
use crate::geo::RegionUpdate;

pub(super) fn download(tx: &Sender<Msg>, model: &mut Model) {
    model.geo_updating = true;
    let tx = tx.clone();
    let region = current_region(model);
    let services = model.config.settings.geo_routing.enabled_services();
    let automatic = model.geo_automatic_update;
    let interval_days = interval_days(model);
    let existing_service_checked_at = model.service_checked_at.clone();
    let schedule_epoch = model.geo_schedule_epoch;
    thread::spawn(move || {
        let gm = match crate::geo::GeoManager::new() {
            Ok(gm) => gm,
            Err(e) => {
                let _ = tx.send(Msg::GeoUpdated(GeoResult::Error {
                    message: e.to_string(),
                    service_checked_at: existing_service_checked_at,
                    schedule: None,
                    updated_parts: Vec::new(),
                }));
                return;
            }
        };
        let regional = gm.update_if_needed(region);
        let services = refresh_service_rule_sets(
            &gm,
            &services,
            automatic.then_some(interval_days),
            schedule_epoch,
        );
        let result = finalize_geo_result(
            &gm,
            region,
            regional,
            services,
            ScheduleWrite {
                retry_enabled: automatic,
                update_region: true,
                interval_days,
                epoch: schedule_epoch,
            },
        );
        let _ = tx.send(Msg::GeoUpdated(result));
    });
}

pub(super) fn download_service_rule_sets_if_missing(tx: &Sender<Msg>, model: &mut Model) {
    // May run directly when the kill switch is off, or through a
    // tunnel after connect. Always report partial failures so the
    // reducer can log them and run any pending reconnect.
    model.geo_updating = true;
    let services = model.config.settings.geo_routing.enabled_services();
    let schedule_enabled = model.config.settings.geo_routing.auto_update != GeoAutoUpdate::Off;
    let schedule_epoch = model.geo_schedule_epoch;
    let tx = tx.clone();
    thread::spawn(move || {
        let msg = match crate::geo::GeoManager::new() {
            Ok(gm) => {
                let _ = gm.ensure_update_schedules(
                    GeoRegion::Global,
                    &services,
                    schedule_enabled,
                    schedule_epoch,
                );
                let missing: Vec<_> = services
                    .into_iter()
                    .filter(|s| !gm.has_service_databases(*s))
                    .collect();
                let refreshed = refresh_service_rule_sets(&gm, &missing, None, schedule_epoch);
                Msg::ServiceRuleSetsReady {
                    checked_at: gm.service_checked_at(),
                    schedule: Some(gm.service_schedule(schedule_epoch)),
                    updated_parts: refreshed.updated_parts,
                    errors: refreshed.errors,
                }
            }
            Err(e) => {
                tracing::warn!("Failed to init geo manager for service rule-sets: {e:#}");
                Msg::ServiceRuleSetsReady {
                    checked_at: Default::default(),
                    schedule: None,
                    updated_parts: Vec::new(),
                    errors: vec![e.to_string()],
                }
            }
        };
        let _ = tx.send(msg);
    });
}

pub(super) fn retry_service_rule_sets(
    tx: &Sender<Msg>,
    model: &mut Model,
    services: Vec<RoutedService>,
) {
    model.geo_updating = true;
    let tx = tx.clone();
    let region = current_region(model);
    let existing_service_checked_at = model.service_checked_at.clone();
    let automatic = model.geo_automatic_update;
    let interval_days = interval_days(model);
    let schedule_epoch = model.geo_schedule_epoch;
    thread::spawn(move || {
        let result = match crate::geo::GeoManager::new() {
            Ok(gm) => {
                let checked_at = gm.last_checked_at(region);
                let services = refresh_service_rule_sets(
                    &gm,
                    &services,
                    automatic.then_some(interval_days),
                    schedule_epoch,
                );
                finalize_geo_result(
                    &gm,
                    region,
                    Ok(RegionUpdate::UpToDate { checked_at }),
                    services,
                    ScheduleWrite {
                        retry_enabled: false,
                        update_region: false,
                        interval_days,
                        epoch: schedule_epoch,
                    },
                )
            }
            Err(e) => GeoResult::Error {
                message: e.to_string(),
                service_checked_at: existing_service_checked_at,
                schedule: None,
                updated_parts: Vec::new(),
            },
        };
        let _ = tx.send(Msg::GeoUpdated(result));
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
            service_checked_at: manager
                .as_ref()
                .map(|g| g.service_checked_at())
                .unwrap_or_default(),
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
    let services = model.config.settings.geo_routing.enabled_services();
    let enabled = model.config.settings.geo_routing.auto_update != GeoAutoUpdate::Off;
    match manager.reset_update_schedules(region, &services, enabled) {
        Ok(schedule_epoch) => {
            model.geo_schedule_epoch = schedule_epoch;
            model.apply_geo_schedule(manager.schedule(region, schedule_epoch));
        }
        Err(error) => {
            let message = format!("Rule-set update schedule reset failed: {error:#}");
            model.set_status(AppStatus::Error(message.clone()));
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
        let result = match crate::geo::GeoManager::new() {
            Ok(gm) => {
                if gm.has_databases(region) {
                    let _ = gm.clear_retry_state(region);
                    GeoResult::UpToDate {
                        checked_at: gm.last_checked_at(region),
                        service_checked_at: gm.service_checked_at(),
                        schedule: Some(gm.schedule(region, schedule_epoch)),
                        warnings: Vec::new(),
                    }
                } else {
                    finalize_geo_result(
                        &gm,
                        region,
                        gm.update_if_needed(region),
                        ServiceRefreshResult::default(),
                        ScheduleWrite {
                            retry_enabled,
                            update_region: true,
                            interval_days,
                            epoch: schedule_epoch,
                        },
                    )
                }
            }
            Err(e) => GeoResult::Error {
                message: e.to_string(),
                service_checked_at: Default::default(),
                schedule: None,
                updated_parts: Vec::new(),
            },
        };
        let _ = tx.send(Msg::GeoUpdated(result));
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

/// Best-effort check/download of service rule-sets, shared by the periodic
/// geo-update thread and the post-connect fetch. Failures are logged, never
/// surfaced — the route builder just omits a service's rules until its files
/// appear.
fn refresh_service_rule_sets(
    gm: &crate::geo::GeoManager,
    services: &[RoutedService],
    automatic_interval_days: Option<i64>,
    schedule_epoch: u64,
) -> ServiceRefreshResult {
    let mut result = ServiceRefreshResult::default();
    for service in services {
        match gm.update_service_if_needed(*service) {
            Ok(updated) => {
                if let Some(days) = automatic_interval_days
                    && let Err(e) =
                        gm.record_service_schedule_success(*service, days, schedule_epoch)
                {
                    tracing::warn!("Failed to schedule {} update: {e}", service.label());
                }
                if updated {
                    result
                        .updated_parts
                        .push(format!("service-{}", service.label().to_lowercase()));
                }
            }
            Err(e) => {
                let message = format!("{} rule-sets: {e:#}", service.label());
                tracing::warn!("Failed to update {message}");
                if automatic_interval_days.is_some() {
                    let _ = gm.record_service_failure(*service, schedule_epoch);
                }
                result.errors.push(message);
            }
        }
    }
    result
}

struct ScheduleWrite {
    retry_enabled: bool,
    update_region: bool,
    interval_days: i64,
    epoch: u64,
}

#[derive(Default)]
struct ServiceRefreshResult {
    updated_parts: Vec<String>,
    errors: Vec<String>,
}

fn finalize_geo_result(
    manager: &crate::geo::GeoManager,
    region: GeoRegion,
    regional: anyhow::Result<RegionUpdate>,
    services: ServiceRefreshResult,
    write: ScheduleWrite,
) -> GeoResult {
    if write.update_region && write.retry_enabled {
        if regional.is_err() {
            let _ = manager.record_update_failure(region, write.epoch);
        } else if let Err(e) =
            manager.record_region_schedule_success(region, write.interval_days, write.epoch)
        {
            tracing::warn!("Failed to schedule geo update: {e}");
        }
    }
    let service_checked_at = manager.service_checked_at();
    let schedule = Some(manager.schedule(region, write.epoch));

    match regional {
        Ok(RegionUpdate::Updated {
            mut parts,
            checked_at,
        }) => {
            parts.extend(services.updated_parts);
            GeoResult::Updated {
                parts,
                checked_at,
                service_checked_at,
                schedule,
                warnings: services.errors,
            }
        }
        Ok(RegionUpdate::UpToDate { checked_at }) if !services.updated_parts.is_empty() => {
            GeoResult::Updated {
                parts: services.updated_parts,
                checked_at: checked_at.unwrap_or_else(chrono::Local::now),
                service_checked_at,
                schedule,
                warnings: services.errors,
            }
        }
        Ok(RegionUpdate::UpToDate { checked_at }) => GeoResult::UpToDate {
            checked_at,
            service_checked_at,
            schedule,
            warnings: services.errors,
        },
        Err(e) => {
            let mut message = e.to_string();
            if !services.errors.is_empty() {
                message.push_str("; ");
                message.push_str(&services.errors.join("; "));
            }
            GeoResult::Error {
                message,
                service_checked_at,
                schedule,
                updated_parts: services.updated_parts,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ScheduleWrite, ServiceRefreshResult, finalize_geo_result};
    use crate::app::msg::{GeoResult, GeoSchedule};
    use crate::config::profile::{GeoAutoUpdate, GeoRegion, RoutedService, ServiceRoute};
    use crate::geo::RegionUpdate;
    use std::collections::HashMap;

    fn automatic_schedule(epoch: u64) -> ScheduleWrite {
        ScheduleWrite {
            retry_enabled: true,
            update_region: true,
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
        manager
            .reset_update_schedules(GeoRegion::Ru, &[], true)
            .unwrap();
        let reset_next = manager.region_next_update(GeoRegion::Ru);

        let result = finalize_geo_result(
            &manager,
            GeoRegion::Ru,
            Err(anyhow::anyhow!("regional unavailable")),
            ServiceRefreshResult::default(),
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
        model
            .config
            .settings
            .geo_routing
            .service_routes
            .insert(RoutedService::Steam, ServiceRoute::Proxy);
        model.config.settings.geo_routing.auto_update = GeoAutoUpdate::Off;
        let epoch = manager.schedule_epoch();
        model.geo_retry_state = manager.record_update_failure(GeoRegion::Ru, epoch).unwrap();
        model.service_retry_states = HashMap::from([(
            RoutedService::Steam,
            manager
                .record_service_failure(RoutedService::Steam, epoch)
                .unwrap()
                .unwrap(),
        )]);

        super::reset_update_schedules(&mut model);

        assert!(model.geo_retry_state.is_none());
        assert!(model.service_retry_states.is_empty());
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
    fn geo_batch_keeps_updates_and_schedules_retry_after_service_failure() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let manager = crate::geo::GeoManager::new().unwrap();
        let checked_at = chrono::Local::now();
        let regional = Ok(RegionUpdate::Updated {
            parts: vec!["geoip-ru".into()],
            checked_at,
        });
        let services = ServiceRefreshResult {
            updated_parts: vec!["service-steam".into()],
            errors: vec!["Telegram rule-sets: unavailable".into()],
        };
        manager
            .record_service_failure(RoutedService::Telegram, 0)
            .unwrap();

        let result = finalize_geo_result(
            &manager,
            GeoRegion::Ru,
            regional,
            services,
            automatic_schedule(0),
        );

        let GeoResult::Updated {
            parts,
            schedule:
                Some(GeoSchedule {
                    retry_state,
                    service_retry_states,
                    ..
                }),
            warnings,
            ..
        } = result
        else {
            panic!("expected a partial updated result");
        };
        assert_eq!(parts, vec!["geoip-ru", "service-steam"]);
        assert_eq!(warnings, vec!["Telegram rule-sets: unavailable"]);
        assert!(retry_state.is_none());
        assert_eq!(
            service_retry_states[&RoutedService::Telegram].consecutive_failures,
            1
        );
    }

    #[test]
    fn geo_batch_reports_service_update_when_region_failed() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let manager = crate::geo::GeoManager::new().unwrap();
        let services = ServiceRefreshResult {
            updated_parts: vec!["service-steam".into()],
            errors: Vec::new(),
        };

        let result = finalize_geo_result(
            &manager,
            GeoRegion::Ru,
            Err(anyhow::anyhow!("regional unavailable")),
            services,
            automatic_schedule(0),
        );

        let GeoResult::Error {
            updated_parts,
            schedule: Some(schedule),
            ..
        } = result
        else {
            panic!("expected a partial error result");
        };
        assert_eq!(updated_parts, vec!["service-steam"]);
        assert_eq!(schedule.retry_state.unwrap().consecutive_failures, 1);
    }
}

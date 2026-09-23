use std::sync::mpsc::Sender;
use std::thread;

use anyhow::Result;

use crate::app::model::Model;
use crate::app::msg::{GeoResult, Msg};
use crate::config::profile::{GeoAutoUpdate, GeoRegion, RoutedService};

pub(super) fn download(tx: &Sender<Msg>, model: &mut Model) {
    model.geo_updating = true;
    let tx = tx.clone();
    let region = model
        .config
        .settings
        .geo_routing
        .current_region
        .unwrap_or(GeoRegion::Global);
    let services = model.config.settings.geo_routing.enabled_services();
    let automatic = model.geo_automatic_update;
    let interval_days = (model
        .config
        .settings
        .geo_routing
        .auto_update
        .interval_minutes()
        / 1_440) as i64;
    let existing_service_retries = model.service_retry_states.clone();
    let existing_service_checked_at = model.service_checked_at.clone();
    thread::spawn(move || {
        let gm = match crate::geo::GeoManager::new() {
            Ok(gm) => gm,
            Err(e) => {
                let _ = tx.send(Msg::GeoUpdated(GeoResult::Error {
                    message: e.to_string(),
                    retry_state: None,
                    service_retry_states: existing_service_retries,
                    service_checked_at: existing_service_checked_at,
                    next_update: None,
                    service_next_updates: Default::default(),
                    updated_parts: Vec::new(),
                }));
                return;
            }
        };
        let regional = gm.update_if_needed(region);
        let services =
            refresh_service_rule_sets(&gm, &services, automatic.then_some(interval_days));
        let result = finalize_geo_result(
            &gm,
            region,
            regional,
            services,
            automatic,
            true,
            interval_days,
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
    let tx = tx.clone();
    thread::spawn(move || {
        let (retry_states, checked_at, next_updates, updated_parts, errors) =
            match crate::geo::GeoManager::new() {
                Ok(gm) => {
                    let _ =
                        gm.ensure_update_schedules(GeoRegion::Global, &services, schedule_enabled);
                    let missing: Vec<_> = services
                        .into_iter()
                        .filter(|s| !gm.has_service_databases(*s))
                        .collect();
                    let refreshed = refresh_service_rule_sets(&gm, &missing, None);
                    (
                        gm.service_retry_states(),
                        gm.service_checked_at(),
                        gm.service_next_updates(),
                        refreshed.updated_parts,
                        refreshed.errors,
                    )
                }
                Err(e) => {
                    tracing::warn!("Failed to init geo manager for service rule-sets: {e:#}");
                    (
                        Default::default(),
                        Default::default(),
                        Default::default(),
                        Vec::new(),
                        vec![e.to_string()],
                    )
                }
            };
        let _ = tx.send(Msg::ServiceRuleSetsReady {
            retry_states,
            checked_at,
            next_updates,
            updated_parts,
            errors,
        });
    });
}

pub(super) fn retry_service_rule_sets(
    tx: &Sender<Msg>,
    model: &mut Model,
    services: Vec<RoutedService>,
) {
    model.geo_updating = true;
    let tx = tx.clone();
    let region = model
        .config
        .settings
        .geo_routing
        .current_region
        .unwrap_or(GeoRegion::Global);
    let existing_service_retries = model.service_retry_states.clone();
    let existing_service_checked_at = model.service_checked_at.clone();
    let automatic = model.geo_automatic_update;
    let interval_days = (model
        .config
        .settings
        .geo_routing
        .auto_update
        .interval_minutes()
        / 1_440) as i64;
    thread::spawn(move || {
        let result = match crate::geo::GeoManager::new() {
            Ok(gm) => {
                let checked_at = gm.last_checked_at(region);
                let services =
                    refresh_service_rule_sets(&gm, &services, automatic.then_some(interval_days));
                finalize_geo_result(
                    &gm,
                    region,
                    Ok(GeoResult::UpToDate {
                        checked_at,
                        retry_state: gm.retry_state(region),
                        service_retry_states: gm.service_retry_states(),
                        service_checked_at: gm.service_checked_at(),
                        next_update: gm.region_next_update(region),
                        service_next_updates: gm.service_next_updates(),
                        warnings: Vec::new(),
                    }),
                    services,
                    false,
                    false,
                    interval_days,
                )
            }
            Err(e) => GeoResult::Error {
                message: e.to_string(),
                retry_state: None,
                service_retry_states: existing_service_retries,
                service_checked_at: existing_service_checked_at,
                next_update: None,
                service_next_updates: Default::default(),
                updated_parts: Vec::new(),
            },
        };
        let _ = tx.send(Msg::GeoUpdated(result));
    });
}

pub(super) fn refresh_last_updated(tx: &Sender<Msg>, model: &Model) {
    let tx = tx.clone();
    let region = model
        .config
        .settings
        .geo_routing
        .current_region
        .unwrap_or(GeoRegion::Global);
    thread::spawn(move || {
        let manager = crate::geo::GeoManager::new().ok();
        let last_updated = manager.as_ref().and_then(|g| g.last_updated(region));
        let last_checked_at = manager.as_ref().and_then(|g| g.last_checked_at(region));
        let retry_state = manager.as_ref().and_then(|g| g.retry_state(region));
        let service_retry_states = manager
            .as_ref()
            .map(|g| g.service_retry_states())
            .unwrap_or_default();
        let service_checked_at = manager
            .as_ref()
            .map(|g| g.service_checked_at())
            .unwrap_or_default();
        let _ = tx.send(Msg::GeoMetadataRefreshed {
            last_updated,
            last_checked_at,
            retry_state,
            service_retry_states,
            service_checked_at,
            next_update: manager.as_ref().and_then(|g| g.region_next_update(region)),
            service_next_updates: manager
                .as_ref()
                .map(|g| g.service_next_updates())
                .unwrap_or_default(),
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

pub(super) fn reset_update_schedules(model: &mut Model) -> Result<()> {
    if let Ok(manager) = crate::geo::GeoManager::new() {
        let region = model
            .config
            .settings
            .geo_routing
            .current_region
            .unwrap_or(GeoRegion::Global);
        let services = model.config.settings.geo_routing.enabled_services();
        let enabled = model.config.settings.geo_routing.auto_update != GeoAutoUpdate::Off;
        manager.reset_update_schedules(region, &services, enabled)?;
        model.geo_next_update = manager.region_next_update(region);
        model.service_next_updates = manager.service_next_updates();
    }
    Ok(())
}

pub(super) fn download_if_missing(tx: &Sender<Msg>, model: &mut Model) {
    model.geo_updating = true;
    let tx = tx.clone();
    let region = model
        .config
        .settings
        .geo_routing
        .current_region
        .unwrap_or(GeoRegion::Global);
    let retry_enabled = model.config.settings.geo_routing.auto_update != GeoAutoUpdate::Off;
    let interval_days = (model
        .config
        .settings
        .geo_routing
        .auto_update
        .interval_minutes()
        / 1_440) as i64;
    thread::spawn(move || {
        let result = match crate::geo::GeoManager::new() {
            Ok(gm) => {
                if gm.has_databases(region) {
                    let _ = gm.clear_retry_state(region);
                    GeoResult::UpToDate {
                        checked_at: gm.last_checked_at(region),
                        retry_state: None,
                        service_retry_states: gm.service_retry_states(),
                        service_checked_at: gm.service_checked_at(),
                        next_update: gm.region_next_update(region),
                        service_next_updates: gm.service_next_updates(),
                        warnings: Vec::new(),
                    }
                } else {
                    finalize_geo_result(
                        &gm,
                        region,
                        gm.update_if_needed(region),
                        ServiceRefreshResult::default(),
                        retry_enabled,
                        true,
                        interval_days,
                    )
                }
            }
            Err(e) => GeoResult::Error {
                message: e.to_string(),
                retry_state: None,
                service_retry_states: Default::default(),
                service_checked_at: Default::default(),
                next_update: None,
                service_next_updates: Default::default(),
                updated_parts: Vec::new(),
            },
        };
        let _ = tx.send(Msg::GeoUpdated(result));
    });
}

/// Best-effort check/download of service rule-sets, shared by the periodic
/// geo-update thread and the post-connect fetch. Failures are logged, never
/// surfaced — the route builder just omits a service's rules until its files
/// appear.
fn refresh_service_rule_sets(
    gm: &crate::geo::GeoManager,
    services: &[RoutedService],
    automatic_interval_days: Option<i64>,
) -> ServiceRefreshResult {
    let mut result = ServiceRefreshResult::default();
    for service in services {
        match gm.update_service_if_needed(*service) {
            Ok(updated) => {
                if let Some(days) = automatic_interval_days
                    && let Err(e) = gm.record_service_schedule_success(*service, days)
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
                    let _ = gm.record_service_failure(*service);
                }
                result.errors.push(message);
            }
        }
    }
    result
}

#[derive(Default)]
struct ServiceRefreshResult {
    updated_parts: Vec<String>,
    errors: Vec<String>,
}

fn finalize_geo_result(
    manager: &crate::geo::GeoManager,
    region: GeoRegion,
    regional: anyhow::Result<GeoResult>,
    services: ServiceRefreshResult,
    retry_enabled: bool,
    update_region: bool,
    interval_days: i64,
) -> GeoResult {
    let regional_failed = regional.is_err();
    let retry_state = if !update_region || !retry_enabled {
        manager.retry_state(region)
    } else if regional_failed && retry_enabled {
        manager.record_update_failure(region).ok()
    } else {
        if !regional_failed
            && let Err(e) = manager.record_region_schedule_success(region, interval_days)
        {
            tracing::warn!("Failed to schedule geo update: {e}");
        }
        None
    };
    let service_retry_states = manager.service_retry_states();
    let service_checked_at = manager.service_checked_at();
    let next_update = manager.region_next_update(region);
    let service_next_updates = manager.service_next_updates();

    match regional {
        Ok(GeoResult::Updated {
            mut parts,
            last_updated,
            checked_at,
            ..
        }) => {
            parts.extend(services.updated_parts);
            GeoResult::Updated {
                parts,
                last_updated,
                checked_at,
                retry_state,
                service_retry_states,
                service_checked_at,
                next_update,
                service_next_updates,
                warnings: services.errors,
            }
        }
        Ok(GeoResult::UpToDate { checked_at, .. }) if !services.updated_parts.is_empty() => {
            GeoResult::Updated {
                parts: services.updated_parts,
                last_updated: manager.last_updated(region),
                checked_at: checked_at.unwrap_or_else(chrono::Local::now),
                retry_state,
                service_retry_states,
                service_checked_at,
                next_update,
                service_next_updates,
                warnings: services.errors,
            }
        }
        Ok(GeoResult::UpToDate { checked_at, .. }) => GeoResult::UpToDate {
            checked_at,
            retry_state,
            service_retry_states,
            service_checked_at,
            next_update,
            service_next_updates,
            warnings: services.errors,
        },
        Ok(GeoResult::Error { .. }) => unreachable!("GeoManager never returns GeoResult::Error"),
        Err(e) => {
            let mut message = e.to_string();
            if !services.errors.is_empty() {
                message.push_str("; ");
                message.push_str(&services.errors.join("; "));
            }
            GeoResult::Error {
                message,
                retry_state,
                service_retry_states,
                service_checked_at,
                next_update,
                service_next_updates,
                updated_parts: services.updated_parts,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ServiceRefreshResult, finalize_geo_result};
    use crate::app::msg::GeoResult;
    use crate::config::profile::{GeoRegion, RoutedService};

    #[test]
    fn geo_batch_keeps_updates_and_schedules_retry_after_service_failure() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let manager = crate::geo::GeoManager::new().unwrap();
        let checked_at = chrono::Local::now();
        let regional = Ok(GeoResult::Updated {
            parts: vec!["geoip-ru".into()],
            last_updated: None,
            checked_at,
            retry_state: None,
            service_retry_states: Default::default(),
            service_checked_at: Default::default(),
            next_update: None,
            service_next_updates: Default::default(),
            warnings: Vec::new(),
        });
        let services = ServiceRefreshResult {
            updated_parts: vec!["service-steam".into()],
            errors: vec!["Telegram rule-sets: unavailable".into()],
        };
        manager
            .record_service_failure(RoutedService::Telegram)
            .unwrap();

        let result =
            finalize_geo_result(&manager, GeoRegion::Ru, regional, services, true, true, 1);

        let GeoResult::Updated {
            parts,
            retry_state,
            service_retry_states,
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
            true,
            true,
            1,
        );

        let GeoResult::Error {
            updated_parts,
            retry_state,
            ..
        } = result
        else {
            panic!("expected a partial error result");
        };
        assert_eq!(updated_parts, vec!["service-steam"]);
        assert_eq!(retry_state.unwrap().consecutive_failures, 1);
    }
}

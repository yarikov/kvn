use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model, RoutingSettings, StatusKey};
use crate::config::profile::{GeoRegion, RoutingMode};

use crate::app::update::connection::queue_connect;
use crate::app::update::status::{
    DownloadKind, download_allowed, push_download_blocked, push_status,
};

pub(in crate::app::update) fn commit_routing_mode(
    model: &mut Model,
    mode: RoutingMode,
) -> Vec<Effect> {
    let region = model.config.settings.geo_routing.current_region;
    if !RoutingMode::available(region).contains(&mode) {
        let mut effects = vec![];
        push_status(
            &mut effects,
            model,
            StatusKey::Routing,
            AppStatus::Error(format!(
                "Routing mode change failed: {mode} is unavailable for region {}",
                region.map(|r| r.code_upper()).unwrap_or("GLOBAL")
            )),
        );
        return effects;
    }
    let changed = model.config.settings.geo_routing.mode() != mode;
    model.config.settings.geo_routing.set_mode(mode);
    let mut effects =
        super::onboarding::outcome(model, super::onboarding::Trigger::RoutingCommitted);
    effects.push(Effect::SaveConfig);
    let reconnecting = if changed && model.connection == ConnectionState::Connected {
        model
            .active_profile_id
            .is_some_and(|active_id| queue_connect(model, active_id))
    } else {
        false
    };
    let status = if reconnecting {
        format!("Routing mode changed: {mode} — reconnecting")
    } else {
        format!("Routing mode changed: {mode}")
    };
    push_status(
        &mut effects,
        model,
        StatusKey::Routing,
        AppStatus::Info(status),
    );
    effects
}

pub(in crate::app::update) fn commit_routing_settings(
    model: &mut Model,
    settings: RoutingSettings,
) -> Vec<Effect> {
    if !RoutingMode::available(Some(settings.region)).contains(&settings.mode) {
        let mut effects = vec![];
        push_status(
            &mut effects,
            model,
            StatusKey::Routing,
            AppStatus::Error(format!(
                "Routing mode change failed: {} is unavailable for region {}",
                settings.mode,
                settings.region.code_upper()
            )),
        );
        return effects;
    }

    let old_region = model.config.settings.geo_routing.current_region;
    let old_mode = model.config.settings.geo_routing.mode();
    let region_changed = old_region != Some(settings.region);
    let mode_changed = old_mode != settings.mode;
    if !region_changed && !mode_changed {
        return vec![];
    }

    if region_changed && let Some(region) = old_region {
        model
            .config
            .settings
            .geo_routing
            .selected_region_modes
            .insert(region, old_mode);
    }
    model
        .config
        .settings
        .geo_routing
        .set_region(settings.region);
    model.config.settings.geo_routing.set_mode(settings.mode);

    let connection = model.connection;
    let mut effects = vec![Effect::SaveConfig, Effect::BroadcastState];
    push_status(
        &mut effects,
        model,
        StatusKey::Routing,
        AppStatus::Info("Routing settings changed".into()),
    );

    if region_changed {
        effects.push(Effect::RefreshRuleSetStates);
        if settings.region != GeoRegion::Global {
            if download_allowed(model) {
                effects.extend(download_missing_selectable_rule_sets(model));
            } else {
                push_download_blocked(&mut effects, model, DownloadKind::Geo);
            }
        }
    }

    if connection == ConnectionState::Connected {
        if let Some(active_id) = model.active_profile_id {
            queue_connect(model, active_id);
        }
    } else if connection == ConnectionState::Idle
        && region_changed
        && model.config.settings.auto_connect
        && let Some(profile_id) = model.config.settings.last_connected_profile
        && let Some(idx) = model
            .config
            .profiles
            .iter()
            .position(|profile| profile.id == profile_id)
    {
        model.selected = crate::app::model::row_for_profile(&model.config, idx);
        queue_connect(model, profile_id);
    }

    effects
}

pub(in crate::app::update) fn commit_geo_region(
    model: &mut Model,
    region: GeoRegion,
) -> Vec<Effect> {
    let old_region = model.config.settings.geo_routing.current_region;
    let old_mode = model.config.settings.geo_routing.mode();
    let changed = old_region != Some(region);
    model.config.settings.geo_routing.set_region(region);
    let mut effects =
        super::onboarding::outcome(model, super::onboarding::Trigger::RegionCommitted);
    effects.push(Effect::SaveConfig);
    if changed {
        effects.push(Effect::RefreshRuleSetStates);
    }
    push_status(
        &mut effects,
        model,
        StatusKey::Routing,
        AppStatus::Info(format!("Geo region selected: {}", region.as_str())),
    );

    if changed && region != GeoRegion::Global {
        if download_allowed(model) {
            if let Some(download) = download_missing_selectable_rule_sets(model) {
                push_status(
                    &mut effects,
                    model,
                    StatusKey::Geo,
                    AppStatus::Info("Checking geo databases…".to_string()),
                );
                effects.push(download);
            }
        } else {
            push_download_blocked(&mut effects, model, DownloadKind::Geo);
        }
    }

    if changed {
        if let Some(old_region) = old_region {
            model
                .config
                .settings
                .geo_routing
                .selected_region_modes
                .insert(old_region, old_mode);
        }
        let new_mode = model.config.settings.geo_routing.mode();
        if new_mode != old_mode {
            push_status(
                &mut effects,
                model,
                StatusKey::Routing,
                AppStatus::Info(format!("Routing mode changed: {new_mode}")),
            );
        }
    }

    if changed
        && model.connection == ConnectionState::Connected
        && let Some(active_id) = model.active_profile_id
        && queue_connect(model, active_id)
    {
        model.logs.push_back("Region changed — reconnecting".into());
    }
    effects
}

pub(in crate::app::update) fn download_missing_selectable_rule_sets(
    model: &mut Model,
) -> Option<Effect> {
    let selectable = model.selectable_rule_sets();
    crate::app::update::geo::start_rule_set_download(
        model,
        selectable,
        crate::app::model::UpdateTrigger::Manual,
        true,
    )
}

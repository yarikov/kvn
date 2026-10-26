use crate::app::effect::Effect;
use crate::app::model::{AppStatus, ConnectionState, Model, StatusKey};
use crate::config::profile::GeoRegion;

use crate::app::update::connection::queue_connect;
use crate::app::update::status::{
    DownloadKind, download_allowed, push_download_blocked, push_status,
};

pub(in crate::app::update) fn commit_routing_mode(model: &mut Model, id: &str) -> Vec<Effect> {
    if !model.routing.declares(id) {
        let mut effects = vec![];
        push_status(
            &mut effects,
            model,
            StatusKey::Routing,
            AppStatus::Error(format!(
                "Routing mode change failed: {id} is not a mode of routing.json"
            )),
        );
        return effects;
    }
    let changed = select_mode(model, id);
    let mut effects =
        super::onboarding::outcome(model, super::onboarding::Trigger::RoutingCommitted);
    effects.push(Effect::SaveConfig);
    let reconnecting = changed && reconnect_to_apply_mode(model, &mut effects);
    let name = model.routing.mode_name(id);
    let status = if reconnecting {
        format!("Routing mode changed: {name} — reconnecting")
    } else {
        format!("Routing mode changed: {name}")
    };
    push_status(
        &mut effects,
        model,
        StatusKey::Routing,
        AppStatus::Info(status),
    );
    effects
}

pub(in crate::app::update) fn commit_routing_settings(model: &mut Model, id: &str) -> Vec<Effect> {
    if !model.routing.declares(id) {
        let mut effects = vec![];
        push_status(
            &mut effects,
            model,
            StatusKey::Routing,
            AppStatus::Error(format!(
                "Routing mode change failed: {id} is not a mode of routing.json"
            )),
        );
        return effects;
    }
    if !select_mode(model, id) {
        return vec![];
    }
    let mut effects = vec![Effect::SaveConfig, Effect::BroadcastState];
    push_status(
        &mut effects,
        model,
        StatusKey::Routing,
        AppStatus::Info("Routing settings changed".into()),
    );
    reconnect_to_apply_mode(model, &mut effects);
    effects
}

fn select_mode(model: &mut Model, id: &str) -> bool {
    let changed = model.config.settings.geo_routing.mode(&model.routing) != id;
    model.config.settings.geo_routing.set_mode(id);
    changed
}

fn reconnect_to_apply_mode(model: &mut Model, effects: &mut Vec<Effect>) -> bool {
    model.connection == ConnectionState::Connected
        && model
            .active_profile_id
            .is_some_and(|active_id| queue_connect(model, active_id, effects))
}

pub(in crate::app::update) fn commit_geo_region(
    model: &mut Model,
    region: GeoRegion,
) -> Vec<Effect> {
    let added = model.routing.add_region_modes(region);
    let mut effects =
        super::onboarding::outcome(model, super::onboarding::Trigger::RegionCommitted);
    push_status(
        &mut effects,
        model,
        StatusKey::Routing,
        AppStatus::Info(format!("Geo region selected: {}", region.as_str())),
    );
    if !added {
        return effects;
    }
    effects.push(Effect::AddRegionModes { region });
    effects.push(Effect::RefreshRuleSetStates);
    if !download_allowed(model) {
        push_download_blocked(&mut effects, model, DownloadKind::Geo);
    } else if let Some(download) = download_missing_selectable_rule_sets(model) {
        push_status(
            &mut effects,
            model,
            StatusKey::Geo,
            AppStatus::Info("Checking geo databases…".to_string()),
        );
        effects.push(download);
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

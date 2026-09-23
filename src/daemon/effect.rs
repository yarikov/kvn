use std::sync::mpsc::Sender;

use anyhow::Result;

use crate::app::effect::Effect;
use crate::app::model::Model;
use crate::app::msg::Msg;

use super::process_slot::lock_process_slot;
use super::{DaemonShared, config_io, connection, geo, profile_test, subscription, traffic};

pub(super) fn execute_daemon_effect(
    effect: Effect,
    tx: &Sender<Msg>,
    model: &mut Model,
    shared: &DaemonShared,
) -> Result<()> {
    match effect {
        Effect::Connect {
            profile,
            settings,
            attempt_id,
        } => connection::connect(tx, model, shared, profile, settings, attempt_id),
        Effect::Disconnect => connection::disconnect(model, shared),
        Effect::RevokeKillSwitchExceptions => connection::revoke_kill_switch_exceptions(model),
        Effect::ApplyKillSwitch { enabled } => connection::apply_kill_switch(tx, enabled),
        Effect::CheckAutoConnectPolkit => connection::check_auto_connect_polkit(tx),
        Effect::DownloadGeo => geo::download(tx, model),
        Effect::DownloadGeoIfMissing => geo::download_if_missing(tx, model),
        Effect::DownloadServiceRuleSetsIfMissing => {
            geo::download_service_rule_sets_if_missing(tx, model)
        }
        Effect::RetryServiceRuleSets { services } => {
            geo::retry_service_rule_sets(tx, model, services)
        }
        Effect::RefreshGeoLastUpdated => geo::refresh_last_updated(tx, model),
        Effect::ClearGeoRetryState { region } => geo::clear_retry_state(region),
        Effect::ResetGeoUpdateSchedules => geo::reset_update_schedules(model)?,
        Effect::SaveConfig => config_io::report_uncommitted_save(model),
        Effect::PersistSupportPrompt { previous } => {
            config_io::persist_support_prompt(model, previous)
        }
        Effect::SaveConfigConflict { edited, conflicts } => {
            config_io::save_conflict(model, edited, conflicts)
        }
        Effect::CommitEditedConfig { base, edited } => {
            config_io::commit_edited(tx, model, shared, base, edited)?
        }
        Effect::ReloadConfig => config_io::reload(tx),
        Effect::UpdateSubscription { id } => subscription::fetch(tx, model, id),
        Effect::FetchTrafficStats {
            attempt_id,
            request_id,
        } => traffic::fetch(tx, shared, attempt_id, request_id),
        Effect::TestProfile { id } => profile_test::start(tx, model, id),
        Effect::WriteState => crate::services::waybar::write_state(model),
        Effect::AppendAppLog { level, message } => {
            crate::services::log_tailer::append_app_log(&level, &message)
        }
        Effect::BroadcastState => {}
        Effect::Quit => {
            model.connect_attempt_id = model.connect_attempt_id.wrapping_add(1);
            lock_process_slot(&shared.process_slot).attempt_id = model.connect_attempt_id;
            model.should_quit = true;
        }
    }
    Ok(())
}

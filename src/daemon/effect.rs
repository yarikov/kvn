use std::sync::mpsc::Sender;

use crate::app::effect::Effect;
use crate::app::model::Model;
use crate::app::msg::{ConfigEditResult, Msg, RoutingEditResult};
use crate::config::profile::{Config, ConfigFile};

use super::process_slot::lock_process_slot;
use super::{DaemonShared, config_io, connection, geo, profile_test, subscription, traffic};

pub(super) enum EditReply {
    Config {
        file: ConfigFile,
        result: ConfigEditResult<Config>,
    },
    Routing(RoutingEditResult),
}

pub(super) fn execute_daemon_effect(
    effect: Effect,
    tx: &Sender<Msg>,
    model: &mut Model,
    shared: &DaemonShared,
    reply_requested: bool,
) -> Option<EditReply> {
    match effect {
        Effect::Connect {
            profile,
            settings,
            attempt_id,
        } => connection::connect(tx, model, shared, profile, settings, attempt_id),
        Effect::Disconnect => connection::disconnect(model, shared),
        Effect::ApplyKillSwitch { enabled } => connection::apply_kill_switch(tx, enabled),
        Effect::CheckAutoConnectPolkit => connection::check_auto_connect_polkit(tx),
        Effect::CheckIntegrationSetup => connection::check_integration_setup(tx),
        Effect::CheckOmakvn => connection::check_omakvn(tx, model),
        Effect::DownloadRuleSets {
            rule_sets,
            only_missing,
            record_schedule,
        } => geo::download_rule_sets(tx, model, rule_sets, only_missing, record_schedule),
        Effect::RefreshRuleSetStates => geo::refresh_rule_set_states(tx, model),
        Effect::ResetGeoUpdateSchedules => geo::reset_update_schedules(model),
        Effect::SaveProfiles
        | Effect::SaveSettings
        | Effect::PersistConfirmedProfiles
        | Effect::PersistConfirmedSettings => config_io::report_uncommitted_save(model),
        Effect::PersistSupportPrompt {
            previous,
            reopen_prompt,
        } => config_io::persist_support_prompt(model, previous, reopen_prompt),
        Effect::PersistOnboarding { previous, recovery } => {
            config_io::persist_onboarding(model, previous, recovery)
        }
        Effect::SaveConfigConflict {
            file,
            edited,
            conflicts,
        } => {
            if reply_requested {
                return Some(EditReply::Config {
                    file,
                    result: ConfigEditResult::Failed {
                        message: conflicts.join(", "),
                    },
                });
            }
            config_io::save_conflict(model, file, edited, conflicts)
        }
        Effect::CommitEditedConfig { file, base, edited } => {
            let result =
                config_io::commit_edited(tx, model, shared, file, base, edited, reply_requested);
            return Some(EditReply::Config { file, result });
        }
        Effect::CommitEditedRouting { base, edited } => {
            return Some(EditReply::Routing(config_io::commit_edited_routing(
                tx, model, shared, base, edited,
            )));
        }
        Effect::ReloadConfig => config_io::reload(tx),
        Effect::AddRegionModes { region } => config_io::add_region_modes(tx, region),
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
    None
}

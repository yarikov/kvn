use crate::app::effect::Effect;
use crate::app::model::{ConnectionState, MigrationPhase, MigrationStatus, Model, Overlay};

pub(super) fn begin(model: &mut Model, status: MigrationStatus) -> Vec<Effect> {
    if model
        .migration
        .as_ref()
        .is_none_or(|active| active.session_id == status.session_id)
        && model.kill_switch_pending.is_none()
        && !matches!(
            model.connection,
            ConnectionState::Connecting | ConnectionState::ConnectPending
        )
    {
        model.migration = Some(status);
        model.overlay = Overlay::Migration;
    }
    vec![]
}

pub(super) fn progress(model: &mut Model, status: MigrationStatus) -> Vec<Effect> {
    if model
        .migration
        .as_ref()
        .is_some_and(|active| active.session_id == status.session_id)
    {
        model.migration = Some(status);
        model.overlay = Overlay::Migration;
    }
    vec![]
}

pub(super) fn end(model: &mut Model, session_id: String) -> Vec<Effect> {
    if model
        .migration
        .as_ref()
        .is_some_and(|active| active.session_id == session_id)
    {
        model.migration = None;
        model.overlay = if model.config.settings.geo_routing.current_region.is_none() {
            Overlay::GeoRegions
        } else {
            Overlay::None
        };
    }
    vec![]
}

pub(super) fn stop_daemon(model: &Model, session_id: String) -> Vec<Effect> {
    let authorized = model.migration.as_ref().is_some_and(|active| {
        active.session_id == session_id && active.phase == MigrationPhase::Finalizing
    });
    if authorized {
        vec![Effect::Quit]
    } else {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::AppStatus;
    use crate::app::update::key::ipc::handle_ipc_command;
    use crate::test_helpers::*;

    #[test]
    fn ipc_error_status_can_be_cleared_during_migration() {
        let mut model = model_with_profiles(vec![]);
        model.set_status(AppStatus::Error("migration failed".into()));
        model.migration = Some(crate::app::model::MigrationStatus {
            session_id: "session".into(),
            phase: crate::app::model::MigrationPhase::Running,
            completed: 0,
            total: 1,
            summary: "Migrating".into(),
            error: None,
        });
        let revision = model.status_revision;

        let effects = handle_ipc_command(
            &mut model,
            crate::app::msg::IpcCommand::ClearErrorStatus {
                status_revision: revision,
            },
        );

        assert_eq!(model.status, AppStatus::Info(String::new()));
        assert_eq!(model.status_revision, revision);
        assert_eq!(effects, vec![Effect::BroadcastState]);
    }

    #[test]
    fn migration_session_blocks_commands_and_authorizes_only_its_cutover() {
        use crate::app::model::{MigrationPhase, MigrationStatus};
        use crate::app::msg::IpcCommand;

        let mut model = model_with_profiles(vec![]);
        let status = MigrationStatus {
            session_id: "session-a".into(),
            phase: MigrationPhase::Running,
            completed: 1,
            total: 3,
            summary: "Migrating profiles".into(),
            error: None,
        };
        handle_ipc_command(
            &mut model,
            IpcCommand::MigrationBegin {
                status: status.clone(),
            },
        );
        assert_eq!(model.overlay, Overlay::Migration);
        assert_eq!(model.migration.as_ref(), Some(&status));

        assert_eq!(
            handle_ipc_command(&mut model, IpcCommand::Quit),
            vec![Effect::BroadcastState]
        );
        assert_eq!(
            handle_ipc_command(
                &mut model,
                IpcCommand::MigrationStopDaemon {
                    session_id: "session-a".into(),
                },
            ),
            vec![Effect::BroadcastState]
        );

        let finalizing = MigrationStatus {
            phase: MigrationPhase::Finalizing,
            ..status
        };
        handle_ipc_command(
            &mut model,
            IpcCommand::MigrationProgress { status: finalizing },
        );
        assert_eq!(
            handle_ipc_command(
                &mut model,
                IpcCommand::MigrationStopDaemon {
                    session_id: "wrong-session".into(),
                },
            ),
            vec![Effect::BroadcastState]
        );
        assert_eq!(
            handle_ipc_command(
                &mut model,
                IpcCommand::MigrationStopDaemon {
                    session_id: "session-a".into(),
                },
            ),
            vec![Effect::Quit, Effect::BroadcastState]
        );

        handle_ipc_command(
            &mut model,
            IpcCommand::MigrationEnd {
                session_id: "wrong-session".into(),
            },
        );
        assert!(model.migration.is_some());
        handle_ipc_command(
            &mut model,
            IpcCommand::MigrationEnd {
                session_id: "session-a".into(),
            },
        );
        assert!(model.migration.is_none());
        assert_eq!(model.overlay, Overlay::GeoRegions);
    }

    #[test]
    fn migration_begin_waits_for_connection_and_kill_switch_transitions() {
        use crate::app::model::{MigrationPhase, MigrationStatus};
        use crate::app::msg::IpcCommand;

        let status = MigrationStatus {
            session_id: "session".into(),
            phase: MigrationPhase::Running,
            completed: 0,
            total: 1,
            summary: "Preparing".into(),
            error: None,
        };
        let mut model = model_with_profiles(vec![]);
        model.connection = ConnectionState::Connecting;
        handle_ipc_command(
            &mut model,
            IpcCommand::MigrationBegin {
                status: status.clone(),
            },
        );
        assert!(model.migration.is_none());

        model.connection = ConnectionState::Idle;
        model.kill_switch_pending = Some(true);
        handle_ipc_command(&mut model, IpcCommand::MigrationBegin { status });
        assert!(model.migration.is_none());
    }
}

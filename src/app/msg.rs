use crate::app::model::{
    ConnectionSettingsDraft, ConnectionState, MainPaneFocus, Overlay, RoutingSettingsDraft,
    TrafficStats,
};
use crate::config::profile::{
    GeoRegion, Profile, RoutedService, RoutingMode, Settings, Subscription,
};
use crate::ui::styles::Theme;
use chrono::{DateTime, Local};
use crossterm::event::{KeyEvent, MouseEvent};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IpcError {
    pub chain: Vec<String>,
}

impl IpcError {
    #[cfg(test)]
    pub fn new<S: Into<String>>(msg: S) -> Self {
        Self {
            chain: vec![msg.into()],
        }
    }
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, msg) in self.chain.iter().enumerate() {
            if i > 0 {
                write!(f, ": ")?;
            }
            write!(f, "{msg}")?;
        }
        Ok(())
    }
}

impl From<anyhow::Error> for IpcError {
    fn from(err: anyhow::Error) -> Self {
        Self {
            chain: err.chain().map(|c| c.to_string()).collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectRetryReason {
    BinaryBeingWritten,
    TunNotPermitted,
}

pub enum Msg {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    Tick,
    Resize,
    GeoUpdated {
        result: GeoResult,
        region_ready: bool,
    },
    GeoMetadataRefreshed {
        last_updated: Option<String>,
        last_checked_at: Option<chrono::DateTime<chrono::Local>>,
        service_checked_at: ServiceCheckTimes,
        schedule: Option<GeoSchedule>,
    },
    SystemResumed,
    Connected {
        pid: u32,
        attempt_id: u64,
        profile_id: Uuid,
        routing_fallback: Option<crate::config::profile::RoutingFallback>,
    },
    ConnectFailed {
        attempt_id: u64,
        error: IpcError,
    },
    ConnectRetrying {
        attempt_id: u64,
        reason: ConnectRetryReason,
        limit: std::time::Duration,
    },
    SingBoxExited {
        attempt_id: u64,
        code: Option<i32>,
        signal: Option<i32>,
    },
    ServiceRuleSetsReady {
        checked_at: ServiceCheckTimes,
        schedule: Option<ServiceSchedule>,
        updated_parts: Vec<String>,
        errors: Vec<String>,
    },
    SubscriptionFetched {
        id: Uuid,
        result: Result<Vec<crate::config::profile::Profile>, IpcError>,
    },

    IpcCommand(IpcCommand),
    IpcRequest {
        command: IpcCommand,
        request_id: Uuid,
    },
    StateUpdate {
        snapshot: Box<StateSnapshot>,
    },
    IpcReadFailed {
        message: String,
    },
    DaemonDisconnected,
    ConfigReloaded(Box<Result<crate::config::profile::Config, IpcError>>),
    KillSwitchApplied {
        enabled: bool,
        error: Option<IpcError>,
    },
    AutoConnectPolkitChecked {
        error: Option<IpcError>,
    },
    IntegrationSetupChecked(crate::onboarding::IntegrationSetup),
    TrafficStatsUpdated {
        attempt_id: u64,
        request_id: u64,
        up_total: u64,
        down_total: u64,
        conn_count: usize,
        sampled_at_ms: u64,
    },
    ThemeChanged(Theme),
    TestResult {
        id: Uuid,
        latency_ms: Option<u64>,
    },
}

pub type ServiceCheckTimes = std::collections::HashMap<RoutedService, DateTime<Local>>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GeoSchedule {
    pub epoch: u64,
    pub retry_state: Option<crate::config::profile::RetryState>,
    pub next_update: Option<chrono::NaiveDate>,
    pub service_retry_states:
        std::collections::HashMap<RoutedService, crate::config::profile::RetryState>,
    pub service_next_updates: std::collections::HashMap<RoutedService, chrono::NaiveDate>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceSchedule {
    pub epoch: u64,
    pub retry_states: std::collections::HashMap<RoutedService, crate::config::profile::RetryState>,
    pub next_updates: std::collections::HashMap<RoutedService, chrono::NaiveDate>,
}

#[derive(Debug)]
pub enum GeoResult {
    Updated {
        parts: Vec<String>,
        checked_at: DateTime<Local>,
        service_checked_at: ServiceCheckTimes,
        schedule: Option<GeoSchedule>,
        warnings: Vec<String>,
    },
    UpToDate {
        checked_at: Option<DateTime<Local>>,
        service_checked_at: ServiceCheckTimes,
        schedule: Option<GeoSchedule>,
        warnings: Vec<String>,
    },
    Error {
        message: String,
        service_checked_at: ServiceCheckTimes,
        schedule: Option<GeoSchedule>,
        updated_parts: Vec<String>,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "cmd")]
pub enum IpcCommand {
    Attach,
    ClearErrorStatus {
        status_revision: u64,
    },
    CheckOnboarding,
    CheckSupportPrompt,
    Detach,
    Key {
        code: String,
        char: Option<char>,
        ctrl: bool,
        #[serde(default)]
        alt: bool,
    },
    SelectSource {
        index: usize,
    },
    ScrollViewport {
        context: Overlay,
        start: usize,
        visible: usize,
        delta: isize,
    },
    MoveSourceSelection {
        delta: isize,
    },
    SetMainPaneFocus {
        focus: MainPaneFocus,
    },
    GoFirst,
    ConnectProfile {
        profile_id: Uuid,
    },
    Disconnect,
    Reconnect,
    Toggle,
    SetRoutingMode {
        mode: RoutingMode,
    },
    SetGeoRegion {
        region: GeoRegion,
    },
    SetKillSwitch {
        enabled: bool,
    },
    SetAutoConnect {
        enabled: bool,
    },
    ResolveSupportPrompt {
        resolution: SupportPromptResolution,
    },
    DismissSupportPrompt,
    Paste {
        text: String,
    },
    Copied {
        target: CopiedTarget,
    },
    ReloadConfig,
    ApplyEditedConfig {
        base: Box<crate::config::profile::Config>,
        edited: Box<crate::config::profile::Config>,
    },
    Quit,
    ClientError {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CopiedTarget {
    Profile { name: String },
    Subscription { name: String },
    Logs,
    Command,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SupportPromptResolution {
    RemindLater,
    Supported,
    Dismiss,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_error_display_joins_chain_with_colons() {
        let err = IpcError {
            chain: vec!["top".into(), "middle".into(), "root cause".into()],
        };
        assert_eq!(err.to_string(), "top: middle: root cause");
    }

    #[test]
    fn ipc_error_display_single_element() {
        let err = IpcError::new("only");
        assert_eq!(err.to_string(), "only");
    }

    #[test]
    fn ipc_error_display_empty_chain() {
        let err = IpcError { chain: vec![] };
        assert_eq!(err.to_string(), "");
    }

    #[test]
    fn ipc_error_from_anyhow_captures_context_chain() {
        let err: anyhow::Error = anyhow::anyhow!("inner").context("middle").context("top");
        let ipc: IpcError = err.into();
        assert!(ipc.chain.first().unwrap().contains("top"));
        assert!(ipc.chain.last().unwrap().contains("inner"));
        assert!(ipc.chain.len() >= 3);
    }

    #[test]
    fn ipc_command_serde_roundtrip_each_variant() {
        let cmds = vec![
            IpcCommand::Attach,
            IpcCommand::ClearErrorStatus { status_revision: 7 },
            IpcCommand::CheckOnboarding,
            IpcCommand::CheckSupportPrompt,
            IpcCommand::Detach,
            IpcCommand::Key {
                code: "Char".into(),
                char: Some('a'),
                ctrl: false,
                alt: false,
            },
            IpcCommand::SelectSource { index: 1 },
            IpcCommand::MoveSourceSelection { delta: -3 },
            IpcCommand::ScrollViewport {
                context: Overlay::None,
                start: 5,
                visible: 10,
                delta: -3,
            },
            IpcCommand::SetMainPaneFocus {
                focus: MainPaneFocus::Logs,
            },
            IpcCommand::GoFirst,
            IpcCommand::ConnectProfile {
                profile_id: Uuid::nil(),
            },
            IpcCommand::Disconnect,
            IpcCommand::Reconnect,
            IpcCommand::Toggle,
            IpcCommand::SetRoutingMode {
                mode: RoutingMode::Bypass(GeoRegion::Ru),
            },
            IpcCommand::SetGeoRegion {
                region: GeoRegion::Cn,
            },
            IpcCommand::SetKillSwitch { enabled: true },
            IpcCommand::SetAutoConnect { enabled: false },
            IpcCommand::ResolveSupportPrompt {
                resolution: SupportPromptResolution::RemindLater,
            },
            IpcCommand::ResolveSupportPrompt {
                resolution: SupportPromptResolution::Supported,
            },
            IpcCommand::Paste {
                text: "hello".into(),
            },
            IpcCommand::Copied {
                target: CopiedTarget::Profile { name: "A".into() },
            },
            IpcCommand::Copied {
                target: CopiedTarget::Subscription { name: "B".into() },
            },
            IpcCommand::Copied {
                target: CopiedTarget::Logs,
            },
            IpcCommand::Copied {
                target: CopiedTarget::Command,
            },
            IpcCommand::ReloadConfig,
            IpcCommand::ApplyEditedConfig {
                base: Box::new(crate::config::profile::Config::default()),
                edited: Box::new(crate::config::profile::Config::default()),
            },
            IpcCommand::Quit,
        ];
        for cmd in cmds {
            let json = serde_json::to_string(&cmd).unwrap();
            let back: IpcCommand = serde_json::from_str(&json).unwrap();
            assert_eq!(format!("{:?}", cmd), format!("{:?}", back));
        }
    }

    #[test]
    fn ipc_error_serde_roundtrip() {
        let err = IpcError {
            chain: vec!["a".into(), "b".into()],
        };
        let json = serde_json::to_string(&err).unwrap();
        let back: IpcError = serde_json::from_str(&json).unwrap();
        assert_eq!(err, back);
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ConfigEditResult {
    Saved,
    Conflict {
        current: Box<crate::config::profile::Config>,
        paths: Vec<String>,
    },
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateSnapshot {
    #[serde(default)]
    pub daemon_version: String,
    #[serde(default)]
    pub ipc_version: u32,
    #[serde(default)]
    pub supports_viewport_scroll: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_to: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_edit_result: Option<ConfigEditResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_result: Option<crate::app::scroll::ScrollPosition>,
    pub connection: ConnectionState,
    pub status: String,
    pub status_is_error: bool,
    #[serde(default)]
    pub status_revision: u64,
    #[serde(default)]
    pub status_events: Vec<crate::app::model::StatusEvent>,
    pub singbox_pid: Option<u32>,
    pub active_profile_id: Option<String>,
    pub selected: usize,
    pub routing_selected: usize,
    pub geo_region_selected: usize,
    pub dns_selected: usize,
    #[serde(default)]
    pub dns_preset_draft: Option<String>,
    #[serde(default)]
    pub dns_fakeip_draft: Option<bool>,
    #[serde(default)]
    pub theme_draft: Option<String>,
    #[serde(default)]
    pub onboarding_awaiting: Option<crate::onboarding::OnboardingStep>,
    #[serde(default)]
    pub onboarding_omarchy: bool,
    #[serde(default)]
    pub onboarding_complete: bool,
    #[serde(default)]
    pub integration_setup: crate::onboarding::IntegrationSetup,
    #[serde(default)]
    pub settings_menu_selected: usize,
    #[serde(default)]
    pub routing_settings_draft: Option<RoutingSettingsDraft>,
    #[serde(default)]
    pub connection_settings_draft: Option<ConnectionSettingsDraft>,
    #[serde(default)]
    pub auto_connect_pending: bool,
    #[serde(default)]
    pub kill_switch_pending: Option<bool>,
    #[serde(default)]
    pub interface_settings_draft: Option<crate::config::profile::IconSet>,
    pub geo_updating: bool,
    #[serde(default)]
    pub routing_fallback: Option<crate::config::profile::RoutingFallback>,
    pub geo_last_updated: Option<String>,
    #[serde(default)]
    pub geo_last_checked_at: Option<DateTime<Local>>,
    #[serde(default)]
    pub service_checked_at: std::collections::HashMap<RoutedService, DateTime<Local>>,
    pub overlay: Overlay,
    #[serde(default)]
    pub main_pane_focus: MainPaneFocus,
    pub profiles: Vec<Profile>,
    pub subscriptions: Vec<Subscription>,
    pub settings: Settings,
    #[serde(default)]
    pub traffic: TrafficStats,
    #[serde(default)]
    pub log_session_offsets: Option<LogSessionOffsets>,
    #[serde(default)]
    pub profile_latencies: std::collections::HashMap<String, Option<u64>>,
    #[serde(default)]
    pub testing_profiles: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct LogSessionOffsets {
    pub app: u64,
    pub singbox: u64,
}

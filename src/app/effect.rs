use crate::config::profile::{Config, Profile, Settings};
use uuid::Uuid;

#[derive(Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Effect {
    Connect {
        profile: Profile,
        settings: Settings,
        attempt_id: u64,
    },
    Disconnect,
    RevokeKillSwitchExceptions,
    DownloadRuleSets {
        rule_sets: Vec<crate::geo::RuleSet>,
        only_missing: bool,
        record_schedule: bool,
    },
    RefreshRuleSetStates,
    ResetGeoUpdateSchedules,
    WriteState,
    SaveConfig,
    PersistConfirmedState,
    PersistSupportPrompt {
        previous: crate::support_prompt::SupportPromptState,
        reopen_prompt: bool,
    },
    PersistOnboarding {
        previous: crate::onboarding::OnboardingProgress,
        recovery: crate::onboarding::OnboardingRecovery,
    },
    SaveConfigConflict {
        edited: Box<Config>,
        conflicts: Vec<String>,
    },
    CommitEditedConfig {
        base: Box<Config>,
        edited: Box<Config>,
    },
    CommitEditedRouting {
        base: crate::config::routing::RoutingEditBase,
        edited: Box<crate::config::routing::RoutingConfig>,
    },
    UpdateSubscription {
        id: Uuid,
    },
    BroadcastState,
    Quit,
    AppendAppLog {
        level: String,
        message: String,
    },
    ReloadConfig,
    AddRegionModes {
        region: crate::config::profile::GeoRegion,
    },
    ApplyKillSwitch {
        enabled: bool,
    },
    CheckAutoConnectPolkit,
    CheckIntegrationSetup,
    CheckOmakvn,
    FetchTrafficStats {
        attempt_id: u64,
        request_id: u64,
    },
    TestProfile {
        id: Uuid,
    },
}

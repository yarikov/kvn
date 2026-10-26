use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;
use uuid::Uuid;

use crate::config::load_config;
use crate::config::profile::{Config, Profile};
use crate::config::routing::RoutingConfig;
use crate::ui::styles::Theme;
use chrono::{DateTime, Local};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Overlay {
    #[default]
    None,
    Help(HelpState),
    SettingsMenu(SettingsMenuPage),
    ConfirmDelete,
    ConfirmDisable(DisableTarget),
    RoutingMode,
    GeoRegions,
    DnsSettings,
    Support,
    Onboarding(crate::onboarding::OnboardingStep),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisableTarget {
    AutoConnect,
    KillSwitch,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsMenuPage {
    #[default]
    Root,
    Routing,
    Connection,
    Interface,
}

impl SettingsMenuPage {
    pub fn entry(self) -> Option<SettingsMenuEntry> {
        match self {
            Self::Root => None,
            Self::Routing => Some(SettingsMenuEntry::Routing),
            Self::Connection => Some(SettingsMenuEntry::Connection),
            Self::Interface => Some(SettingsMenuEntry::Interface),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsMenuEntry {
    Connection,
    Dns,
    Interface,
    Routing,
}

impl SettingsMenuEntry {
    pub const ALL: [Self; 4] = [Self::Connection, Self::Dns, Self::Interface, Self::Routing];

    pub fn key(self) -> char {
        match self {
            Self::Connection => 'c',
            Self::Dns => 'd',
            Self::Interface => 'i',
            Self::Routing => 'r',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Connection => "Connection",
            Self::Dns => "DNS",
            Self::Interface => "Interface",
            Self::Routing => "Routing",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|entry| *entry == self)
            .expect("every entry is listed in ALL")
    }

    pub fn from_key(key: char) -> Option<Self> {
        Self::ALL.into_iter().find(|entry| entry.key() == key)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingSettingsDraft {
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionSettingsDraft {
    #[serde(default)]
    pub auto_connect: Option<bool>,
    #[serde(default)]
    pub kill_switch: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionSettings {
    pub auto_connect: bool,
    pub kill_switch: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateTrigger {
    Manual,
    Automatic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsSettingsItem {
    Preset,
    ToggleFakeIp,
}

impl DnsSettingsItem {
    pub const ALL: [DnsSettingsItem; 2] = [DnsSettingsItem::Preset, DnsSettingsItem::ToggleFakeIp];

    pub fn from_index(idx: usize) -> Option<Self> {
        Self::ALL.get(idx).copied()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelpContext {
    #[default]
    Sources,
    Logs,
    SettingsMenu(SettingsMenuPage),
    ConfirmDelete,
    ConfirmDisable(DisableTarget),
    RoutingMode,
    GeoRegions,
    DnsSettings,
    Support,
    Onboarding(crate::onboarding::OnboardingStep),
}

impl HelpContext {
    pub fn restore_overlay(self) -> Overlay {
        match self {
            Self::Sources | Self::Logs => Overlay::None,
            Self::SettingsMenu(page) => Overlay::SettingsMenu(page),
            Self::ConfirmDelete => Overlay::ConfirmDelete,
            Self::ConfirmDisable(target) => Overlay::ConfirmDisable(target),
            Self::RoutingMode => Overlay::RoutingMode,
            Self::GeoRegions => Overlay::GeoRegions,
            Self::DnsSettings => Overlay::DnsSettings,
            Self::Support => Overlay::Support,
            Self::Onboarding(step) => Overlay::Onboarding(step),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelpState {
    pub context: HelpContext,
    pub selected: usize,
}

impl Default for HelpState {
    fn default() -> Self {
        Self {
            context: HelpContext::Sources,
            selected: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MainPaneFocus {
    #[default]
    Sources,
    Logs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRow {
    StandaloneProfile(usize),
    SubscriptionHeader(usize),
    SubscriptionProfile { sub_idx: usize, profile_idx: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ConnectionState {
    #[default]
    Idle,
    Connecting,
    ConnectPending,
    Connected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "level", content = "text", rename_all = "snake_case")]
pub enum AppStatus {
    Info(String),
    Error(String),
}

impl AppStatus {
    // TODO: carry Option<AppStatus> on the wire instead, which removes this
    // decoder and StateSnapshot::status_is_error. Blocked on an IPC_VERSION
    // bump released together with the omakvn plugin, which reads
    // status_is_error to colour the bar icon.
    pub fn from_snapshot(text: String, is_error: bool) -> Option<Self> {
        match (text.is_empty(), is_error) {
            (true, _) => None,
            (false, true) => Some(AppStatus::Error(text)),
            (false, false) => Some(AppStatus::Info(text)),
        }
    }

    pub fn text(&self) -> &str {
        match self {
            AppStatus::Info(s) | AppStatus::Error(s) => s.as_str(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum StatusKey {
    Connection,
    ConnectionWarning(usize),
    KillSwitch,
    AutoConnect,
    Routing,
    Dns,
    Geo,
    Subscription(Uuid),
    Sources,
    Clipboard,
    Interface,
    Config,
    Client,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusEvent {
    pub revision: u64,
    pub key: StatusKey,
    pub status: AppStatus,
}

const STATUS_EVENT_CAPACITY: usize = 16;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    pub connected: bool,
    pub profile_name: Option<String>,
    pub active_profile_id: Option<String>,
    pub singbox_pid: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficStats {
    #[serde(default)]
    pub up_rate_bps: u64,
    #[serde(default)]
    pub down_rate_bps: u64,
    #[serde(default)]
    pub up_total: u64,
    #[serde(default)]
    pub down_total: u64,
    #[serde(default)]
    pub conn_count: usize,
}

pub(crate) const MAX_LOG_LINES: usize = 1000;

fn source_rows(config: &Config) -> Vec<SourceRow> {
    let mut rows = Vec::new();
    for (idx, profile) in config.profiles.iter().enumerate() {
        if profile.subscription_id.is_none() {
            rows.push(SourceRow::StandaloneProfile(idx));
        }
    }
    for (sub_idx, sub) in config.subscriptions.iter().enumerate() {
        rows.push(SourceRow::SubscriptionHeader(sub_idx));
        for (profile_idx, profile) in config.profiles.iter().enumerate() {
            if profile.subscription_id == Some(sub.id) {
                rows.push(SourceRow::SubscriptionProfile {
                    sub_idx,
                    profile_idx,
                });
            }
        }
    }
    rows
}

pub(crate) fn row_for_profile(config: &Config, profile_idx: usize) -> usize {
    source_rows(config)
        .iter()
        .position(|row| match row {
            SourceRow::StandaloneProfile(idx) => *idx == profile_idx,
            SourceRow::SubscriptionProfile {
                profile_idx: idx, ..
            } => *idx == profile_idx,
            _ => false,
        })
        .unwrap_or(0)
}

pub(crate) fn row_for_subscription_header(config: &Config, sub_idx: usize) -> usize {
    source_rows(config)
        .iter()
        .position(|row| matches!(row, SourceRow::SubscriptionHeader(idx) if *idx == sub_idx))
        .unwrap_or(0)
}

pub struct Model {
    pub overlay: Overlay,
    pub settings_menu_selected: usize,
    pub routing_settings_draft: Option<RoutingSettingsDraft>,
    pub connection_settings_draft: Option<ConnectionSettingsDraft>,
    pub interface_settings_draft: Option<crate::config::profile::IconSet>,
    pub main_pane_focus: MainPaneFocus,
    pub connection: ConnectionState,
    pub config: Config,
    pub routing: RoutingConfig,
    pub routing_error: Option<String>,
    pub config_persistence_blocked: bool,
    pub unsaved_state_merge_base: Option<Config>,
    pub support_prompt: crate::support_prompt::SupportPromptState,
    pub onboarding: crate::onboarding::OnboardingProgress,
    pub integration_setup: crate::onboarding::IntegrationSetup,
    pub support_selected: usize,
    pub sources_scroll: usize,
    pub overlay_scroll: Option<usize>,
    pub scroll_result: Option<crate::app::scroll::ScrollPosition>,
    pub selected: usize,
    pub status: Option<AppStatus>,
    pub status_revision: u64,
    pub status_events: VecDeque<StatusEvent>,
    pub singbox_pid: Option<u32>,
    pub active_profile_id: Option<Uuid>,
    pub connecting_profile_id: Option<Uuid>,
    pub connect_attempt_id: u64,
    pub kill_switch_pending: Option<bool>,
    pub auto_connect_pending: bool,
    pub routing_selected: usize,
    pub geo_region_selected: usize,
    pub dns_selected: usize,
    pub dns_preset_draft: Option<String>,
    pub dns_fakeip_draft: Option<bool>,
    pub routing_fallback: Option<crate::config::profile::RoutingFallback>,
    pub logs: VecDeque<String>,
    pub rule_set_updates: HashMap<String, UpdateTrigger>,
    pub rule_set_states: crate::geo::RuleSetStates,
    pub rule_set_automatic_attempts: HashMap<String, DateTime<Local>>,
    pub subscription_updates: HashMap<Uuid, UpdateTrigger>,
    pub needs_redraw: bool,
    pub should_quit: bool,
    pub geo_schedule_epoch: u64,
    pub traffic: TrafficStats,
    pub last_traffic_sample_at_ms: u64,
    pub traffic_request_id: u64,
    pub last_traffic_response_id: u64,
    pub last_traffic_fetch_at: Option<Instant>,
    pub last_integration_check_at: Option<Instant>,
    pub integration_check_pending: bool,
    pub theme: Theme,
    pub theme_draft: Option<String>,
    pub profile_latencies: HashMap<Uuid, Option<u64>>,
    pub testing_profiles: HashSet<Uuid>,
    pub pending_tests: VecDeque<Uuid>,
}

fn note_startup_error(startup_error: &mut Option<String>, message: String) {
    tracing::error!("{message}");
    *startup_error = Some(match startup_error.take() {
        Some(previous) => format!("{previous}; {message}"),
        None => message,
    });
}

fn load_rule_set_states(
    settings: &crate::config::profile::Settings,
    routing: &RoutingConfig,
) -> (u64, crate::geo::RuleSetStates) {
    let Ok(manager) = crate::geo::GeoManager::new() else {
        return (0, crate::geo::RuleSetStates::new());
    };
    let selectable = crate::route_plan::selectable_rule_sets(routing);
    let epoch = manager.schedule_epoch();
    let _ = manager.ensure_update_schedules(
        &selectable,
        settings.geo_routing.auto_update != crate::config::profile::GeoAutoUpdate::Off,
        epoch,
    );
    (epoch, manager.states(&selectable))
}

fn show_omarchy_card() -> bool {
    crate::omarchy::detect_omarchy_theme().is_some() && !crate::omarchy::omakvn_plugin_installed()
}

impl Model {
    pub fn push_log(&mut self, line: String) {
        self.logs.push_back(line);
        if self.logs.len() > MAX_LOG_LINES {
            self.logs.pop_front();
        }
    }

    pub fn status_text(&self) -> &str {
        self.status.as_ref().map_or("", AppStatus::text)
    }

    pub fn status_is_error(&self) -> bool {
        matches!(self.status, Some(AppStatus::Error(_)))
    }

    pub fn set_status(&mut self, key: StatusKey, status: AppStatus) {
        self.status_revision = self.status_revision.wrapping_add(1);
        self.status_events.retain(|event| event.key != key);
        if !status.text().is_empty() {
            self.push_log(status.text().to_string());
            if self.status_events.len() == STATUS_EVENT_CAPACITY {
                self.status_events.pop_front();
            }
            self.status_events.push_back(StatusEvent {
                revision: self.status_revision,
                key,
                status: status.clone(),
            });
        }
        self.status = Some(status);
    }

    pub fn geo_updating(&self) -> bool {
        !self.rule_set_updates.is_empty()
    }

    pub fn active_rule_sets(&self) -> Vec<crate::geo::RuleSet> {
        crate::route_plan::active_rule_sets(&self.config.settings, &self.routing)
    }

    pub fn required_rule_sets(&self) -> Vec<crate::geo::RuleSet> {
        crate::route_plan::RoutePlan::for_settings(&self.config.settings, &self.routing)
            .required_rule_sets()
            .cloned()
            .collect()
    }

    pub fn selectable_rule_sets(&self) -> Vec<crate::geo::RuleSet> {
        crate::route_plan::selectable_rule_sets(&self.routing)
    }

    pub fn selectable_rule_sets_checked_at(&self) -> Option<DateTime<Local>> {
        self.selectable_rule_sets()
            .iter()
            .map(|rule_set| {
                self.rule_set_states
                    .get(&rule_set.key())
                    .and_then(|state| state.checked_at)
            })
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .min()
    }

    pub fn mark_selectable_rule_sets_checked(&mut self, checked_at: DateTime<Local>) {
        for rule_set in self.selectable_rule_sets() {
            self.rule_set_states
                .entry(rule_set.key())
                .or_default()
                .checked_at = Some(checked_at);
        }
    }

    pub fn rule_set_present(&self, rule_set: &crate::geo::RuleSet) -> bool {
        crate::geo::rule_set_present(&self.rule_set_states, rule_set)
    }

    pub fn rule_set_retry_pending(
        &self,
        rule_set: &crate::geo::RuleSet,
        now: chrono::DateTime<chrono::Local>,
    ) -> bool {
        self.rule_set_states
            .get(&rule_set.key())
            .and_then(|state| state.retry.as_ref())
            .is_some_and(|retry| retry.retry_at > now)
    }

    pub fn apply_rule_set_states(&mut self, states: crate::geo::RuleSetStates, epoch: u64) {
        let current_schedule = epoch == self.geo_schedule_epoch;
        for (key, mut state) in states {
            if !current_schedule {
                let known = self.rule_set_states.get(&key);
                state.retry = known.and_then(|known| known.retry);
                state.next_update = known.and_then(|known| known.next_update);
            }
            self.rule_set_states.insert(key, state);
        }
    }

    pub fn clear_error_status(&mut self, status_revision: u64) -> bool {
        if self.status_revision != status_revision
            || !matches!(self.status, Some(AppStatus::Error(_)))
        {
            return false;
        }
        self.status = None;
        true
    }

    pub fn new() -> anyhow::Result<Self> {
        let mut startup_error: Option<String> = None;
        let mut config_persistence_blocked = false;
        let mut config = match load_config() {
            Ok(cfg) => match cfg.validate() {
                Ok(()) => cfg,
                Err(e) => {
                    note_startup_error(
                        &mut startup_error,
                        format!("Configuration load failed: invalid config; using defaults: {e:#}"),
                    );
                    config_persistence_blocked = true;
                    Config::default()
                }
            },
            Err(e) => {
                note_startup_error(
                    &mut startup_error,
                    format!("Configuration load failed; using defaults: {e:#}"),
                );
                config_persistence_blocked = true;
                Config::default()
            }
        };
        let predates_mode_selection = config.settings.geo_routing.predates_mode_selection();
        let mut routing_error = None;
        let mut routing = match crate::config::load_routing(
            &config.settings.geo_routing,
            !config_persistence_blocked,
        ) {
            Ok(routing) => routing,
            Err(e) => {
                note_startup_error(
                    &mut startup_error,
                    format!(
                        "Routing configuration load failed; connecting is blocked until routing.json is fixed (Space r e): {e:#}"
                    ),
                );
                routing_error = Some(format!("{e:#}"));
                RoutingConfig::for_new_file(&Default::default())
            }
        };
        config.settings.geo_routing.forget_legacy_regions();
        if routing_error.is_none() && !config_persistence_blocked {
            match crate::config::move_service_routes(&mut config, &mut routing) {
                Ok(true) => {
                    tracing::info!("Moved the Steam and Telegram overrides into routing.json rules")
                }
                Ok(false) => {}
                Err(e) => note_startup_error(
                    &mut startup_error,
                    format!("Steam and Telegram overrides were not moved to routing.json: {e:#}"),
                ),
            }
        }
        let default_selected = 0;

        let stale_cleanup_failed = match crate::services::waybar::cleanup_stale_session() {
            Ok(stale_pid) => {
                if let Some(pid) = stale_pid {
                    tracing::warn!("Stopped stale sing-box process {pid} during crash recovery");
                }
                crate::services::waybar::clear_state();
                false
            }
            Err(e) => {
                note_startup_error(
                    &mut startup_error,
                    format!("Stale VPN cleanup failed: {e:#}"),
                );
                true
            }
        };

        let now = chrono::Utc::now();
        let onboarding = crate::onboarding::OnboardingProgress::new(
            crate::onboarding::load_for_daemon(predates_mode_selection, now),
            show_omarchy_card(),
        );
        let support_prompt = crate::support_prompt::load_for_daemon(onboarding.state.completed_at);
        let (mut connection, selected, status) =
            Self::resolve_startup_state(&config, default_selected);
        if stale_cleanup_failed || routing_error.is_some() {
            connection = ConnectionState::Idle;
        }
        let connecting_profile_id = (connection == ConnectionState::Connecting)
            .then_some(config.settings.last_connected_profile)
            .flatten();

        let (geo_schedule_epoch, rule_set_states) =
            load_rule_set_states(&config.settings, &routing);

        let mut model = Self {
            overlay: Overlay::None,
            settings_menu_selected: 0,
            routing_settings_draft: None,
            connection_settings_draft: None,
            interface_settings_draft: None,
            main_pane_focus: MainPaneFocus::Sources,
            connection,
            config,
            routing,
            routing_error,
            config_persistence_blocked,
            unsaved_state_merge_base: None,
            support_prompt,
            onboarding,
            integration_setup: crate::onboarding::IntegrationSetup::default(),
            support_selected: 0,
            sources_scroll: 0,
            overlay_scroll: None,
            scroll_result: None,
            selected,
            status: None,
            status_revision: 0,
            status_events: VecDeque::new(),
            singbox_pid: None,
            active_profile_id: None,
            connecting_profile_id,
            connect_attempt_id: u64::from(connecting_profile_id.is_some()),
            kill_switch_pending: None,
            auto_connect_pending: false,
            routing_selected: 0,
            geo_region_selected: 0,
            dns_selected: 0,
            dns_preset_draft: None,
            dns_fakeip_draft: None,
            routing_fallback: None,
            logs: VecDeque::new(),
            rule_set_updates: HashMap::new(),
            subscription_updates: HashMap::new(),
            needs_redraw: false,
            should_quit: false,
            rule_set_states,
            rule_set_automatic_attempts: HashMap::new(),
            geo_schedule_epoch,
            traffic: TrafficStats::default(),
            last_traffic_sample_at_ms: 0,
            traffic_request_id: 0,
            last_traffic_response_id: 0,
            last_traffic_fetch_at: None,
            last_integration_check_at: None,
            integration_check_pending: false,
            theme: Theme::legacy(),
            theme_draft: None,
            profile_latencies: HashMap::new(),
            testing_profiles: HashSet::new(),
            pending_tests: VecDeque::new(),
        };
        if !model.onboarding.is_complete() {
            model.overlay = Overlay::Onboarding(model.onboarding.step());
        }
        if let Some(status) = status {
            model.set_status(StatusKey::Connection, status);
        }
        if let Some(msg) = startup_error {
            crate::services::log_tailer::append_app_log("ERROR", &msg);
            model.set_status(StatusKey::Config, AppStatus::Error(msg));
        }
        Ok(model)
    }

    pub fn from_config(config: Config) -> Self {
        let selected = 0;

        Self {
            overlay: Overlay::None,
            settings_menu_selected: 0,
            routing_settings_draft: None,
            connection_settings_draft: None,
            interface_settings_draft: None,
            main_pane_focus: MainPaneFocus::Sources,
            connection: ConnectionState::Idle,
            config,
            routing: RoutingConfig::default(),
            routing_error: None,
            config_persistence_blocked: false,
            unsaved_state_merge_base: None,
            support_prompt: crate::support_prompt::SupportPromptState::default(),
            onboarding: crate::onboarding::OnboardingProgress::new(
                crate::onboarding::OnboardingState::default(),
                show_omarchy_card(),
            ),
            integration_setup: crate::onboarding::IntegrationSetup::default(),
            support_selected: 0,
            sources_scroll: 0,
            overlay_scroll: None,
            scroll_result: None,
            selected,
            status: None,
            status_revision: 0,
            status_events: VecDeque::new(),
            singbox_pid: None,
            active_profile_id: None,
            connecting_profile_id: None,
            connect_attempt_id: 0,
            kill_switch_pending: None,
            auto_connect_pending: false,
            routing_selected: 0,
            geo_region_selected: 0,
            dns_selected: 0,
            dns_preset_draft: None,
            dns_fakeip_draft: None,
            routing_fallback: None,
            logs: VecDeque::new(),
            rule_set_updates: HashMap::new(),
            subscription_updates: HashMap::new(),
            needs_redraw: false,
            should_quit: false,
            rule_set_states: crate::geo::RuleSetStates::new(),
            rule_set_automatic_attempts: HashMap::new(),
            geo_schedule_epoch: 0,
            traffic: TrafficStats::default(),
            last_traffic_sample_at_ms: 0,
            traffic_request_id: 0,
            last_traffic_response_id: 0,
            last_traffic_fetch_at: None,
            last_integration_check_at: None,
            integration_check_pending: false,
            theme: Theme::legacy(),
            theme_draft: None,
            profile_latencies: HashMap::new(),
            testing_profiles: HashSet::new(),
            pending_tests: VecDeque::new(),
        }
    }

    fn resolve_startup_state(
        config: &Config,
        default_selected: usize,
    ) -> (ConnectionState, usize, Option<AppStatus>) {
        if config.settings.auto_connect
            && let Some(idx) = config
                .settings
                .last_connected_profile
                .and_then(|id| config.profiles.iter().position(|p| p.id == id))
        {
            let status = config
                .profiles
                .get(idx)
                .map(|profile| AppStatus::Info(format!("Auto-connecting to {}…", profile.name)));
            return (
                ConnectionState::Connecting,
                row_for_profile(config, idx),
                status,
            );
        }
        (ConnectionState::Idle, default_selected, None)
    }

    pub fn source_rows(&self) -> Vec<SourceRow> {
        source_rows(&self.config)
    }

    pub fn selected_row(&self) -> Option<SourceRow> {
        self.source_rows().get(self.selected).copied()
    }

    pub fn select_next(&mut self) {
        let len = self.source_rows().len();
        crate::ui::nav::select_next(&mut self.selected, len);
    }

    pub fn select_prev(&mut self) {
        crate::ui::nav::select_prev(&mut self.selected);
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn select_last(&mut self) {
        let len = self.source_rows().len();
        self.selected = len.saturating_sub(1);
    }

    pub fn selected_profile(&self) -> Option<&Profile> {
        match self.selected_row()? {
            SourceRow::StandaloneProfile(idx) => self.config.profiles.get(idx),
            SourceRow::SubscriptionProfile { profile_idx, .. } => {
                self.config.profiles.get(profile_idx)
            }
            _ => None,
        }
    }

    pub fn selected_subscription(&self) -> Option<&crate::config::profile::Subscription> {
        match self.selected_row()? {
            SourceRow::SubscriptionHeader(idx) => self.config.subscriptions.get(idx),
            _ => None,
        }
    }

    pub fn selected_subscription_index(&self) -> Option<usize> {
        match self.selected_row()? {
            SourceRow::SubscriptionHeader(idx) => Some(idx),
            _ => None,
        }
    }

    pub fn icon_set(&self) -> crate::config::profile::IconSet {
        self.interface_settings_draft
            .unwrap_or(self.config.settings.icons)
    }

    pub fn connection_settings(&self) -> ConnectionSettings {
        ConnectionSettings {
            auto_connect: self.config.settings.auto_connect || self.auto_connect_pending,
            kill_switch: self
                .kill_switch_pending
                .unwrap_or(self.config.settings.kill_switch),
        }
    }

    pub fn settings_menu_len(&self, page: SettingsMenuPage) -> usize {
        match page {
            SettingsMenuPage::Root => SettingsMenuEntry::ALL.len(),
            SettingsMenuPage::Routing => 1,
            SettingsMenuPage::Connection | SettingsMenuPage::Interface => 2,
        }
    }

    pub fn shown_routing_mode(&self) -> &str {
        self.routing_settings_draft
            .as_ref()
            .and_then(|draft| draft.mode.as_deref())
            .unwrap_or_else(|| self.config.settings.geo_routing.mode(&self.routing))
    }

    pub fn shown_connection_settings(&self) -> ConnectionSettings {
        let live = self.connection_settings();
        let draft = self.connection_settings_draft.unwrap_or_default();
        ConnectionSettings {
            auto_connect: draft.auto_connect.unwrap_or(live.auto_connect),
            kill_switch: draft.kill_switch.unwrap_or(live.kill_switch),
        }
    }

    pub fn replace_config_preserving_selection(&mut self, config: Config) {
        let selected_row = self.selected;
        let selected_profile_id = self.selected_profile().map(|profile| profile.id);
        let selected_subscription_id = self.selected_subscription().map(|sub| sub.id);

        self.config = config;
        self.selected = selected_profile_id
            .and_then(|id| {
                self.config
                    .profiles
                    .iter()
                    .position(|profile| profile.id == id)
            })
            .map(|idx| row_for_profile(&self.config, idx))
            .or_else(|| {
                selected_subscription_id
                    .and_then(|id| {
                        self.config
                            .subscriptions
                            .iter()
                            .position(|sub| sub.id == id)
                    })
                    .map(|idx| row_for_subscription_header(&self.config, idx))
            })
            .unwrap_or_else(|| selected_row.min(self.source_rows().len().saturating_sub(1)));
    }

    pub fn delete_selected(&mut self) {
        let rows = self.source_rows();
        let Some(row) = rows.get(self.selected).copied() else {
            return;
        };
        match row {
            SourceRow::StandaloneProfile(idx)
            | SourceRow::SubscriptionProfile {
                profile_idx: idx, ..
            } => {
                if idx < self.config.profiles.len() {
                    self.config.profiles.remove(idx);
                }
            }
            SourceRow::SubscriptionHeader(idx) => {
                if let Some(sub) = self.config.subscriptions.get(idx) {
                    let sub_id = sub.id;
                    self.config
                        .profiles
                        .retain(|p| p.subscription_id != Some(sub_id));
                    self.config.subscriptions.remove(idx);
                }
            }
        }
        self.overlay = Overlay::None;
        let len = self.source_rows().len();
        if self.selected >= len && len > 0 {
            self.selected = len - 1;
        }
    }

    pub fn add_profile(&mut self, profile: Profile) {
        self.config.profiles.push(profile);
        self.selected = row_for_profile(&self.config, self.config.profiles.len() - 1);
    }

    pub fn has_duplicate(&self, profile: &Profile) -> bool {
        let key = profile.dedup_key();
        self.config.profiles.iter().any(|p| p.dedup_key() == key)
    }
}

impl Model {
    pub(crate) fn in_memory(config: Config) -> Self {
        let selected = 0;
        Self {
            overlay: Overlay::None,
            settings_menu_selected: 0,
            routing_settings_draft: None,
            connection_settings_draft: None,
            interface_settings_draft: None,
            main_pane_focus: MainPaneFocus::Sources,
            connection: ConnectionState::Idle,
            config,
            routing: RoutingConfig::default(),
            routing_error: None,
            config_persistence_blocked: false,
            unsaved_state_merge_base: None,
            support_prompt: crate::support_prompt::SupportPromptState::default(),
            onboarding: crate::onboarding::OnboardingProgress::default(),
            integration_setup: crate::onboarding::IntegrationSetup::default(),
            support_selected: 0,
            sources_scroll: 0,
            overlay_scroll: None,
            scroll_result: None,
            selected,
            status: None,
            status_revision: 0,
            status_events: VecDeque::new(),
            singbox_pid: None,
            active_profile_id: None,
            connecting_profile_id: None,
            connect_attempt_id: 0,
            kill_switch_pending: None,
            auto_connect_pending: false,
            routing_selected: 0,
            geo_region_selected: 0,
            dns_selected: 0,
            dns_preset_draft: None,
            dns_fakeip_draft: None,
            routing_fallback: None,
            logs: VecDeque::new(),
            rule_set_updates: HashMap::new(),
            subscription_updates: HashMap::new(),
            needs_redraw: false,
            should_quit: false,
            rule_set_states: HashMap::new(),
            rule_set_automatic_attempts: HashMap::new(),
            geo_schedule_epoch: 0,
            traffic: TrafficStats::default(),
            last_traffic_sample_at_ms: 0,
            traffic_request_id: 0,
            last_traffic_response_id: 0,
            last_traffic_fetch_at: None,
            last_integration_check_at: None,
            integration_check_pending: false,
            theme: Theme::legacy(),
            theme_draft: None,
            profile_latencies: HashMap::new(),
            testing_profiles: HashSet::new(),
            pending_tests: VecDeque::new(),
        }
    }
}

#[cfg(test)]
impl Model {
    pub fn test_new(config: Config) -> Self {
        let mut model = Self::in_memory(config);
        model.routing = RoutingConfig::for_new_file(&Default::default());
        model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;

    #[test]
    fn settings_menu_overlay_serde_roundtrip() {
        for page in [
            SettingsMenuPage::Root,
            SettingsMenuPage::Routing,
            SettingsMenuPage::Connection,
        ] {
            let overlay = Overlay::SettingsMenu(page);
            let json = serde_json::to_string(&overlay).unwrap();
            assert_eq!(serde_json::from_str::<Overlay>(&json).unwrap(), overlay);
        }
    }

    #[test]
    fn confirm_disable_overlay_serde_roundtrip() {
        for target in [DisableTarget::AutoConnect, DisableTarget::KillSwitch] {
            let overlay = Overlay::ConfirmDisable(target);
            let json = serde_json::to_string(&overlay).unwrap();
            assert_eq!(serde_json::from_str::<Overlay>(&json).unwrap(), overlay);
            assert_eq!(
                HelpContext::ConfirmDisable(target).restore_overlay(),
                overlay
            );
        }
    }

    #[test]
    fn select_next_basic() {
        let mut model = model_with_profiles(sample_profiles());
        assert_eq!(model.selected, 0);
        model.select_next();
        assert_eq!(model.selected, 1);
        model.select_next();
        assert_eq!(model.selected, 2);
        model.select_next();
        assert_eq!(model.selected, 2);
    }

    #[test]
    fn select_prev_basic() {
        let mut model = model_with_profiles(sample_profiles());
        model.selected = 2;
        model.select_prev();
        assert_eq!(model.selected, 1);
        model.select_prev();
        assert_eq!(model.selected, 0);
        model.select_prev();
        assert_eq!(model.selected, 0);
    }

    #[test]
    fn select_first_last() {
        let mut model = model_with_profiles(sample_profiles());
        model.selected = 1;
        model.select_first();
        assert_eq!(model.selected, 0);
        model.select_last();
        assert_eq!(model.selected, 2);
    }

    #[test]
    fn select_next_empty_list() {
        let mut model = model_with_profiles(vec![]);
        model.select_next();
        assert_eq!(model.selected, 0);
    }

    #[test]
    fn selected_profile_some_and_none() {
        let mut model = model_with_profiles(sample_profiles());
        assert_eq!(model.selected_profile().unwrap().name, "A");
        model.config.profiles.clear();
        assert!(model.selected_profile().is_none());
    }

    #[test]
    fn delete_selected_basic() {
        let mut model = model_with_profiles(sample_profiles());
        model.selected = 1;
        model.delete_selected();
        assert_eq!(model.config.profiles.len(), 2);
        assert_eq!(model.config.profiles[0].name, "A");
        assert_eq!(model.config.profiles[1].name, "C");
        assert_eq!(model.selected, 1);
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn delete_selected_last_item() {
        let mut model = model_with_profiles(sample_profiles());
        model.selected = 2;
        model.delete_selected();
        assert_eq!(model.config.profiles.len(), 2);
        assert_eq!(model.selected, 1);
    }

    #[test]
    fn delete_selected_only_item() {
        let mut model = model_with_profiles(vec![Profile::new_vless(
            "Only".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u".to_string(),
        )]);
        model.selected = 0;
        model.delete_selected();
        assert!(model.config.profiles.is_empty());
        assert_eq!(model.selected, 0);
    }

    #[test]
    fn add_profile_updates_state() {
        let mut model = model_with_profiles(sample_profiles());
        let p = Profile::new_vless(
            "D".to_string(),
            "4.4.4.4".to_string(),
            443,
            "u4".to_string(),
        );
        model.add_profile(p);
        assert_eq!(model.config.profiles.len(), 4);
        assert_eq!(model.selected, 3);
    }

    #[test]
    fn from_snapshot_maps_an_empty_status_to_none() {
        assert_eq!(AppStatus::from_snapshot(String::new(), false), None);
        assert_eq!(AppStatus::from_snapshot(String::new(), true), None);
        assert_eq!(
            AppStatus::from_snapshot("busy".into(), false),
            Some(AppStatus::Info("busy".into()))
        );
        assert_eq!(
            AppStatus::from_snapshot("boom".into(), true),
            Some(AppStatus::Error("boom".into()))
        );
    }

    #[test]
    fn from_snapshot_round_trips_the_wire_representation() {
        for status in [
            None,
            Some(AppStatus::Info("busy".into())),
            Some(AppStatus::Error("boom".into())),
        ] {
            let text = status.as_ref().map_or("", AppStatus::text).to_string();
            let is_error = matches!(status, Some(AppStatus::Error(_)));
            assert_eq!(AppStatus::from_snapshot(text, is_error), status);
        }
    }

    #[test]
    fn set_status_clears_error_and_mode() {
        let mut model = model_with_profiles(vec![]);
        model.status = Some(AppStatus::Error("oops".into()));
        model.status = Some(AppStatus::Info("ok".into()));
        assert_eq!(model.status_text(), "ok");
        assert!(!model.status_is_error());
    }

    #[test]
    fn set_error_sets_message_and_mode() {
        let mut model = model_with_profiles(vec![]);
        model.status = Some(AppStatus::Error("fail".into()));
        assert_eq!(model.status_text(), "fail");
        assert!(model.status_is_error());
    }

    #[test]
    fn resolve_startup_state_auto_connect() {
        let mut config = Config::default();
        let profile = Profile::new_vless(
            "Auto".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        );
        let id = profile.id;
        config.profiles.push(profile);
        config.settings.auto_connect = true;
        config.settings.last_connected_profile = Some(id);

        let (state, selected, status) = Model::resolve_startup_state(&config, 0);
        assert_eq!(state, ConnectionState::Connecting);
        assert_eq!(selected, 0);
        assert!(status.is_some_and(|status| status.text().contains("Auto-connecting")));
    }

    #[test]
    fn resolve_startup_state_auto_connect_missing_profile() {
        let mut config = Config::default();
        config.settings.auto_connect = true;
        config.settings.last_connected_profile = Some(Uuid::new_v4());
        config.profiles.push(Profile::new_vless(
            "A".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        ));

        let (state, selected, status) = Model::resolve_startup_state(&config, 0);
        assert_eq!(state, ConnectionState::Idle);
        assert_eq!(selected, 0);
        assert!(status.is_none());
    }

    #[test]
    fn resolve_startup_state_no_auto_connect() {
        let config = Config::default();
        let (state, selected, status) = Model::resolve_startup_state(&config, 0);
        assert_eq!(state, ConnectionState::Idle);
        assert_eq!(selected, 0);
        assert!(status.is_none());
    }

    #[test]
    fn a_v7_region_becomes_the_selected_mode_after_routing_json_holds_its_modes() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _config_home = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", dir.path());
        let state_dir = tempfile::tempdir().unwrap();
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", state_dir.path());
        crate::paths::ensure_config_dirs().unwrap();
        let profiles = crate::paths::profiles_path().unwrap();
        std::fs::write(
            &profiles,
            r#"{ "schema_version": 7, "settings": { "geo_routing": {
                "current_region": "cn",
                "selected_region_modes": { "ru": "bypass_ru", "cn": "only_cn" }
            } } }"#,
        )
        .unwrap();

        let model = Model::new().unwrap();

        assert_eq!(
            model.config.settings.geo_routing.mode(&model.routing),
            "only_cn"
        );
        let on_disk = crate::config::routing::load_routing_at(
            &crate::paths::routing_path().unwrap(),
            &Default::default(),
            false,
        )
        .unwrap();
        assert_eq!(
            on_disk.mode_ids(),
            ["global", "bypass_ru", "only_ru", "bypass_cn", "only_cn"]
        );
        assert_eq!(model.routing, on_disk);
        assert!(!model.config.settings.geo_routing.predates_mode_selection());
        assert!(model.onboarding.is_complete());
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&profiles).unwrap()).unwrap();
        assert_eq!(saved["schema_version"], 8);
        assert_eq!(saved["settings"]["geo_routing"]["mode"], "only_cn");
    }

    #[test]
    fn a_completed_tour_without_a_region_opens_the_main_screen() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _config_home = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", dir.path());
        let state_dir = tempfile::tempdir().unwrap();
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", state_dir.path());

        let mut ended = crate::onboarding::OnboardingState::default();
        ended.complete(chrono::Utc::now());
        crate::onboarding::save_at(&crate::paths::onboarding_path().unwrap(), &ended).unwrap();

        let model = Model::new().unwrap();
        assert_eq!(model.config.settings.geo_routing.mode, "global");
        assert_eq!(model.overlay, Overlay::None);
    }

    #[test]
    fn the_omarchy_card_is_dropped_once_the_plugin_is_installed() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let state = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", state.path());
        let _config_home = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", config.path());

        let current = state.path().join("omarchy/current");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("theme.name"), "tokyo-night\n").unwrap();
        assert!(
            Model::from_config(Config::default())
                .onboarding
                .include_omarchy_card
        );

        let plugin = config
            .path()
            .join("omarchy/plugins")
            .join(crate::omarchy::OMAKVN_PLUGIN_ID);
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("manifest.json"),
            format!(r#"{{"id":"{}"}}"#, crate::omarchy::OMAKVN_PLUGIN_ID),
        )
        .unwrap();
        assert!(
            !Model::from_config(Config::default())
                .onboarding
                .include_omarchy_card
        );
    }

    #[test]
    fn new_surfaces_invalid_config_in_status_and_log() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", dir.path());

        crate::paths::ensure_config_dirs().unwrap();

        let path = crate::paths::profiles_path().unwrap();
        let json = r#"{
            "schema_version": 5,
            "profiles": [{
                "name": "Broken",
                "protocol": "vless",
                "address": "1.2.3.4",
                "port": 443,
                "uuid": "not-a-uuid"
            }]
        }"#;
        std::fs::write(&path, json).unwrap();

        let model = Model::new().unwrap();
        assert!(
            model.status_is_error(),
            "status not error: {:?}",
            model.status
        );
        assert!(
            model.status_text().contains("Configuration load failed"),
            "status was: {}",
            model.status_text(),
        );
        assert!(model.config.profiles.is_empty());
        assert!(
            model
                .logs
                .iter()
                .any(|l| l.contains("Configuration load failed")),
            "logs: {:?}",
            model.logs,
        );
        let on_disk = std::fs::read_to_string(crate::paths::app_log_path()).unwrap();
        assert!(
            on_disk.contains("Configuration load failed") && on_disk.contains("ERROR"),
            "app.log was: {on_disk:?}",
        );
    }

    #[test]
    fn an_invalid_routing_json_blocks_auto_connect_instead_of_running_on_defaults() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _config_home = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", dir.path());
        let state_dir = tempfile::tempdir().unwrap();
        let _state_home = crate::test_helpers::EnvVarGuard::set("XDG_STATE_HOME", state_dir.path());
        crate::paths::ensure_config_dirs().unwrap();
        let profile = crate::config::profile::Profile::new_vless(
            "A".into(),
            "1.1.1.1".into(),
            443,
            uuid::Uuid::new_v4().to_string(),
        );
        let mut config = Config::default();
        config.settings.auto_connect = true;
        config.settings.last_connected_profile = Some(profile.id);
        config.settings.geo_routing.set_mode("bypass_ru");
        config.profiles.push(profile);
        crate::config::save_config_at(&crate::paths::profiles_path().unwrap(), &config).unwrap();
        let routing = crate::paths::routing_path().unwrap();
        std::fs::write(&routing, "{ not json").unwrap();

        let model = Model::new().unwrap();

        assert!(model.routing_error.is_some());
        assert_eq!(model.connection, ConnectionState::Idle);
        assert_eq!(model.connecting_profile_id, None);
        assert_eq!(model.routing.mode_ids(), ["global"]);
        assert!(
            model
                .status_text()
                .contains("connecting is blocked until routing.json is fixed"),
            "status was: {}",
            model.status_text(),
        );
        assert_eq!(std::fs::read_to_string(routing).unwrap(), "{ not json");
    }

    #[test]
    fn new_surfaces_load_error_when_json_is_malformed() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", dir.path());

        let path = crate::paths::profiles_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json at all").unwrap();

        let model = Model::new().unwrap();
        assert!(model.status_is_error());
        assert!(
            model.status_text().contains("Configuration load failed"),
            "status was: {}",
            model.status_text(),
        );
        assert!(
            model
                .logs
                .iter()
                .any(|l| l.contains("Configuration load failed")),
            "logs: {:?}",
            model.logs,
        );
        assert!(model.config_persistence_blocked);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "not json at all");
    }

    #[test]
    fn push_log_truncates() {
        let mut model = Model::test_new(Config::default());
        for i in 0..1005 {
            model.push_log(format!("line {}", i));
        }
        assert_eq!(model.logs.len(), 1000);
        assert_eq!(model.logs[0], "line 5");
        assert_eq!(model.logs[999], "line 1004");
    }

    #[test]
    fn set_status_logs_plain_text() {
        let mut model = Model::test_new(Config::default());
        let initial_revision = model.status_revision;
        model.set_status(StatusKey::Connection, AppStatus::Info("hello".into()));
        assert_eq!(model.status_text(), "hello");
        assert_eq!(model.logs.back().unwrap(), "hello");
        assert_eq!(model.status_revision, initial_revision + 1);

        model.set_status(StatusKey::Connection, AppStatus::Error("oops".into()));
        assert_eq!(model.status_text(), "oops");
        assert_eq!(model.logs.back().unwrap(), "oops");
        assert_eq!(model.status_revision, initial_revision + 2);
    }

    #[test]
    fn set_status_keeps_the_latest_event_of_each_key() {
        let mut model = Model::test_new(Config::default());
        model.set_status(StatusKey::Connection, AppStatus::Info("Connecting…".into()));
        model.set_status(StatusKey::Geo, AppStatus::Error("Geo update failed".into()));
        model.set_status(StatusKey::Connection, AppStatus::Info("Connected".into()));

        let events: Vec<_> = model
            .status_events
            .iter()
            .map(|event| (event.revision, event.key, event.status.text()))
            .collect();
        assert_eq!(
            events,
            vec![
                (2, StatusKey::Geo, "Geo update failed"),
                (3, StatusKey::Connection, "Connected"),
            ]
        );
    }

    #[test]
    fn set_status_drops_the_oldest_event_beyond_capacity() {
        let mut model = Model::test_new(Config::default());
        for index in 0..=STATUS_EVENT_CAPACITY {
            model.set_status(
                StatusKey::ConnectionWarning(index),
                AppStatus::Info(format!("warning {index}")),
            );
        }

        assert_eq!(model.status_events.len(), STATUS_EVENT_CAPACITY);
        assert_eq!(
            model.status_events.front().unwrap().status.text(),
            "warning 1"
        );
    }

    #[test]
    fn empty_status_replaces_its_key_without_an_event() {
        let mut model = Model::test_new(Config::default());
        model.set_status(
            StatusKey::Sources,
            AppStatus::Info("Profile imported".into()),
        );
        model.set_status(StatusKey::Sources, AppStatus::Info(String::new()));

        assert!(model.status_events.is_empty());
    }

    #[test]
    fn clear_error_status_requires_the_current_revision() {
        let mut model = Model::test_new(Config::default());
        model.set_status(StatusKey::Connection, AppStatus::Error("first".into()));
        let revision = model.status_revision;

        assert!(!model.clear_error_status(revision.wrapping_sub(1)));
        assert_eq!(model.status_text(), "first");
        assert!(model.clear_error_status(revision));
        assert!(model.status.is_none());
        assert_eq!(model.status_revision, revision);
        assert!(!model.clear_error_status(revision));
    }

    #[test]
    fn set_status_does_not_log_empty() {
        let mut model = Model::test_new(Config::default());
        model.set_status(StatusKey::Connection, AppStatus::Info("".into()));
        assert!(model.logs.is_empty());
    }
}

use std::collections::{HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::subscription::validate_hwid;
use super::{
    ConfigDiagnostic, GeoAutoUpdate, OMARCHY_THEME_SENTINEL, Profile, Settings, Subscription,
    into_result,
};

pub const CURRENT_SCHEMA_VERSION: u32 = 7;

fn default_schema_version() -> u32 {
    0
}

/// Root configuration file structure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = super::json_schema::pin_current_schema_version)]
pub struct Config {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<String>,
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<Subscription>,
    #[serde(default = "Settings::for_this_desktop")]
    #[schemars(transform = super::json_schema::without_default)]
    pub settings: Settings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            json_schema: None,
            schema_version: CURRENT_SCHEMA_VERSION,
            profiles: Vec::new(),
            subscriptions: Vec::new(),
            settings: Settings::default(),
        }
    }
}

impl Config {
    pub fn for_first_run() -> Self {
        let mut config = Self {
            settings: Settings::for_this_desktop(),
            ..Self::default()
        };
        config.settings.geo_routing.auto_update = GeoAutoUpdate::Every7d;
        if crate::omarchy::detect_omarchy_theme().is_some() {
            config.settings.theme = OMARCHY_THEME_SENTINEL.to_string();
        }
        config
    }

    pub(crate) fn normalize(&mut self) -> bool {
        let tun_interface_trimmed = self.trim_tun_interface();
        let profiles_sorted = self.sort_profiles_by_source();
        tun_interface_trimmed || profiles_sorted
    }

    fn trim_tun_interface(&mut self) -> bool {
        let tun_interface = self.settings.tun_interface.trim();
        if tun_interface == self.settings.tun_interface {
            return false;
        }
        self.settings.tun_interface = tun_interface.to_string();
        true
    }

    fn sort_profiles_by_source(&mut self) -> bool {
        let subscription_ranks: HashMap<Uuid, usize> = self
            .subscriptions
            .iter()
            .enumerate()
            .map(|(position, subscription)| (subscription.id, position + 1))
            .collect();
        let source_rank = |profile: &Profile| match profile.subscription_id {
            None => 0,
            Some(id) => subscription_ranks.get(&id).copied().unwrap_or(usize::MAX),
        };
        if self.profiles.is_sorted_by_key(source_rank) {
            return false;
        }
        self.profiles.sort_by_key(source_rank);
        true
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        into_result(self.diagnostics())
    }

    pub fn diagnostics(&self) -> Vec<ConfigDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut profile_ids = HashSet::with_capacity(self.profiles.len());
        for (idx, profile) in self.profiles.iter().enumerate() {
            let pointer = format!("/profiles/{idx}");
            let label = format!("Profile {}", idx + 1);
            if !profile_ids.insert(profile.id) {
                diagnostics.push(ConfigDiagnostic::new(
                    format!("{pointer}/id"),
                    format!("{label}: duplicate id {}", profile.id),
                ));
            }
            diagnostics.extend(
                profile
                    .diagnostics()
                    .into_iter()
                    .map(|d| d.within(&pointer).labelled(&label)),
            );
        }

        let mut subscription_ids = HashSet::with_capacity(self.subscriptions.len());
        for (idx, subscription) in self.subscriptions.iter().enumerate() {
            let pointer = format!("/subscriptions/{idx}");
            if !subscription_ids.insert(subscription.id) {
                diagnostics.push(ConfigDiagnostic::new(
                    format!("{pointer}/id"),
                    format!("Subscription {}: duplicate id {}", idx + 1, subscription.id),
                ));
            }
            if !subscription.send_hwid {
                continue;
            }

            let hwid = subscription
                .effective_hwid(&self.settings)
                .unwrap_or_default();
            if let Err(error) = validate_hwid(hwid) {
                let hwid_pointer = match subscription.hwid {
                    Some(_) => format!("{pointer}/hwid"),
                    None => "/settings/hwid".to_string(),
                };
                diagnostics.push(ConfigDiagnostic::new(
                    hwid_pointer,
                    format!(
                        "Subscription {} ({:?}): invalid HWID: {error}",
                        idx + 1,
                        subscription.name
                    ),
                ));
            }
        }

        for (idx, profile) in self.profiles.iter().enumerate() {
            if let Some(id) = profile.subscription_id
                && !subscription_ids.contains(&id)
            {
                diagnostics.push(ConfigDiagnostic::new(
                    format!("/profiles/{idx}/subscription_id"),
                    format!(
                        "Profile {}: subscription_id ({id}) references a non-existent subscription",
                        idx + 1
                    ),
                ));
            }
        }

        diagnostics.extend(
            self.settings
                .diagnostics()
                .into_iter()
                .map(|d| d.within("/settings")),
        );
        diagnostics.extend(
            self.settings
                .dns
                .diagnostics()
                .into_iter()
                .map(|d| d.within("/settings/dns")),
        );

        diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::*;
    use crate::test_helpers::subscription_with_hwid;
    use uuid::Uuid;

    #[test]
    fn normalize_orders_profiles_standalone_then_by_subscription() {
        let first = subscription_with_hwid(false, None);
        let second = subscription_with_hwid(false, None);
        let profile = |name: &str, subscription_id: Option<Uuid>| {
            let mut profile = Profile::new_vless(name.into(), "h".into(), 1, "u".into());
            profile.subscription_id = subscription_id;
            profile
        };
        let mut config = Config {
            profiles: vec![
                profile("Orphan", Some(Uuid::new_v4())),
                profile("Q1", Some(second.id)),
                profile("P1", Some(first.id)),
                profile("A", None),
                profile("P2", Some(first.id)),
                profile("B", None),
            ],
            subscriptions: vec![first, second],
            ..Config::default()
        };

        assert!(config.normalize());
        let names: Vec<&str> = config.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["A", "B", "P1", "P2", "Q1", "Orphan"]);
        assert!(!config.normalize());
    }

    #[test]
    fn defaulted_icons_follow_the_desktop() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let state = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", state.path()) };
        let defaulted = || {
            [
                Config::for_first_run(),
                serde_json::from_str("{}").unwrap(),
                serde_json::from_str(r#"{"settings": {}}"#).unwrap(),
            ]
            .map(|config| config.settings.icons)
        };

        assert_eq!(defaulted(), [IconSet::Unicode; 3]);

        let current = state.path().join("omarchy").join("current");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("theme.name"), "gruvbox\n").unwrap();
        assert_eq!(defaulted(), [IconSet::Nerd; 3]);
    }

    #[test]
    fn first_run_theme_follows_omarchy() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let state = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", state.path()) };
        assert_eq!(Config::for_first_run().settings.theme, "tokyo-night");

        let current = state.path().join("omarchy").join("current");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("theme.name"), "catppuccin-mocha\n").unwrap();
        assert_eq!(
            Config::for_first_run().settings.theme,
            OMARCHY_THEME_SENTINEL
        );
    }

    #[test]
    fn first_run_preferences_never_leak_into_a_parsed_config() {
        assert_eq!(
            Config::for_first_run().settings.geo_routing.auto_update,
            GeoAutoUpdate::Every7d
        );
        for json in [
            r#"{"settings": {"geo_routing": {"auto_update": "off"}}}"#,
            r#"{"settings": {"geo_routing": {}}}"#,
            r#"{"settings": {}}"#,
            r#"{}"#,
        ] {
            let parsed: Config = serde_json::from_str(json).unwrap();
            assert_eq!(
                parsed.settings.geo_routing.auto_update,
                GeoAutoUpdate::Off,
                "{json}"
            );
        }
    }

    #[test]
    fn config_default() {
        let c = Config::default();
        assert!(c.profiles.is_empty());
        assert_eq!(c.settings.tun_interface, "kvn0");
    }

    #[test]
    fn config_serde_roundtrip() {
        let mut config = Config::default();
        let mut profile = Profile::new_vless(
            "Example".to_string(),
            "203.0.113.1".to_string(),
            443,
            "550e8400-e29b-41d4-a716-446655440000".to_string(),
        );
        if let ProtocolConfig::Vless(ref mut cfg) = profile.config {
            cfg.security = Some(Security::Reality);
            cfg.tls.reality = Some(RealitySettings {
                public_key: "pk".to_string(),
                short_id: "sid".to_string(),
                server_name: "sni".to_string(),
                spider_x: "/".to_string(),
            });
        }
        profile.tags = vec!["tag1".to_string()];
        config.profiles.push(profile);
        config.settings.geo_routing.set_region(GeoRegion::Ru);
        config
            .settings
            .geo_routing
            .set_mode(RoutingMode::Bypass(GeoRegion::Ru));

        let json = serde_json::to_string(&config).unwrap();
        let restored: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(config, restored);
    }

    #[test]
    fn config_serde_roundtrip_with_geo_routing() {
        let mut config = Config::default();
        config
            .settings
            .geo_routing
            .selected_region_modes
            .insert(GeoRegion::Ru, RoutingMode::Bypass(GeoRegion::Ru));
        config
            .settings
            .geo_routing
            .selected_region_modes
            .insert(GeoRegion::Cn, RoutingMode::Only(GeoRegion::Cn));
        config.settings.geo_routing.current_region = Some(GeoRegion::Ru);

        let json = serde_json::to_string(&config).unwrap();
        let restored: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(config, restored);
        assert_eq!(
            restored
                .settings
                .geo_routing
                .selected_region_modes
                .get(&GeoRegion::Ru)
                .copied()
                .unwrap_or(RoutingMode::Global),
            RoutingMode::Bypass(GeoRegion::Ru)
        );
        assert_eq!(
            restored
                .settings
                .geo_routing
                .selected_region_modes
                .get(&GeoRegion::Cn)
                .copied()
                .unwrap_or(RoutingMode::Global),
            RoutingMode::Only(GeoRegion::Cn)
        );
    }

    #[test]
    fn config_deserialize_missing_fields() {
        let json = r#"{}"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert!(c.profiles.is_empty());
        assert_eq!(c.settings.tun_interface, "kvn0");
    }

    #[test]
    fn config_rejects_unknown_top_level_field() {
        let json = r#"{"unknown_field": 42}"#;
        let result: Result<Config, _> = serde_json::from_str(json);
        assert!(result.is_err(), "Should reject unknown top-level field");
    }

    #[test]
    fn config_diagnostics_collects_every_problem_under_its_document_pointer() {
        let mut config = Config::default();
        let mut profile = Profile::new_vless(
            "".into(),
            "1.2.3.4".into(),
            0,
            crate::test_helpers::TEST_UUID.into(),
        );
        profile.subscription_id = Some(uuid::Uuid::nil());
        config.profiles.push(profile);
        config.settings.tun_interface = "tun0".into();
        config.settings.dns.current_preset = "missing".into();
        let pointers: Vec<_> = config
            .diagnostics()
            .into_iter()
            .map(|diagnostic| diagnostic.pointer)
            .collect();
        assert_eq!(
            pointers,
            [
                "/profiles/0/name",
                "/profiles/0/port",
                "/profiles/0/subscription_id",
                "/settings/tun_interface",
                "/settings/dns/current_preset",
            ]
        );
        let error = config.validate().unwrap_err().to_string();
        assert!(error.starts_with("Profile 1: name must not be empty; Profile 1: port"));
    }

    #[test]
    fn config_validate_accepts_valid_config() {
        let mut config = Config::default();
        config.profiles.push(Profile::new_vless(
            "Valid".to_string(),
            "1.2.3.4".to_string(),
            443,
            crate::test_helpers::TEST_UUID.to_string(),
        ));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn config_validate_rejects_empty_profile_name() {
        let mut config = Config::default();
        config.profiles.push(Profile::new_vless(
            "   ".to_string(),
            "1.2.3.4".to_string(),
            443,
            "uuid".to_string(),
        ));
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("name must not be empty"), "Error was: {}", err);
    }

    #[test]
    fn config_validate_rejects_empty_profile_address() {
        let mut config = Config::default();
        config.profiles.push(Profile::new_vless(
            "Name".to_string(),
            "".to_string(),
            443,
            "uuid".to_string(),
        ));
        let err = config.validate().unwrap_err().to_string();
        assert!(
            err.contains("address must not be empty"),
            "Error was: {}",
            err
        );
    }

    #[test]
    fn config_validate_rejects_empty_profile_uuid() {
        let mut config = Config::default();
        config.profiles.push(Profile::new_vless(
            "Name".to_string(),
            "1.2.3.4".to_string(),
            443,
            "  ".to_string(),
        ));
        let err = config.validate().unwrap_err().to_string();
        assert!(
            err.contains("vless.uuid must not be empty"),
            "Error was: {}",
            err
        );
    }

    #[test]
    fn config_validate_rejects_reality_plus_ech() {
        let mut config = Config::default();
        let mut profile = Profile::new_vless(
            "RealityEch".to_string(),
            "1.2.3.4".to_string(),
            443,
            "uuid".to_string(),
        );
        if let ProtocolConfig::Vless(ref mut cfg) = profile.config {
            cfg.tls.reality = Some(RealitySettings::default());
            cfg.tls.ech = Some(EchSettings {
                enabled: true,
                config: Vec::new(),
            });
        }
        config.profiles.push(profile);
        let err = config.validate().unwrap_err().to_string();
        assert!(
            err.contains("tls.reality and tls.ech are mutually exclusive"),
            "Error was: {}",
            err
        );
    }

    #[test]
    fn save_config_at_rejects_invalid_config() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut config = Config::default();
        config.profiles.push(Profile::new_vless(
            "Broken".to_string(),
            "1.2.3.4".to_string(),
            443,
            "not-a-uuid".to_string(),
        ));
        let err = crate::config::save_config_at(file.path(), &config).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("Refusing to save"), "Error was: {}", msg);
    }

    #[test]
    fn config_validate_rejects_duplicate_profile_ids() {
        let profile = Profile::new_vless(
            "A".into(),
            "1.2.3.4".into(),
            443,
            crate::test_helpers::TEST_UUID.into(),
        );
        let config = Config {
            profiles: vec![profile.clone(), profile],
            ..Config::default()
        };

        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("duplicate id")
        );
    }

    #[test]
    fn config_validate_rejects_duplicate_subscription_ids() {
        let subscription = subscription_with_hwid(false, None);
        let config = Config {
            subscriptions: vec![subscription.clone(), subscription],
            ..Config::default()
        };

        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("duplicate id")
        );
    }

    #[test]
    fn config_validate_rejects_dangling_profile_subscription() {
        let mut profile = Profile::new_vless(
            "A".into(),
            "1.2.3.4".into(),
            443,
            crate::test_helpers::TEST_UUID.into(),
        );
        profile.subscription_id = Some(Uuid::new_v4());
        let config = Config {
            profiles: vec![profile],
            ..Config::default()
        };
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("subscription_id")
        );
    }
}

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::subscription::validate_hwid;
use super::{GeoAutoUpdate, Profile, Settings, Subscription};

/// Current schema version for `profiles.json`. Bumped on every breaking
/// change to the persisted shape; new migrations go in `Config::migrate`.
pub const CURRENT_SCHEMA_VERSION: u32 = 5;

fn default_schema_version() -> u32 {
    // Files written before the version was introduced are treated as v0 by
    // the explicit package migration path.
    0
}

/// Root configuration file structure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Schema version of the persisted file. See [`CURRENT_SCHEMA_VERSION`].
    /// Absent in pre-versioned files; defaults to 0 for the explicit package
    /// migration path.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<Subscription>,
    #[serde(default)]
    pub settings: Settings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            profiles: Vec::new(),
            subscriptions: Vec::new(),
            settings: Settings::default(),
        }
    }
}

impl Config {
    /// Preferences for a machine that has never run kvn. Deliberately separate
    /// from `Default`, which also backs every `#[serde(default)]` in the tree:
    /// a config file that merely omits a field or a whole section must keep its
    /// current meaning rather than silently gain background downloads.
    pub fn for_first_run() -> Self {
        let mut config = Self::default();
        config.settings.geo_routing.auto_update = GeoAutoUpdate::Every7d;
        config
    }

    /// Canonicalize user-provided values that tolerate surrounding whitespace.
    /// Returns whether any value changed.
    pub(crate) fn normalize(&mut self) -> bool {
        let tun_interface = self.settings.tun_interface.trim();
        if tun_interface == self.settings.tun_interface {
            return false;
        }
        self.settings.tun_interface = tun_interface.to_string();
        true
    }

    /// Validate semantic constraints that serde cannot enforce.
    ///
    /// Checks:
    /// - Profile and subscription IDs are unique.
    /// - Subscription-owned profiles reference an existing subscription.
    /// - Each profile has non-empty `name`, `address`, and `uuid`.
    /// - `settings.default_profile` references an existing profile if set.
    /// - DNS server tags are non-empty and unique; `dns.final_server` and every
    ///   `dns.rules[*].server` reference an existing tag; when `fakeip_enabled`
    ///   at least one server is of type `fakeip`.
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut profile_ids = HashSet::with_capacity(self.profiles.len());
        for (idx, profile) in self.profiles.iter().enumerate() {
            let num = idx + 1;
            if !profile_ids.insert(profile.id) {
                anyhow::bail!("Profile {num}: duplicate id {}", profile.id);
            }
            if profile.name.trim().is_empty() {
                anyhow::bail!("Profile {num}: name must not be empty");
            }
            if profile.address.trim().is_empty() {
                anyhow::bail!("Profile {num}: address must not be empty");
            }
            if let Err(e) = profile.config.validate() {
                anyhow::bail!("Profile {num}: {e}");
            }
            if let Err(e) = profile.validate_semantic() {
                anyhow::bail!("Profile {num}: {e}");
            }
        }

        let mut subscription_ids = HashSet::with_capacity(self.subscriptions.len());
        for (idx, subscription) in self.subscriptions.iter().enumerate() {
            if !subscription_ids.insert(subscription.id) {
                anyhow::bail!("Subscription {}: duplicate id {}", idx + 1, subscription.id);
            }
            if !subscription.send_hwid {
                continue;
            }

            let hwid = subscription
                .effective_hwid(&self.settings)
                .unwrap_or_default();
            if let Err(error) = validate_hwid(hwid) {
                anyhow::bail!(
                    "Subscription {} ({:?}): invalid HWID: {error}",
                    idx + 1,
                    subscription.name
                );
            }
        }

        for (idx, profile) in self.profiles.iter().enumerate() {
            if let Some(id) = profile.subscription_id
                && !subscription_ids.contains(&id)
            {
                anyhow::bail!(
                    "Profile {}: subscription_id ({id}) references a non-existent subscription",
                    idx + 1
                );
            }
        }

        if let Some(id) = self.settings.default_profile
            && !self.profiles.iter().any(|p| p.id == id)
        {
            anyhow::bail!("settings.default_profile ({id}) references a non-existent profile");
        }

        self.settings.validate()?;
        self.settings.dns.validate()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::*;
    use crate::test_helpers::subscription_with_hwid;
    use uuid::Uuid;

    #[test]
    fn first_run_preferences_never_leak_into_a_parsed_config() {
        assert_eq!(
            Config::for_first_run().settings.geo_routing.auto_update,
            GeoAutoUpdate::Every7d
        );
        // Every `#[serde(default)]` in the chain must keep meaning "off", at
        // whichever level the field or section is missing.
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
    fn config_validate_accepts_valid_config() {
        let mut config = Config::default();
        config.profiles.push(Profile::new_vless(
            "Valid".to_string(),
            "1.2.3.4".to_string(),
            443,
            crate::test_helpers::TEST_UUID.to_string(),
        ));
        config.settings.default_profile = Some(config.profiles[0].id);
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
        // Fail-close: save must run Config::validate first so a corrupted
        // in-memory state cannot overwrite a good profiles.json on disk.
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

    #[test]
    fn config_validate_rejects_dangling_default_profile() {
        let mut config = Config::default();
        config.settings.default_profile = Some(Uuid::new_v4());
        let err = config.validate().unwrap_err().to_string();
        assert!(
            err.contains("references a non-existent profile"),
            "Error was: {}",
            err
        );
    }
}

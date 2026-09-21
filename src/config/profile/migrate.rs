use chrono::{Local, NaiveDate};

use super::schedule::next_update_window_date;
use super::settings::default_tun_interface;
use super::{
    CURRENT_SCHEMA_VERSION, Config, ConnectivityProbeConfig, DnsStrategy, GeoAutoUpdate,
    ProtocolConfig, SubscriptionAutoUpdate,
};

impl Config {
    /// Apply schema migrations needed to bring an explicitly staged config up
    /// to [`CURRENT_SCHEMA_VERSION`]. Package migration scripts call this via
    /// `kvn config migrate`; ordinary config loading never calls it. Each
    /// migration step is idempotent.
    ///
    /// Files written by a newer kvn version (higher `schema_version` than
    /// this build knows about) are rejected here — loading them would silently
    /// drop future-only fields; the user must upgrade the client instead.
    pub fn migrate(&mut self) -> anyhow::Result<()> {
        self.migrate_to(CURRENT_SCHEMA_VERSION)
    }

    /// Apply only the ordered schema steps up to `target_version`. Migration
    /// scripts pin this value so a package jump cannot run later config steps
    /// before the intervening release scripts.
    pub(crate) fn migrate_to(&mut self, target_version: u32) -> anyhow::Result<()> {
        self.migrate_with_update_date_to(target_version, next_update_window_date(Local::now()))
    }

    fn migrate_with_update_date_to(
        &mut self,
        target_version: u32,
        next_update_date: NaiveDate,
    ) -> anyhow::Result<()> {
        if self.schema_version > CURRENT_SCHEMA_VERSION {
            anyhow::bail!(
                "config schema_version {} is newer than this build supports (max {}); upgrade kvn",
                self.schema_version,
                CURRENT_SCHEMA_VERSION,
            );
        }
        if target_version > CURRENT_SCHEMA_VERSION {
            anyhow::bail!(
                "target config schema_version {target_version} is newer than this build supports (max {CURRENT_SCHEMA_VERSION})"
            );
        }
        if self.schema_version > target_version {
            anyhow::bail!(
                "config schema_version {} is already newer than migration target {}",
                self.schema_version,
                target_version
            );
        }
        if self.schema_version == 0 && target_version >= 1 {
            self.migrate_v0_to_v1();
            self.schema_version = 1;
        }
        if self.schema_version == 1 && target_version >= 2 {
            self.migrate_v1_to_v2();
            self.schema_version = 2;
        }
        if self.schema_version == 2 && target_version >= 3 {
            self.migrate_v2_to_v3(next_update_date);
            self.schema_version = 3;
        }
        if self.schema_version == 3 && target_version >= 4 {
            self.migrate_v3_to_v4();
            self.schema_version = 4;
        }
        if self.schema_version == 4 && target_version >= 5 {
            self.migrate_v4_to_v5();
            self.schema_version = 5;
        }
        debug_assert_eq!(self.schema_version, target_version);
        Ok(())
    }

    /// v0 → v1: promote the legacy `Settings.dns_strategy` field into
    /// `Settings.dns.strategy`. Idempotent.
    fn migrate_v0_to_v1(&mut self) {
        if self.settings.dns.strategy == DnsStrategy::default()
            && self.settings.dns_strategy != DnsStrategy::default()
        {
            self.settings.dns.strategy = self.settings.dns_strategy.clone();
        }
        // Keep both fields in sync going forward; `dns.strategy` is the source.
        self.settings.dns_strategy = self.settings.dns.strategy.clone();
    }

    /// v1 → v2: promote the legacy top-level VLESS `fingerprint` field into
    /// `cfg.tls.utls_fingerprint`. The pre-v2 sibling fields `reality` and
    /// `ech` already deserialize straight into `cfg.tls.*` via
    /// `#[serde(flatten)]` (identical key names), so they need no code path
    /// here. Idempotent: `.take()` clears the legacy slot on the first run.
    fn migrate_v1_to_v2(&mut self) {
        for profile in &mut self.profiles {
            if let ProtocolConfig::Vless(cfg) = &mut profile.config
                && let Some(fp) = cfg.legacy_fingerprint.take()
                && cfg.tls.utls_fingerprint.is_none()
            {
                cfg.tls.utls_fingerprint = Some(fp);
            }
        }
    }

    fn migrate_v2_to_v3(&mut self, next: NaiveDate) {
        for subscription in &mut self.subscriptions {
            if matches!(
                subscription.auto_update,
                SubscriptionAutoUpdate::Every1h | SubscriptionAutoUpdate::Every12h
            ) {
                subscription.auto_update = SubscriptionAutoUpdate::Every1d;
            }
            if subscription.auto_update != SubscriptionAutoUpdate::Off
                && subscription.next_auto_update.is_none()
            {
                subscription.next_auto_update = Some(next);
            }
            subscription.retry_state = None;
        }
        if self.settings.geo_routing.auto_update == GeoAutoUpdate::Every12h {
            self.settings.geo_routing.auto_update = GeoAutoUpdate::Every1d;
        }
    }

    fn migrate_v3_to_v4(&mut self) {
        if let Some(level) = self.settings.legacy_log_level.take() {
            self.settings.logs.level = level;
        }
    }

    /// v4 → v5: preserve the historical always-on latency probe and HTTP
    /// subscription support while making both behaviors configurable, and
    /// reset the TUN interface name to the app-specific one.
    fn migrate_v4_to_v5(&mut self) {
        self.settings.connectivity_probe = ConnectivityProbeConfig::default();
        self.settings.allow_insecure_http_subscriptions = true;
        self.settings.tun_interface = default_tun_interface();
    }
}

#[cfg(test)]
mod tests {
    use crate::config::profile::*;
    use crate::test_helpers::vless_cfg;
    use uuid::Uuid;

    // ---- migrate: reject configs from the future ----

    #[test]
    fn migrate_rejects_schema_version_from_the_future() {
        let mut cfg = Config {
            schema_version: CURRENT_SCHEMA_VERSION + 1,
            ..Config::default()
        };
        let err = cfg.migrate().unwrap_err().to_string();
        assert!(err.contains("newer than this build"), "Error was: {}", err);
        assert!(err.contains("upgrade kvn"), "Error was: {}", err);
    }

    #[test]
    fn migrate_accepts_current_schema_version() {
        let mut cfg = Config::default();
        cfg.migrate().unwrap();
        assert_eq!(cfg.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn migrate_to_stops_at_the_release_owned_schema() {
        let mut cfg = Config {
            schema_version: 0,
            ..Config::default()
        };

        cfg.migrate_to(3).unwrap();
        assert_eq!(cfg.schema_version, 3);
        cfg.migrate_to(CURRENT_SCHEMA_VERSION).unwrap();
        assert_eq!(cfg.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn migrate_to_rejects_downgrades_and_unknown_targets() {
        let mut current = Config::default();
        assert!(current.migrate_to(CURRENT_SCHEMA_VERSION - 1).is_err());

        let mut legacy = Config {
            schema_version: 0,
            ..Config::default()
        };
        assert!(legacy.migrate_to(CURRENT_SCHEMA_VERSION + 1).is_err());
    }

    // ---- Config::migrate ----

    #[test]
    fn migrate_v0_promotes_legacy_dns_strategy() {
        let mut cfg = Config {
            schema_version: 0,
            ..Config::default()
        };
        // Legacy field set, new field at default → promote.
        cfg.settings.dns_strategy = DnsStrategy::OnlyIpv6;
        cfg.settings.dns.strategy = DnsStrategy::default(); // PreferIpv4
        cfg.migrate().unwrap();
        assert_eq!(cfg.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(cfg.settings.dns.strategy, DnsStrategy::OnlyIpv6);
        assert_eq!(cfg.settings.dns_strategy, DnsStrategy::OnlyIpv6);
    }

    #[test]
    fn migrate_v0_keeps_new_dns_strategy_when_legacy_is_default() {
        let mut cfg = Config {
            schema_version: 0,
            ..Config::default()
        };
        cfg.settings.dns.strategy = DnsStrategy::OnlyIpv4;
        cfg.settings.dns_strategy = DnsStrategy::default();
        cfg.migrate().unwrap();
        // New field wins; legacy is synced from it.
        assert_eq!(cfg.settings.dns.strategy, DnsStrategy::OnlyIpv4);
        assert_eq!(cfg.settings.dns_strategy, DnsStrategy::OnlyIpv4);
    }

    #[test]
    fn migrate_is_noop_when_schema_already_current() {
        let mut cfg = Config {
            schema_version: CURRENT_SCHEMA_VERSION,
            ..Config::default()
        };
        cfg.settings.dns_strategy = DnsStrategy::OnlyIpv6;
        cfg.settings.dns.strategy = DnsStrategy::OnlyIpv4;
        cfg.migrate().unwrap();
        // No promotion when schema_version != 0.
        assert_eq!(cfg.settings.dns.strategy, DnsStrategy::OnlyIpv4);
        assert_eq!(cfg.settings.dns_strategy, DnsStrategy::OnlyIpv6);
    }

    #[test]
    fn migrate_v0_is_idempotent() {
        let mut cfg = Config {
            schema_version: 0,
            ..Config::default()
        };
        cfg.settings.dns_strategy = DnsStrategy::OnlyIpv6;
        cfg.migrate().unwrap();
        let after_first = cfg.clone();
        cfg.migrate().unwrap();
        assert_eq!(cfg.schema_version, after_first.schema_version);
        assert_eq!(cfg.settings.dns.strategy, after_first.settings.dns.strategy);
        assert_eq!(cfg.settings.dns_strategy, after_first.settings.dns_strategy);
    }

    fn vless_profile_with_legacy_fingerprint(fp: &str) -> Profile {
        let mut p = Profile::new_vless(
            "Legacy".to_string(),
            "1.2.3.4".to_string(),
            443,
            "u".to_string(),
        );
        if let ProtocolConfig::Vless(ref mut cfg) = p.config {
            cfg.legacy_fingerprint = Some(fp.to_string());
        }
        p
    }

    #[test]
    fn migrate_v1_to_v2_promotes_legacy_vless_fingerprint() {
        let mut cfg = Config {
            schema_version: 1,
            ..Config::default()
        };
        cfg.profiles
            .push(vless_profile_with_legacy_fingerprint("chrome"));
        cfg.migrate().unwrap();
        assert_eq!(cfg.schema_version, CURRENT_SCHEMA_VERSION);
        let vc = vless_cfg(&cfg.profiles[0]);
        assert_eq!(vc.tls.utls_fingerprint.as_deref(), Some("chrome"));
        assert!(vc.legacy_fingerprint.is_none());
    }

    #[test]
    fn migrate_v1_to_v2_keeps_new_fingerprint_when_both_set() {
        // If a future writer somehow produced both, the new slot wins
        // (legacy is treated purely as a one-way input).
        let mut cfg = Config {
            schema_version: 1,
            ..Config::default()
        };
        let mut p = vless_profile_with_legacy_fingerprint("legacy-fp");
        if let ProtocolConfig::Vless(ref mut vc) = p.config {
            vc.tls.utls_fingerprint = Some("new-fp".to_string());
        }
        cfg.profiles.push(p);
        cfg.migrate().unwrap();
        let vc = vless_cfg(&cfg.profiles[0]);
        assert_eq!(vc.tls.utls_fingerprint.as_deref(), Some("new-fp"));
        assert!(vc.legacy_fingerprint.is_none());
    }

    #[test]
    fn migrate_v1_to_v2_is_idempotent() {
        let mut cfg = Config {
            schema_version: 1,
            ..Config::default()
        };
        cfg.profiles
            .push(vless_profile_with_legacy_fingerprint("chrome"));
        cfg.migrate().unwrap();
        let first = cfg.clone();
        cfg.migrate().unwrap();
        assert_eq!(cfg, first);
    }

    #[test]
    fn migrate_v1_to_v2_noop_for_non_vless_profiles() {
        let mut cfg = Config {
            schema_version: 1,
            ..Config::default()
        };
        cfg.profiles.push(Profile {
            id: Uuid::new_v4(),
            name: "VM".to_string(),
            address: "1.1.1.1".to_string(),
            port: 443,
            config: ProtocolConfig::Vmess(VmessConfig {
                uuid: "vm-uuid".to_string(),
                ..VmessConfig::default()
            }),
            tags: Vec::new(),
            subscription_id: None,
        });
        cfg.migrate().unwrap();
        assert_eq!(cfg.schema_version, CURRENT_SCHEMA_VERSION);
        // No panic, no mutation.
    }

    #[test]
    fn migrate_v2_to_v3_promotes_short_update_intervals() {
        let mut cfg = Config {
            schema_version: 2,
            ..Config::default()
        };
        cfg.subscriptions.push(Subscription {
            id: Uuid::new_v4(),
            name: "short".into(),
            url: "https://example.com/sub".into(),
            auto_update: SubscriptionAutoUpdate::Every1h,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        });
        cfg.settings.geo_routing.auto_update = GeoAutoUpdate::Every12h;

        cfg.migrate().unwrap();

        assert_eq!(cfg.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(
            cfg.subscriptions[0].auto_update,
            SubscriptionAutoUpdate::Every1d
        );
        assert!(cfg.subscriptions[0].next_auto_update.is_some());
        assert_eq!(cfg.settings.geo_routing.auto_update, GeoAutoUpdate::Every1d);
    }

    #[test]
    fn migrate_v3_to_v4_nests_legacy_log_level() {
        let mut cfg = Config {
            schema_version: 3,
            ..Config::default()
        };
        cfg.settings.legacy_log_level = Some("debug".into());

        cfg.migrate().unwrap();

        assert_eq!(cfg.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(cfg.settings.logs.level, "debug");
        assert!(cfg.settings.legacy_log_level.is_none());
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.contains("log_level"));
    }

    #[test]
    fn migrate_v4_to_v5_enables_default_connectivity_probe() {
        let mut cfg = Config {
            schema_version: 4,
            ..Config::default()
        };
        cfg.settings.tun_interface = "tun0".into();
        cfg.settings.connectivity_probe = ConnectivityProbeConfig {
            enabled: false,
            url: None,
        };
        cfg.settings.allow_insecure_http_subscriptions = false;

        cfg.migrate().unwrap();

        assert_eq!(cfg.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(cfg.settings.connectivity_probe.enabled);
        assert_eq!(
            cfg.settings.connectivity_probe.url.as_deref(),
            Some("https://connectivitycheck.gstatic.com/generate_204")
        );
        assert!(cfg.settings.allow_insecure_http_subscriptions);
        assert_eq!(cfg.settings.tun_interface, "kvn0");
    }

    #[test]
    fn migrate_v4_to_v5_replaces_custom_tun_interface() {
        let mut cfg = Config {
            schema_version: 4,
            ..Config::default()
        };
        cfg.settings.tun_interface = "work-vpn".into();

        cfg.migrate().unwrap();

        assert_eq!(cfg.settings.tun_interface, "kvn0");
    }

    #[test]
    fn migrate_chains_v0_through_current() {
        // schema_version=0 must arrive at the current version in one call.
        let mut cfg = Config {
            schema_version: 0,
            ..Config::default()
        };
        cfg.settings.dns_strategy = DnsStrategy::OnlyIpv6;
        cfg.profiles
            .push(vless_profile_with_legacy_fingerprint("chrome"));
        cfg.migrate().unwrap();
        assert_eq!(cfg.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(cfg.settings.dns.strategy, DnsStrategy::OnlyIpv6);
        assert!(cfg.settings.allow_insecure_http_subscriptions);
        let vc = vless_cfg(&cfg.profiles[0]);
        assert_eq!(vc.tls.utls_fingerprint.as_deref(), Some("chrome"));
    }
}

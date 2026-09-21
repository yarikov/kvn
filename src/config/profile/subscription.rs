use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::schedule::minutes_until_next_update_window;
use super::{Settings, SubscriptionAutoUpdate};

fn is_false(value: &bool) -> bool {
    !*value
}

/// A subscription URL that can be refreshed to import a set of profiles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Subscription {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub auto_update: SubscriptionAutoUpdate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<DateTime<Local>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_auto_update: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_state: Option<SubscriptionRetryState>,
    /// When true, HWID identification headers are sent with subscription
    /// requests (see [`Subscription::effective_hwid`]). Default false: raw
    /// identifiers must not reach unrelated hosts.
    #[serde(default, skip_serializing_if = "is_false")]
    pub send_hwid: bool,
    /// Per-subscription HWID override; requires `send_hwid` to take effect.
    /// When absent, `settings.hwid` is used. Its presence alone never enables
    /// sending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hwid: Option<String>,
}

/// Persisted retry metadata for an auto-updating subscription.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionRetryState {
    pub consecutive_failures: u32,
    pub retry_at: DateTime<Local>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_date: Option<NaiveDate>,
}

impl Subscription {
    /// Resolve the HWID to send for this subscription.
    ///
    /// Resolution rules:
    /// - `send_hwid` absent or false → `None` (no device headers are sent);
    /// - `send_hwid: true` with a per-subscription `hwid` override → the
    ///   override;
    /// - otherwise → fall back to `settings.hwid`.
    ///
    /// The presence of `self.hwid` alone never enables sending.
    pub fn effective_hwid<'a>(&'a self, settings: &'a Settings) -> Option<&'a str> {
        if !self.send_hwid {
            return None;
        }
        self.hwid.as_deref().or(Some(settings.hwid.as_str()))
    }

    /// Record a failed fetch and schedule the next retry using the bounded
    /// 1/5/15/60-minute backoff sequence.
    pub fn record_fetch_failure(&mut self, now: DateTime<Local>) {
        let today = now.date_naive();
        let consecutive_failures = self
            .retry_state
            .as_ref()
            .filter(|state| state.attempt_date == Some(today))
            .map_or(1, |state| state.consecutive_failures.saturating_add(1));
        let delay_minutes = match consecutive_failures {
            1 => 1,
            2 => 5,
            3 => 15,
            4 => 60,
            _ => minutes_until_next_update_window(now),
        };
        self.retry_state = Some(SubscriptionRetryState {
            consecutive_failures,
            retry_at: now + chrono::Duration::minutes(delay_minutes),
            attempt_date: Some(today),
        });
    }

    pub fn clear_retry_state(&mut self) {
        self.retry_state = None;
    }

    pub fn schedule_next_after_success(&mut self, now: DateTime<Local>) {
        self.clear_retry_state();
        let days = self.auto_update.interval_minutes() / 1_440;
        self.next_auto_update =
            (days != 0).then(|| now.date_naive() + chrono::Duration::days(days as i64));
    }
}

const MAX_HWID_LEN: usize = 256;

pub(super) fn validate_hwid(hwid: &str) -> anyhow::Result<()> {
    if hwid.trim().is_empty() {
        anyhow::bail!("must not be empty when send_hwid is enabled");
    }
    if hwid.len() > MAX_HWID_LEN {
        anyhow::bail!("must not exceed {MAX_HWID_LEN} bytes");
    }
    hwid.parse::<ureq::http::HeaderValue>()
        .map_err(|_| anyhow::anyhow!("must be a valid HTTP header value"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::*;
    use crate::test_helpers::subscription_with_hwid;

    fn settings_with_hwid() -> Settings {
        Settings {
            hwid: "lnx-installation-hwid".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn config_validate_accepts_effective_subscription_hwid() {
        let mut config = Config::default();
        config.settings.hwid = "lnx-installation-hwid".to_string();
        config
            .subscriptions
            .push(subscription_with_hwid(true, None));
        config.validate().unwrap();
    }

    #[test]
    fn config_validate_rejects_empty_effective_subscription_hwid() {
        let mut config = Config::default();
        config
            .subscriptions
            .push(subscription_with_hwid(true, None));
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("Subscription 1"), "got: {error}");
        assert!(error.contains("must not be empty"), "got: {error}");
    }

    #[test]
    fn config_validate_rejects_empty_subscription_hwid_override() {
        let mut config = Config::default();
        config.settings.hwid = "lnx-installation-hwid".to_string();
        config
            .subscriptions
            .push(subscription_with_hwid(true, Some("")));
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("must not be empty"), "got: {error}");
    }

    #[test]
    fn config_validate_rejects_invalid_http_hwid_value() {
        let mut config = Config::default();
        config
            .subscriptions
            .push(subscription_with_hwid(true, Some("device\r\ninjected")));
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("HTTP header value"), "got: {error}");
    }

    #[test]
    fn config_validate_rejects_overlong_hwid() {
        let mut config = Config::default();
        let hwid = "a".repeat(MAX_HWID_LEN + 1);
        config
            .subscriptions
            .push(subscription_with_hwid(true, Some(&hwid)));
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("must not exceed"), "got: {error}");
    }

    #[test]
    fn config_validate_ignores_unused_hwid() {
        let mut config = Config::default();
        config
            .subscriptions
            .push(subscription_with_hwid(false, Some("device\r\ninjected")));
        config.validate().unwrap();
    }

    #[test]
    fn effective_hwid_absent_send_hwid_sends_nothing() {
        // Backward compat: `send_hwid` missing from JSON defaults to false.
        let sub: Subscription =
            serde_json::from_str(r#"{"name":"S","url":"https://e.com/s"}"#).unwrap();
        assert!(!sub.send_hwid);
        assert_eq!(sub.hwid, None);
        assert_eq!(sub.effective_hwid(&settings_with_hwid()), None);
    }

    #[test]
    fn subscription_serialization_omits_disabled_hwid_fields() {
        let sub = subscription_with_hwid(false, None);
        let json = serde_json::to_value(sub).unwrap();

        assert!(json.get("send_hwid").is_none());
        assert!(json.get("hwid").is_none());
    }

    #[test]
    fn subscription_serialization_keeps_enabled_hwid_flag() {
        let sub = subscription_with_hwid(true, None);
        let json = serde_json::to_value(sub).unwrap();

        assert_eq!(json.get("send_hwid"), Some(&serde_json::Value::Bool(true)));
        assert!(json.get("hwid").is_none());
    }

    #[test]
    fn effective_hwid_false_sends_nothing_even_with_override() {
        let sub: Subscription = serde_json::from_str(
            r#"{"name":"S","url":"https://e.com/s","send_hwid":false,"hwid":"custom"}"#,
        )
        .unwrap();
        // The presence of `hwid` alone must not enable sending.
        assert_eq!(sub.effective_hwid(&settings_with_hwid()), None);
    }

    #[test]
    fn effective_hwid_true_falls_back_to_settings_hwid() {
        let sub: Subscription =
            serde_json::from_str(r#"{"name":"S","url":"https://e.com/s","send_hwid":true}"#)
                .unwrap();
        assert_eq!(
            sub.effective_hwid(&settings_with_hwid()),
            Some("lnx-installation-hwid"),
        );
    }

    #[test]
    fn effective_hwid_override_wins_over_settings() {
        let sub: Subscription = serde_json::from_str(
            r#"{"name":"S","url":"https://e.com/s","send_hwid":true,"hwid":"provider-registered-device-id"}"#,
        )
        .unwrap();
        assert_eq!(
            sub.effective_hwid(&settings_with_hwid()),
            Some("provider-registered-device-id"),
        );
    }

    #[test]
    fn effective_hwid_is_never_derived_from_url() {
        let sub: Subscription = serde_json::from_str(
            r#"{"name":"S","url":"https://e.com/token-rotated","send_hwid":true}"#,
        )
        .unwrap();
        assert_eq!(
            sub.effective_hwid(&settings_with_hwid()),
            Some("lnx-installation-hwid"),
        );
    }

    #[test]
    fn subscription_retry_backoff_is_bounded_and_serializable() {
        use chrono::TimeZone;

        let now = Local.with_ymd_and_hms(2026, 8, 22, 12, 0, 0).unwrap();
        let mut sub = Subscription {
            id: Uuid::nil(),
            name: "Sub".into(),
            url: "https://example.com/sub".into(),
            auto_update: SubscriptionAutoUpdate::Every1h,
            last_updated: None,
            next_auto_update: None,
            retry_state: None,
            send_hwid: false,
            hwid: None,
        };

        for (failures, delay) in [(1, 1), (2, 5), (3, 15), (4, 60)] {
            sub.record_fetch_failure(now);
            let state = sub.retry_state.as_ref().unwrap();
            assert_eq!(state.consecutive_failures, failures);
            assert_eq!(state.retry_at, now + chrono::Duration::minutes(delay));
        }
        sub.record_fetch_failure(now);
        assert_eq!(
            sub.retry_state.as_ref().unwrap().retry_at,
            Local.with_ymd_and_hms(2026, 8, 23, 8, 0, 0).unwrap()
        );

        let json = serde_json::to_string(&sub).unwrap();
        let restored: Subscription = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, sub);
    }

    #[test]
    fn subscription_without_retry_state_remains_backward_compatible() {
        let json = r#"{
            "id":"00000000-0000-0000-0000-000000000000",
            "name":"Sub",
            "url":"https://example.com/sub",
            "auto_update":"every1h"
        }"#;

        let sub: Subscription = serde_json::from_str(json).unwrap();
        assert!(sub.retry_state.is_none());
        assert!(!serde_json::to_string(&sub).unwrap().contains("retry_state"));
    }
}

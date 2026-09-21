use chrono::{DateTime, Datelike, Local, NaiveDate, Timelike};
use serde::{Deserialize, Serialize};

/// Auto-update schedule for a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionAutoUpdate {
    #[default]
    Off,
    #[doc(hidden)]
    Every1h,
    #[doc(hidden)]
    Every12h,
    Every1d,
    Every3d,
    Every7d,
}

impl SubscriptionAutoUpdate {
    /// Return the interval in minutes.
    pub fn interval_minutes(self) -> u64 {
        match self {
            Self::Off => 0,
            Self::Every1h | Self::Every12h => 1_440,
            Self::Every1d => 1440,
            Self::Every3d => 4320,
            Self::Every7d => 10080,
        }
    }

    /// Cycle to the next schedule.
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Every1d,
            Self::Every1h | Self::Every12h => Self::Every1d,
            Self::Every1d => Self::Every3d,
            Self::Every3d => Self::Every7d,
            Self::Every7d => Self::Off,
        }
    }

    /// Compact display label shared with rule-set auto-update controls.
    pub fn label(self) -> String {
        auto_update_label(self.interval_label())
    }

    /// Short interval label without icon.
    pub fn interval_label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Every1h | Self::Every12h => "1d",
            Self::Every1d => "1d",
            Self::Every3d => "3d",
            Self::Every7d => "7d",
        }
    }
}

fn auto_update_label(interval: &str) -> String {
    format!("({interval})")
}

/// Background update schedule for geo rule-sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GeoAutoUpdate {
    #[default]
    Off,
    #[serde(rename = "every_12h")]
    Every12h,
    #[serde(rename = "every_1d")]
    Every1d,
    #[serde(rename = "every_3d")]
    Every3d,
    #[serde(rename = "every_7d")]
    Every7d,
}

impl GeoAutoUpdate {
    pub fn interval_minutes(self) -> u64 {
        match self {
            Self::Off => 0,
            Self::Every12h => 1_440,
            Self::Every1d => 1_440,
            Self::Every3d => 4_320,
            Self::Every7d => 10_080,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Every1d,
            Self::Every12h => Self::Every1d,
            Self::Every1d => Self::Every3d,
            Self::Every3d => Self::Every7d,
            Self::Every7d => Self::Off,
        }
    }

    pub fn label(self) -> String {
        auto_update_label(self.interval_label())
    }

    pub fn interval_label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Every12h => "1d",
            Self::Every1d => "1d",
            Self::Every3d => "3d",
            Self::Every7d => "7d",
        }
    }
}

pub fn next_update_window_date(now: DateTime<Local>) -> NaiveDate {
    if now.hour() < 8 {
        now.date_naive()
    } else {
        now.date_naive()
            .succ_opt()
            .expect("local date must advance")
    }
}

pub fn retry_delay_minutes(failure: u32, now: DateTime<Local>) -> i64 {
    match failure {
        1 => 1,
        2 => 5,
        3 => 15,
        4 => 60,
        _ => minutes_until_next_update_window(now),
    }
}

pub(super) fn minutes_until_next_update_window(now: DateTime<Local>) -> i64 {
    let next = next_update_window_date(now);
    let days = next.num_days_from_ce() - now.date_naive().num_days_from_ce();
    i64::from(days) * 1_440 - i64::from(now.hour()) * 60 - i64::from(now.minute()) + 8 * 60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geo_auto_update_serde_values_round_trip() {
        for schedule in [
            GeoAutoUpdate::Off,
            GeoAutoUpdate::Every12h,
            GeoAutoUpdate::Every1d,
            GeoAutoUpdate::Every3d,
            GeoAutoUpdate::Every7d,
        ] {
            let json = serde_json::to_string(&schedule).unwrap();
            let restored: GeoAutoUpdate = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, schedule);
        }
        assert_eq!(
            serde_json::to_string(&GeoAutoUpdate::Every3d).unwrap(),
            r#""every_3d""#
        );
    }

    #[test]
    fn subscription_auto_update_cycles_and_labels() {
        assert_eq!(SubscriptionAutoUpdate::Off.interval_minutes(), 0);
        assert_eq!(SubscriptionAutoUpdate::Every1h.interval_minutes(), 1440);
        assert_eq!(SubscriptionAutoUpdate::Every12h.interval_minutes(), 1440);
        assert_eq!(SubscriptionAutoUpdate::Every1d.interval_minutes(), 1440);
        assert_eq!(SubscriptionAutoUpdate::Every3d.interval_minutes(), 4320);
        assert_eq!(SubscriptionAutoUpdate::Every7d.interval_minutes(), 10080);

        assert_eq!(
            SubscriptionAutoUpdate::Off.next(),
            SubscriptionAutoUpdate::Every1d
        );
        assert_eq!(
            SubscriptionAutoUpdate::Every7d.next(),
            SubscriptionAutoUpdate::Off
        );

        assert_eq!(SubscriptionAutoUpdate::Off.label(), "(off)");
        assert_eq!(SubscriptionAutoUpdate::Every1d.label(), "(1d)");
    }

    #[test]
    fn update_window_is_fixed_at_eight_local_time() {
        use chrono::TimeZone;

        let before = Local.with_ymd_and_hms(2026, 8, 22, 7, 59, 0).unwrap();
        let after = Local.with_ymd_and_hms(2026, 8, 22, 11, 0, 0).unwrap();
        assert_eq!(next_update_window_date(before), before.date_naive());
        assert_eq!(
            next_update_window_date(after),
            after.date_naive().succ_opt().unwrap()
        );
    }

    #[test]
    fn geo_auto_update_cycles_intervals_and_labels() {
        let schedules = [
            (GeoAutoUpdate::Off, 0, "off"),
            (GeoAutoUpdate::Every1d, 1_440, "1d"),
            (GeoAutoUpdate::Every3d, 4_320, "3d"),
            (GeoAutoUpdate::Every7d, 10_080, "7d"),
        ];
        for (schedule, minutes, interval) in schedules {
            assert_eq!(schedule.interval_minutes(), minutes);
            assert_eq!(schedule.interval_label(), interval);
            assert_eq!(schedule.label(), format!("({interval})"));
        }
        assert_eq!(GeoAutoUpdate::Off.next(), GeoAutoUpdate::Every1d);
        assert_eq!(GeoAutoUpdate::Every12h.next(), GeoAutoUpdate::Every1d);
        assert_eq!(GeoAutoUpdate::Every1d.next(), GeoAutoUpdate::Every3d);
        assert_eq!(GeoAutoUpdate::Every3d.next(), GeoAutoUpdate::Every7d);
        assert_eq!(GeoAutoUpdate::Every7d.next(), GeoAutoUpdate::Off);
    }
}

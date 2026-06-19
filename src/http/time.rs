// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Time helpers for query handlers: `parse_time` (ISO 8601, unix millis, `now`,
//! relative offsets) and `auto_interval` (histogram bucket size, ~60 buckets).

use chrono::{DateTime, Duration, Utc};

pub(super) fn parse_time(s: Option<&str>) -> Option<DateTime<Utc>> {
    let s = s?.trim();
    if s.is_empty() {
        return None;
    }
    if s == "now" {
        return Some(Utc::now());
    }
    if let Some(rest) = s.strip_prefix('-') {
        return Some(Utc::now() - parse_duration(rest)?);
    }
    if let Ok(n) = s.parse::<i64>() {
        return DateTime::<Utc>::from_timestamp_millis(n);
    }
    s.parse::<DateTime<Utc>>().ok()
}

fn parse_duration(s: &str) -> Option<Duration> {
    let (num, unit) = s.split_at(s.find(|c: char| c.is_alphabetic())?);
    let n: i64 = num.parse().ok()?;
    Some(match unit {
        "s" => Duration::seconds(n),
        "m" => Duration::minutes(n),
        "h" => Duration::hours(n),
        "d" => Duration::days(n),
        _ => return None,
    })
}

pub(super) fn auto_interval(start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> String {
    let span = match (start, end) {
        (Some(s), Some(e)) => (e - s).num_seconds().max(60),
        _ => 3600,
    };
    let secs = (span / 60).max(1);
    if secs < 10 {
        "10s".into()
    } else if secs < 60 {
        format!("{}s", round_to(secs as u64, 10))
    } else if secs < 3600 {
        format!("{}m", round_to((secs / 60) as u64, 1))
    } else {
        format!("{}h", ((secs / 3600) as u64).max(1))
    }
}

fn round_to(n: u64, step: u64) -> u64 {
    ((n + step / 2) / step * step).max(step)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_time_none_and_empty() {
        assert!(parse_time(None).is_none());
        assert!(parse_time(Some("")).is_none());
        assert!(parse_time(Some("   ")).is_none());
    }

    #[test]
    fn parse_time_unix_millis() {
        let t = parse_time(Some("0")).unwrap();
        assert_eq!(t.timestamp_millis(), 0);
    }

    #[test]
    fn parse_time_iso8601() {
        let t = parse_time(Some("2026-01-01T00:00:00Z")).unwrap();
        assert_eq!(t.timestamp(), 1_767_225_600);
    }

    #[test]
    fn parse_time_relative_and_now() {
        let now = Utc::now();
        let hour_ago = parse_time(Some("-1h")).unwrap();
        let delta = (now - hour_ago).num_seconds();
        assert!((3590..=3610).contains(&delta), "delta was {delta}");
        assert!(parse_time(Some("now")).is_some());
    }

    #[test]
    fn parse_duration_units() {
        assert_eq!(parse_duration("30s"), Some(Duration::seconds(30)));
        assert_eq!(parse_duration("15m"), Some(Duration::minutes(15)));
        assert_eq!(parse_duration("2h"), Some(Duration::hours(2)));
        assert_eq!(parse_duration("7d"), Some(Duration::days(7)));
    }

    #[test]
    fn parse_duration_rejects_bad_input() {
        assert_eq!(parse_duration("5x"), None); // unknown unit
        assert_eq!(parse_duration("10"), None); // no unit
        assert_eq!(parse_duration("m"), None); // no number
    }

    #[test]
    fn round_to_nearest_step() {
        assert_eq!(round_to(24, 10), 20);
        assert_eq!(round_to(25, 10), 30);
        assert_eq!(round_to(3, 10), 10); // floors to at least one step
    }

    #[test]
    fn auto_interval_targets_about_60_buckets() {
        // No bounds -> assume a 1h window -> 1m buckets.
        assert_eq!(auto_interval(None, None), "1m");
        let start = Utc::now();
        // 1h window -> ~60 buckets of 1m.
        assert_eq!(
            auto_interval(Some(start), Some(start + Duration::hours(1))),
            "1m"
        );
        // 1m window -> sub-10s clamps to 10s.
        assert_eq!(
            auto_interval(Some(start), Some(start + Duration::minutes(1))),
            "10s"
        );
        // 1d window -> ~60 buckets of 24m.
        assert_eq!(
            auto_interval(Some(start), Some(start + Duration::days(1))),
            "24m"
        );
        // 5d window crosses into hour buckets.
        assert_eq!(
            auto_interval(Some(start), Some(start + Duration::days(5))),
            "2h"
        );
    }
}

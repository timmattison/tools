//! Format durations as short human strings and pick a dim level by age.

use std::time::Duration;

/// How "fresh" an age is — drives display brightness.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum AgeDim {
    /// `< 5m` — bright.
    Fresh,
    /// `< 1h` — normal.
    Recent,
    /// `< 1d` — dim.
    Aging,
    /// `>= 1d` — very dim.
    Stale,
}

/// Format a duration like `30s`, `5m`, `2h`, `3d`. Always 1–3 chars + unit.
///
/// - `< 60s`              → `Ns`
/// - `< 60m`              → `Nm`
/// - `< 24h`              → `Nh`
/// - everything else      → `Nd`
pub fn format_age(age: Duration) -> String {
    let secs = age.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 60 * 60 {
        format!("{}m", secs / 60)
    } else if secs < 60 * 60 * 24 {
        format!("{}h", secs / (60 * 60))
    } else {
        format!("{}d", secs / (60 * 60 * 24))
    }
}

/// Classify an age into a display brightness bucket.
pub fn age_dim_level(age: Duration) -> AgeDim {
    let secs = age.as_secs();
    if secs < 60 * 5 {
        AgeDim::Fresh
    } else if secs < 60 * 60 {
        AgeDim::Recent
    } else if secs < 60 * 60 * 24 {
        AgeDim::Aging
    } else {
        AgeDim::Stale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_seconds_below_a_minute() {
        assert_eq!(format_age(Duration::from_secs(0)), "0s");
        assert_eq!(format_age(Duration::from_secs(30)), "30s");
        assert_eq!(format_age(Duration::from_secs(59)), "59s");
    }

    #[test]
    fn format_minutes_below_an_hour() {
        assert_eq!(format_age(Duration::from_secs(60)), "1m");
        assert_eq!(format_age(Duration::from_secs(60 * 5)), "5m");
        assert_eq!(format_age(Duration::from_secs(60 * 59)), "59m");
    }

    #[test]
    fn format_hours_below_a_day() {
        assert_eq!(format_age(Duration::from_secs(60 * 60)), "1h");
        assert_eq!(format_age(Duration::from_secs(60 * 60 * 2)), "2h");
        assert_eq!(format_age(Duration::from_secs(60 * 60 * 23)), "23h");
    }

    #[test]
    fn format_days_and_above() {
        assert_eq!(format_age(Duration::from_secs(60 * 60 * 24)), "1d");
        assert_eq!(format_age(Duration::from_secs(60 * 60 * 24 * 3)), "3d");
        assert_eq!(format_age(Duration::from_secs(60 * 60 * 24 * 365)), "365d");
    }

    #[test]
    fn dim_level_buckets_by_boundary() {
        assert_eq!(age_dim_level(Duration::from_secs(0)), AgeDim::Fresh);
        assert_eq!(age_dim_level(Duration::from_secs(60 * 5 - 1)), AgeDim::Fresh);
        assert_eq!(age_dim_level(Duration::from_secs(60 * 5)), AgeDim::Recent);
        assert_eq!(age_dim_level(Duration::from_secs(60 * 60 - 1)), AgeDim::Recent);
        assert_eq!(age_dim_level(Duration::from_secs(60 * 60)), AgeDim::Aging);
        assert_eq!(age_dim_level(Duration::from_secs(60 * 60 * 24 - 1)), AgeDim::Aging);
        assert_eq!(age_dim_level(Duration::from_secs(60 * 60 * 24)), AgeDim::Stale);
        assert_eq!(age_dim_level(Duration::from_secs(60 * 60 * 24 * 30)), AgeDim::Stale);
    }
}

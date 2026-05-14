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

/// Format a duration with two units, e.g. `5m23s`, `2h14m`, `3d12h`.
pub fn format_age_detailed(age: Duration) -> String {
    let secs = age.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else if secs < 86400 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d{}h", secs / 86400, (secs % 86400) / 3600)
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
    fn detailed_age_seconds_only() {
        assert_eq!(format_age_detailed(Duration::from_secs(0)), "0s");
        assert_eq!(format_age_detailed(Duration::from_secs(5)), "5s");
        assert_eq!(format_age_detailed(Duration::from_secs(59)), "59s");
    }

    #[test]
    fn detailed_age_minutes_and_seconds() {
        assert_eq!(format_age_detailed(Duration::from_secs(60)), "1m0s");
        assert_eq!(format_age_detailed(Duration::from_secs(5 * 60 + 23)), "5m23s");
        assert_eq!(
            format_age_detailed(Duration::from_secs(59 * 60 + 59)),
            "59m59s",
        );
    }

    #[test]
    fn detailed_age_hours_and_minutes() {
        assert_eq!(format_age_detailed(Duration::from_secs(60 * 60)), "1h0m");
        assert_eq!(
            format_age_detailed(Duration::from_secs(2 * 3600 + 14 * 60)),
            "2h14m",
        );
        assert_eq!(
            format_age_detailed(Duration::from_secs(23 * 3600 + 59 * 60)),
            "23h59m",
        );
    }

    #[test]
    fn detailed_age_days_and_hours() {
        assert_eq!(format_age_detailed(Duration::from_secs(86400)), "1d0h");
        assert_eq!(
            format_age_detailed(Duration::from_secs(3 * 86400 + 12 * 3600)),
            "3d12h",
        );
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

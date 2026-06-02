//! Pure aggregation helpers over the per-language counter maps.
//!
//! These functions turn the raw `HashMap<String, u64>` buckets that
//! [`crate::stats`] parses into the scalar values `seescc` renders: a sum of
//! the counts for a selected set of languages, and the overall cache hit rate.

use std::collections::HashMap;

use crate::config::{MetricKey, MetricKind};
use crate::stats;

/// A single metric's extracted value, tagged with how it should be rendered.
///
/// The three variants mirror [`MetricKind`]: `Count` and `Size` both wrap a
/// `u64` but format differently (thousands separators vs. human-readable
/// bytes), while `Rate` carries the already-computed percentage.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(
    dead_code,
    reason = "consumed when the CLI wiring lands in the final Phase 2 slice"
)]
pub(crate) enum MetricValue {
    /// A plain tally, formatted with thousands separators.
    Count(u64),
    /// A byte size, formatted in human-readable units.
    Size(u64),
    /// A percentage rate, formatted to one decimal place with a `%` suffix.
    Rate(f64),
}

impl MetricValue {
    /// Render this value as the user-facing display string.
    ///
    /// `Count` uses thousands separators (`4786 -> "4,786"`), `Size` uses
    /// human-readable byte units (`809_212_237 -> "809 MB"`), and `Rate` is
    /// formatted to one decimal place with a trailing `%` (`64.08 -> "64.1%"`).
    #[allow(
        dead_code,
        reason = "consumed when the CLI wiring lands in the final Phase 2 slice"
    )]
    pub(crate) fn format(&self) -> String {
        todo!("Slice 3 green implements MetricValue::format")
    }
}

/// Extract the [`MetricValue`] for `key` from the parsed `stats`.
///
/// Per-language metrics (`cache_hits`, `cache_misses`, `cache_errors`,
/// `hit_rate`) are filtered by `languages`; an empty `languages` slice sums
/// across all languages. Global counts and sizes ignore `languages`.
#[allow(
    dead_code,
    reason = "consumed when the CLI wiring lands in the final Phase 2 slice"
)]
pub(crate) fn metric_value(
    key: MetricKey,
    stats: &stats::Stats,
    languages: &[String],
) -> MetricValue {
    let _ = (key, stats, languages, MetricKind::Count);
    todo!("Slice 3 green implements metric_value")
}

/// Sum the counter values for the selected `languages`.
///
/// An empty `languages` slice means "sum across all languages". A requested
/// language that is absent from `counts` contributes `0` rather than being an
/// error.
pub(crate) fn lang_sum(counts: &HashMap<String, u64>, languages: &[String]) -> u64 {
    if languages.is_empty() {
        counts.values().sum()
    } else {
        languages.iter().filter_map(|lang| counts.get(lang)).sum()
    }
}

/// Compute the cache hit rate as a percentage in `0.0..=100.0`.
///
/// Returns `hits / (hits + misses) * 100`. When `hits + misses == 0`, returns
/// `0.0` (never `NaN`).
pub(crate) fn hit_rate(hits: u64, misses: u64) -> f64 {
    let total = hits + misses;
    if total == 0 {
        0.0
    } else {
        hits as f64 / total as f64 * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/sccache-0.15.0.json");

    /// Parse the captured fixture into a [`stats::Stats`] for realistic data.
    fn fixture_stats() -> stats::Stats {
        stats::parse(FIXTURE).expect("fixture should parse")
    }

    #[test]
    fn metric_value_cache_hits_rust_only() {
        let fixture = fixture_stats();
        assert_eq!(
            metric_value(MetricKey::CacheHits, &fixture, &["Rust".to_string()]),
            MetricValue::Count(1718)
        );
    }

    /// Build the realistic per-language count map from the captured fixture.
    fn fixture_counts() -> HashMap<String, u64> {
        let mut counts = HashMap::new();
        counts.insert("Assembler".to_string(), 196);
        counts.insert("Rust".to_string(), 1718);
        counts.insert("C/C++".to_string(), 516);
        counts
    }

    #[test]
    fn lang_sum_rust_only() {
        let counts = fixture_counts();
        let languages = ["Rust".to_string()];
        assert_eq!(lang_sum(&counts, &languages), 1718);
    }

    #[test]
    fn lang_sum_empty_means_all() {
        let counts = fixture_counts();
        let languages: [String; 0] = [];
        assert_eq!(lang_sum(&counts, &languages), 196 + 1718 + 516);
    }

    #[test]
    fn lang_sum_multiple_langs() {
        let counts = fixture_counts();
        let languages = ["Rust".to_string(), "C/C++".to_string()];
        assert_eq!(lang_sum(&counts, &languages), 1718 + 516);
    }

    #[test]
    fn lang_sum_absent_language_is_zero() {
        let counts = fixture_counts();
        let languages = ["Go".to_string()];
        assert_eq!(lang_sum(&counts, &languages), 0);
    }

    #[test]
    fn lang_sum_empty_map_is_zero() {
        let counts: HashMap<String, u64> = HashMap::new();
        let languages = ["Rust".to_string()];
        assert_eq!(lang_sum(&counts, &languages), 0);

        let all: [String; 0] = [];
        assert_eq!(lang_sum(&counts, &all), 0);
    }

    #[test]
    fn hit_rate_matches_fixture() {
        // 1718 / (1718 + 963) * 100 = 1718 / 2681 * 100 = 64.08056...
        assert!((hit_rate(1718, 963) - 64.080_567).abs() < 0.001);
    }

    #[test]
    fn hit_rate_zero_activity_is_zero_not_nan() {
        assert_eq!(hit_rate(0, 0), 0.0);
        assert!(!hit_rate(0, 0).is_nan());
    }

    #[test]
    fn hit_rate_all_hits_is_100() {
        assert!((hit_rate(100, 0) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn hit_rate_half() {
        assert!((hit_rate(50, 50) - 50.0).abs() < f64::EPSILON);
    }
}

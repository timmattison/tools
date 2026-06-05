//! Pure aggregation helpers over the per-language counter maps.
//!
//! These functions turn the raw `HashMap<String, u64>` buckets that
//! [`crate::stats`] parses into the scalar values `seescc` renders: a sum of
//! the counts for a selected set of languages, and the overall cache hit rate.

use std::collections::HashMap;

use crate::config::MetricKey;
use crate::stats;

/// A single metric's extracted value, tagged with how it should be rendered.
///
/// The three variants mirror [`crate::config::MetricKind`]: `Count` and `Size`
/// both wrap a `u64` but format differently (thousands separators vs.
/// human-readable bytes), while `Rate` carries the already-computed percentage.
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// human-readable byte units (`809_212_237 -> "771.7 MiB"`), and `Rate` is
    /// formatted to one decimal place with a trailing `%` (`64.08 -> "64.1%"`).
    pub(crate) fn format(&self) -> String {
        use num_format::{Locale, ToFormattedString};
        match *self {
            MetricValue::Count(n) => n.to_formatted_string(&Locale::en),
            MetricValue::Size(n) => format_size(n),
            MetricValue::Rate(r) => format!("{r:.1}%"),
        }
    }
}

/// Format `bytes` as a human-readable byte size string.
///
/// This is the single source of truth for size formatting in `seescc`:
/// [`MetricValue::Size`] and the watch-mode footer both route through it so the
/// cache-size display stays consistent (`809_212_237 -> "771.7 MiB"`). It wraps
/// `human_bytes::human_bytes`, which selects binary units (KiB/MiB/GiB) and one
/// decimal place.
pub(crate) fn format_size(bytes: u64) -> String {
    human_bytes::human_bytes(bytes as f64)
}

/// Extract the [`MetricValue`] for `key` from the parsed `stats`.
///
/// Per-language metrics (`cache_hits`, `cache_misses`, `cache_errors`,
/// `hit_rate`) are filtered by `languages`; an empty `languages` slice sums
/// across all languages. Global counts and sizes ignore `languages`.
pub(crate) fn metric_value(
    key: MetricKey,
    stats: &stats::Stats,
    languages: &[String],
) -> MetricValue {
    let counters = &stats.stats;
    match key {
        MetricKey::CacheHits => {
            MetricValue::Count(lang_sum(&counters.cache_hits.counts, languages))
        }
        MetricKey::CacheMisses => {
            MetricValue::Count(lang_sum(&counters.cache_misses.counts, languages))
        }
        MetricKey::CacheErrors => {
            MetricValue::Count(lang_sum(&counters.cache_errors.counts, languages))
        }
        MetricKey::HitRate => MetricValue::Rate(hit_rate(
            lang_sum(&counters.cache_hits.counts, languages),
            lang_sum(&counters.cache_misses.counts, languages),
        )),
        MetricKey::CompileRequests => MetricValue::Count(counters.compile_requests),
        MetricKey::RequestsExecuted => MetricValue::Count(counters.requests_executed),
        MetricKey::RequestsNotCacheable => MetricValue::Count(counters.requests_not_cacheable),
        MetricKey::RequestsNotCompile => MetricValue::Count(counters.requests_not_compile),
        MetricKey::RequestsUnsupportedCompiler => {
            MetricValue::Count(counters.requests_unsupported_compiler)
        }
        MetricKey::CacheWrites => MetricValue::Count(counters.cache_writes),
        MetricKey::Compilations => MetricValue::Count(counters.compilations),
        MetricKey::CompileFails => MetricValue::Count(counters.compile_fails),
        MetricKey::ForcedRecaches => MetricValue::Count(counters.forced_recaches),
        MetricKey::CacheSize => MetricValue::Size(stats.cache_size),
        MetricKey::MaxCacheSize => MetricValue::Size(stats.max_cache_size),
    }
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

    #[test]
    fn metric_value_cache_hits_all_languages() {
        // Empty `languages` sums every bucket: Assembler 196 + Rust 1718 +
        // C/C++ 516 = 2430.
        let fixture = fixture_stats();
        assert_eq!(
            metric_value(MetricKey::CacheHits, &fixture, &[]),
            MetricValue::Count(196 + 1718 + 516)
        );
    }

    #[test]
    fn metric_value_global_count_ignores_languages() {
        // `compile_requests` is global: the `languages` filter must not change it.
        let fixture = fixture_stats();
        assert_eq!(
            metric_value(MetricKey::CompileRequests, &fixture, &["Rust".to_string()]),
            MetricValue::Count(4786)
        );
        assert_eq!(
            metric_value(MetricKey::CompileRequests, &fixture, &[]),
            MetricValue::Count(4786)
        );
    }

    #[test]
    fn metric_value_cache_size_is_size() {
        let fixture = fixture_stats();
        assert_eq!(
            metric_value(MetricKey::CacheSize, &fixture, &[]),
            MetricValue::Size(809_212_237)
        );
    }

    #[test]
    fn metric_value_hit_rate_rust_only() {
        // Rust hits 1718, Rust misses 963 -> 1718 / 2681 * 100 = 64.0805...%.
        let fixture = fixture_stats();
        let value = metric_value(MetricKey::HitRate, &fixture, &["Rust".to_string()]);
        let MetricValue::Rate(rate) = value else {
            panic!("HitRate must extract to MetricValue::Rate, got {value:?}");
        };
        assert!((rate - 64.080_567).abs() < 1e-3, "rate was {rate}");
        assert_eq!(value.format(), "64.1%");
    }

    #[test]
    fn format_count_uses_thousands_separators() {
        assert_eq!(MetricValue::Count(4786).format(), "4,786");
        assert!(MetricValue::Count(4786).format().contains(','));
    }

    #[test]
    fn format_size_matches_human_bytes() {
        // The exact string `human_bytes` 0.4 returns for this byte count; later
        // slices assert against it, so pin it here.
        assert_eq!(MetricValue::Size(809_212_237).format(), "771.7 MiB");
    }

    #[test]
    fn format_rate_one_decimal_with_percent() {
        assert_eq!(MetricValue::Rate(64.080_567).format(), "64.1%");
        assert_eq!(MetricValue::Rate(0.0).format(), "0.0%");
        assert_eq!(MetricValue::Rate(100.0).format(), "100.0%");
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

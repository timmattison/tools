//! Pure aggregation helpers over the per-language counter maps.
//!
//! These functions turn the raw `HashMap<String, u64>` buckets that
//! [`crate::stats`] parses into the scalar values `seescc` renders: a sum of
//! the counts for a selected set of languages, and the overall cache hit rate.

use std::collections::HashMap;

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

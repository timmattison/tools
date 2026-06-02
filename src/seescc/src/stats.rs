//! Defensive serde types for the sccache `--show-stats --stats-format=json`
//! payload.
//!
//! Every field is annotated `#[serde(default)]` so that unknown or
//! newly-added fields in future sccache versions are silently ignored and
//! missing fields fall back to zero/empty. This keeps parsing resilient across
//! sccache upgrades — a new field never breaks `seescc`.

use std::collections::HashMap;

use serde::Deserialize;

/// A per-language counter bucket (`cache_hits`, `cache_misses`,
/// `cache_errors`).
///
/// sccache reports two maps per bucket: a coarse `counts` map keyed by language
/// label (e.g. `"Rust"`, `"C/C++"`) and a finer `adv_counts` map keyed by
/// toolchain. We intentionally model only `counts`; `adv_counts` is ignored.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct LangCounts {
    /// Per-language compilation counts keyed by sccache's language label.
    #[serde(default)]
    pub counts: HashMap<String, u64>,
}

/// The nested `stats` object inside the sccache payload.
///
/// Holds the request tallies, per-language hit/miss/error buckets, and the
/// write/compile counters that `seescc` surfaces.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Counters {
    /// Total number of compile requests seen.
    #[serde(default)]
    pub compile_requests: u64,
    /// Requests that resulted in actual compilation work being dispatched.
    #[serde(default)]
    pub requests_executed: u64,
    /// Requests rejected as not cacheable.
    #[serde(default)]
    pub requests_not_cacheable: u64,
    /// Requests that were not compilation invocations at all.
    #[serde(default)]
    pub requests_not_compile: u64,
    /// Requests using a compiler sccache does not support.
    #[serde(default)]
    pub requests_unsupported_compiler: u64,
    /// Cache hits, bucketed by language.
    #[serde(default)]
    pub cache_hits: LangCounts,
    /// Cache misses, bucketed by language.
    #[serde(default)]
    pub cache_misses: LangCounts,
    /// Cache errors, bucketed by language.
    #[serde(default)]
    pub cache_errors: LangCounts,
    /// Number of cache writes performed.
    #[serde(default)]
    pub cache_writes: u64,
    /// Number of compilations performed.
    #[serde(default)]
    pub compilations: u64,
    /// Number of compilations that failed.
    #[serde(default)]
    pub compile_fails: u64,
    /// Number of forced recaches.
    #[serde(default)]
    pub forced_recaches: u64,
}

/// The top-level sccache `--show-stats --stats-format=json` payload.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Stats {
    /// The nested counter object.
    #[serde(default)]
    pub stats: Counters,
    /// Current on-disk cache size, in bytes.
    #[serde(default)]
    pub cache_size: u64,
    /// Configured maximum cache size, in bytes.
    #[serde(default)]
    pub max_cache_size: u64,
    /// The reporting sccache version string (e.g. `"0.15.0"`).
    #[serde(default)]
    pub version: String,
}

/// Parse an sccache `--show-stats --stats-format=json` payload into [`Stats`].
pub(crate) fn parse(_json: &str) -> anyhow::Result<Stats> {
    Ok(Stats::default()) // STUB — replaced in GREEN step
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/sccache-0.15.0.json");

    #[test]
    fn parses_captured_fixture() {
        let stats = parse(FIXTURE).expect("fixture should parse");

        assert_eq!(stats.stats.compile_requests, 4786);
        assert_eq!(stats.stats.requests_executed, 3880);
        assert_eq!(stats.stats.cache_hits.counts["Rust"], 1718);
        assert_eq!(stats.stats.cache_misses.counts["Rust"], 963);
        assert_eq!(stats.cache_size, 809_212_237);
        assert_eq!(stats.max_cache_size, 10_737_418_240);
        assert_eq!(stats.version, "0.15.0");
    }

    #[test]
    fn tolerates_unknown_fields() {
        let json = r#"{
            "future_top": 99,
            "stats": {
                "compile_requests": 42,
                "some_new_counter": 1234,
                "cache_hits": { "counts": { "Rust": 7 }, "adv_counts": {} }
            },
            "cache_size": 1,
            "version": "9.9.9"
        }"#;

        let stats = parse(json).expect("unknown fields should be tolerated");

        assert_eq!(stats.stats.compile_requests, 42);
        assert_eq!(stats.stats.cache_hits.counts["Rust"], 7);
        assert_eq!(stats.version, "9.9.9");
    }

    #[test]
    fn empty_counts_maps_ok() {
        let json = r#"{"stats":{"cache_hits":{"counts":{},"adv_counts":{}}}}"#;

        let stats = parse(json).expect("empty counts maps should parse");

        assert!(stats.stats.cache_hits.counts.is_empty());
        assert_eq!(stats.stats.compile_requests, 0);
        assert_eq!(stats.stats.requests_executed, 0);
    }

    #[test]
    fn missing_fields_default_to_zero() {
        let stats = parse("{}").expect("empty object should parse to defaults");

        assert_eq!(stats.stats.compile_requests, 0);
        assert_eq!(stats.stats.cache_writes, 0);
        assert!(stats.stats.cache_hits.counts.is_empty());
        assert!(stats.stats.cache_misses.counts.is_empty());
        assert_eq!(stats.cache_size, 0);
        assert_eq!(stats.max_cache_size, 0);
        assert!(stats.version.is_empty());
    }
}

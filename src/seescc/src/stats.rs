//! Defensive serde types for the sccache `--show-stats --stats-format=json`
//! payload.
//!
//! Every field is annotated `#[serde(default)]` so that unknown or
//! newly-added fields in future sccache versions are silently ignored and
//! missing fields fall back to zero/empty. This keeps parsing resilient across
//! sccache upgrades — a new field never breaks `seescc`.

use std::collections::HashMap;

use serde::Deserialize;

/// Deserialize helper mapping an explicit JSON `null` to the type's default.
///
/// `#[serde(default)]` only supplies a value when a field is *absent*; a field
/// present with value `null` is still routed through the field type's
/// `Deserialize`, which fails for non-`Option` types like `u64` (the source of
/// "invalid type: null, expected u64"). sccache emits `null` for several
/// numeric fields depending on the cache backend — `cache_size`/`max_cache_size`
/// on non-local caches, unset counters — so every field is parsed through this
/// helper, treating `null` the same as an absent key.
fn null_to_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// A per-language counter bucket (`cache_hits`, `cache_misses`,
/// `cache_errors`).
///
/// sccache reports two maps per bucket: a coarse `counts` map keyed by language
/// label (e.g. `"Rust"`, `"C/C++"`) and a finer `adv_counts` map keyed by
/// toolchain. We intentionally model only `counts`; `adv_counts` is ignored.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct LangCounts {
    /// Per-language compilation counts keyed by sccache's language label.
    #[serde(default, deserialize_with = "null_to_default")]
    pub counts: HashMap<String, u64>,
}

/// The nested `stats` object inside the sccache payload.
///
/// Holds the request tallies, per-language hit/miss/error buckets, and the
/// write/compile counters that `seescc` surfaces.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Counters {
    /// Total number of compile requests seen.
    #[serde(default, deserialize_with = "null_to_default")]
    pub compile_requests: u64,
    /// Requests that resulted in actual compilation work being dispatched.
    #[serde(default, deserialize_with = "null_to_default")]
    pub requests_executed: u64,
    /// Requests rejected as not cacheable.
    #[serde(default, deserialize_with = "null_to_default")]
    pub requests_not_cacheable: u64,
    /// Requests that were not compilation invocations at all.
    #[serde(default, deserialize_with = "null_to_default")]
    pub requests_not_compile: u64,
    /// Requests using a compiler sccache does not support.
    #[serde(default, deserialize_with = "null_to_default")]
    pub requests_unsupported_compiler: u64,
    /// Cache hits, bucketed by language.
    #[serde(default, deserialize_with = "null_to_default")]
    pub cache_hits: LangCounts,
    /// Cache misses, bucketed by language.
    #[serde(default, deserialize_with = "null_to_default")]
    pub cache_misses: LangCounts,
    /// Cache errors, bucketed by language.
    #[serde(default, deserialize_with = "null_to_default")]
    pub cache_errors: LangCounts,
    /// Number of cache writes performed.
    #[serde(default, deserialize_with = "null_to_default")]
    pub cache_writes: u64,
    /// Number of compilations performed.
    #[serde(default, deserialize_with = "null_to_default")]
    pub compilations: u64,
    /// Number of compilations that failed.
    #[serde(default, deserialize_with = "null_to_default")]
    pub compile_fails: u64,
    /// Number of forced recaches.
    #[serde(default, deserialize_with = "null_to_default")]
    pub forced_recaches: u64,
}

/// The top-level sccache `--show-stats --stats-format=json` payload.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Stats {
    /// The nested counter object.
    #[serde(default, deserialize_with = "null_to_default")]
    pub stats: Counters,
    /// Current on-disk cache size, in bytes.
    #[serde(default, deserialize_with = "null_to_default")]
    pub cache_size: u64,
    /// Configured maximum cache size, in bytes.
    #[serde(default, deserialize_with = "null_to_default")]
    pub max_cache_size: u64,
    /// The reporting sccache version string (e.g. `"0.15.0"`).
    #[serde(default, deserialize_with = "null_to_default")]
    pub version: String,
}

/// Parse an sccache `--show-stats --stats-format=json` payload into [`Stats`].
pub(crate) fn parse(json: &str) -> anyhow::Result<Stats> {
    serde_json::from_str(json)
        .map_err(|e| anyhow::anyhow!("failed to parse sccache --show-stats JSON: {e}"))
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

    #[test]
    fn tolerates_explicit_null_numeric_fields() {
        // Real sccache emits an explicit `null` for some numeric fields depending
        // on the cache backend — e.g. `cache_size`/`max_cache_size` are null for
        // non-local caches (S3/GHA/redis), and some counters can be null when
        // unset. `#[serde(default)]` only covers *absent* keys; a present `null`
        // is still routed to `u64`'s deserializer and fails with
        // "invalid type: null, expected u64". These must parse as the default.
        let json = r#"{
            "stats": {
                "compile_requests": 10,
                "requests_executed": null,
                "cache_writes": null,
                "cache_hits": { "counts": { "Rust": 5 }, "adv_counts": {} }
            },
            "cache_size": null,
            "max_cache_size": null,
            "version": "0.15.0",
            "multi_level": null
        }"#;

        let stats = parse(json).expect("explicit null numeric fields should parse as defaults");

        assert_eq!(stats.stats.compile_requests, 10);
        assert_eq!(stats.stats.requests_executed, 0);
        assert_eq!(stats.stats.cache_writes, 0);
        assert_eq!(stats.stats.cache_hits.counts["Rust"], 5);
        assert_eq!(stats.cache_size, 0);
        assert_eq!(stats.max_cache_size, 0);
        assert_eq!(stats.version, "0.15.0");
    }
}

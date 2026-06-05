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
    ///
    /// Deliberately parsed and retained as part of the modeled payload — it
    /// identifies which sccache produced a snapshot and is asserted on by the
    /// parse/state tests — but no configured metric surfaces it, so production
    /// code never reads it. The narrow allow documents that this is intentional
    /// rather than an oversight.
    #[allow(
        dead_code,
        reason = "modeled sccache payload field, exercised by tests but not surfaced by any metric"
    )]
    #[serde(default, deserialize_with = "null_to_default")]
    pub version: String,
}

/// Parse an sccache `--show-stats --stats-format=json` payload into [`Stats`].
///
/// The happy path is a buffer that is pure JSON: it is handed straight to
/// `serde_json::from_str` and parsed in a single pass, so a clean payload never
/// pays for a second attempt.
///
/// During the server-start window sccache can prepend warning/progress lines to
/// stdout *before* the JSON object (e.g. `Warning: sccache server is busy`).
/// A direct parse of that buffer fails at line 1 column 1 even though valid JSON
/// follows, which the design spec (§6) says to tolerate rather than surface as a
/// persistent watch-mode error. So when the direct parse fails, this retries
/// from the first line whose trimmed contents begin with `{` and parses the
/// remainder of the buffer from there.
///
/// # Errors
/// Returns an error if the buffer is not valid JSON. When no line begins with
/// `{` there is no JSON object to retry from, so the *original* (line-1) parse
/// error is returned rather than a confusing secondary error from the retry.
pub(crate) fn parse(json: &str) -> anyhow::Result<Stats> {
    let original = match serde_json::from_str(json) {
        Ok(stats) => return Ok(stats),
        Err(e) => e,
    };

    // Retry from the first line whose trimmed contents begin with `{`, skipping
    // leading noise lines. `split_inclusive` keeps each line's terminator, so the
    // running byte offset stays exact across both `\n` and `\r\n` endings, and a
    // line start is always a UTF-8 char boundary, so `get(offset..)` slices
    // safely (and returns `None` rather than panicking if it somehow weren't).
    let mut offset = 0;
    for line in json.split_inclusive('\n') {
        if line.trim_start().starts_with('{') {
            if let Some(stats) = json
                .get(offset..)
                .and_then(|tail| serde_json::from_str(tail).ok())
            {
                return Ok(stats);
            }
            break;
        }
        offset += line.len();
    }

    Err(anyhow::anyhow!(
        "failed to parse sccache --show-stats JSON: {original}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/sccache-0.15.0.json");

    /// A full payload from a remote (S3) backend: identical shape to the local
    /// `FIXTURE`, but `cache_size`/`max_cache_size` are explicit JSON `null`
    /// because non-local caches don't report on-disk sizes. The inline-string
    /// `tolerates_explicit_null_numeric_fields` test covers a trimmed snippet;
    /// this fixture pins the same tolerance against a realistic full payload
    /// shared with the integration suite's stub-driven path.
    const FIXTURE_NULL_SIZES: &str =
        include_str!("../tests/fixtures/sccache-0.15.0-null-sizes.json");

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
    fn parses_remote_backend_fixture_with_null_sizes() {
        // A remote (S3/GHA/redis) backend emits explicit `null` for
        // `cache_size`/`max_cache_size` since there is no on-disk cache to
        // measure. This pins that real-world payload shape end-to-end through
        // the same `parse` the binary uses: the null sizes must land as their
        // documented default (0) while every other field keeps its real value.
        let stats = parse(FIXTURE_NULL_SIZES).expect("null-size remote fixture should parse");

        // Null numerics default to 0.
        assert_eq!(stats.cache_size, 0);
        assert_eq!(stats.max_cache_size, 0);

        // Non-null fields keep their real values, matching the local fixture.
        assert_eq!(stats.stats.compile_requests, 4786);
        assert_eq!(stats.stats.requests_executed, 3880);
        assert_eq!(stats.stats.cache_writes, 1373);
        assert_eq!(stats.stats.cache_hits.counts["Rust"], 1718);
        assert_eq!(stats.stats.cache_misses.counts["Rust"], 963);
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

    #[test]
    fn skips_leading_noise_lines_before_json() {
        // During the server-start window sccache can prepend warning/progress
        // lines to stdout before the JSON object proper (e.g. "Warning: sccache
        // server is busy"). A direct `from_str` on that whole buffer fails with
        // "expected value at line 1 column 1" even though valid JSON follows.
        // `parse` must recover by retrying from the first line that begins with
        // `{`, so the false failure does not raise the watch error banner.
        let json =
            format!("Warning: sccache server is busy\nStarting sccache server...\n{FIXTURE}");

        let stats = parse(&json).expect("leading noise lines should be skipped");

        assert_eq!(stats.stats.compile_requests, 4786);
        assert_eq!(stats.stats.cache_hits.counts["Rust"], 1718);
        assert_eq!(stats.cache_size, 809_212_237);
        assert_eq!(stats.version, "0.15.0");
    }

    #[test]
    fn garbage_without_any_json_object_errors() {
        // If no line ever starts with `{`, there is no JSON object to retry from.
        // `parse` must still surface an error rather than silently succeed.
        let json = "Warning: sccache server is busy\nStarting sccache server...\n";

        assert!(
            parse(json).is_err(),
            "noise with no JSON object anywhere must still error"
        );
    }
}

//! TOML configuration and CLI-override plumbing for `seescc`.
//!
//! This module owns the small, human-friendly value parsers that both the
//! config file and the command-line layer share. The first of these is
//! [`parse_duration`], which turns strings like `500ms`, `1s`, `15m`, and `1h`
//! into [`std::time::Duration`] values. Parse failures surface as
//! [`ConfigError`] so the CLI can report exactly which input was rejected.

use std::time::Duration;

/// Errors produced while interpreting `seescc` configuration values.
///
/// Designed to grow: later slices add variants for malformed TOML, unknown
/// keys, and out-of-range numeric settings. Each variant carries enough context
/// to name the offending input in a user-facing message.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigError {
    /// A duration string was empty, missing its magnitude, used an unknown unit
    /// suffix, or had a non-integer / overflowing magnitude.
    #[error(
        "invalid duration {input:?}: expected an integer magnitude followed by one of \
         the unit suffixes `ms`, `s`, `m`, or `h` (e.g. `500ms`, `1s`, `15m`, `1h`)"
    )]
    InvalidDuration {
        /// The rejected input, echoed back so the user can spot the typo.
        input: String,
    },

    /// A config string did not name any known metric in the catalog.
    ///
    /// The message lists every valid key so the user can correct the typo
    /// without consulting external documentation.
    #[error("unknown metric key {key:?}; valid keys are: {valid}")]
    UnknownMetricKey {
        /// The rejected key, echoed back so the user can spot the typo.
        key: String,
        /// The full catalog of valid keys, joined with `", "`.
        valid: String,
    },
}

/// The classification of a metric's value, which controls how it renders.
///
/// `seescc` formats counts with thousands separators, sizes in human-friendly
/// byte units, and rates as percentages — so each catalog key declares which
/// presentation it expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetricKind {
    /// A plain integer tally (the common case).
    Count,
    /// A byte size (`cache_size`, `max_cache_size`).
    Size,
    /// A percentage rate (`hit_rate`).
    Rate,
}

/// A single key in the metric catalog `seescc` knows how to surface.
///
/// Variants split into two families: *per-language* keys, whose values are
/// filtered by the `languages` setting ([`MetricKey::is_per_language`] is
/// `true`), and *global* keys that apply to the whole cache. Each variant maps
/// to a canonical TOML string via [`MetricKey::as_config_key`] and a pretty
/// display string via [`MetricKey::default_label`]. The [`MetricKey::ALL`]
/// catalog and [`MetricKey::parse`] are kept in lock-step by a round-trip test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetricKey {
    /// Total compile requests seen. Global.
    CompileRequests,
    /// Requests that dispatched actual compilation work. Global.
    RequestsExecuted,
    /// Cache hits, filtered by the selected languages. Per-language.
    CacheHits,
    /// Cache misses, filtered by the selected languages. Per-language.
    CacheMisses,
    /// Cache errors, filtered by the selected languages. Per-language.
    CacheErrors,
    /// Cache hit rate as a percentage. Per-language.
    HitRate,
    /// Requests rejected as not cacheable. Global.
    RequestsNotCacheable,
    /// Requests that were not compilation invocations. Global.
    RequestsNotCompile,
    /// Requests using a compiler sccache does not support. Global.
    RequestsUnsupportedCompiler,
    /// Cache writes performed. Global.
    CacheWrites,
    /// Compilations performed. Global.
    Compilations,
    /// Compilations that failed. Global.
    CompileFails,
    /// Forced recaches performed. Global.
    ForcedRecaches,
    /// Current on-disk cache size, in bytes. Global.
    CacheSize,
    /// Configured maximum cache size, in bytes. Global.
    MaxCacheSize,
}

impl MetricKey {
    /// The full catalog of metric keys, in stable display order.
    ///
    /// The four keys Phase 1 surfaces first (`compile_requests`,
    /// `requests_executed`, `cache_hits`, `cache_misses`) lead the list; the
    /// remaining keys follow. This ordering is the single source of truth for
    /// the unknown-key error listing and is exercised by the round-trip test.
    pub(crate) const ALL: [MetricKey; 15] = [
        MetricKey::CompileRequests,
        MetricKey::RequestsExecuted,
        MetricKey::CacheHits,
        MetricKey::CacheMisses,
        MetricKey::HitRate,
        MetricKey::CacheErrors,
        MetricKey::RequestsNotCacheable,
        MetricKey::RequestsNotCompile,
        MetricKey::RequestsUnsupportedCompiler,
        MetricKey::CacheWrites,
        MetricKey::Compilations,
        MetricKey::CompileFails,
        MetricKey::ForcedRecaches,
        MetricKey::CacheSize,
        MetricKey::MaxCacheSize,
    ];

    /// The canonical TOML string key for this metric (e.g. `"compile_requests"`).
    pub(crate) fn as_config_key(&self) -> &'static str {
        match self {
            MetricKey::CompileRequests => "compile_requests",
            MetricKey::RequestsExecuted => "requests_executed",
            MetricKey::CacheHits => "cache_hits",
            MetricKey::CacheMisses => "cache_misses",
            MetricKey::HitRate => "hit_rate",
            MetricKey::CacheErrors => "cache_errors",
            MetricKey::RequestsNotCacheable => "requests_not_cacheable",
            MetricKey::RequestsNotCompile => "requests_not_compile",
            MetricKey::RequestsUnsupportedCompiler => "requests_unsupported_compiler",
            MetricKey::CacheWrites => "cache_writes",
            MetricKey::Compilations => "compilations",
            MetricKey::CompileFails => "compile_fails",
            MetricKey::ForcedRecaches => "forced_recaches",
            MetricKey::CacheSize => "cache_size",
            MetricKey::MaxCacheSize => "max_cache_size",
        }
    }

    /// The pretty, human-facing label for this metric (e.g. `"Compile requests"`).
    pub(crate) fn default_label(&self) -> &'static str {
        match self {
            MetricKey::CompileRequests => "Compile requests",
            MetricKey::RequestsExecuted => "Requests executed",
            MetricKey::CacheHits => "Cache hits",
            MetricKey::CacheMisses => "Cache misses",
            MetricKey::HitRate => "Hit rate",
            MetricKey::CacheErrors => "Cache errors",
            MetricKey::RequestsNotCacheable => "Requests not cacheable",
            MetricKey::RequestsNotCompile => "Requests not compile",
            MetricKey::RequestsUnsupportedCompiler => "Unsupported compiler",
            MetricKey::CacheWrites => "Cache writes",
            MetricKey::Compilations => "Compilations",
            MetricKey::CompileFails => "Compile fails",
            MetricKey::ForcedRecaches => "Forced recaches",
            MetricKey::CacheSize => "Cache size",
            MetricKey::MaxCacheSize => "Max cache size",
        }
    }

    /// Whether this metric's value is filtered by the `languages` setting.
    pub(crate) fn is_per_language(&self) -> bool {
        matches!(
            self,
            MetricKey::CacheHits
                | MetricKey::CacheMisses
                | MetricKey::CacheErrors
                | MetricKey::HitRate
        )
    }

    /// The presentation [`MetricKind`] for this metric.
    pub(crate) fn kind(&self) -> MetricKind {
        match self {
            MetricKey::CacheSize | MetricKey::MaxCacheSize => MetricKind::Size,
            MetricKey::HitRate => MetricKind::Rate,
            _ => MetricKind::Count,
        }
    }

    /// Parse a config string into a [`MetricKey`].
    ///
    /// # Errors
    /// Returns [`ConfigError::UnknownMetricKey`] when `s` does not exactly match
    /// any catalog key's [`MetricKey::as_config_key`]; the error lists the full
    /// catalog so the user can correct the typo.
    pub(crate) fn parse(s: &str) -> Result<MetricKey, ConfigError> {
        MetricKey::ALL
            .iter()
            .find(|key| key.as_config_key() == s)
            .copied()
            .ok_or_else(|| ConfigError::UnknownMetricKey {
                key: s.to_string(),
                valid: MetricKey::ALL
                    .iter()
                    .map(MetricKey::as_config_key)
                    .collect::<Vec<_>>()
                    .join(", "),
            })
    }
}

/// Parse a human-friendly duration string into a [`Duration`].
///
/// Accepts an integer magnitude immediately followed by one of the unit
/// suffixes `ms`, `s`, `m`, or `h` (for example `500ms`, `1s`, `15m`, `1h`).
/// Surrounding whitespace is trimmed. The `ms` suffix is checked before `s`
/// because both end in `s`.
///
/// # Errors
/// Returns [`ConfigError::InvalidDuration`] when `s` is empty, omits the
/// magnitude (`"s"`), omits or uses an unknown unit suffix (`"10"`, `"10x"`),
/// has a non-integer magnitude (`"1.5s"`), or whose magnitude overflows the
/// internal `u64` second/millisecond arithmetic.
pub(crate) fn parse_duration(s: &str) -> Result<Duration, ConfigError> {
    let trimmed = s.trim();
    let invalid = || ConfigError::InvalidDuration {
        input: trimmed.to_string(),
    };

    // Match the longest suffix first so `ms` is not mistaken for `s`.
    let (digits, build): (&str, fn(u64) -> Option<Duration>) =
        if let Some(d) = trimmed.strip_suffix("ms") {
            (d, |n| Some(Duration::from_millis(n)))
        } else if let Some(d) = trimmed.strip_suffix('s') {
            (d, |n| Some(Duration::from_secs(n)))
        } else if let Some(d) = trimmed.strip_suffix('m') {
            (d, |n| n.checked_mul(60).map(Duration::from_secs))
        } else if let Some(d) = trimmed.strip_suffix('h') {
            (d, |n| n.checked_mul(3600).map(Duration::from_secs))
        } else {
            return Err(invalid());
        };

    let magnitude: u64 = digits.parse().map_err(|_| invalid())?;
    build(magnitude).ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_maps_known_keys_to_variants() {
        assert_eq!(
            MetricKey::parse("cache_hits").expect("`cache_hits` is a known key"),
            MetricKey::CacheHits
        );
        assert_eq!(
            MetricKey::parse("compile_requests").expect("`compile_requests` is a known key"),
            MetricKey::CompileRequests
        );
    }

    #[test]
    fn parse_round_trips_every_catalog_key() {
        // The catalog and the parser must never drift: every key's canonical
        // config string must parse back to that exact key.
        for key in MetricKey::ALL {
            let parsed = MetricKey::parse(key.as_config_key())
                .unwrap_or_else(|e| panic!("round-trip failed for {key:?}: {e}"));
            assert_eq!(
                parsed,
                key,
                "round-trip mismatch for {key:?} ({:?})",
                key.as_config_key()
            );
        }
    }

    #[test]
    fn parse_rejects_unknown_key_and_lists_catalog() {
        let err = MetricKey::parse("bogus").expect_err("`bogus` is not a catalog key");
        let message = err.to_string();
        assert!(message.contains("bogus"), "message was: {message}");
        // The error must actually enumerate the catalog, not just fire.
        assert!(message.contains("cache_hits"), "message was: {message}");
        assert!(
            message.contains("compile_requests"),
            "message was: {message}"
        );
    }

    #[test]
    fn is_per_language_splits_the_two_families() {
        assert!(MetricKey::CacheHits.is_per_language());
        assert!(MetricKey::CacheMisses.is_per_language());
        assert!(MetricKey::CacheErrors.is_per_language());
        assert!(MetricKey::HitRate.is_per_language());

        assert!(!MetricKey::CompileRequests.is_per_language());
        assert!(!MetricKey::CacheSize.is_per_language());
    }

    #[test]
    fn kind_classifies_size_rate_and_count() {
        assert_eq!(MetricKey::CacheSize.kind(), MetricKind::Size);
        assert_eq!(MetricKey::MaxCacheSize.kind(), MetricKind::Size);
        assert_eq!(MetricKey::HitRate.kind(), MetricKind::Rate);
        assert_eq!(MetricKey::CompileRequests.kind(), MetricKind::Count);
        assert_eq!(MetricKey::CacheHits.kind(), MetricKind::Count);
    }

    #[test]
    fn default_label_matches_phase_one_labels() {
        assert_eq!(
            MetricKey::CompileRequests.default_label(),
            "Compile requests"
        );
        assert_eq!(
            MetricKey::RequestsExecuted.default_label(),
            "Requests executed"
        );
        assert_eq!(MetricKey::CacheHits.default_label(), "Cache hits");
        assert_eq!(MetricKey::CacheMisses.default_label(), "Cache misses");
        assert_eq!(MetricKey::HitRate.default_label(), "Hit rate");
    }

    #[test]
    fn catalog_has_fifteen_unique_keys() {
        assert_eq!(MetricKey::ALL.len(), 15);

        let mut seen: Vec<&'static str> = MetricKey::ALL
            .iter()
            .map(MetricKey::as_config_key)
            .collect();
        seen.sort_unstable();
        let unique = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), unique, "duplicate as_config_key in catalog");
    }

    #[test]
    fn parses_each_unit_suffix() {
        assert_eq!(
            parse_duration("500ms").expect("500ms should parse"),
            Duration::from_millis(500)
        );
        assert_eq!(
            parse_duration("1s").expect("1s should parse"),
            Duration::from_secs(1)
        );
        assert_eq!(
            parse_duration("15m").expect("15m should parse"),
            Duration::from_secs(900)
        );
        assert_eq!(
            parse_duration("1h").expect("1h should parse"),
            Duration::from_secs(3600)
        );
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(
            parse_duration("  250ms  ").expect("padded input should parse"),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn rejects_empty_string() {
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn rejects_missing_magnitude() {
        assert!(parse_duration("s").is_err());
    }

    #[test]
    fn rejects_missing_suffix() {
        assert!(parse_duration("10").is_err());
    }

    #[test]
    fn rejects_unknown_suffix() {
        assert!(parse_duration("10x").is_err());
    }

    #[test]
    fn rejects_non_integer_magnitude() {
        assert!(parse_duration("1.5s").is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_duration("garbage").is_err());
    }

    #[test]
    fn rejects_overflowing_magnitude() {
        // u64::MAX hours overflows the n * 3600 multiplication.
        assert!(parse_duration("18446744073709551615h").is_err());
    }

    #[test]
    fn error_message_names_input_and_units() {
        let err = parse_duration("10x").expect_err("`10x` must be rejected");
        let message = err.to_string();
        assert!(message.contains("\"10x\""), "message was: {message}");
        assert!(message.contains("ms"), "message was: {message}");
        assert!(message.contains('h'), "message was: {message}");
    }
}

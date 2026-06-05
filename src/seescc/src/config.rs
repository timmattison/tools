//! TOML configuration and CLI-override plumbing for `seescc`.
//!
//! This module owns the small, human-friendly value parsers that both the
//! config file and the command-line layer share. The first of these is
//! [`parse_duration`], which turns strings like `500ms`, `1s`, `15m`, and `1h`
//! into [`std::time::Duration`] values. Parse failures surface as
//! [`ConfigError`] so the CLI can report exactly which input was rejected.

use std::time::Duration;

use serde::Deserialize;

/// Errors produced while interpreting `seescc` configuration values.
///
/// Designed to grow: later slices add variants for malformed TOML, unknown
/// keys, and out-of-range numeric settings. Each variant carries enough context
/// to name the offending input in a user-facing message.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigError {
    /// A duration string was empty, missing its magnitude, used an unknown unit
    /// suffix, had a zero, non-integer, or overflowing magnitude.
    #[error(
        "invalid duration {input:?}: expected a positive (non-zero) integer magnitude \
         followed by one of the unit suffixes `ms`, `s`, `m`, or `h` \
         (e.g. `500ms`, `1s`, `15m`, `1h`)"
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

    /// The supplied config string was not syntactically valid TOML, or did not
    /// match the expected shape (wrong types, missing required fields).
    #[error("invalid config TOML: {0}")]
    Toml(#[from] toml::de::Error),

    /// A config file could not be read from disk — for example, an explicit
    /// `--config` path that does not exist or is not readable. The `path` is
    /// echoed so the user can see exactly which file failed.
    #[error("failed to read config file {path}: {source}")]
    Io {
        /// The path that failed to read, as a display string.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// [`write_default_config`] was asked to write to a path that already exists
    /// and `force` was not set, so the existing file was left untouched.
    #[error("config file already exists at {path} (use --force to overwrite)")]
    AlreadyExists {
        /// The path that already exists, as a display string.
        path: String,
    },

    /// A config file (or one of its parent directories) could not be written —
    /// for example, a permission failure under [`write_default_config`]. The
    /// `path` is echoed so the user can see exactly which write failed.
    #[error("failed to write config file {path}: {source}")]
    WriteFailed {
        /// The path that failed to write, as a display string.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
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
/// Variants split into two families: *per-language* keys
/// (`cache_hits`/`cache_misses`/`cache_errors`/`hit_rate`), whose values are
/// filtered by the `languages` setting, and *global* keys that apply to the
/// whole cache. The per-language filtering lives in
/// [`crate::aggregate::metric_value`] (which routes those keys through
/// [`crate::aggregate::lang_sum`]) rather than as a predicate here. Each variant
/// maps to a canonical TOML string via [`MetricKey::as_config_key`] and a pretty
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
/// Accepts a positive (non-zero) integer magnitude immediately followed by one
/// of the unit suffixes `ms`, `s`, `m`, or `h` (for example `500ms`, `1s`,
/// `15m`, `1h`). Surrounding whitespace is trimmed. The `ms` suffix is checked
/// before `s` because both end in `s`. A zero magnitude is rejected: a zero
/// `poll_interval` would make the watch loop spin without delay.
///
/// # Errors
/// Returns [`ConfigError::InvalidDuration`] when `s` is empty, omits the
/// magnitude (`"s"`), omits or uses an unknown unit suffix (`"10"`, `"10x"`),
/// has a non-integer magnitude (`"1.5s"`), has a zero magnitude (`"0s"`), or
/// whose magnitude overflows the internal `u64` second/millisecond arithmetic.
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
    // Reject a zero magnitude: a zero `poll_interval` makes the watch loop's
    // `recv_timeout(ZERO)` time out immediately, busy-looping the CPU and
    // spawning an `sccache --show-stats` subprocess as fast as the OS allows.
    if magnitude == 0 {
        return Err(invalid());
    }
    build(magnitude).ok_or_else(invalid)
}

/// The default poll interval literal, parsed by [`Config::from_toml`] when the
/// `poll_interval` field is absent. Kept as a named const so the fallback and
/// the annotated [`DEFAULT_CONFIG_TOML`] never drift.
const DEFAULT_POLL_INTERVAL: &str = "1s";

/// The default sparkline-history window literal, used as the `window` fallback.
const DEFAULT_WINDOW: &str = "15m";

/// The default language filter applied when `languages` is absent.
const DEFAULT_LANGUAGE: &str = "Rust";

/// The application subdirectory under the platform config directory in which
/// `seescc` looks for its config file (i.e. `<config_dir>/seescc/`).
const APP_CONFIG_SUBDIR: &str = "seescc";

/// The config file name `seescc` reads from the XDG/platform config directory.
const CONFIG_FILE_NAME: &str = "config.toml";

/// The built-in default configuration, expressed as an annotated TOML document.
///
/// This is the single source of truth for `seescc`'s defaults: [`Config::default`]
/// parses it, and a later slice writes it verbatim to disk when the user has no
/// config file. The comments are intentional and survive the round trip because
/// they are part of the on-disk artifact, not of the parsed [`Config`].
///
/// It must stay byte-for-byte parseable by [`Config::from_toml`]; a test asserts
/// `Config::from_toml(DEFAULT_CONFIG_TOML) == Config::default()`.
pub(crate) const DEFAULT_CONFIG_TOML: &str = r#"# seescc configuration
# poll_interval / window accept a positive integer + unit suffix: ms, s, m, h
poll_interval = "1s"      # how often to query sccache
window        = "15m"     # sparkline history retention

# Per-language metrics are filtered to these languages; [] means sum across all.
languages = ["Rust"]

# Rows to show, in order. `label` is optional (a sensible default per key is used).
# `spark` defaults to false.
metrics = [
  { key = "compile_requests",  label = "Compile requests" },
  { key = "requests_executed", label = "Requests executed" },
  { key = "cache_hits",        label = "Cache hits",   spark = true },
  { key = "cache_misses",      label = "Cache misses", spark = true },
  { key = "hit_rate",          label = "Hit rate",     spark = true },
]
"#;

/// A fully resolved `seescc` configuration.
///
/// Produced by [`Config::from_toml`] (and, for the built-in defaults, by
/// [`Config::default`]). All durations are concrete, the language filter is a
/// plain list (empty means "sum across all languages"), and every metric row has
/// its label resolved — the user's explicit `label` when present, otherwise the
/// metric's [`MetricKey::default_label`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Config {
    /// How often the live watch loop re-queries sccache.
    pub poll_interval: Duration,
    /// How much sparkline history to retain.
    pub window: Duration,
    /// Languages whose per-language buckets are surfaced; empty sums across all.
    pub languages: Vec<String>,
    /// The rows to display, in order.
    pub metrics: Vec<MetricSpec>,
}

/// A single resolved metric row in a [`Config`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MetricSpec {
    /// Which catalog metric this row surfaces.
    pub key: MetricKey,
    /// The display label: the user's `label` if given, else [`MetricKey::default_label`].
    pub label: String,
    /// Whether to render a sparkline of this metric's history.
    pub spark: bool,
}

/// The raw, untyped shape of a `seescc` config file, deserialized straight from
/// TOML before validation/conversion into a [`Config`]. Every field is optional
/// so absent keys can fall back to the built-in defaults.
#[derive(Debug, Deserialize)]
struct RawConfig {
    /// Raw `poll_interval` duration string, still to be parsed.
    poll_interval: Option<String>,
    /// Raw `window` duration string, still to be parsed.
    window: Option<String>,
    /// Raw language filter; `None` means "use the default".
    languages: Option<Vec<String>>,
    /// Raw metric rows; `None` means "use the default set".
    metrics: Option<Vec<RawMetric>>,
}

/// The raw, untyped shape of a single `[[metrics]]` entry.
#[derive(Debug, Deserialize)]
struct RawMetric {
    /// The metric key string, validated against the catalog by [`MetricKey::parse`].
    key: String,
    /// The optional override label.
    label: Option<String>,
    /// Whether to render a sparkline; defaults to `false` when omitted.
    #[serde(default)]
    spark: bool,
}

impl Config {
    /// Parse and validate a `seescc` config from a TOML string.
    ///
    /// Absent top-level fields fall back to the built-in defaults
    /// (`poll_interval = "1s"`, `window = "15m"`, `languages = ["Rust"]`, and the
    /// Phase-1 five-metric set). An explicit empty `metrics = []` is honored as an
    /// empty row list rather than replaced by the defaults; only an *absent*
    /// `metrics` key triggers the default set. Each metric's label resolves to the
    /// user's `label` when present, otherwise [`MetricKey::default_label`].
    ///
    /// # Errors
    /// Returns [`ConfigError::Toml`] when `s` is not valid TOML or does not match
    /// the expected shape, [`ConfigError::InvalidDuration`] when `poll_interval`
    /// or `window` cannot be parsed, and [`ConfigError::UnknownMetricKey`] when a
    /// metric names a key that is not in the catalog.
    pub(crate) fn from_toml(s: &str) -> Result<Config, ConfigError> {
        let raw: RawConfig = toml::from_str(s)?;

        let poll_interval = match raw.poll_interval {
            Some(value) => parse_duration(&value)?,
            None => parse_duration(DEFAULT_POLL_INTERVAL)?,
        };
        let window = match raw.window {
            Some(value) => parse_duration(&value)?,
            None => parse_duration(DEFAULT_WINDOW)?,
        };
        let languages = raw
            .languages
            .unwrap_or_else(|| vec![DEFAULT_LANGUAGE.to_string()]);

        // An *absent* `metrics` key falls back to the default set; an explicit
        // empty `metrics = []` is honored as an empty list.
        let metrics = match raw.metrics {
            Some(raw_metrics) => raw_metrics
                .into_iter()
                .map(MetricSpec::from_raw)
                .collect::<Result<Vec<_>, _>>()?,
            None => default_metrics()?,
        };

        Ok(Config {
            poll_interval,
            window,
            languages,
            metrics,
        })
    }

    /// Apply `--poll-interval` / `--window` CLI overrides (each an optional
    /// duration string) on top of a loaded config, replacing the corresponding
    /// field. An override of `None` leaves that field unchanged; a present
    /// override is parsed via [`parse_duration`] and replaces the field.
    ///
    /// # Errors
    /// Returns [`ConfigError::InvalidDuration`] when a present override string is
    /// not a valid duration (empty, missing magnitude, unknown unit suffix,
    /// non-integer, or overflowing).
    pub(crate) fn with_overrides(
        mut self,
        poll_interval: Option<&str>,
        window: Option<&str>,
    ) -> Result<Config, ConfigError> {
        if let Some(value) = poll_interval {
            self.poll_interval = parse_duration(value)?;
        }
        if let Some(value) = window {
            self.window = parse_duration(value)?;
        }
        Ok(self)
    }
}

impl MetricSpec {
    /// Validate and resolve a single raw metric row into a [`MetricSpec`].
    ///
    /// The key is checked against the catalog and the label resolves to the
    /// user's `label` when present, otherwise [`MetricKey::default_label`].
    ///
    /// # Errors
    /// Returns [`ConfigError::UnknownMetricKey`] when `raw.key` is not a catalog key.
    fn from_raw(raw: RawMetric) -> Result<MetricSpec, ConfigError> {
        let key = MetricKey::parse(&raw.key)?;
        let label = raw.label.unwrap_or_else(|| key.default_label().to_string());
        Ok(MetricSpec {
            key,
            label,
            spark: raw.spark,
        })
    }
}

/// The built-in default metric rows, derived from [`DEFAULT_CONFIG_TOML`].
///
/// Used as the fallback when a config omits `metrics` entirely. Parsing the
/// default document's `metrics` table here (rather than hand-coding the list)
/// keeps [`DEFAULT_CONFIG_TOML`] the single source of truth.
///
/// # Errors
/// Returns a [`ConfigError`] only if the built-in document is malformed, which a
/// test guards against.
fn default_metrics() -> Result<Vec<MetricSpec>, ConfigError> {
    let raw: RawConfig = toml::from_str(DEFAULT_CONFIG_TOML)?;
    raw.metrics
        .unwrap_or_default()
        .into_iter()
        .map(MetricSpec::from_raw)
        .collect()
}

impl Default for Config {
    fn default() -> Self {
        Self::from_toml(DEFAULT_CONFIG_TOML).expect("built-in DEFAULT_CONFIG_TOML must parse")
    }
}

/// Read a config file from disk, wrapping any I/O failure into a
/// [`ConfigError::Io`] that names the offending path.
///
/// # Errors
/// Returns [`ConfigError::Io`] when `path` cannot be read (missing, permission
/// denied, etc.), with the path rendered into the message.
fn read_config_file(path: &std::path::Path) -> Result<String, ConfigError> {
    std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.display().to_string(),
        source,
    })
}

/// Write the annotated built-in default config to `path`, creating any missing
/// parent directories.
///
/// Refuses to overwrite an existing file unless `force` is set; in that case the
/// existing file is left untouched. The bytes written are [`DEFAULT_CONFIG_TOML`]
/// verbatim, so the on-disk artifact keeps its explanatory comments.
///
/// # Errors
/// Returns [`ConfigError::AlreadyExists`] when `path` already exists and `force`
/// is `false`, and [`ConfigError::WriteFailed`] when creating the parent
/// directories or writing the file fails.
pub(crate) fn write_default_config(path: &std::path::Path, force: bool) -> Result<(), ConfigError> {
    if path.exists() && !force {
        return Err(ConfigError::AlreadyExists {
            path: path.display().to_string(),
        });
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::WriteFailed {
                path: parent.display().to_string(),
                source,
            })?;
        }
    }

    std::fs::write(path, DEFAULT_CONFIG_TOML).map_err(|source| ConfigError::WriteFailed {
        path: path.display().to_string(),
        source,
    })?;

    Ok(())
}

/// Resolve the effective `seescc` configuration, honoring config-file
/// precedence: an explicit `--config` path wins; otherwise the per-user XDG
/// config file is used if it exists; otherwise the built-in defaults apply.
///
/// This is the real-world entry point. It locates the platform config directory
/// via [`dirs::config_dir`], joins on `seescc/config.toml`, and delegates the
/// precedence logic to [`load_from`] (which is what tests exercise, so the real
/// `dirs` lookup never participates in tests).
///
/// # Errors
/// Returns [`ConfigError::Io`] when an explicit path cannot be read, and
/// [`ConfigError::Toml`] / [`ConfigError::InvalidDuration`] /
/// [`ConfigError::UnknownMetricKey`] when the chosen file fails to parse.
pub(crate) fn load(explicit: Option<&std::path::Path>) -> Result<Config, ConfigError> {
    load_from(explicit, default_config_path().as_deref())
}

/// The per-user XDG/platform config path `seescc` reads by default and writes
/// when `--write-default-config` is given without an explicit `--config`.
///
/// Resolves to `<config_dir>/seescc/config.toml`, where `<config_dir>` is the
/// platform config directory from [`dirs::config_dir`]. Returns `None` when the
/// platform exposes no config directory (rare; e.g. `$HOME` unset on Unix).
pub(crate) fn default_config_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join(APP_CONFIG_SUBDIR).join(CONFIG_FILE_NAME))
}

/// The testable core of [`load`]: apply config-file precedence given an
/// already-resolved XDG candidate path.
///
/// - If `explicit` is `Some(path)`, read it (a missing explicit path is an
///   error, since the user asked for that file specifically) and parse it.
/// - Else if `xdg_candidate` is `Some(path)` and the file exists, read and parse
///   it (read/parse errors propagate).
/// - Else fall back to [`Config::default`].
///
/// Keeping the XDG candidate injectable lets tests drive every branch without
/// touching the developer's real `dirs::config_dir()`.
///
/// # Errors
/// Returns [`ConfigError::Io`] when the explicit path cannot be read, and the
/// parse-related [`ConfigError`] variants when the chosen file is not valid.
fn load_from(
    explicit: Option<&std::path::Path>,
    xdg_candidate: Option<&std::path::Path>,
) -> Result<Config, ConfigError> {
    if let Some(path) = explicit {
        let contents = read_config_file(path)?;
        return Config::from_toml(&contents);
    }

    if let Some(path) = xdg_candidate {
        if path.exists() {
            let contents = read_config_file(path)?;
            return Config::from_toml(&contents);
        }
    }

    Ok(Config::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A minimal explicit config: no language filter, a single `cache_writes`
    /// row. Distinguishable from the XDG fixture below.
    const EXPLICIT_TOML: &str = r#"
languages = []
metrics = [ { key = "cache_writes" } ]
"#;

    /// A distinguishable XDG config: a single `forced_recaches` row. Different
    /// from both the explicit fixture and the built-in defaults.
    const XDG_TOML: &str = r#"
metrics = [ { key = "forced_recaches" } ]
"#;

    #[test]
    fn explicit_config_wins_over_xdg() {
        let dir = tempfile::tempdir().expect("tempdir");
        let explicit_path = dir.path().join("explicit.toml");
        let xdg_path = dir.path().join("xdg.toml");
        fs::write(&explicit_path, EXPLICIT_TOML).expect("write explicit");
        fs::write(&xdg_path, XDG_TOML).expect("write xdg");

        let loaded =
            load_from(Some(&explicit_path), Some(&xdg_path)).expect("explicit config should load");

        let expected_explicit = Config::from_toml(EXPLICIT_TOML).expect("explicit fixture parses");
        let xdg_config = Config::from_toml(XDG_TOML).expect("xdg fixture parses");

        assert_eq!(
            loaded, expected_explicit,
            "explicit --config must win over the XDG file"
        );
        assert_ne!(
            loaded, xdg_config,
            "explicit win must not silently use the XDG config"
        );
    }

    #[test]
    fn xdg_config_is_used_when_no_explicit_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let xdg_path = dir.path().join("xdg.toml");
        fs::write(&xdg_path, XDG_TOML).expect("write xdg");

        let loaded = load_from(None, Some(&xdg_path)).expect("xdg config should load");

        assert_eq!(
            loaded,
            Config::from_toml(XDG_TOML).expect("xdg fixture parses"),
            "with no explicit path, an existing XDG file must be used"
        );
    }

    #[test]
    fn defaults_used_when_xdg_candidate_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A path inside the tempdir that is never created.
        let missing = dir.path().join("does-not-exist.toml");
        assert!(!missing.exists(), "precondition: file must not exist");

        let loaded = load_from(None, Some(&missing)).expect("missing XDG file falls back");

        assert_eq!(
            loaded,
            Config::default(),
            "a non-existent XDG candidate must fall back to defaults"
        );
    }

    #[test]
    fn defaults_used_when_no_xdg_candidate_at_all() {
        let loaded = load_from(None, None).expect("no candidate falls back to defaults");
        assert_eq!(
            loaded,
            Config::default(),
            "with no explicit path and no XDG candidate, defaults apply"
        );
    }

    #[test]
    fn explicit_missing_path_is_an_io_error_naming_the_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope.toml");
        assert!(!missing.exists(), "precondition: file must not exist");

        let err = load_from(Some(&missing), None)
            .expect_err("an explicit path that does not exist must error");
        assert!(
            matches!(err, ConfigError::Io { .. }),
            "expected ConfigError::Io, got: {err:?}"
        );
        let message = err.to_string();
        assert!(
            message.contains(&missing.display().to_string()),
            "error must name the failing path; message was: {message}"
        );
    }

    #[test]
    fn invalid_toml_in_a_real_file_propagates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bad.toml");
        fs::write(&path, r#"poll_interval = "nope""#).expect("write bad config");

        let err = load_from(Some(&path), None)
            .expect_err("an unparseable duration in a real file must error");
        assert!(
            matches!(err, ConfigError::InvalidDuration { .. }),
            "expected InvalidDuration to propagate, got: {err:?}"
        );
    }

    #[test]
    fn default_config_has_phase_one_five_metrics_in_order() {
        let config = Config::default();
        let keys: Vec<MetricKey> = config.metrics.iter().map(|m| m.key).collect();
        assert_eq!(
            keys,
            vec![
                MetricKey::CompileRequests,
                MetricKey::RequestsExecuted,
                MetricKey::CacheHits,
                MetricKey::CacheMisses,
                MetricKey::HitRate,
            ]
        );
    }

    #[test]
    fn default_config_matches_phase_one_exactly() {
        let config = Config::default();

        assert_eq!(config.languages, vec!["Rust".to_string()]);
        assert_eq!(config.poll_interval, Duration::from_secs(1));
        assert_eq!(config.window, Duration::from_secs(900));

        let keys: Vec<MetricKey> = config.metrics.iter().map(|m| m.key).collect();
        assert_eq!(
            keys,
            vec![
                MetricKey::CompileRequests,
                MetricKey::RequestsExecuted,
                MetricKey::CacheHits,
                MetricKey::CacheMisses,
                MetricKey::HitRate,
            ]
        );

        let labels: Vec<&str> = config.metrics.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "Compile requests",
                "Requests executed",
                "Cache hits",
                "Cache misses",
                "Hit rate",
            ]
        );

        let sparks: Vec<bool> = config.metrics.iter().map(|m| m.spark).collect();
        assert_eq!(sparks, vec![false, false, true, true, true]);
    }

    #[test]
    fn custom_toml_resolves_labels_languages_and_spark() {
        let toml = r#"
languages = []
metrics = [ { key = "cache_writes" }, { key = "cache_hits", label = "Hits!", spark = true } ]
"#;
        let config = Config::from_toml(toml).expect("custom config should parse");

        assert_eq!(config.languages, Vec::<String>::new());
        assert_eq!(config.metrics.len(), 2);

        // `cache_writes` has no explicit label/spark: default label, spark off.
        assert_eq!(config.metrics[0].key, MetricKey::CacheWrites);
        assert_eq!(config.metrics[0].label, "Cache writes");
        assert!(!config.metrics[0].spark);

        // `cache_hits` overrides both label and spark.
        assert_eq!(config.metrics[1].key, MetricKey::CacheHits);
        assert_eq!(config.metrics[1].label, "Hits!");
        assert!(config.metrics[1].spark);
    }

    #[test]
    fn explicit_empty_metrics_list_is_honored() {
        let config =
            Config::from_toml("metrics = []").expect("explicit empty metrics should parse");
        assert!(
            config.metrics.is_empty(),
            "an explicit `metrics = []` must stay empty, not fall back to defaults"
        );
    }

    #[test]
    fn unknown_metric_key_in_toml_errors_and_lists_catalog() {
        let err = Config::from_toml(r#"metrics = [ { key = "bogus" } ]"#)
            .expect_err("`bogus` is not a catalog key");
        let message = err.to_string();
        assert!(message.contains("bogus"), "message was: {message}");
        assert!(message.contains("cache_hits"), "message was: {message}");
    }

    #[test]
    fn unknown_top_level_key_in_toml_is_rejected() {
        // A typo'd top-level key (`interval_ms` instead of `poll_interval`) must
        // not be silently ignored — the default would then apply with no hint
        // that the user's setting was discarded.
        let err = Config::from_toml(r#"interval_ms = "500ms""#)
            .expect_err("an unknown top-level key must be rejected, not silently ignored");
        assert!(
            matches!(err, ConfigError::Toml(_)),
            "expected ConfigError::Toml, got: {err:?}"
        );
        let message = err.to_string();
        assert!(
            message.contains("interval_ms"),
            "error must name the offending key; message was: {message}"
        );
    }

    #[test]
    fn unknown_metric_field_in_toml_is_rejected() {
        // A typo'd field inside a `[[metrics]]` entry (`sparks` instead of
        // `spark`) must be rejected rather than silently dropped, which would
        // leave the sparkline off with no diagnostic.
        let err = Config::from_toml(r#"metrics = [ { key = "cache_hits", sparks = true } ]"#)
            .expect_err("an unknown metric field must be rejected, not silently ignored");
        assert!(
            matches!(err, ConfigError::Toml(_)),
            "expected ConfigError::Toml, got: {err:?}"
        );
        let message = err.to_string();
        assert!(
            message.contains("sparks"),
            "error must name the offending field; message was: {message}"
        );
    }

    #[test]
    fn invalid_duration_in_toml_errors() {
        let err = Config::from_toml(r#"poll_interval = "nope""#)
            .expect_err("`nope` is not a valid duration");
        assert!(
            matches!(err, ConfigError::InvalidDuration { .. }),
            "expected InvalidDuration, got: {err:?}"
        );
    }

    #[test]
    fn default_config_toml_round_trips_to_default() {
        let parsed =
            Config::from_toml(DEFAULT_CONFIG_TOML).expect("DEFAULT_CONFIG_TOML must parse");
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn default_does_not_panic() {
        // Guards the `expect` in `impl Default` — the built-in TOML must parse.
        let _ = Config::default();
    }

    #[test]
    fn with_overrides_replaces_poll_interval_and_leaves_window() {
        let config = Config::default()
            .with_overrides(Some("5s"), None)
            .expect("a valid poll_interval override should apply");
        assert_eq!(
            config.poll_interval,
            Duration::from_secs(5),
            "a present poll_interval override must replace the field"
        );
        assert_eq!(
            config.window,
            Duration::from_secs(900),
            "an absent window override must leave the field unchanged"
        );
    }

    #[test]
    fn with_overrides_rejects_an_invalid_duration() {
        let err = Config::default()
            .with_overrides(Some("nope"), None)
            .expect_err("an invalid poll_interval override must error");
        assert!(
            matches!(err, ConfigError::InvalidDuration { .. }),
            "expected InvalidDuration, got: {err:?}"
        );
    }

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
    fn rejects_zero_magnitude_for_every_unit() {
        // A zero duration would make the watch loop's `recv_timeout(ZERO)` time
        // out immediately, busy-looping and spawning an `sccache --show-stats`
        // subprocess as fast as the OS allows. Every unit must reject zero.
        for input in ["0ms", "0s", "0m", "0h"] {
            assert!(
                parse_duration(input).is_err(),
                "a zero magnitude must be rejected; `{input}` parsed"
            );
        }
    }

    #[test]
    fn zero_duration_error_names_the_input() {
        let err = parse_duration("0s").expect_err("`0s` must be rejected");
        let message = err.to_string();
        assert!(message.contains("\"0s\""), "message was: {message}");
    }

    #[test]
    fn zero_poll_interval_in_toml_errors() {
        let err = Config::from_toml(r#"poll_interval = "0s""#)
            .expect_err("a zero poll_interval must error");
        assert!(
            matches!(err, ConfigError::InvalidDuration { .. }),
            "expected InvalidDuration, got: {err:?}"
        );
    }

    #[test]
    fn with_overrides_rejects_a_zero_poll_interval() {
        let err = Config::default()
            .with_overrides(Some("0ms"), None)
            .expect_err("a zero poll_interval override must error");
        assert!(
            matches!(err, ConfigError::InvalidDuration { .. }),
            "expected InvalidDuration, got: {err:?}"
        );
    }

    #[test]
    fn error_message_names_input_and_units() {
        let err = parse_duration("10x").expect_err("`10x` must be rejected");
        let message = err.to_string();
        assert!(message.contains("\"10x\""), "message was: {message}");
        assert!(message.contains("ms"), "message was: {message}");
        assert!(message.contains('h'), "message was: {message}");
    }

    #[test]
    fn write_default_config_writes_to_a_fresh_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A nested, not-yet-existing subpath: parent dirs must be created.
        let path = dir.path().join("a").join("b").join("config.toml");
        assert!(!path.exists(), "precondition: target must not exist");

        write_default_config(&path, false).expect("writing to a fresh path should succeed");

        assert!(path.exists(), "the config file must exist after writing");
        let written = fs::read_to_string(&path).expect("read back written config");
        assert_eq!(
            written, DEFAULT_CONFIG_TOML,
            "written contents must be DEFAULT_CONFIG_TOML verbatim"
        );

        // The artifact must round-trip back to the built-in defaults.
        assert_eq!(
            Config::from_toml(&written).expect("written config re-parses"),
            Config::default(),
            "the written config must re-parse to Config::default()"
        );
    }

    /// Sentinel content for the overwrite-guard tests; distinct from the default.
    const SENTINEL: &str = "# do not clobber\n";

    #[test]
    fn write_default_config_refuses_overwrite_without_force() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, SENTINEL).expect("pre-create sentinel file");

        let err = write_default_config(&path, false)
            .expect_err("must refuse to overwrite an existing file without force");
        assert!(
            matches!(err, ConfigError::AlreadyExists { .. }),
            "expected AlreadyExists, got: {err:?}"
        );
        assert!(
            err.to_string().contains(&path.display().to_string()),
            "error must name the existing path; message was: {err}"
        );

        // The existing file must be left completely untouched.
        let after = fs::read_to_string(&path).expect("read back sentinel");
        assert_eq!(
            after, SENTINEL,
            "the refused write must leave the existing file untouched"
        );
    }

    #[test]
    fn write_default_config_force_overwrites_existing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, SENTINEL).expect("pre-create sentinel file");

        write_default_config(&path, true).expect("force must overwrite the existing file");

        let after = fs::read_to_string(&path).expect("read back overwritten config");
        assert_eq!(
            after, DEFAULT_CONFIG_TOML,
            "force must replace the existing file with DEFAULT_CONFIG_TOML"
        );
    }
}

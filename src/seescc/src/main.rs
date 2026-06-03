//! seescc — a self-refreshing terminal viewer for sccache statistics.
//!
//! Phases 1–4 are complete: a single poll → parse → aggregate → render → stdout
//! pass, a TOML config, a JSON one-shot format, and a live watch loop that owns
//! the terminal and refreshes on a timer. Phase 5 adds sparkline history.

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::{Parser, ValueEnum};

mod aggregate;
#[allow(
    dead_code,
    reason = "the MetricKind enum and the MetricKey::kind / MetricKey::is_per_language catalog accessors are consumed by Phase 5 sparklines"
)]
mod config;
#[allow(
    dead_code,
    reason = "consumed by the Phase 5 sparkline wiring slice"
)]
mod history;
mod render;
#[allow(
    dead_code,
    reason = "Counters/Stats carry fields (compilations, cache_errors, …) consumed by Phase 5 sparklines"
)]
mod stats;
mod watch;

/// The one-shot output format selected by `--format`.
///
/// clap lowercases the variant names for its [`ValueEnum`] parsing, so the CLI
/// accepts `--format human` and `--format json`. The watch loop always renders
/// the human view regardless of this setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
enum OutputFormat {
    /// Human-readable table (the default).
    #[default]
    Human,
    /// Compact JSON object keyed by metric key, for scripting.
    Json,
}

/// Command-line arguments for `seescc`.
#[derive(Parser)]
#[command(name = "seescc", version = version_string!())]
#[command(about = "Self-refreshing terminal viewer for sccache statistics")]
struct Cli {
    /// Render once and exit instead of entering the live watch loop. On a
    /// non-TTY (piped) stdout this is implied, so `seescc | cat` and capture
    /// wrappers get a single frame without the flag.
    #[arg(long)]
    one_shot: bool,

    /// Path to an explicit config file. Overrides the per-user config file.
    /// When omitted, seescc reads `<config_dir>/seescc/config.toml` if present,
    /// otherwise the built-in defaults apply.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Write the annotated built-in default config and exit. Targets `--config`
    /// when given, otherwise the per-user config path. Refuses to overwrite an
    /// existing file unless `--force` is also passed.
    #[arg(long)]
    write_default_config: bool,

    /// Overwrite an existing file when used with `--write-default-config`.
    #[arg(long)]
    force: bool,

    /// Override the config's poll interval (e.g. `2s`, `500ms`). Accepts an
    /// integer magnitude plus a `ms`/`s`/`m`/`h` unit suffix.
    #[arg(long)]
    poll_interval: Option<String>,

    /// Override the config's sparkline history window (e.g. `30m`, `1h`).
    /// Accepts an integer magnitude plus a `ms`/`s`/`m`/`h` unit suffix.
    #[arg(long)]
    window: Option<String>,

    /// One-shot output format: `human` (default) or `json`. Ignored in the live
    /// watch loop, which always renders the human view.
    #[arg(long, value_enum, default_value = "human")]
    format: OutputFormat,
}

/// The sccache binary we shell out to for stats.
const SCCACHE_BIN: &str = "sccache";

/// Render width used for the one-shot human/JSON frame, which is emitted to a
/// pipe or capture where no live terminal size applies. The watch loop queries
/// the real terminal size per frame instead (see [`watch::run`]).
const DEFAULT_WIDTH: usize = 80;

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Writing the default config must work without sccache installed, so handle
    // it before the which() check and before any polling.
    if cli.write_default_config {
        let target = match cli.config {
            Some(path) => path,
            None => config::default_config_path().context(
                "no config path available: pass --config <path> (this platform exposes \
                 no default config directory)",
            )?,
        };
        config::write_default_config(&target, cli.force)?;
        println!("Wrote default config to {}", target.display());
        return Ok(());
    }

    // Detect sccache up front so a missing install fails with a clear message.
    which::which(SCCACHE_BIN).with_context(|| {
        format!(
            "`{SCCACHE_BIN}` not found on PATH — install sccache \
             (https://github.com/mozilla/sccache)"
        )
    })?;

    let config = config::load(cli.config.as_deref())?
        .with_overrides(cli.poll_interval.as_deref(), cli.window.as_deref())?;

    // Color policy, mirroring gsw: NO_COLOR is an explicit kill switch, and a
    // watch-like wrapper (no TTY but COLUMNS exported) gets colors forced back
    // on so they pass through to the wrapper's own TTY-backed UI instead of
    // being stripped as they would be for a plain pipe.
    let stdout_is_tty = std::io::stdout().is_terminal();
    let columns_env_present = std::env::var_os("COLUMNS").is_some();
    let no_color_env = std::env::var_os("NO_COLOR").is_some();
    if no_color_env {
        colored::control::set_override(false);
    } else if watch::should_force_colors(stdout_is_tty, columns_env_present, no_color_env) {
        colored::control::set_override(true);
    }

    // Watch mode is the default on a live terminal; `--one-shot` and any non-TTY
    // stdout fall back to a single render. `--format` is one-shot-only — the
    // watch loop always renders the human view.
    match watch::decide_mode(cli.one_shot, stdout_is_tty) {
        watch::Mode::OneShot => {
            let stats = poll_sccache()?;
            let output = match cli.format {
                OutputFormat::Human => render_oneshot(&config, &stats),
                OutputFormat::Json => render_oneshot_json(&config, &stats),
            };
            println!("{output}");
            Ok(())
        }
        watch::Mode::Watch => watch::run(&config, poll_sccache),
    }
}

/// Run `sccache --show-stats --stats-format=json` and return the parsed stats.
///
/// # Errors
/// Errors if sccache cannot be launched, exits non-zero, emits non-UTF-8, or
/// produces JSON that fails to parse.
fn poll_sccache() -> Result<stats::Stats> {
    let output = std::process::Command::new(SCCACHE_BIN)
        .args(["--show-stats", "--stats-format=json"])
        .output()
        .with_context(|| {
            format!("failed to run `{SCCACHE_BIN} --show-stats --stats-format=json`")
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "`{SCCACHE_BIN} --show-stats` failed ({}): {}",
            output.status,
            stderr.trim()
        );
    }

    let json = String::from_utf8(output.stdout)
        .context("sccache --show-stats output was not valid UTF-8")?;
    stats::parse(&json)
}

/// Build the one-shot human frame from a resolved [`config::Config`].
///
/// The header label and the metric rows are built by the same shared helpers the
/// watch frame uses ([`watch::languages_label`] and [`watch::build_rows`]) so the
/// one-shot and watch tables can never drift apart. The resulting rows are handed
/// to [`render::build_human`] with the current wall-clock time.
fn render_oneshot(config: &config::Config, stats: &stats::Stats) -> String {
    let languages_label = watch::languages_label(config);
    let rows = watch::build_rows(config, stats);
    let clock = chrono::Local::now().format("%H:%M:%S").to_string();
    render::build_human(&languages_label, &clock, DEFAULT_WIDTH, &rows)
}

/// Build the one-shot JSON frame from a resolved [`config::Config`].
///
/// Emits a compact, single-line JSON object keyed by each metric's canonical
/// config key, in the config's metric order. Counts and byte sizes serialize as
/// raw integers (sizes are the underlying byte count, not the human "771.7 MiB"
/// string — scripting wants the number); rates serialize as floats rounded to
/// two decimals. The `languages` filter is applied exactly as in the human view.
fn render_oneshot_json(config: &config::Config, stats: &stats::Stats) -> String {
    let fields: Vec<render::JsonField> = config
        .metrics
        .iter()
        .map(|spec| {
            let value = match aggregate::metric_value(spec.key, stats, &config.languages) {
                aggregate::MetricValue::Count(n) | aggregate::MetricValue::Size(n) => {
                    render::JsonValue::Int(n)
                }
                aggregate::MetricValue::Rate(r) => render::JsonValue::Float(round_rate(r)),
            };
            render::JsonField {
                key: spec.key.as_config_key(),
                value,
            }
        })
        .collect();
    render::build_json(&fields)
}

/// Round a percentage to two decimals for stable, script-friendly JSON output.
fn round_rate(rate: f64) -> f64 {
    (rate * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/sccache-0.15.0.json");

    /// Parse the captured fixture into a [`stats::Stats`] for realistic data.
    fn fixture_stats() -> stats::Stats {
        stats::parse(FIXTURE).expect("fixture should parse")
    }

    /// Render the one-shot JSON and parse it back into a [`serde_json::Value`].
    fn json_value(config: &config::Config) -> serde_json::Value {
        let out = render_oneshot_json(config, &fixture_stats());
        serde_json::from_str(&out)
            .unwrap_or_else(|e| panic!("output must be valid JSON: {e}\n{out}"))
    }

    #[test]
    fn json_oneshot_default_config_is_rust_filtered() {
        let config = config::Config::default();
        let value = json_value(&config);
        let object = value.as_object().expect("JSON output must be an object");

        // Exactly the five default keys, no more, no less.
        assert_eq!(
            object.len(),
            5,
            "object should have exactly 5 keys: {object:?}"
        );
        for key in [
            "compile_requests",
            "requests_executed",
            "cache_hits",
            "cache_misses",
            "hit_rate",
        ] {
            assert!(
                object.contains_key(key),
                "missing expected key {key}: {object:?}"
            );
        }

        assert_eq!(value["compile_requests"], 4786);
        assert_eq!(value["requests_executed"], 3880);

        // Rust-only per-language values, NOT the all-language sums (2430 / 1373).
        assert_eq!(
            value["cache_hits"], 1718,
            "cache_hits must be Rust-only, not the all-language sum"
        );
        assert_eq!(
            value["cache_misses"], 963,
            "cache_misses must be Rust-only, not the all-language sum"
        );

        let hit_rate = value["hit_rate"]
            .as_f64()
            .expect("hit_rate must be a JSON number");
        assert!(
            (hit_rate - 64.08).abs() < 1e-9,
            "hit_rate should round to 64.08, got {hit_rate}"
        );
    }

    #[test]
    fn json_oneshot_languages_empty_sums_all() {
        let config = config::Config::from_toml(
            r#"
languages = []
metrics = [ { key = "cache_hits" } ]
"#,
        )
        .expect("config should parse");
        let value = json_value(&config);
        assert_eq!(
            value["cache_hits"],
            196 + 1718 + 516,
            "empty languages must sum cache_hits across all languages"
        );
    }

    #[test]
    fn json_oneshot_size_metric_is_raw_bytes() {
        let config = config::Config::from_toml(
            r#"
metrics = [ { key = "cache_size" } ]
"#,
        )
        .expect("config should parse");
        let value = json_value(&config);
        assert_eq!(
            value["cache_size"], 809_212_237,
            "cache_size must be the raw byte count as an integer, not a human string"
        );
        assert!(
            value["cache_size"].is_number(),
            "cache_size must be a JSON number, not a string: {:?}",
            value["cache_size"]
        );
    }

    #[test]
    fn json_oneshot_is_valid_parseable_json() {
        let config = config::Config::default();
        let out = render_oneshot_json(&config, &fixture_stats());
        assert!(
            serde_json::from_str::<serde_json::Value>(&out).is_ok(),
            "default-config JSON output must be valid (jq-pipeable): {out}"
        );
    }
}

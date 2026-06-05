//! seescc — a self-refreshing terminal viewer for sccache statistics.
//!
//! Phases 1–5 are complete: a single poll → parse → aggregate → render → stdout
//! pass, a TOML config, a JSON one-shot format, a live watch loop that owns the
//! terminal and refreshes on a timer, and per-metric Unicode sparklines drawn
//! from an in-memory history ring that fills in over the configured window.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::{Parser, ValueEnum};

mod aggregate;
mod config;
mod history;
mod render;
mod sparkline;
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

    /// Override the config's poll interval (e.g. `2s`, `500ms`). Accepts a
    /// positive integer magnitude plus a `ms`/`s`/`m`/`h` unit suffix.
    #[arg(long)]
    poll_interval: Option<String>,

    /// Override the config's sparkline history window (e.g. `30m`, `1h`).
    /// Accepts a positive integer magnitude plus a `ms`/`s`/`m`/`h` unit suffix.
    #[arg(long)]
    window: Option<String>,

    /// One-shot output format: `human` (default) or `json`. Ignored in the live
    /// watch loop, which always renders the human view.
    #[arg(long, value_enum, default_value = "human")]
    format: OutputFormat,
}

/// The sccache binary we shell out to for stats.
const SCCACHE_BIN: &str = "sccache";

/// How long a single sccache stats poll may run before it is abandoned.
///
/// The watch loop polls → renders → paints on one thread (see
/// [`watch::run`]), so a sccache child that never returns — a wedged network
/// cache backend, a server mid-restart, socket exhaustion — would otherwise
/// block the *entire* UI indefinitely: no repaint, no quit key. Capping each
/// poll at this deadline turns a hung server into an ordinary failed poll,
/// which flows down the existing non-fatal banner path (design §6: error
/// banner + keep the last good frame) and keeps the loop responsive.
const POLL_TIMEOUT: Duration = Duration::from_secs(10);

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

    // Color and width policy, mirroring gsw: NO_COLOR is an explicit kill
    // switch, and a watch-like wrapper (no TTY but COLUMNS exported) gets colors
    // forced back on so they pass through to the wrapper's own TTY-backed UI
    // instead of being stripped as they would be for a plain pipe. COLUMNS is
    // parsed once here and shared by both decisions: an unparseable value is
    // treated as absent for colors *and* width alike, so the two never disagree
    // about whether a wrapper is present.
    let stdout_is_tty = std::io::stdout().is_terminal();
    let columns_env: Option<usize> = std::env::var("COLUMNS").ok().and_then(|s| s.parse().ok());
    let no_color_env = std::env::var_os("NO_COLOR").is_some();
    if no_color_env {
        colored::control::set_override(false);
    } else if watch::should_force_colors(stdout_is_tty, columns_env.is_some(), no_color_env) {
        colored::control::set_override(true);
    }

    // Watch mode is the default on a live terminal; `--one-shot` and any non-TTY
    // stdout fall back to a single render. `--format` is one-shot-only — the
    // watch loop always renders the human view.
    match watch::decide_mode(cli.one_shot, stdout_is_tty) {
        watch::Mode::OneShot => {
            let stats = poll_sccache()?;
            let output = match cli.format {
                OutputFormat::Human => {
                    // Lay the human frame out at the real width, mirroring gsw: a
                    // direct `--one-shot` TTY uses the queried terminal width, a
                    // watch-like wrapper uses its exported COLUMNS, and a plain
                    // pipe falls back to the shared default — all minus the
                    // one-cell safety margin baked into `effective_width`. The
                    // JSON path is width-independent, so it is resolved only here.
                    let tty_width = terminal_size::terminal_size().map(|(w, _h)| usize::from(w.0));
                    let width = watch::effective_width(tty_width, columns_env, stdout_is_tty);
                    render_oneshot(&config, &stats, width)
                }
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
    let json = run_stats_process(SCCACHE_BIN, POLL_TIMEOUT)?;
    stats::parse(&json)
}

/// Run `bin --show-stats --stats-format=json` under a `timeout` deadline and
/// return its stdout as a UTF-8 string.
///
/// Split out of [`poll_sccache`] so the deadline behavior is unit-testable with
/// a stub executable (a real sccache server is not needed). `poll_sccache`
/// supplies [`SCCACHE_BIN`] and [`POLL_TIMEOUT`]; the parse step stays in the
/// caller.
///
/// # Errors
/// Errors if the process cannot be launched, does not finish within `timeout`,
/// exits non-zero, or emits non-UTF-8 on stdout.
fn run_stats_process(bin: &str, _timeout: Duration) -> Result<String> {
    let output = std::process::Command::new(bin)
        .args(["--show-stats", "--stats-format=json"])
        .output()
        .with_context(|| format!("failed to run `{bin} --show-stats --stats-format=json`"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "`{bin} --show-stats` failed ({}): {}",
            output.status,
            stderr.trim()
        );
    }

    String::from_utf8(output.stdout).context("sccache --show-stats output was not valid UTF-8")
}

/// Build the one-shot human frame from a resolved [`config::Config`], laid out
/// for `width` display columns.
///
/// The header label and the metric rows are built by the same shared helpers the
/// watch frame uses ([`watch::languages_label`] and [`watch::build_rows`]) so the
/// one-shot and watch tables can never drift apart. The resulting rows are handed
/// to [`render::build_human`] with the current wall-clock time. `width` is the
/// caller-resolved frame width from [`watch::effective_width`] — the queried TTY
/// width, a wrapper's `COLUMNS`, or the shared fallback, each less the one-cell
/// safety margin — so the clock right-justifies against the real terminal rather
/// than a hardcoded column count.
fn render_oneshot(config: &config::Config, stats: &stats::Stats, width: usize) -> String {
    let languages_label = watch::languages_label(config);
    let rows = watch::build_rows(config, stats);
    let clock = chrono::Local::now().format("%H:%M:%S").to_string();
    render::build_human(&languages_label, &clock, width, &rows)
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

    /// Write an executable stub at `path` with the given shell `body`, marked
    /// `0o755` so it can be spawned directly. Unix-only (the crate's process
    /// path is Unix-shaped), keyed off a parallel-safe tempdir by the caller.
    #[cfg(unix)]
    fn write_executable_stub(path: &std::path::Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, body).expect("write stub");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
    }

    #[cfg(unix)]
    #[test]
    fn run_stats_process_times_out_on_a_hung_child() {
        // A wedged sccache (network backend stall, server restart, socket
        // exhaustion) must NOT block the single-threaded watch loop forever.
        // run_stats_process must enforce its deadline: a stub that sleeps far
        // past a short timeout has to return an Err naming the timeout, and the
        // call must return well before the stub's own sleep elapses — proving
        // the deadline ended the wait, not the child exiting.
        let dir = tempfile::tempdir().expect("tempdir");
        let stub = dir.path().join("sccache");
        // Sleep 5s, well past the 200ms deadline below; if it ever printed it
        // would print valid-looking output, so an Ok result means the deadline
        // was ignored.
        write_executable_stub(&stub, "#!/bin/sh\nsleep 5\necho '{}'\n");

        let timeout = Duration::from_millis(200);
        let start = std::time::Instant::now();
        let result = run_stats_process(&stub.display().to_string(), timeout);
        let elapsed = start.elapsed();

        assert!(
            result.is_err(),
            "a child that outruns the deadline must return Err, got Ok"
        );
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("timed out") || message.contains("timeout"),
            "the error must name the timeout, got: {message}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "the deadline (200ms) must cut the wait short, not the child's 5s \
             sleep — returned after {elapsed:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_stats_process_returns_stdout_for_a_fast_child() {
        // A child that finishes inside the deadline returns its stdout verbatim
        // for the caller to parse — the happy path through the helper.
        let dir = tempfile::tempdir().expect("tempdir");
        let stub = dir.path().join("sccache");
        write_executable_stub(&stub, "#!/bin/sh\nprintf '{\"ok\":true}'\n");

        let out = run_stats_process(&stub.display().to_string(), Duration::from_secs(10))
            .expect("a fast child must succeed");
        assert_eq!(out, "{\"ok\":true}");
    }

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

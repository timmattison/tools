//! seescc — a self-refreshing terminal viewer for sccache statistics.
//!
//! Phase 1 implements a single poll → parse → aggregate → render → stdout pass.
//! Later phases add a TOML config, a JSON one-shot format, a live watch loop,
//! and sparkline history.

use std::path::PathBuf;

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;

mod aggregate;
#[allow(
    dead_code,
    reason = "the MetricKind enum and the MetricKey::kind / MetricKey::is_per_language catalog accessors are consumed by later phases (Phase 3 JSON output, Phase 5 sparklines)"
)]
mod config;
mod render;
#[allow(
    dead_code,
    reason = "Counters/Stats carry fields (cache_size, compilations, …) consumed by later phases"
)]
mod stats;

/// Command-line arguments for `seescc`.
#[derive(Parser)]
#[command(name = "seescc", version = version_string!())]
#[command(about = "Self-refreshing terminal viewer for sccache statistics")]
struct Cli {
    /// Render once and exit instead of entering the live watch loop. On a
    /// non-TTY (piped) stdout this is implied. The live watch loop arrives in a
    /// later version; for now seescc always renders a single frame.
    #[arg(long)]
    #[allow(
        dead_code,
        reason = "accepted now for forward compatibility; selects single-render mode once the watch loop lands"
    )]
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
}

/// The sccache binary we shell out to for stats.
const SCCACHE_BIN: &str = "sccache";

/// Render width used when no terminal size is detected (one-shot / piped).
/// Real terminal-size detection arrives with the live watch loop.
const DEFAULT_WIDTH: usize = 80;

/// The header label used when the config selects no specific languages, meaning
/// per-language metrics are summed across every language.
const ALL_LANGUAGES_LABEL: &str = "all";

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

    let stats = poll_sccache()?;
    println!("{}", render_oneshot(&config, &stats));
    Ok(())
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
/// The header label is the config's languages joined with `", "`, or `"all"`
/// when the list is empty (per-language metrics summed across all languages).
/// Each configured metric row is extracted from `stats` via
/// [`aggregate::metric_value`] and formatted for display, then handed to
/// [`render::build_human`] with the current wall-clock time.
fn render_oneshot(config: &config::Config, stats: &stats::Stats) -> String {
    let languages_label = if config.languages.is_empty() {
        ALL_LANGUAGES_LABEL.to_string()
    } else {
        config.languages.join(", ")
    };

    let rows: Vec<render::Row> = config
        .metrics
        .iter()
        .map(|spec| render::Row {
            label: spec.label.clone(),
            value: aggregate::metric_value(spec.key, stats, &config.languages).format(),
        })
        .collect();

    let clock = chrono::Local::now().format("%H:%M:%S").to_string();
    render::build_human(&languages_label, &clock, DEFAULT_WIDTH, &rows)
}

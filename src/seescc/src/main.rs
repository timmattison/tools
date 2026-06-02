//! seescc — a self-refreshing terminal viewer for sccache statistics.
//!
//! Phase 1 implements a single poll → parse → aggregate → render → stdout pass.
//! Later phases add a TOML config, a JSON one-shot format, a live watch loop,
//! and sparkline history.

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use num_format::{Locale, ToFormattedString};

mod aggregate;
#[allow(
    dead_code,
    reason = "config API is consumed when the CLI wiring lands in the final Phase 2 slice"
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
}

/// The sccache binary we shell out to for stats.
const SCCACHE_BIN: &str = "sccache";

/// Languages whose per-language cache buckets are shown by default. sccache
/// also reports C/C++ and Assembler; the default Rust-focused view hides them.
const DEFAULT_LANGUAGES: [&str; 1] = ["Rust"];

/// Render width used when no terminal size is detected (one-shot / piped).
/// Real terminal-size detection arrives with the live watch loop.
const DEFAULT_WIDTH: usize = 80;

fn main() -> Result<()> {
    let _cli = Cli::parse();

    // Detect sccache up front so a missing install fails with a clear message.
    which::which(SCCACHE_BIN).with_context(|| {
        format!(
            "`{SCCACHE_BIN}` not found on PATH — install sccache \
             (https://github.com/mozilla/sccache)"
        )
    })?;

    let stats = poll_sccache()?;
    println!("{}", render_default(&stats));
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

/// Build the default one-shot human frame: the Rust-focused five-metric table
/// with the current wall-clock time in the header.
fn render_default(stats: &stats::Stats) -> String {
    let langs: Vec<String> = DEFAULT_LANGUAGES.iter().map(|s| s.to_string()).collect();
    let hits = aggregate::lang_sum(&stats.stats.cache_hits.counts, &langs);
    let misses = aggregate::lang_sum(&stats.stats.cache_misses.counts, &langs);
    let rate = aggregate::hit_rate(hits, misses);

    let rows = vec![
        render::Row {
            label: "Compile requests".to_string(),
            value: fmt_count(stats.stats.compile_requests),
        },
        render::Row {
            label: "Requests executed".to_string(),
            value: fmt_count(stats.stats.requests_executed),
        },
        render::Row {
            label: "Cache hits".to_string(),
            value: fmt_count(hits),
        },
        render::Row {
            label: "Cache misses".to_string(),
            value: fmt_count(misses),
        },
        render::Row {
            label: "Hit rate".to_string(),
            value: format!("{rate:.1}%"),
        },
    ];

    let clock = chrono::Local::now().format("%H:%M:%S").to_string();
    let label = DEFAULT_LANGUAGES.join(", ");
    render::build_human(&label, &clock, DEFAULT_WIDTH, &rows)
}

/// Format a counter with thousands separators (e.g. 4786 -> "4,786").
fn fmt_count(n: u64) -> String {
    n.to_formatted_string(&Locale::en)
}

//! seescc — a self-refreshing terminal viewer for sccache statistics.
//!
//! Phase 1 implements a single poll → parse → aggregate → render → stdout pass.
//! Later phases add a TOML config, a JSON one-shot format, a live watch loop,
//! and sparkline history.

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;

#[allow(
    dead_code,
    reason = "consumed by later Phase 1 slices (aggregate/main wiring)"
)]
mod stats;

#[allow(dead_code, reason = "consumed by the Phase 1 main-wiring slice")]
mod aggregate;

/// Command-line arguments for `seescc`.
#[derive(Parser)]
#[command(name = "seescc", version = version_string!())]
#[command(about = "Self-refreshing terminal viewer for sccache statistics")]
struct Cli {}

/// The sccache binary we shell out to for stats.
const SCCACHE_BIN: &str = "sccache";

fn main() -> Result<()> {
    let _cli = Cli::parse();

    // Detect sccache up front so a missing install fails with a clear message
    // before any terminal takeover. Later slices add the actual poll/render.
    which::which(SCCACHE_BIN).with_context(|| {
        format!(
            "`{SCCACHE_BIN}` not found on PATH — install sccache \
             (https://github.com/mozilla/sccache)"
        )
    })?;

    Ok(())
}

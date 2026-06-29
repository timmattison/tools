//! `bm` — Bulk Move.
//!
//! Recursively find files matching a pattern (suffix, prefix, or substring) and
//! move them into a destination directory. Unlike the original Go tool, bm plans
//! moves collision-safely (it refuses to clobber by default) and falls back to a
//! copy-then-delete when the destination is on a different volume — the case a
//! bare `rename(2)` cannot handle.

#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]

use anyhow::{bail, Context, Result};
use bm::{
    collect_sources, execute_plan, plan_moves, select_filter, CollisionError, CollisionKind,
    CollisionPolicy, MovePlan, Summary,
};
use buildinfo::version_string;
use clap::Parser;
use indicatif::ProgressBar;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};
use termbar::{ProgressStyleBuilder, TerminalWidth};

const AFTER_HELP: &str = "\
EXAMPLES:
    # Move all .mkv files from the current directory to a backup drive
    bm --suffix .mkv --destination /Volumes/Backup/videos

    # Move camera photos (IMG_ prefix) from Downloads to Pictures
    bm --prefix IMG_ --destination ~/Pictures/camera ~/Downloads

    # Preview what moving every 2024 file would do, without moving anything
    bm --substring 2024 --destination ~/archive/2024 --dry-run

NOTES:
    - Exactly one of --suffix, --prefix, or --substring must be given.
    - The destination must already exist and be a directory.
    - Cross-volume moves are handled by copying then deleting (with a progress bar).
    - By default bm aborts rather than overwrite; see --on-collision.";

/// Recursively move files matching a pattern into a destination directory.
#[derive(Parser)]
#[command(name = "bm", version = version_string!(), about, long_about = None, after_help = AFTER_HELP)]
struct Cli {
    /// Move files whose name ends with this suffix (e.g. .mkv)
    #[arg(short, long, value_name = "SUFFIX")]
    suffix: Option<String>,

    /// Move files whose name starts with this prefix (e.g. IMG_)
    #[arg(short, long, value_name = "PREFIX")]
    prefix: Option<String>,

    /// Move files whose name contains this substring (e.g. 2024)
    #[arg(long, value_name = "SUBSTRING")]
    substring: Option<String>,

    /// Destination directory to move matching files into (must exist)
    #[arg(short, long, value_name = "DIR")]
    destination: PathBuf,

    /// What to do when a destination filename already exists or repeats
    #[arg(long, value_enum, default_value_t = CollisionPolicy::Abort, value_name = "POLICY")]
    on_collision: CollisionPolicy,

    /// Show what would be moved without moving anything
    #[arg(long)]
    dry_run: bool,

    /// Directories to search (defaults to the current directory)
    #[arg(value_name = "DIR")]
    directories: Vec<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("bm: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();

    let filter =
        select_filter(cli.suffix, cli.prefix, cli.substring).map_err(|e| anyhow::anyhow!("{e}"))?;

    // The destination must already exist and be a directory; fail clearly if not.
    let metadata = std::fs::metadata(&cli.destination).with_context(|| {
        format!(
            "destination {} does not exist or is not accessible",
            cli.destination.display()
        )
    })?;
    if !metadata.is_dir() {
        bail!(
            "destination {} is not a directory",
            cli.destination.display()
        );
    }

    let directories = if cli.directories.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        cli.directories
    };

    let sources = collect_sources(&directories, filter, &cli.destination)?;
    if sources.is_empty() {
        println!("No matching files found.");
        return Ok(ExitCode::SUCCESS);
    }

    let plan = match plan_moves(&sources, &cli.destination, cli.on_collision, |p| p.exists()) {
        Ok(plan) => plan,
        Err(collision) => {
            report_collisions(&collision);
            return Ok(ExitCode::FAILURE);
        }
    };

    if cli.dry_run {
        print_dry_run(&plan);
        return Ok(ExitCode::SUCCESS);
    }

    let start = Instant::now();
    let summary = match execute_plan(&plan, copy_across_volumes) {
        Ok(summary) => summary,
        Err(err) => {
            // A move failed mid-batch: surface what already moved before the
            // error so a partial run isn't silent, then report the error.
            if err.summary.moved() > 0 || err.summary.skipped > 0 {
                print_interrupted(&err.summary, start.elapsed());
            }
            return Err(err.into());
        }
    };
    print_summary(&summary, start.elapsed());

    Ok(ExitCode::SUCCESS)
}

/// Copy a file across a filesystem boundary, showing a per-file progress bar.
fn copy_across_volumes(source: &Path, destination: &Path) -> std::io::Result<u64> {
    let size = std::fs::metadata(source).map(|m| m.len()).unwrap_or(0);
    let filename = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let bar = ProgressBar::new(size);
    if let Ok(style) = ProgressStyleBuilder::copy(&filename).build(TerminalWidth::get_or_default())
    {
        bar.set_style(style);
    }

    let result = bm::copy_file(source, destination, |bytes| bar.set_position(bytes));
    bar.finish_and_clear();
    result
}

/// Print the collisions that caused an abort, with a hint at how to proceed.
fn report_collisions(error: &CollisionError) {
    eprintln!(
        "bm: {} destination collision(s) detected:",
        error.collisions.len()
    );
    for collision in &error.collisions {
        match collision.kind {
            CollisionKind::DestinationExists => {
                eprintln!("  {} already exists", collision.destination.display());
            }
            CollisionKind::DuplicateBasename => {
                let sources: Vec<String> = collision
                    .sources
                    .iter()
                    .map(|s| s.display().to_string())
                    .collect();
                eprintln!(
                    "  {} <- {}",
                    collision.destination.display(),
                    sources.join(", ")
                );
            }
        }
    }
    eprintln!("No files moved. Re-run with --on-collision=skip, rename, or overwrite.");
}

/// Print the planned moves and skips without touching the filesystem.
fn print_dry_run(plan: &MovePlan) {
    println!("Dry run — no files will be moved.");
    for planned in &plan.moves {
        println!(
            "  move {} -> {}",
            planned.source.display(),
            planned.destination.display()
        );
    }
    for skipped in &plan.skipped {
        println!(
            "  skip {} ({})",
            skipped.source.display(),
            describe_kind(skipped.reason)
        );
    }
    println!(
        "{} to move, {} to skip.",
        plan.moves.len(),
        plan.skipped.len()
    );
}

/// Print the final tally after a real run.
fn print_summary(summary: &Summary, duration: Duration) {
    let moved = summary.moved();
    let seconds = duration.as_secs_f64();
    let rate = if seconds > 0.0 {
        moved as f64 / seconds
    } else {
        0.0
    };
    println!(
        "Move complete: {moved} moved ({} renamed, {} copied across volumes), {} skipped in {:.2?} ({rate:.0} files/sec)",
        summary.renamed, summary.copied, summary.skipped, duration
    );
}

/// Print the partial tally when a run stops early because a move failed.
fn print_interrupted(summary: &Summary, duration: Duration) {
    println!(
        "Move interrupted: {} moved ({} renamed, {} copied across volumes), {} skipped in {:.2?} before the error below:",
        summary.moved(),
        summary.renamed,
        summary.copied,
        summary.skipped,
        duration
    );
}

/// Human-readable reason for a skipped move.
fn describe_kind(kind: CollisionKind) -> &'static str {
    match kind {
        CollisionKind::DestinationExists => "destination exists",
        CollisionKind::DuplicateBasename => "duplicate name in this batch",
    }
}

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use buildinfo::version_string;
use clap::Parser;

use crate::git::{parse_numstat, parse_status, FileEntry, NumStat};
use crate::render::{render, RenderOptions};
use crate::snapshot::build_snapshot;

mod age;
mod bar;
mod git;
mod render;
mod snapshot;

#[derive(Parser)]
#[command(name = "gsw")]
#[command(version = version_string!())]
#[command(
    about = "Compact git status watch — one-shot pretty output for use with viddy",
    long_about = "Prints a compact, color-coded view of the current branch's state: \
                  commits ahead of the base branch, last-commit age, and a per-file \
                  list showing a magnitude bar, +/- counts, and recency. Designed to \
                  be run repeatedly under `viddy`."
)]
struct Cli {
    /// Strip ANSI color codes from output.
    #[arg(long)]
    no_color: bool,

    /// Base ref to compare against (default: main, then master, then origin/HEAD).
    #[arg(long)]
    base: Option<String>,

    /// Maximum number of file rows to show (default: unlimited).
    #[arg(long)]
    max_files: Option<usize>,

    /// Width of the magnitude bar in cells.
    #[arg(long, default_value_t = 6)]
    bar_width: usize,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.no_color {
        colored::control::set_override(false);
    }

    let branch = run_git(&["rev-parse", "--abbrev-ref", "HEAD"])
        .context("not inside a git repository")?
        .trim()
        .to_string();

    let base = cli.base.unwrap_or_else(|| resolve_base_ref().unwrap_or_else(|_| "HEAD".to_string()));

    let commits_ahead = run_git(&["rev-list", "--count", &format!("{base}..HEAD")])
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

    let last_commit_age = last_commit_age().unwrap_or(Duration::ZERO);

    let status_raw = run_git(&["status", "--porcelain=v2", "-z"])?;
    let entries = parse_status(&status_raw);

    let staged_numstat = run_git(&["diff", "--cached", "--numstat", "-z"])
        .map(|s| parse_numstat(&s))
        .unwrap_or_default();
    let unstaged_numstat = run_git(&["diff", "--numstat", "-z"])
        .map(|s| parse_numstat(&s))
        .unwrap_or_default();

    let ages = collect_ages(&entries);

    let snapshot = build_snapshot(
        branch,
        base,
        commits_ahead,
        last_commit_age,
        entries,
        &staged_numstat,
        &unstaged_numstat,
        &ages,
    );

    let terminal_width = terminal_size::terminal_size()
        .map_or(80, |(w, _)| usize::from(w.0));

    let opts = RenderOptions {
        terminal_width,
        bar_width: cli.bar_width,
        max_files: cli.max_files,
    };

    println!("{}", render(&snapshot, &opts));
    Ok(())
}

/// Run `git` with the given args and return captured stdout as UTF-8.
fn run_git(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .with_context(|| format!("invoking `git {}`", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "`git {}` exited with status {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Pick the first base ref that actually resolves: `main`, then `master`,
/// then whatever `origin/HEAD` points to.
fn resolve_base_ref() -> Result<String> {
    for candidate in ["main", "master"] {
        if run_git(&["rev-parse", "--verify", "--quiet", candidate]).is_ok() {
            return Ok(candidate.to_string());
        }
    }
    if let Ok(out) = run_git(&["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        let trimmed = out.trim();
        if let Some(name) = trimmed.strip_prefix("refs/remotes/origin/") {
            return Ok(format!("origin/{name}"));
        }
    }
    Ok("HEAD".to_string())
}

/// How long ago the current HEAD commit was authored.
fn last_commit_age() -> Result<Duration> {
    let raw = run_git(&["log", "-1", "--format=%ct"])?;
    let secs: u64 = raw.trim().parse().unwrap_or(0);
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    Ok(SystemTime::now().duration_since(when).unwrap_or(Duration::ZERO))
}

/// Get mtime ages for each entry's path, where the path still exists on disk.
fn collect_ages(entries: &[FileEntry]) -> HashMap<String, Duration> {
    let now = SystemTime::now();
    let mut out = HashMap::with_capacity(entries.len());
    for e in entries {
        if out.contains_key(&e.path) {
            continue;
        }
        let path = Path::new(&e.path);
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        let elapsed = now.duration_since(mtime).unwrap_or(Duration::ZERO);
        out.insert(e.path.clone(), elapsed);
    }
    out
}

// Silence: NumStat is only constructed inside parse_numstat / tests.
const _: fn() -> NumStat = || NumStat::default();

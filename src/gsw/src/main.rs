use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;

use crate::git::{parse_numstat, parse_status, FileEntry};
use crate::render::{default_max_files, render, RenderOptions};
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

    /// Columns to subtract from the detected terminal width. Useful when a
    /// wrapping TUI (e.g. viddy) eats a column for its own chrome that the
    /// child process can't see.
    #[arg(long, default_value_t = 0)]
    width_offset: usize,
}

/// Decide the effective terminal width gsw should render for.
///
/// - When stdout is a TTY, trust `tty_width` directly (interactive use).
/// - When stdout is *not* a TTY but `COLUMNS` is set in env, a watch-like
///   wrapper (e.g. viddy) is framing us. Viddy reports the full terminal
///   width via `COLUMNS` but renders into a content area that's one column
///   narrower (its scroll indicator). So we use `columns_env - 1`.
/// - Otherwise fall back to 80 columns.
///
/// `width_offset` always stacks on top, and the result is at least 1.
fn effective_terminal_width(
    _tty_width: Option<usize>,
    _columns_env: Option<usize>,
    _stdout_is_tty: bool,
    _width_offset: usize,
) -> usize {
    // RED stub — green commit replaces this with real behavior.
    80
}

/// Should `colored::control::set_override(true)` be called?
///
/// True only when output is captured by a watch-like wrapper (stdout is not
/// a TTY *and* `COLUMNS` is set in env), and the user has not asked to
/// suppress colors via `NO_COLOR`. The wrapper renders the captured bytes
/// inside its own TTY-backed UI, so colors should pass through.
fn should_force_colors(
    _stdout_is_tty: bool,
    _columns_env_present: bool,
    _no_color_env: bool,
) -> bool {
    // RED stub — green commit replaces this with real behavior.
    false
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.no_color {
        colored::control::set_override(false);
    }

    if !inside_git_repo() {
        println!("{}", "gsw • not a git repository".dimmed());
        return Ok(());
    }

    let branch = run_git(&["rev-parse", "--abbrev-ref", "HEAD"])
        .context("failed to read HEAD ref")?
        .trim()
        .to_string();

    let base = cli.base.unwrap_or_else(resolve_base_ref);

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

    let repo_root = run_git(&["rev-parse", "--show-toplevel"])
        .ok()
        .map(|s| PathBuf::from(s.trim()));
    let ages = collect_ages(&entries, repo_root.as_deref());

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

    let (terminal_width, terminal_height) = terminal_size::terminal_size()
        .map_or((80, 24), |(w, h)| (usize::from(w.0), h.0));

    let opts = RenderOptions {
        terminal_width,
        bar_width: cli.bar_width,
        max_files: cli.max_files.or(Some(default_max_files(terminal_height))),
    };

    println!("{}", render(&snapshot, &opts));
    Ok(())
}

/// True if the current working directory is inside a git work tree.
///
/// `git rev-parse --is-inside-work-tree` returns status 0 with stdout
/// `false` for bare repos, so we have to inspect the output, not just
/// the exit code.
fn inside_git_repo() -> bool {
    let Ok(output) = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    String::from_utf8_lossy(&output.stdout).trim() == "true"
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
/// then whatever `origin/HEAD` points to. Falls back to `HEAD` so the
/// commits-ahead count degrades gracefully to zero.
fn resolve_base_ref() -> String {
    for candidate in ["main", "master"] {
        if run_git(&["rev-parse", "--verify", "--quiet", candidate]).is_ok() {
            return candidate.to_string();
        }
    }
    if let Ok(out) = run_git(&["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        let trimmed = out.trim();
        if let Some(name) = trimmed.strip_prefix("refs/remotes/origin/") {
            return format!("origin/{name}");
        }
    }
    "HEAD".to_string()
}

/// How long ago the current HEAD commit was authored.
fn last_commit_age() -> Result<Duration> {
    let raw = run_git(&["log", "-1", "--format=%ct"])?;
    let secs: u64 = raw.trim().parse().unwrap_or(0);
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    Ok(SystemTime::now().duration_since(when).unwrap_or(Duration::ZERO))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_uses_columns_minus_one_when_stdout_not_tty() {
        // viddy case: pipes captured, COLUMNS exported.
        assert_eq!(effective_terminal_width(None, Some(120), false, 0), 119);
    }

    #[test]
    fn width_uses_tty_width_when_stdout_is_tty() {
        // Interactive: trust the ioctl-reported width, not the env.
        assert_eq!(effective_terminal_width(Some(200), None, true, 0), 200);
    }

    #[test]
    fn width_ignores_columns_when_stdout_is_tty() {
        // If a shell leaked COLUMNS into our env but we have a real TTY,
        // the TTY measurement wins.
        assert_eq!(effective_terminal_width(Some(200), Some(120), true, 0), 200);
    }

    #[test]
    fn width_falls_back_to_eighty_when_no_signal() {
        // Piped to a plain file with no COLUMNS in env: nothing to go on.
        assert_eq!(effective_terminal_width(None, None, false, 0), 80);
    }

    #[test]
    fn width_offset_stacks_on_top_of_detection() {
        assert_eq!(effective_terminal_width(Some(200), None, true, 3), 197);
        // 120 (COLUMNS) - 1 (scroll bar) - 2 (offset) = 117
        assert_eq!(effective_terminal_width(None, Some(120), false, 2), 117);
    }

    #[test]
    fn width_never_drops_below_one() {
        // A pathologically large offset should clamp to 1, not underflow.
        assert_eq!(effective_terminal_width(Some(10), None, true, 999), 1);
    }

    #[test]
    fn force_colors_when_piped_to_wrapper_with_columns_env() {
        assert!(should_force_colors(false, true, false));
    }

    #[test]
    fn no_force_colors_when_interactive() {
        // TTY → let colored auto-detect (it will say yes anyway).
        assert!(!should_force_colors(true, true, false));
        assert!(!should_force_colors(true, false, false));
    }

    #[test]
    fn no_force_colors_when_piped_without_columns_env() {
        // Plain pipe to file: respect the colored crate's default (off).
        assert!(!should_force_colors(false, false, false));
    }

    #[test]
    fn no_force_colors_when_no_color_env_set() {
        // Honor https://no-color.org even when under viddy.
        assert!(!should_force_colors(false, true, true));
    }
}

/// Get mtime ages for each entry's path, where the path still exists on disk.
///
/// `repo_root` anchors the lookup: `git status --porcelain=v2 -z` reports
/// paths relative to the repo root, not the cwd, so resolving against the
/// cwd misses every file when gsw runs from a subdirectory. Falls back to
/// cwd-relative resolution when the root can't be determined.
fn collect_ages(entries: &[FileEntry], repo_root: Option<&Path>) -> HashMap<String, Duration> {
    let now = SystemTime::now();
    let mut out = HashMap::with_capacity(entries.len());
    for e in entries {
        if out.contains_key(&e.path) {
            continue;
        }
        let full = match repo_root {
            Some(root) => root.join(&e.path),
            None => PathBuf::from(&e.path),
        };
        let Ok(meta) = std::fs::metadata(&full) else {
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

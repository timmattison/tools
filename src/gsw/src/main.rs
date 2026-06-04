use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;

use crate::git::FileEntry;
use crate::render::{plan_section_caps, render, LogEntry, RenderOptions, Snapshot};
use crate::snapshot::build_snapshot;

mod age;
mod bar;
mod git;
mod render;
mod repo;
mod snapshot;
mod watch;

#[derive(Parser)]
#[command(name = "gsw")]
#[command(version = version_string!())]
#[command(
    about = "Compact git status watch — event-driven, self-refreshing branch-state view",
    long_about = "Prints a compact, color-coded view of the current branch's state: \
                  commits ahead of the base branch, last-commit age, and a per-file \
                  list showing a magnitude bar, +/- counts, and recency. On a TTY it \
                  runs as a self-refreshing watch that repaints on filesystem changes; \
                  with `--one-shot` (or when its output is piped) it renders once and \
                  exits."
)]
struct Cli {
    /// Render once and exit instead of entering the live watch loop. This is
    /// the classic behavior; on a TTY, watch mode is the default. Output that
    /// is piped/captured (not a TTY) always falls back to this single render.
    #[arg(long)]
    one_shot: bool,

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

    /// Number of recent commits to show in the `git log --oneline`-style
    /// section appended after the file list.
    #[arg(long, default_value_t = 20)]
    log_lines: usize,

    /// Disable the recent-commit section entirely.
    #[arg(long)]
    no_log: bool,

    /// Force the 24-bit truecolor commit-log gradient on, regardless of
    /// what `COLORTERM` says. Useful when a wrapper (cargo run, viddy)
    /// strips the env var or your terminal doesn't export it.
    #[arg(long, conflicts_with = "no_truecolor")]
    truecolor: bool,

    /// Force the 24-bit truecolor commit-log gradient off, falling back
    /// to the 8-color path even on a terminal that supports truecolor.
    #[arg(long, conflicts_with = "truecolor")]
    no_truecolor: bool,
}

/// Decide the effective terminal width gsw should render for.
///
/// Always leaves one cell of margin against the detected column count:
/// - Direct TTY: rendering a row exactly `cols` cells wide collides with
///   DECAWM auto-wrap quirks and right-edge chrome (scrollbars, padding)
///   on many terminals, pushing the last glyph onto the next line. The
///   margin keeps the rightmost cell empty.
/// - Watch-like wrapper (stdout not a TTY but `COLUMNS` set, e.g. viddy):
///   `COLUMNS` reports the full terminal width but the wrapper renders
///   into a content area one column narrower (its scroll indicator).
/// - Fallback (no signal): treat the implicit 80-column default the same
///   way for consistency.
///
/// `width_offset` always stacks on top, and the result is at least 1.
pub(crate) fn effective_terminal_width(
    tty_width: Option<usize>,
    columns_env: Option<usize>,
    stdout_is_tty: bool,
    width_offset: usize,
) -> usize {
    let detected = match (stdout_is_tty, columns_env) {
        (false, Some(cols)) => cols,
        _ => tty_width.unwrap_or(DEFAULT_TERMINAL_WIDTH),
    };
    detected
        .saturating_sub(1)
        .saturating_sub(width_offset)
        .max(1)
}

/// Rows a watch-like wrapper paints for its own chrome (header, status/help
/// bar, surrounding padding) before and after our output. The wrapper exports
/// the *full* terminal height via `LINES` but only hands the command a smaller
/// content area, so we reserve these rows or the bottom of our frame — the
/// file list — gets clipped below the fold.
///
/// Measured empirically for viddy 1.3.0 (gsw's primary wrapper, per Cargo.toml):
/// a 30-row terminal shows exactly 26 lines of command output, i.e. 4 rows of
/// chrome, and this holds constant across terminal heights (20→16, 40→36).
/// `watch(1)` uses fewer (~2); reserving the larger value only leaves a couple
/// of harmless blank rows there, whereas reserving too few clips real content.
pub(crate) const WRAPPER_CHROME_ROWS: usize = 4;

/// Width assumed when no terminal-size signal is available at all (stdout is
/// piped and the wrapper didn't export `COLUMNS`). The classic 80-column
/// default; the one-cell DECAWM safety margin still applies on top.
pub(crate) const DEFAULT_TERMINAL_WIDTH: usize = 80;

/// Height assumed when no terminal-size signal is available at all (stdout is
/// piped and the wrapper didn't export `LINES`). Matches the classic VT100
/// default and the width fallback's spirit.
pub(crate) const DEFAULT_TERMINAL_HEIGHT: usize = 24;

/// Decide how many terminal rows gsw should fit its output within.
///
/// Mirrors [`effective_terminal_width`]: when stdout is captured by a
/// watch-like wrapper (not a TTY) that exports `LINES`, trust that height —
/// minus [`WRAPPER_CHROME_ROWS`] for the wrapper's own header — because
/// `terminal_size()` can't see through the pipe. With a direct TTY, use the
/// queried height. With no signal at all, fall back to
/// [`DEFAULT_TERMINAL_HEIGHT`].
pub(crate) fn effective_terminal_height(
    tty_height: Option<usize>,
    lines_env: Option<usize>,
    stdout_is_tty: bool,
) -> usize {
    match (stdout_is_tty, lines_env) {
        (false, Some(lines)) => lines.saturating_sub(WRAPPER_CHROME_ROWS).max(1),
        _ => tty_height.unwrap_or(DEFAULT_TERMINAL_HEIGHT),
    }
}

/// Should `colored::control::set_override(true)` be called?
///
/// True only when output is captured by a watch-like wrapper (stdout is not
/// a TTY *and* `COLUMNS` is set in env), and the user has not asked to
/// suppress colors via `NO_COLOR`. The wrapper renders the captured bytes
/// inside its own TTY-backed UI, so colors should pass through.
fn should_force_colors(stdout_is_tty: bool, columns_env_present: bool, no_color_env: bool) -> bool {
    !stdout_is_tty && columns_env_present && !no_color_env
}

/// Does the active terminal advertise 24-bit color support?
///
/// We trust the `COLORTERM` env var (the de facto signal) — the canonical
/// values are `truecolor` and `24bit`. Anything else, including a missing
/// var, is treated as "no truecolor" and the renderer falls back to the
/// eight-color path. Comparison is case-insensitive.
fn truecolor_supported(colorterm_env: Option<&str>) -> bool {
    matches!(
        colorterm_env.map(str::to_ascii_lowercase).as_deref(),
        Some("truecolor" | "24bit")
    )
}

/// Resolve the effective truecolor setting from CLI flags, env, and detection.
///
/// Priority, highest first:
///   1. `--no-color` or `NO_COLOR` → false (kills all color)
///   2. `--no-truecolor` → false (force the 8-color path)
///   3. `--truecolor` → true (force the gradient regardless of detection)
///   4. otherwise, auto-detect via `COLORTERM`
fn effective_truecolor(
    cli_no_color: bool,
    cli_force_truecolor: bool,
    cli_force_no_truecolor: bool,
    no_color_env: bool,
    colorterm_env: Option<&str>,
) -> bool {
    if cli_no_color || no_color_env || cli_force_no_truecolor {
        false
    } else if cli_force_truecolor {
        true
    } else {
        truecolor_supported(colorterm_env)
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let stdout_is_tty = std::io::stdout().is_terminal();
    let columns_env: Option<usize> = std::env::var("COLUMNS").ok().and_then(|s| s.parse().ok());
    let no_color_env = std::env::var_os("NO_COLOR").is_some();
    let colorterm_env = std::env::var("COLORTERM").ok();
    let truecolor = effective_truecolor(
        cli.no_color,
        cli.truecolor,
        cli.no_truecolor,
        no_color_env,
        colorterm_env.as_deref(),
    );

    if cli.no_color {
        colored::control::set_override(false);
    } else if should_force_colors(stdout_is_tty, columns_env.is_some(), no_color_env) {
        // A watch-like wrapper (e.g. viddy) is rendering our output inside
        // its own TTY-backed UI. The colored crate would otherwise strip
        // colors because our stdout is a pipe.
        colored::control::set_override(true);
    }

    let Some(repo) = repo::open() else {
        println!("{}", "gsw • not a git repository".dimmed());
        return Ok(());
    };

    // Everything the renderer needs that doesn't depend on the live terminal
    // size. In watch mode this is computed once and reused for every repaint.
    let cfg = RenderConfig {
        base: cli.base,
        max_files: cli.max_files,
        bar_width: cli.bar_width,
        log_lines: if cli.no_log { 0 } else { cli.log_lines },
        truecolor,
        width_offset: cli.width_offset,
    };

    match decide_mode(cli.one_shot, stdout_is_tty) {
        watch::Mode::OneShot => {
            // Preserve the viddy-aware env sizing and the trailing newline of
            // the historical one-shot output exactly.
            let tty_size =
                terminal_size::terminal_size().map(|(w, h)| (usize::from(w.0), usize::from(h.0)));
            let lines_env: Option<usize> = std::env::var("LINES").ok().and_then(|s| s.parse().ok());
            let dims = watch::resolve_dimensions(
                watch::Mode::OneShot,
                &watch::SizeInputs {
                    tty_width: tty_size.map(|(w, _)| w),
                    tty_height: tty_size.map(|(_, h)| h),
                    columns_env,
                    lines_env,
                    stdout_is_tty,
                    width_offset: cfg.width_offset,
                },
            );
            println!("{}", build_output(&repo, &cfg, dims)?.output);
            Ok(())
        }
        watch::Mode::Watch => watch::run(&repo, &cfg),
    }
}

/// Which rendering mode to run in once a working-tree repo is in hand.
///
/// Watch mode is the default, but it only makes sense when there is a live
/// terminal to take over: `--one-shot` and any non-TTY stdout (a pipe, a file,
/// a stale `viddy gsw` wrapper) fall back to a single render. The not-a-repo
/// case is handled earlier and never reaches here.
fn decide_mode(force_one_shot: bool, stdout_is_tty: bool) -> watch::Mode {
    if force_one_shot || !stdout_is_tty {
        watch::Mode::OneShot
    } else {
        watch::Mode::Watch
    }
}

/// Per-render tunables derived from the CLI, independent of the live terminal
/// size. Watch mode recomputes the output on every repaint from this plus the
/// current [`watch::Dimensions`]; one-shot uses it once.
pub(crate) struct RenderConfig {
    /// Explicit base ref, or `None` to auto-resolve (main → master → origin/HEAD).
    pub base: Option<String>,
    /// User-set file-row cap (`--max-files`), or `None` for the adaptive split.
    pub max_files: Option<usize>,
    /// Magnitude-bar width in cells.
    pub bar_width: usize,
    /// Recent-commit rows to request; `0` when `--no-log` suppressed the section.
    pub log_lines: usize,
    /// Whether the 24-bit truecolor commit-log gradient is in effect.
    pub truecolor: bool,
    /// Columns to subtract from the detected width (`--width-offset`).
    pub width_offset: usize,
}

/// A rendered frame plus the metadata watch mode needs to schedule its next
/// time-driven refresh. One-shot mode reads only [`Render::output`]; watch mode
/// also uses [`Render::freshest_age`] to pick the decay-timer cadence.
pub(crate) struct Render {
    /// The colored, multi-line frame, ready to print or paint.
    pub output: String,
    /// Age of the freshest displayed item (newest commit or working-tree
    /// change), or `None` when nothing aging is on screen. Drives the adaptive
    /// decay-timer cadence via [`watch::next_tick`].
    pub freshest_age: Option<Duration>,
}

/// Age of the freshest item the frame displays — the newest commit or the most
/// recent working-tree change, whichever is younger.
///
/// This is what the watch-mode decay timer keys its cadence off: a young frame
/// ticks fast to keep the live seconds/minutes text and the color fade current,
/// while an old one lets the timer idle. `None` means nothing aging is on
/// screen (no commits *and* a clean tree), which the caller reads as "disable
/// the timer". Files with no recorded mtime (deleted files, untracked dirs)
/// contribute nothing.
fn snapshot_freshest_age(snapshot: &Snapshot) -> Option<Duration> {
    // The youngest item wins, so the timer ticks fast enough for whatever is
    // freshest. Files with no recorded mtime contribute nothing.
    let freshest_change = snapshot.files.iter().filter_map(|f| f.age).min();
    [snapshot.last_commit_age, freshest_change]
        .into_iter()
        .flatten()
        .min()
}

/// Walk the repository and render the full status output for `dims`.
///
/// Pure with respect to the terminal: it returns the rendered frame (and the
/// freshest displayed-item age for the decay timer) and never touches stdout,
/// so callers decide how to display it — one-shot prints it, watch mode paints
/// it into the alternate screen. The row-budget split (file list vs. log
/// section) is computed here from `dims.height`.
pub(crate) fn build_output(
    repo: &gix::Repository,
    cfg: &RenderConfig,
    dims: watch::Dimensions,
) -> Result<Render> {
    let branch = repo::branch_name(repo);

    let base = cfg.base.clone().unwrap_or_else(|| repo::resolve_base(repo));
    let commits_ahead = repo::base_status(repo, &base).ahead;

    let last_commit_age = last_commit_age(repo);

    let repo::Changes {
        entries,
        staged_numstat,
        unstaged_numstat,
    } = repo::collect_changes(repo)?;

    let ages = collect_ages(&entries, repo.workdir());

    let mut snapshot = build_snapshot(
        branch,
        base,
        commits_ahead,
        last_commit_age,
        entries,
        &staged_numstat,
        &unstaged_numstat,
        &ages,
    );

    snapshot.log = fetch_log(repo, cfg.log_lines);

    snapshot.upstream = repo::upstream_status(repo);

    let terminal_width = dims.width;
    let terminal_height = dims.height;

    // Split available terminal rows between the file list and the log
    // section based on what each actually needs to show. Chrome we
    // deduct up front:
    //   header                                                          1
    //   post-header separator                                            1
    //   inter-section separator (only when both sections render)         0 or 1
    //   reserved row for a `+N more files` footer (only when files > 0)  0 or 1
    // Whatever's left goes to the file list first — it's the primary
    // content and renders at the bottom, so it must stay fully on-screen
    // rather than being squeezed by a long log (`--log-lines` defaults to
    // 20). The log takes the remaining rows; only when the file list is
    // itself truncated does a floor claw rows back to it. See
    // `plan_section_caps`.
    let file_count = snapshot.files.len();
    let log_count = snapshot.log.len();
    let header_chrome: usize = 2;
    let inter_chrome: usize = if file_count > 0 && log_count > 0 {
        1
    } else {
        0
    };
    let footer_chrome: usize = if file_count > 0 { 1 } else { 0 };
    let chrome = header_chrome + inter_chrome + footer_chrome;
    let available_rows = terminal_height.saturating_sub(chrome).max(1);
    let (planned_file_cap, planned_log_cap) =
        plan_section_caps(file_count, log_count, available_rows);

    // `--max-files` always wins when the user has set it (including 0,
    // which means unlimited). When the user pinned a file cap, the log
    // section just takes whatever rows are left over up to its demand.
    let (file_cap_opt, log_cap) = match cfg.max_files {
        Some(n) => {
            let consumed_by_files = if n == 0 {
                file_count
            } else {
                n.min(file_count)
            };
            let log_budget = available_rows.saturating_sub(consumed_by_files);
            (Some(n), log_count.min(log_budget))
        }
        None => (Some(planned_file_cap), planned_log_cap),
    };

    let opts = RenderOptions {
        terminal_width,
        bar_width: cfg.bar_width,
        max_files: file_cap_opt,
        log_lines: log_cap,
        truecolor: cfg.truecolor,
    };

    let output = render(&snapshot, &opts);
    let freshest_age = snapshot_freshest_age(&snapshot);
    Ok(Render {
        output,
        freshest_age,
    })
}

/// Fetch the `n` most recent commits as [`LogEntry`] records via gix.
///
/// Returns an empty list when `n == 0` or the repo has no commits.
fn fetch_log(repo: &gix::Repository, n: usize) -> Vec<LogEntry> {
    let now = SystemTime::now();
    repo::recent_log(repo, n)
        .into_iter()
        .map(|(hash, secs, subject)| {
            let age = u64::try_from(secs)
                .ok()
                .map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s))
                .and_then(|when| now.duration_since(when).ok())
                .unwrap_or(Duration::ZERO);
            LogEntry { hash, subject, age }
        })
        .collect()
}

/// How long ago HEAD was committed, or `None` when undeterminable.
fn last_commit_age(repo: &gix::Repository) -> Option<Duration> {
    let secs = repo::head_commit_secs(repo)?;
    let secs = u64::try_from(secs).ok()?;
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    SystemTime::now().duration_since(when).ok()
}

/// Get mtime ages for each entry's path, where the path still exists on disk.
///
/// `repo_root` anchors the lookup: gix status reports paths relative to the
/// repo root, not the cwd, so resolving against the cwd misses every file
/// when gsw runs from a subdirectory. Falls back to cwd-relative resolution
/// when the root can't be determined.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::FileStatus;
    use crate::render::RenderEntry;

    /// Build a minimal [`Snapshot`] with the given HEAD-commit age and a file
    /// row per supplied mtime age, so the freshest-age tests can exercise the
    /// commit-vs-change comparison without walking a real repo.
    fn snapshot_with(
        last_commit_age: Option<Duration>,
        file_ages: &[Option<Duration>],
    ) -> Snapshot {
        let files = file_ages
            .iter()
            .enumerate()
            .map(|(i, age)| RenderEntry {
                path: format!("f{i}.rs"),
                orig_path: None,
                status: FileStatus::Modified,
                staged: false,
                adds: 0,
                dels: 0,
                binary: false,
                age: *age,
            })
            .collect();
        Snapshot {
            branch: "b".into(),
            base: "main".into(),
            commits_ahead: 0,
            last_commit_age,
            files,
            log: Vec::new(),
            upstream: None,
        }
    }

    #[test]
    fn freshest_age_picks_the_younger_of_commit_and_change() {
        // Both a recent commit and a recent edit are on screen; the timer must
        // key off whichever is younger so it ticks fast enough for both.
        let snap = snapshot_with(
            Some(Duration::from_secs(100)),
            &[Some(Duration::from_secs(30))],
        );
        assert_eq!(
            snapshot_freshest_age(&snap),
            Some(Duration::from_secs(30)),
            "the freshest change (30s) is younger than the commit (100s)",
        );

        let snap = snapshot_with(
            Some(Duration::from_secs(30)),
            &[Some(Duration::from_secs(100))],
        );
        assert_eq!(
            snapshot_freshest_age(&snap),
            Some(Duration::from_secs(30)),
            "the commit (30s) is younger than the freshest change (100s)",
        );
    }

    #[test]
    fn freshest_age_uses_commit_when_tree_is_clean() {
        // No working-tree changes: the newest commit is the only aging item.
        let snap = snapshot_with(Some(Duration::from_secs(42)), &[]);
        assert_eq!(snapshot_freshest_age(&snap), Some(Duration::from_secs(42)));
    }

    #[test]
    fn freshest_age_uses_change_when_there_is_no_commit() {
        // Fresh repo with no commits but a staged/edited file: the change ages.
        let snap = snapshot_with(None, &[Some(Duration::from_secs(45))]);
        assert_eq!(snapshot_freshest_age(&snap), Some(Duration::from_secs(45)));
    }

    #[test]
    fn freshest_age_ignores_files_without_an_mtime() {
        // Deleted files / untracked dirs carry no mtime; they must not drag the
        // freshest age toward zero or otherwise distort the cadence.
        let snap = snapshot_with(
            Some(Duration::from_secs(90)),
            &[None, Some(Duration::from_secs(50)), None],
        );
        assert_eq!(
            snapshot_freshest_age(&snap),
            Some(Duration::from_secs(50)),
            "the only file with an mtime (50s) wins over the 90s commit",
        );
    }

    #[test]
    fn freshest_age_is_none_for_an_empty_clean_repo() {
        // Nothing aging on screen (no commits, clean tree) → the timer should
        // be disabled, which the caller infers from `None`.
        let snap = snapshot_with(None, &[None, None]);
        assert_eq!(snapshot_freshest_age(&snap), None);
    }

    #[test]
    fn decide_mode_truth_table() {
        // Watch mode only when nothing forces a single render: no --one-shot
        // and a real TTY to take over.
        assert_eq!(decide_mode(false, true), watch::Mode::Watch);
        // --one-shot always wins, even on a TTY.
        assert_eq!(decide_mode(true, true), watch::Mode::OneShot);
        // Non-TTY (piped/captured) always falls back to one-shot, regardless
        // of the flag — this keeps `gsw | …` and stale `viddy gsw` working.
        assert_eq!(decide_mode(false, false), watch::Mode::OneShot);
        assert_eq!(decide_mode(true, false), watch::Mode::OneShot);
    }

    #[test]
    fn width_uses_columns_minus_one_when_stdout_not_tty() {
        // viddy case: pipes captured, COLUMNS exported.
        assert_eq!(effective_terminal_width(None, Some(120), false, 0), 119);
    }

    #[test]
    fn height_uses_lines_env_minus_wrapper_chrome_when_stdout_not_tty() {
        // viddy/watch case: stdout piped, LINES exported. We budget to the
        // wrapper's height minus its title chrome so the bottom file list
        // isn't clipped below the wrapper's header.
        assert_eq!(
            effective_terminal_height(None, Some(40), false),
            40 - WRAPPER_CHROME_ROWS,
        );
    }

    #[test]
    fn height_uses_tty_height_when_stdout_is_tty() {
        // Interactive: trust the ioctl-reported height and ignore any stale
        // inherited LINES value.
        assert_eq!(effective_terminal_height(Some(50), Some(9999), true), 50);
    }

    #[test]
    fn height_falls_back_to_default_when_no_signal() {
        // Piped with no LINES exported: nothing to go on, so assume the
        // classic 24-row terminal.
        assert_eq!(
            effective_terminal_height(None, None, false),
            DEFAULT_TERMINAL_HEIGHT,
        );
    }

    #[test]
    fn height_never_collapses_to_zero_under_tiny_wrapper() {
        // A pathologically short wrapper height must still leave at least one
        // row rather than underflowing to zero.
        assert_eq!(effective_terminal_height(None, Some(1), false), 1);
    }

    #[test]
    fn width_leaves_safety_margin_when_stdout_is_tty() {
        // Direct TTY: terminal_size reports the full column count, but if
        // gsw renders a row exactly that many cells wide, terminals with
        // auto-wrap (DECAWM) or right-edge chrome (scrollbars, padding)
        // push the rightmost glyph onto the next line — the user sees the
        // last character of the age column wrap. Leave one cell of margin,
        // matching the viddy path so direct and viddy renderings agree.
        assert_eq!(effective_terminal_width(Some(200), None, true, 0), 199);
    }

    #[test]
    fn width_uses_tty_width_when_stdout_is_tty() {
        // Interactive: trust the ioctl-reported width, not the env — but
        // still subtract the one-cell safety margin.
        assert_eq!(effective_terminal_width(Some(200), None, true, 0), 199);
    }

    #[test]
    fn width_ignores_columns_when_stdout_is_tty() {
        // If a shell leaked COLUMNS into our env but we have a real TTY,
        // the TTY measurement wins.
        assert_eq!(effective_terminal_width(Some(200), Some(120), true, 0), 199);
    }

    #[test]
    fn width_falls_back_to_eighty_minus_margin_when_no_signal() {
        // Piped to a plain file with no COLUMNS in env: nothing to go on,
        // so fall back to the 80-column default. The safety margin still
        // applies so the fallback matches the detected paths.
        assert_eq!(effective_terminal_width(None, None, false, 0), 79);
    }

    #[test]
    fn width_offset_stacks_on_top_of_detection() {
        // 200 (TTY) - 1 (safety margin) - 3 (offset) = 196
        assert_eq!(effective_terminal_width(Some(200), None, true, 3), 196);
        // 120 (COLUMNS) - 1 (safety margin) - 2 (offset) = 117
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

    #[test]
    fn truecolor_supported_when_colorterm_is_truecolor() {
        assert!(truecolor_supported(Some("truecolor")));
    }

    #[test]
    fn truecolor_supported_when_colorterm_is_24bit() {
        assert!(truecolor_supported(Some("24bit")));
    }

    #[test]
    fn truecolor_supported_is_case_insensitive() {
        // Some terminals export uppercase or mixed-case values. Treat them
        // as equivalent so we don't accidentally fall back on, say, gnome's
        // "Truecolor".
        assert!(truecolor_supported(Some("TrueColor")));
        assert!(truecolor_supported(Some("TRUECOLOR")));
        assert!(truecolor_supported(Some("24BIT")));
    }

    #[test]
    fn truecolor_not_supported_when_colorterm_missing() {
        // No COLORTERM at all — typical for old terminals or shells that
        // strip it. Stay safe: assume 8-color until told otherwise.
        assert!(!truecolor_supported(None));
    }

    #[test]
    fn truecolor_not_supported_for_unknown_colorterm_value() {
        // COLORTERM is set but to something we don't recognize (some
        // terminals export "1" or vendor-specific strings). Don't guess —
        // fall back to the 8-color path.
        assert!(!truecolor_supported(Some("1")));
        assert!(!truecolor_supported(Some("xterm-256color")));
        assert!(!truecolor_supported(Some("")));
    }

    // --- --truecolor / --no-truecolor override --------------------------

    #[test]
    fn truecolor_flag_forces_on_when_env_unset() {
        // The escape hatch: some terminals support 24-bit color but don't
        // export COLORTERM (or strip it through wrappers like cargo run /
        // viddy). `--truecolor` lets the user assert capability directly.
        assert!(effective_truecolor(false, true, false, false, None));
    }

    #[test]
    fn truecolor_flag_forces_on_when_env_unrecognized() {
        // Same escape hatch when COLORTERM is set to something we don't
        // know how to interpret.
        assert!(effective_truecolor(false, true, false, false, Some("1")));
    }

    #[test]
    fn no_truecolor_flag_forces_off_even_with_colorterm() {
        // Symmetric escape hatch: users on truecolor terminals can opt
        // back to the legacy 8-color path (e.g. screen-recording, or just
        // preferring the look).
        assert!(!effective_truecolor(
            false,
            false,
            true,
            false,
            Some("truecolor")
        ));
    }

    #[test]
    fn no_color_beats_truecolor_flag() {
        // `--no-color` / `$NO_COLOR` mean "no colors at all" — overriding
        // them with `--truecolor` would re-enable the very thing the user
        // opted out of. Honor the opt-out.
        assert!(!effective_truecolor(
            true,
            true,
            false,
            false,
            Some("truecolor")
        ));
        assert!(!effective_truecolor(
            false,
            true,
            false,
            true,
            Some("truecolor")
        ));
    }

    #[test]
    fn truecolor_auto_uses_colorterm_when_no_flags() {
        // No CLI overrides → fall back to the existing COLORTERM detection.
        assert!(effective_truecolor(
            false,
            false,
            false,
            false,
            Some("truecolor")
        ));
        assert!(!effective_truecolor(false, false, false, false, None));
        assert!(!effective_truecolor(
            false,
            false,
            false,
            false,
            Some("xterm-256color")
        ));
    }
}

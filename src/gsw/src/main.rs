use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;

use crate::git::{FileEntry, FileStatus};
use crate::render::{
    plan_section_caps, render, render_with_offset, LogEntry, RefreshStatus, RenderOptions, Snapshot,
};
use crate::snapshot::build_snapshot;
use termwindow::should_force_colors;

mod age;
mod bar;
/// Starting a child process that cannot reach the terminal gsw is drawing on.
mod child;
/// Measuring a rebase and a merge against the default branch, for the `m` key.
mod conflicts;
mod git;
/// Opening the issue that the branch names, through a command the user supplies.
mod issue;
/// Cutting a child process's byte chunks into lines that are safe to paint.
mod lines;
mod push;
/// Deciding whether the person who reads this screen sits at this machine.
mod remote;
mod render;
mod repo;
/// The shell that runs a command the user supplies, for every key that runs one.
mod shell;
mod snapshot;
/// Shared git fixtures for the unit tests. Test-only: it shells out to `git` to
/// build throwaway repositories, which the shipped binary never does.
#[cfg(test)]
mod testrepo;
/// Bringing the branch up to date with the base, through a command the user
/// supplies, for the `R` and `M` keys.
mod update;
mod watch;
/// The worktrees of the repository, sorted by path, for the arrow keys.
mod worktrees;

#[derive(Parser)]
#[command(name = "gsw")]
#[command(version = version_string!())]
#[command(
    about = "Compact git status watch — event-driven, self-refreshing branch-state view",
    long_about = "Prints a compact, color-coded view of the current branch's state: \
                  commits ahead of (and behind) the base branch, a recent-commit log, and a \
                  per-file list showing a magnitude bar, +/- counts, and recency. On a TTY it \
                  runs as a self-refreshing watch that repaints on filesystem changes and \
                  re-walks the repository every --refresh-interval seconds, with the \
                  separator under the header showing how stale the screen is and how long \
                  until the next refresh; with `--one-shot` (or when its output is piped) \
                  it renders once and exits.\n\n\
                  Watch-mode keys: q or Ctrl-C quits, r refreshes now, p pushes the current \
                  branch after a confirmation that names what it will do — a branch not yet on \
                  the remote is confirmed as creating one — G opens the issue the branch \
                  names, m measures a rebase and a merge against the default branch, and the \
                  arrow keys move the watch between the worktrees of the repository. \
                  A push whose branch stopped being \
                  checked out between the question and the answer is refused, not redirected. \
                  p never force-pushes.\n\n\
                  G runs one command in your own interactive shell, with the work tree as its \
                  current directory. GSW_ISSUE_COMMAND holds that command and it defaults to \
                  `ggs`; set it to an empty string to turn the key off. The value is a whole \
                  command line, so it can carry arguments — `gh issue view --web` works. gsw \
                  asks your shell about the first word and runs the whole line, so the first \
                  word is what has to exist and the shell reads the rest the way it reads \
                  arguments at a prompt. The command is usually a shell function, so gsw asks \
                  your shell once at startup whether it exists. Where it does not, G does \
                  nothing and says nothing, the way an unbound key does — and a function you \
                  add to your rc file after gsw started needs a restart. A run that works opens a browser and costs the frame no row; a run \
                  that fails puts the last line it wrote under the frame, where it waits for a \
                  key. A run gets a minute: after that gsw stops waiting, says so under the \
                  frame, and gives the key back — the command itself keeps running, because it \
                  is yours, and it can be the process that holds the browser open.\n\n\
                  m measures what grind and grime measure, in this process: a rebase of HEAD \
                  onto the default branch (main, else master) and a merge of that branch into \
                  HEAD. The bottom row says `Running grind and grime against main…` until the \
                  run ends. Then one line such as `main: rebase clean · merge 1 hunk in 1 \
                  file` takes its place and fades off after a minute. One run at a time: m \
                  does nothing while a run is in flight. A quit during a run waits for the \
                  replay in flight, so no scratch worktree stays behind.\n\n\
                  Up goes to the home worktree, where gsw started. Left and Right go to the \
                  previous and the next worktree in path order, which is the order of cwt, and \
                  they wrap. Down opens a list of the worktrees: Up and Down move the cursor, \
                  Enter goes to the worktree under it, and Esc or q closes the list. While the \
                  repository has more than one worktree, the header shows the position of the \
                  worktree and marks the home worktree with ⌂. A switch walks the new worktree \
                  at once and removes every line under the frame. The arrow keys do nothing \
                  while a push runs or while the push question is up. When the worktree on the \
                  screen is removed, gsw goes back to the home worktree. gsw does not change the \
                  directory of the shell that started it.\n\n\
                  While a push runs, a notice reports how long it has taken, and up to six rows \
                  under it carry the newest output from git and from any pre-push hook. Each row \
                  arrives as the hook writes it, so a hook that builds and tests a workspace \
                  shows its progress rather than leaving the screen frozen. A push that fails \
                  shows the last lines of what was said, which is where a hook puts its reason."
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

    /// Disable the recent-commit section entirely. The newest commit's age
    /// lives on the first row of that section, so this takes the commit age
    /// off the frame as well.
    #[arg(long)]
    no_log: bool,

    /// Force the 24-bit truecolor fades on, regardless of what `COLORTERM`
    /// says. The fades are the commit-log gradient, the recency fade on the
    /// file rows, and the fade on the push status message. This flag helps
    /// when a wrapper (cargo run, viddy) removes the env var, or when your
    /// terminal does not export it.
    #[arg(long, conflicts_with = "no_truecolor")]
    truecolor: bool,

    /// Force the 24-bit truecolor fades off, even on a terminal that
    /// supports truecolor. The commit-log gradient and the recency fade on
    /// the file rows then use the 8-color path. The push status message
    /// dims once at the half-way mark instead of a smooth fade.
    #[arg(long, conflicts_with = "truecolor")]
    no_truecolor: bool,

    /// Seconds between watch-mode refreshes when nothing on disk changes.
    /// Filesystem events still refresh immediately; this is the floor under
    /// them, and it is what the "next refresh" countdown in the separator
    /// counts down to. `0` turns the timed refresh off, which also removes the
    /// countdown and leaves gsw purely event-driven. Accepts up to a year.
    #[arg(
        long,
        default_value_t = DEFAULT_REFRESH_SECS,
        value_parser = clap::value_parser!(u64).range(0..=MAX_REFRESH_SECS),
    )]
    refresh_interval: u64,
}

/// Default seconds between timed watch-mode refreshes. A minute keeps a screen
/// left open in a pane honest without walking git often enough to matter — and
/// the duty-cycle budget still overrides it on a repository where a walk is
/// expensive.
const DEFAULT_REFRESH_SECS: u64 = 60;

/// Largest accepted `--refresh-interval`, in seconds: one year.
///
/// Every scheduled deadline is an `Instant` plus a `Duration`, and that
/// addition *panics* on overflow — so an unbounded seconds count turns a typo
/// into an abort, after watch mode has already taken the alternate screen.
/// A year is far past the point where a timed refresh is distinguishable from
/// `0`, and small enough to be representable on any clock, so rejecting
/// anything larger costs nothing real and removes the panic.
const MAX_REFRESH_SECS: u64 = 365 * 24 * 60 * 60;

/// Resolve `--refresh-interval` seconds into the schedule watch mode runs on.
///
/// `0` means "no timed refresh": gsw stays purely event-driven, and with no
/// scheduled walk there is no countdown to print.
fn refresh_interval(secs: u64) -> Option<Duration> {
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Does the active terminal advertise 24-bit color support?
///
/// We trust the `COLORTERM` env var (the de facto signal) — the canonical
/// values are `truecolor` and `24bit`. Anything else, including a missing
/// var, is treated as "no truecolor" and the fades fall back to their
/// non-truecolor styling. Comparison is case-insensitive.
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
///   2. `--no-truecolor` → false (force the fades off)
///   3. `--truecolor` → true (force the fades on regardless of detection)
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
        #[allow(
            clippy::disallowed_methods,
            reason = "this process decides its own color output at startup; the ban covers the tests, which must go through testcolor::with_forced_ansi"
        )]
        colored::control::set_override(false);
    } else if should_force_colors(stdout_is_tty, columns_env.is_some(), no_color_env) {
        // A watch-like wrapper (e.g. viddy) is rendering our output inside
        // its own TTY-backed UI. The colored crate would otherwise strip
        // colors because our stdout is a pipe.
        #[allow(
            clippy::disallowed_methods,
            reason = "this process decides its own color output at startup; the ban covers the tests, which must go through testcolor::with_forced_ansi"
        )]
        colored::control::set_override(true);
    }

    let Some(handle) = repo::RepoHandle::open() else {
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
        refresh_interval: refresh_interval(cli.refresh_interval),
    };

    match decide_mode(cli.one_shot, stdout_is_tty) {
        watch::Mode::OneShot => {
            // Preserve the viddy-aware env sizing and the trailing newline of
            // the historical one-shot output exactly.
            let tty_size = termsize::stdout_size().map(|(w, h)| (usize::from(w), usize::from(h)));
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
            // A fresh process opened this handle a moment ago, so its cached
            // config is current by construction — one render, then exit. Only
            // watch mode, which outlives config edits, needs to re-open.
            println!("{}", build_output(handle.repo(), &cfg, dims)?.output);
            Ok(())
        }
        // Hand the handle over: watch mode owns the repository from here and
        // re-opens it on every refresh.
        watch::Mode::Watch => watch::run(handle, &cfg),
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
    /// Whether the 24-bit truecolor fades are in effect. The fades are the
    /// commit-log gradient, the recency fade on the file rows, and the fade
    /// on the push status message.
    pub truecolor: bool,
    /// Columns to subtract from the detected width (`--width-offset`).
    pub width_offset: usize,
    /// How often watch mode re-walks the repository with no filesystem event to
    /// prompt it (`--refresh-interval`), or `None` to stay purely event-driven.
    pub refresh_interval: Option<Duration>,
}

/// Where a frame sits in time: how stale the snapshot behind it is, and when
/// the next one is due.
///
/// The two travel together because the refresh clock prints both, and printing
/// them from one value is what stops the clock and the ages under it from
/// disagreeing — `age_offset` *is* "last refresh N ago".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameTiming {
    /// How long ago the snapshot was collected. Added (saturating) to every
    /// displayed age. `Duration::ZERO` renders a freshly-walked snapshot.
    pub age_offset: Duration,
    /// How long until the next scheduled walk, or `None` when none is
    /// scheduled — one-shot mode, or `--refresh-interval 0`. `None` also
    /// suppresses the refresh clock: with no schedule there is nothing to count
    /// down to.
    pub next_refresh_in: Option<Duration>,
}

impl FrameTiming {
    /// Timing for a frame painted by a walk that has just finished: nothing has
    /// aged since, and the next walk is a full `interval` away.
    ///
    /// This is watch mode's seed frame and one-shot's only frame. One-shot
    /// passes `None` — it exits, so no walk follows.
    pub(crate) fn at_walk(interval: Option<Duration>) -> Self {
        Self {
            age_offset: Duration::ZERO,
            next_refresh_in: interval,
        }
    }

    /// The refresh clock that the separator of a frame at this timing shows,
    /// or `None` when no walk is scheduled.
    ///
    /// The clock reports the same offset that every age on the frame is
    /// advanced by. Every frame takes its clock from here, so no two frames
    /// show different clocks for one timing.
    fn refresh_status(self) -> Option<RefreshStatus> {
        self.next_refresh_in.map(|next_refresh_in| RefreshStatus {
            last_refresh_ago: self.age_offset,
            next_refresh_in,
        })
    }
}

/// Rows at the top of every frame, above its content: the header, the line of
/// a merge or a rebase in progress, and the separator.
///
/// [`render::render_head`] draws exactly these rows. Every row budget counts
/// them from here, so no frame reserves a different number of rows for them.
fn header_chrome(snapshot: &Snapshot) -> usize {
    2 + usize::from(snapshot.operation.is_some())
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
/// screen, which the caller reads as "disable the timer".
///
/// Both halves are read off the rows the frame draws, so the timer paces the
/// screen rather than the repository. The commit age comes from the newest log
/// row — with `--no-log` there is no commit age on screen and none here either.
/// The section caps are the gap: a row a short terminal cannot fit still counts
/// here, which costs a wake whose repaint is then suppressed. Items with no
/// recorded age (deleted files, untracked dirs, a commit timestamp that does
/// not resolve) contribute nothing: they render a fixed mark that never needs
/// repainting.
fn snapshot_freshest_age(snapshot: &Snapshot) -> Option<Duration> {
    // The youngest item wins, so the timer ticks fast enough for whatever is
    // freshest. The log is newest-first, so its head is the newest commit.
    let freshest_change = snapshot.files.iter().filter_map(|f| f.age).min();
    let newest_commit = snapshot.log.first().and_then(|entry| entry.age);
    [newest_commit, freshest_change].into_iter().flatten().min()
}

/// Walk the repository and render the full status frame for `dims`.
///
/// Composes the two halves of the render pipeline: [`collect_snapshot`]
/// assembles the repo state, then [`render_frame`] turns it into a frame at no
/// age offset (`Duration::ZERO`). Because the offset is zero, the output is
/// byte-identical to a direct render; watch mode instead calls the two halves
/// separately so it can re-render a *cached* snapshot at a growing offset
/// without re-walking the repo.
pub(crate) fn build_output(
    repo: &gix::Repository,
    cfg: &RenderConfig,
    dims: watch::Dimensions,
) -> Result<Render> {
    let snapshot = collect_snapshot(repo, cfg)?;
    Ok(render_frame(
        &snapshot,
        cfg,
        dims,
        FrameTiming::at_walk(None),
    ))
}

/// Walk the repository and assemble the fully-populated [`Snapshot`] — the
/// git-work half of the render pipeline.
///
/// This is the expensive, side-effecting step: it queries the current branch,
/// resolves the base ref and counts commits ahead/behind, collects working-tree
/// changes and their mtimes, and fetches the recent-commit log and upstream
/// status. The result is a pure description of
/// repository state, independent of the live terminal — turning it into a frame
/// for a given [`watch::Dimensions`] is the separate, cheap [`render_frame`]
/// half. Watch mode collects once per filesystem change and re-renders the
/// cached snapshot many times. Uses only `cfg.base` and `cfg.log_lines`.
pub(crate) fn collect_snapshot(repo: &gix::Repository, cfg: &RenderConfig) -> Result<Snapshot> {
    let branch = repo::branch_name(repo);

    let base = cfg.base.clone().unwrap_or_else(|| repo::resolve_base(repo));
    let base_status = repo::base_status(repo, &base);

    let repo::Changes {
        entries,
        staged_numstat,
        unstaged_numstat,
    } = repo::collect_changes(repo)?;

    let ages = collect_ages(&entries, repo.workdir());

    let mut snapshot = build_snapshot(
        branch,
        base,
        base_status.ahead,
        base_status.behind,
        entries,
        &staged_numstat,
        &unstaged_numstat,
        &ages,
    );

    snapshot.log = fetch_log(repo, cfg.log_lines);

    snapshot.upstream = repo::upstream_status(repo);
    snapshot.push_remote = repo::push_remote(repo);

    // Surface an in-progress merge/rebase. The conflict count comes for free
    // from the status walk already done — every unmerged path is a
    // `FileStatus::Conflicted` row — so no extra git work is needed.
    let conflicts = u32::try_from(
        snapshot
            .files
            .iter()
            .filter(|f| f.status == FileStatus::Conflicted)
            .count(),
    )
    .unwrap_or(u32::MAX);
    snapshot.operation = repo::operation_state(repo, conflicts);

    Ok(snapshot)
}

/// Render an already-collected [`Snapshot`] into a frame for `dims`, advancing
/// every displayed age — and the returned [`Render::freshest_age`] — by
/// `age_offset`.
///
/// This is the cheap, terminal-shaped half of the pipeline. Given a snapshot
/// from [`collect_snapshot`], it performs the row-budget split (file list vs.
/// log section) from `dims.height`, builds the [`RenderOptions`], and produces
/// the colored frame via [`render_with_offset`]. `age_offset` lets watch mode
/// keep the painted ages and the decay-timer cadence ticking forward between
/// git rescans without re-walking the repo; `Duration::ZERO` reproduces a
/// freshly-collected frame byte-for-byte. Pure with respect to the terminal: it
/// never touches stdout, so callers decide how to display the frame. Uses
/// `cfg.max_files`, `cfg.bar_width`, and `cfg.truecolor`.
pub(crate) fn render_frame(
    snapshot: &Snapshot,
    cfg: &RenderConfig,
    dims: watch::Dimensions,
    timing: FrameTiming,
) -> Render {
    let age_offset = timing.age_offset;
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
    // The operation indicator (merge/rebase) is one extra chrome row between
    // the header and the separator, present only when the snapshot carries an
    // in-progress operation. `header_chrome` reserves it, so the file list at
    // the bottom isn't pushed past the fold.
    let inter_chrome: usize = if file_count > 0 && log_count > 0 {
        1
    } else {
        0
    };
    let footer_chrome: usize = if file_count > 0 { 1 } else { 0 };
    let chrome = header_chrome(snapshot) + inter_chrome + footer_chrome;
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
        refresh: timing.refresh_status(),
    };

    // One-shot mode and the watch seed walk render at offset zero, which is
    // exactly the public `render` entry — route them through it so their output
    // stays the byte-identical call the pre-split `build_output` made. Only the
    // live decay/resize re-renders advance ages and take the offset-aware path.
    let output = if age_offset.is_zero() {
        render(snapshot, &opts)
    } else {
        render_with_offset(snapshot, &opts, age_offset)
    };
    // Advance the freshest age by the same offset so watch mode's decay-timer
    // cadence stays in lockstep with the ages painted above. The uniform shift
    // preserves which item is freshest, so the min() that picked it still holds.
    let freshest_age = snapshot_freshest_age(snapshot).map(|d| d.saturating_add(age_offset));
    Render {
        output,
        freshest_age,
    }
}

/// How many rows of the worktree list the pane shows under the separator.
///
/// The rows under the separator are what the pane leaves under the header
/// chrome ([`header_chrome`]). With two rows or more, the bottom row holds the
/// hint and the list takes the rest. With one row, the list takes it and no
/// hint shows. With no row, the answer is 0: the list cannot open, and an open
/// list closes, so Enter never chooses a row that the user did not see.
pub(crate) fn list_rows(snapshot: &Snapshot, dims: watch::Dimensions) -> usize {
    let under = dims.height.saturating_sub(header_chrome(snapshot));
    match under {
        // No row for the hint: the one row, if the pane has it, is the list.
        0 | 1 => under,
        // The bottom row is the hint.
        _ => under - 1,
    }
}

/// Render the frame of the worktree list for `dims`: the head of the status
/// frame, the rows of `list`, and the hint on the bottom row.
///
/// The header, the line of a merge or a rebase, and the separator with its
/// refresh clock come from [`render::render_head`], as on the status frame.
/// Under them, [`list_rows`] rows show the window of `list` that holds the
/// cursor, and blank rows fill what the list leaves. The hint takes the bottom
/// row of the pane when [`list_rows`] left a row for it. The frame is as tall
/// as the pane, and no row of it is wider than the pane.
///
/// Nothing on the frame ages, so [`Render::freshest_age`] is `None`. Nothing
/// on the frame reads the [`RenderConfig`] either, so the call takes none.
pub(crate) fn render_list_frame(
    snapshot: &Snapshot,
    dims: watch::Dimensions,
    timing: FrameTiming,
    list: &worktrees::WorktreeList,
) -> Render {
    let rows = list_rows(snapshot, dims);
    let window = list.window(rows);
    let mut lines = render::render_head(snapshot, dims.width, timing.refresh_status().as_ref());
    lines.extend(render::list::rows(&window, dims.width));
    // Blank rows fill what a short list leaves, so the hint stays on the
    // bottom row of the pane.
    lines.resize(lines.len() + rows - window.len(), String::new());
    // The hint takes the row that `list_rows` left under the list, when it
    // left one.
    if header_chrome(snapshot) + rows < dims.height {
        lines.push(render::list::hint(dims.width));
    }
    Render {
        output: lines.join("\n"),
        freshest_age: None,
    }
}

/// Fetch the `n` most recent commits as [`LogEntry`] records via gix.
///
/// Returns an empty list when `n == 0` or the repo has no commits.
///
/// A commit whose timestamp does not resolve into an elapsed duration — a
/// negative epoch second, or a time ahead of the local clock through skew or a
/// hand-set `--date` — yields `age: None`. The row then renders the unknown-age
/// mark. Collapsing such a commit to `Duration::ZERO` instead would paint it as
/// the freshest thing on screen, which is the one reading ruled out.
fn fetch_log(repo: &gix::Repository, n: usize) -> Vec<LogEntry> {
    let now = SystemTime::now();
    repo::recent_log(repo, n)
        .into_iter()
        .map(|(hash, secs, subject)| {
            let age = u64::try_from(secs)
                .ok()
                .map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s))
                .and_then(|when| now.duration_since(when).ok());
            LogEntry { hash, subject, age }
        })
        .collect()
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
    use crate::render::Operation;
    use crate::render::RenderEntry;
    use crate::worktrees::{WorktreeBadge, WorktreeEntry, WorktreeList, WorktreePath};

    #[test]
    fn the_frame_a_walk_paints_counts_down_a_whole_interval() {
        // Watch mode's very first frame is painted by the seed walk. It must
        // carry the clock like every frame after it — otherwise gsw opens with a
        // blank rule and grows a clock a second later, which reads as a glitch.
        let interval = Duration::from_secs(60);
        assert_eq!(
            FrameTiming::at_walk(Some(interval)),
            FrameTiming {
                age_offset: Duration::ZERO,
                next_refresh_in: Some(interval),
            },
            "a frame painted by a walk is 0s old with a full interval to run",
        );
    }

    #[test]
    fn a_frame_with_no_schedule_behind_it_shows_no_countdown() {
        // One-shot mode renders and exits, so no walk follows and there is
        // nothing to count down to.
        assert_eq!(FrameTiming::at_walk(None).next_refresh_in, None);
    }

    #[test]
    fn refresh_interval_zero_turns_the_timed_refresh_off() {
        // The escape hatch for anyone who wants gsw's idle cost back at zero:
        // `--refresh-interval 0` schedules nothing, so nothing wakes the loop
        // but the filesystem, and the countdown has nothing to count.
        assert_eq!(
            refresh_interval(0),
            None,
            "0 seconds must mean no timed refresh, not a zero-length one",
        );
    }

    #[test]
    fn refresh_interval_passes_a_positive_value_through() {
        assert_eq!(refresh_interval(60), Some(Duration::from_secs(60)));
        assert_eq!(refresh_interval(1), Some(Duration::from_secs(1)));
        assert_eq!(refresh_interval(3600), Some(Duration::from_secs(3600)));
    }

    #[test]
    fn refresh_interval_rejects_a_value_that_would_overflow_the_clock() {
        // Every scheduled deadline is `Instant + Duration`, which panics on
        // overflow rather than saturating. Verified: an Instant plus
        // Duration::from_secs(1 << 63) has no representable sum, so a
        // 19-digit typo aborts gsw *after* the terminal guard has taken the
        // alternate screen. The parser is where that has to stop, with a
        // message naming the range — not the scheduler, with a panic.
        assert!(
            Cli::try_parse_from(["gsw", "--refresh-interval", "18446744073709551615"]).is_err(),
            "an interval large enough to overflow Instant must be a parse error",
        );
        assert!(
            Cli::try_parse_from(["gsw", "--refresh-interval", &MAX_REFRESH_SECS.to_string()])
                .is_ok(),
            "the largest accepted interval must still parse",
        );
        assert!(
            Cli::try_parse_from(["gsw", "--refresh-interval", "0"]).is_ok(),
            "0 stays valid — it is the documented way to turn the timed refresh off",
        );
    }

    #[test]
    fn refresh_interval_defaults_to_a_minute() {
        let cli = Cli::parse_from(["gsw"]);
        assert_eq!(
            refresh_interval(cli.refresh_interval),
            Some(Duration::from_secs(60)),
            "gsw with no flags should refresh once a minute",
        );
        let explicit = Cli::parse_from(["gsw", "--refresh-interval", "5"]);
        assert_eq!(
            refresh_interval(explicit.refresh_interval),
            Some(Duration::from_secs(5)),
        );
    }

    #[test]
    fn operation_line_reserves_a_chrome_row_so_file_list_is_not_clipped() {
        // When an in-progress operation adds its indicator line between the
        // header and the separator, render_frame must count that line as
        // chrome. Otherwise the row budget is one too generous and the rendered
        // frame overflows the terminal height — a watch wrapper (viddy) then
        // clips the bottom of the file list below the fold. The whole frame
        // must fit within `height` when the indicator is shown.
        let files: Vec<RenderEntry> = (0..20)
            .map(|i| RenderEntry {
                path: format!("f{i}.rs"),
                orig_path: None,
                status: FileStatus::Modified,
                staged: false,
                adds: 1,
                dels: 0,
                binary: false,
                age: Some(Duration::from_secs(30)),
            })
            .collect();
        let cfg = RenderConfig {
            base: None,
            max_files: None,
            bar_width: 6,
            log_lines: 0,
            truecolor: false,
            refresh_interval: None,
            width_offset: 0,
        };
        let dims = watch::Dimensions {
            width: 80,
            height: 8,
        };
        let snap = Snapshot {
            branch: "b".into(),
            base: "main".into(),
            commits_ahead: 0,
            commits_behind: 0,
            files,
            log: Vec::new(),
            upstream: None,
            operation: Some(Operation::Merge { conflicts: 1 }),
            push_remote: None,
            worktree: None,
        };
        let frame = render_frame(&snap, &cfg, dims, FrameTiming::at_walk(None));
        let lines = frame.output.lines().count();
        assert!(
            lines <= dims.height,
            "frame ({lines} lines) must fit within the terminal height ({}) when an \
             operation indicator is shown; the indicator row must be reserved as chrome:\n{}",
            dims.height,
            frame.output,
        );
    }

    /// A pane of `height` rows and 80 columns. The width does not change how
    /// many rows the list takes.
    fn pane_of(height: usize) -> watch::Dimensions {
        watch::Dimensions { width: 80, height }
    }

    #[test]
    fn list_rows_leaves_the_bottom_row_for_the_hint() {
        // The header and the separator take two rows. The list takes every
        // other row but the bottom row, which holds the hint.
        let snap = snapshot_with(None, &[]);
        assert_eq!(list_rows(&snap, pane_of(24)), 21);
        assert_eq!(
            list_rows(&snap, pane_of(8)),
            5,
            "the pane of the example in the issue: four worktrees, one blank row, and the hint",
        );
        assert_eq!(
            list_rows(&snap, pane_of(4)),
            1,
            "two rows under the separator: one row of the list and the hint",
        );
    }

    #[test]
    fn list_rows_gives_one_row_and_no_hint_to_a_pane_with_one_row_under_the_separator() {
        // The one row goes to the list and not to the hint. A hint with no
        // row of the list names the keys of a list that the user cannot see.
        // A pane with no row under the separator shows no list: the list
        // cannot open, and an open list closes, so Enter never chooses a row
        // that the user did not see.
        let snap = snapshot_with(None, &[]);
        assert_eq!(
            list_rows(&snap, pane_of(3)),
            1,
            "one row under the separator"
        );
        for height in [2, 1, 0] {
            assert_eq!(
                list_rows(&snap, pane_of(height)),
                0,
                "a pane of {height} rows has no row under the separator",
            );
        }
    }

    #[test]
    fn list_rows_gives_a_row_to_the_line_of_a_merge_or_a_rebase() {
        // The line of an operation in progress sits between the header and
        // the separator, so the list has one row less.
        for operation in [
            Operation::Merge { conflicts: 1 },
            Operation::Rebase {
                step: None,
                conflicts: 0,
            },
        ] {
            let mut snap = snapshot_with(None, &[]);
            snap.operation = Some(operation.clone());
            assert_eq!(
                list_rows(&snap, pane_of(24)),
                20,
                "{operation:?} in a pane of 24 rows",
            );
            assert_eq!(
                list_rows(&snap, pane_of(5)),
                1,
                "{operation:?}: one row of the list and the hint",
            );
            assert_eq!(
                list_rows(&snap, pane_of(4)),
                1,
                "{operation:?}: one row of the list and no hint",
            );
            assert_eq!(
                list_rows(&snap, pane_of(3)),
                0,
                "{operation:?}: no row under the separator",
            );
        }
    }

    /// The worktrees of the example in the issue, sorted by path: the main
    /// worktree, two worktrees on branches, and a detached worktree.
    fn issue_worktrees() -> Vec<WorktreeEntry> {
        [
            ("/code/tools", "main"),
            ("/code/tools-worktrees/issue-475", "issue-475"),
            ("/code/tools-worktrees/issue-498", "issue-498"),
            ("/code/tools-worktrees/sweep", "HEAD@9ba6951"),
        ]
        .into_iter()
        .map(|(path, label)| WorktreeEntry {
            path: WorktreePath::fake(path),
            label: label.to_string(),
        })
        .collect()
    }

    /// Open the list of `entries` with the cursor on row `cursor` and the
    /// home worktree on row `home`.
    fn list_at(entries: &[WorktreeEntry], cursor: usize, home: usize) -> WorktreeList {
        WorktreeList::open(
            entries.to_vec(),
            &entries[cursor].path,
            entries[home].path.clone(),
        )
        .expect("a list with rows opens")
    }

    /// The snapshot of the example in the issue: the home worktree, the first
    /// of four, on `main`.
    fn issue_snapshot() -> Snapshot {
        let mut snap = snapshot_with(None, &[]);
        snap.branch = "main".into();
        snap.commits_ahead = 0;
        snap.worktree = Some(WorktreeBadge {
            position: 1,
            count: 4,
            home: true,
            label: "main".into(),
        });
        snap
    }

    /// The timing of the example in the issue: the last walk was 12 seconds
    /// ago, and the next walk is 48 seconds away.
    fn issue_timing() -> FrameTiming {
        FrameTiming {
            age_offset: Duration::from_secs(12),
            next_refresh_in: Some(Duration::from_secs(48)),
        }
    }

    /// A render config for the status frame of these tests. The list frame
    /// takes none.
    fn render_config() -> RenderConfig {
        RenderConfig {
            base: None,
            max_files: None,
            bar_width: 6,
            log_lines: 0,
            truecolor: false,
            refresh_interval: None,
            width_offset: 0,
        }
    }

    /// The rows of the list frame, as visible glyphs.
    ///
    /// The frame is painted with the escape codes forced on, and then the
    /// codes are taken out, so the comparison covers the painted rows. The
    /// split is on each line break, so a blank row at the bottom counts.
    fn list_frame_rows(
        snapshot: &Snapshot,
        dims: watch::Dimensions,
        list: &WorktreeList,
    ) -> Vec<String> {
        let painted = testcolor::with_forced_ansi(|| {
            render_list_frame(snapshot, dims, issue_timing(), list).output
        });
        testcolor::strip_ansi(&painted)
            .split('\n')
            .map(str::to_string)
            .collect()
    }

    /// The hint under the list, as the issue states it. Stated here as the
    /// oracle, apart from the constant of the code under test.
    const ISSUE_HINT: &str = "↑↓ move · Enter go · Esc back";

    #[test]
    fn the_list_frame_draws_the_example_of_the_issue() {
        // The user started gsw in /code/tools, pressed Down to open the
        // list, and pressed Down again. The widest visible path is 31
        // columns, so every label starts in the same column. The list leaves
        // one row blank, and the hint takes the bottom row of the pane.
        let entries = issue_worktrees();
        let mut list = list_at(&entries, 0, 0);
        list.down();
        let dims = watch::Dimensions {
            width: 60,
            height: 8,
        };

        assert_eq!(
            list_frame_rows(&issue_snapshot(), dims, &list),
            [
                "gsw ⌂ 1/4 • main • 0 commits ahead of main".to_string(),
                format!(
                    "{} last refresh: 12s ago, next refresh: 48s {}",
                    "─".repeat(13),
                    "─".repeat(5),
                ),
                format!("    {:<31}  [main]  ⌂", "/code/tools"),
                format!("  > {:<31}  [issue-475]", "/code/tools-worktrees/issue-475"),
                format!("    {:<31}  [issue-498]", "/code/tools-worktrees/issue-498"),
                format!("    {:<31}  [HEAD@9ba6951]", "/code/tools-worktrees/sweep"),
                String::new(),
                ISSUE_HINT.to_string(),
            ],
        );
    }

    #[test]
    fn the_list_frame_draws_the_head_of_the_status_frame_byte_for_byte() {
        // The header, the line of a merge or a rebase, and the separator with
        // its refresh clock come from one function, so the list frame paints
        // them exactly as the status frame paints them. The frame is as tall
        // as the pane with or without the line of an operation, and nothing on
        // it ages.
        let entries = issue_worktrees();
        let list = list_at(&entries, 1, 0);
        let dims = watch::Dimensions {
            width: 60,
            height: 12,
        };
        for (operation, head) in [(None, 2), (Some(Operation::Merge { conflicts: 2 }), 3)] {
            let mut snap = issue_snapshot();
            snap.operation = operation;
            let (list_frame, status_frame) = testcolor::with_forced_ansi(|| {
                (
                    render_list_frame(&snap, dims, issue_timing(), &list),
                    render_frame(&snap, &render_config(), dims, issue_timing()),
                )
            });
            let list_painted: Vec<&str> = list_frame.output.split('\n').collect();
            let status_painted: Vec<&str> = status_frame.output.split('\n').collect();

            assert_eq!(
                list_painted.get(..head),
                status_painted.get(..head),
                "the head of the list frame with the operation {:?}",
                snap.operation,
            );
            assert!(
                testcolor::strip_ansi(status_painted[head - 1])
                    .contains("last refresh: 12s ago, next refresh: 48s"),
                "the separator of both frames carries the refresh clock: {:?}",
                status_painted[head - 1],
            );
            assert_eq!(
                list_painted.len(),
                dims.height,
                "the list frame is as tall as the pane with the operation {:?}",
                snap.operation,
            );
            assert_eq!(list_frame.freshest_age, None, "nothing on the list ages");
        }
    }

    #[test]
    fn the_list_frame_marks_the_cursor_row_and_the_home_row_only() {
        let entries = issue_worktrees();
        let dims = watch::Dimensions {
            width: 60,
            height: 8,
        };
        let rows_with = |marker: &str, rows: &[String]| -> Vec<usize> {
            rows.iter()
                .enumerate()
                .filter(|(_, row)| {
                    if marker == "⌂" {
                        row.ends_with("  ⌂")
                    } else {
                        row.starts_with(marker)
                    }
                })
                .map(|(index, _)| index)
                .collect()
        };

        // The cursor on the home row: the one row carries both marks.
        let rows = list_frame_rows(&issue_snapshot(), dims, &list_at(&entries, 0, 0));
        assert_eq!(
            rows.get(2),
            Some(&format!("  > {:<31}  [main]  ⌂", "/code/tools")),
            "{rows:#?}",
        );
        assert_eq!(rows_with("  > ", &rows), [2]);
        assert_eq!(rows_with("⌂", &rows), [2]);

        // The cursor and the home worktree on two other rows.
        let rows = list_frame_rows(&issue_snapshot(), dims, &list_at(&entries, 3, 2));
        assert_eq!(rows_with("  > ", &rows), [5], "{rows:#?}");
        assert_eq!(rows_with("⌂", &rows), [4], "{rows:#?}");

        // A home worktree that is not in the list marks no row.
        let away = WorktreeList::open(
            entries.clone(),
            &entries[1].path,
            WorktreePath::fake("/elsewhere"),
        )
        .expect("a list with rows opens");
        let rows = list_frame_rows(&issue_snapshot(), dims, &away);
        assert_eq!(rows_with("⌂", &rows), Vec::<usize>::new(), "{rows:#?}");
        assert_eq!(rows_with("  > ", &rows), [3], "{rows:#?}");
    }

    #[test]
    fn the_list_frame_paints_the_cursor_row_bold_and_the_hint_dim() {
        const BOLD: &str = "\x1b[1m";
        const DIM: &str = "\x1b[2m";
        let entries = issue_worktrees();
        let list = list_at(&entries, 1, 0);
        let dims = watch::Dimensions {
            width: 60,
            height: 8,
        };
        let painted = testcolor::with_forced_ansi(|| {
            render_list_frame(&issue_snapshot(), dims, issue_timing(), &list).output
        });
        let rows: Vec<&str> = painted.split('\n').collect();

        assert!(
            rows.get(3).is_some_and(|row| row.starts_with(BOLD)),
            "the cursor row is bold: {rows:#?}",
        );
        for row in [2, 4, 5] {
            assert!(
                rows.get(row).is_some_and(|row| !row.contains('\x1b')),
                "row {row} has no cursor and no paint: {rows:#?}",
            );
        }
        assert!(
            rows.get(7).is_some_and(|row| row.starts_with(DIM)),
            "the hint is dim: {rows:#?}",
        );
    }

    #[test]
    fn the_list_frame_shows_the_window_of_a_long_list_that_holds_the_cursor() {
        // Ten worktrees in a pane with three rows for the list. The window
        // holds the cursor row, and it moves only when the cursor leaves it.
        let entries: Vec<WorktreeEntry> = (0..10)
            .map(|n| WorktreeEntry {
                path: WorktreePath::fake(format!("/code/wt-{n}")),
                label: format!("b{n}"),
            })
            .collect();
        let snap = issue_snapshot();
        let dims = watch::Dimensions {
            width: 40,
            height: 6,
        };
        let mut list = list_at(&entries, 7, 0);

        let rows = list_frame_rows(&snap, dims, &list);
        assert_eq!(
            rows.get(2..).unwrap_or_default(),
            [
                "    /code/wt-5  [b5]",
                "    /code/wt-6  [b6]",
                "  > /code/wt-7  [b7]",
                ISSUE_HINT,
            ],
            "the window that holds the cursor on row 7: {rows:#?}",
        );

        list.settle(list_rows(&snap, dims));
        for _ in 0..3 {
            list.up();
        }
        let rows = list_frame_rows(&snap, dims, &list);
        assert_eq!(
            rows.get(2..).unwrap_or_default(),
            [
                "  > /code/wt-4  [b4]",
                "    /code/wt-5  [b5]",
                "    /code/wt-6  [b6]",
                ISSUE_HINT,
            ],
            "the window moves up only as far as the cursor went: {rows:#?}",
        );
    }

    #[test]
    fn the_list_frame_with_one_row_under_the_separator_shows_the_cursor_row_alone() {
        let entries = issue_worktrees();
        let list = list_at(&entries, 2, 0);
        let dims = watch::Dimensions {
            width: 60,
            height: 3,
        };

        let rows = list_frame_rows(&issue_snapshot(), dims, &list);
        assert_eq!(rows.len(), 3, "{rows:#?}");
        assert_eq!(
            rows[2],
            format!("  > {:<31}  [issue-498]", "/code/tools-worktrees/issue-498"),
        );
    }

    #[test]
    fn the_list_frame_of_a_pane_with_no_row_under_the_separator_draws_the_head_alone() {
        // The loop closes the list before it asks for such a frame. A frame
        // that is asked for all the same shows no row of the list and no
        // hint.
        let entries = issue_worktrees();
        let list = list_at(&entries, 1, 0);
        let dims = watch::Dimensions {
            width: 60,
            height: 2,
        };

        let rows = list_frame_rows(&issue_snapshot(), dims, &list);
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert_eq!(rows[0], "gsw ⌂ 1/4 • main • 0 commits ahead of main");
        assert!(rows[1].contains("last refresh:"), "{rows:#?}");
    }

    /// The display width of `text`: the columns that a terminal gives it.
    fn columns(text: &str) -> usize {
        unicode_width::UnicodeWidthStr::width(text)
    }

    /// A directory name and a branch name with multi-byte characters: three
    /// bytes and two columns, four bytes and two columns, and an accent.
    const MULTIBYTE: &str = "日本語-🎉-café";

    /// The worktrees of the issue, and one more whose path and label hold
    /// [`MULTIBYTE`]. Its path sorts last, because every byte of `日` is
    /// higher than the first byte of each other name.
    fn multibyte_worktrees() -> Vec<WorktreeEntry> {
        let mut entries = issue_worktrees();
        entries.push(WorktreeEntry {
            path: WorktreePath::fake(format!("/code/tools-worktrees/{MULTIBYTE}")),
            label: MULTIBYTE.to_string(),
        });
        assert!(
            entries.is_sorted_by(|left, right| left.path < right.path),
            "the list takes worktrees sorted by path",
        );
        entries
    }

    #[test]
    fn every_row_of_the_list_frame_fits_a_narrow_pane_and_the_labels_stay_aligned() {
        // A row never wraps. The path column loses columns first, and for all
        // rows alike, so the labels stay in one column while they fit. The
        // widest label, `  [日本語-🎉-café]`, takes 18 columns and the marker
        // takes 4, so from 22 columns on every label is whole. From 27
        // columns on, the path column holds `…café`. A multi-byte path loses
        // whole characters, by columns, and never panics. A wide character
        // that does not fit stays out whole, and a blank pads the column: at
        // 29 columns the cell is `…-café ` and not half of `🎉`.
        let entries = multibyte_worktrees();
        let list = list_at(&entries, 4, 0);
        let snap = issue_snapshot();
        for width in 0..=80 {
            let dims = watch::Dimensions { width, height: 10 };
            let rows = list_frame_rows(&snap, dims, &list);
            for row in &rows {
                assert!(
                    columns(row) <= width,
                    "a row of {} columns in a pane of {width}: {row:?}",
                    columns(row),
                );
            }

            let shown = rows.get(2..7).unwrap_or_default();
            if width >= 22 {
                let label_columns: Vec<usize> = shown
                    .iter()
                    .map(|row| row.split_once('[').map_or(0, |(before, _)| columns(before)))
                    .collect();
                assert!(
                    label_columns.windows(2).all(|pair| pair[0] == pair[1]),
                    "the labels line up at {width} columns: {shown:#?}",
                );
            }
            if width >= 27 {
                let label = format!("  [{MULTIBYTE}]");
                assert!(
                    shown.get(4).is_some_and(|row| row
                        .strip_suffix(&label)
                        .is_some_and(|cell| cell.trim_end().ends_with("café"))),
                    "the multi-byte row keeps the end of its path at {width} columns: {shown:#?}",
                );
            }
        }
    }

    #[test]
    fn a_row_too_wide_for_the_pane_loses_columns_from_the_left_of_the_path() {
        // The widest end of a row, `  [HEAD@9ba6951]`, takes 16 columns and
        // the marker takes 4, so a pane of 40 columns leaves 20 for the path
        // column. Each path keeps its end, which names the worktree, and the
        // labels stay whole and in one column.
        let entries = issue_worktrees();
        let list = list_at(&entries, 1, 0);
        let dims = watch::Dimensions {
            width: 40,
            height: 8,
        };

        let rows = list_frame_rows(&issue_snapshot(), dims, &list);
        assert_eq!(
            rows.get(2..6).unwrap_or_default(),
            [
                format!("    {:<20}  [main]  ⌂", "/code/tools"),
                "  > …worktrees/issue-475  [issue-475]".to_string(),
                "    …worktrees/issue-498  [issue-498]".to_string(),
                "    …ols-worktrees/sweep  [HEAD@9ba6951]".to_string(),
            ],
        );
    }

    #[test]
    fn a_pane_too_narrow_for_the_labels_cuts_each_row_and_the_hint_from_the_right() {
        // At 16 columns the marker and the widest label leave no column for
        // the path. A row that still does not fit loses columns from the
        // right, as the hint does, so no row wraps.
        let entries = issue_worktrees();
        let list = list_at(&entries, 1, 0);
        let dims = watch::Dimensions {
            width: 16,
            height: 8,
        };

        let rows = list_frame_rows(&issue_snapshot(), dims, &list);
        assert_eq!(
            rows.get(2..).unwrap_or_default(),
            [
                "      [main]  ⌂",
                "  >   [issue-47…",
                "      [issue-49…",
                "      [HEAD@9ba…",
                "",
                "↑↓ move · Enter…",
            ],
        );
    }

    /// Build a minimal [`Snapshot`] with the given HEAD-commit age and a file
    /// row per supplied mtime age, so the freshest-age tests can exercise the
    /// commit-vs-change comparison without walking a real repo.
    ///
    /// `head_commit_age` becomes the single log row, which is where the frame
    /// shows the newest commit's age. `None` means a repository with no commits
    /// on screen at all.
    fn snapshot_with(
        head_commit_age: Option<Duration>,
        file_ages: &[Option<Duration>],
    ) -> Snapshot {
        let log = head_commit_age
            .map(|age| {
                vec![LogEntry {
                    hash: "abc1234".into(),
                    subject: "the newest commit".into(),
                    age: Some(age),
                }]
            })
            .unwrap_or_default();
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
            commits_behind: 0,
            files,
            log,
            upstream: None,
            operation: None,
            push_remote: None,
            worktree: None,
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
    fn freshest_age_reads_the_commit_age_off_the_newest_log_row() {
        // The freshest age sets how often watch mode repaints, so it has to
        // track what the frame actually draws. The newest commit's age is drawn
        // on the first log row and nowhere else, so that row is where the
        // cadence comes from.
        let mut snap = snapshot_with(None, &[]);
        snap.log = vec![
            LogEntry {
                hash: "abc1234".into(),
                subject: "the newest commit".into(),
                age: Some(Duration::from_secs(10)),
            },
            LogEntry {
                hash: "def5678".into(),
                subject: "an older commit".into(),
                age: Some(Duration::from_secs(900)),
            },
        ];
        assert_eq!(
            snapshot_freshest_age(&snap),
            Some(Duration::from_secs(10)),
            "the newest commit row (10s) sets the cadence",
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
    fn render_frame_applies_age_offset_to_output_and_freshest_age() {
        // render_frame must advance BOTH the painted ages and the returned
        // freshest_age by `age_offset`, so watch mode's decay-timer cadence and
        // the frame on screen stay in lockstep between git rescans. The commit
        // (10s) is the freshest item; a file at 40s stays older under any
        // uniform offset, so the freshest age tracks the commit before and
        // after the shift.
        let snap = Snapshot {
            branch: "b".into(),
            base: "main".into(),
            commits_ahead: 0,
            commits_behind: 0,
            files: vec![RenderEntry {
                path: "f.rs".into(),
                orig_path: None,
                status: FileStatus::Modified,
                staged: false,
                adds: 1,
                dels: 0,
                binary: false,
                age: Some(Duration::from_secs(40)),
            }],
            log: vec![LogEntry {
                hash: "abc1234".into(),
                subject: "the newest commit".into(),
                age: Some(Duration::from_secs(10)),
            }],
            upstream: None,
            operation: None,
            push_remote: None,
            worktree: None,
        };
        let cfg = RenderConfig {
            base: None,
            max_files: None,
            bar_width: 6,
            log_lines: 1,
            truecolor: false,
            refresh_interval: None,
            width_offset: 0,
        };
        let dims = watch::Dimensions {
            width: 80,
            height: 40,
        };

        // No offset: the commit's row shows the un-advanced commit age and the
        // freshest age is the raw commit age.
        let frame = render_frame(&snap, &cfg, dims, FrameTiming::at_walk(None));
        assert_eq!(
            frame.freshest_age,
            Some(Duration::from_secs(10)),
            "with no offset the freshest age is the raw 10s commit age",
        );
        assert!(
            frame.output.contains("10s"),
            "the commit row should show the un-advanced commit age: {:?}",
            frame.output,
        );

        // A 50s offset advances the commit age to 60s ("1m0s") on its row AND
        // the returned freshest_age to 60s, in lockstep.
        let frame = render_frame(
            &snap,
            &cfg,
            dims,
            FrameTiming {
                age_offset: Duration::from_secs(50),
                next_refresh_in: None,
            },
        );
        assert_eq!(
            frame.freshest_age,
            Some(Duration::from_secs(60)),
            "freshest_age must advance by the offset (10s + 50s = 60s)",
        );
        assert!(
            frame.output.contains("1m0s"),
            "header should show the advanced commit age (1m0s): {:?}",
            frame.output,
        );
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

    #[test]
    fn truecolor_flag_help_names_every_fade_the_flag_controls() {
        // The two flags gate three fades, not one: the commit-log gradient,
        // the recency fade on the file rows, and the fade on the push status
        // message. Help text that names only the gradient sends the user who
        // wants the other two off hunting for a flag that does not exist.
        use clap::CommandFactory;

        const EFFECTS: [&str; 3] = ["commit-log gradient", "file rows", "push status message"];

        let cmd = Cli::command();
        for id in ["truecolor", "no_truecolor"] {
            let arg = cmd
                .get_arguments()
                .find(|a| a.get_id() == id)
                .unwrap_or_else(|| panic!("clap must expose an argument with the id `{id}`"));
            let raw = arg
                .get_long_help()
                .or_else(|| arg.get_help())
                .unwrap_or_else(|| panic!("the argument `{id}` must carry help text"))
                .to_string();
            // Collapse the source's line wrapping so a phrase split across two
            // doc-comment lines still matches.
            let help = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            for effect in EFFECTS {
                assert!(
                    help.contains(effect),
                    "the help for `{id}` must name the `{effect}` fade the flag controls, \
                     but it reads: {help}",
                );
            }
        }
    }
}

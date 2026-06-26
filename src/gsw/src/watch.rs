//! Watch mode: event loop, terminal lifecycle, and the pure helpers that drive
//! refresh decisions.
//!
//! The watch loop owns all rendering on a single thread and is fed by a
//! `std::sync::mpsc` channel. Every *decision* — which terminal dimensions to
//! render for, which filesystem events matter, and how fast the decay timer
//! should tick — lives in a pure, terminal-free function here
//! ([`resolve_dimensions`], [`should_react`], [`next_tick`]) so it can be
//! unit-tested without a pty.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event as CtEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::render::Snapshot;
use crate::{
    collect_snapshot, effective_terminal_height, effective_terminal_width, render_frame, Render,
    RenderConfig, DEFAULT_TERMINAL_HEIGHT, DEFAULT_TERMINAL_WIDTH,
};

/// Which rendering mode `gsw` is running in. The mode — not ambient env
/// detection — decides where terminal dimensions come from (see
/// [`resolve_dimensions`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    /// Single render and exit. Honors the viddy-aware `COLUMNS`/`LINES` env
    /// logic so `gsw | …` and `viddy gsw` keep working unchanged.
    OneShot,
    /// Long-lived watch loop that owns the whole pane.
    Watch,
}

/// Resolved terminal dimensions to render within.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Dimensions {
    pub width: usize,
    pub height: usize,
}

/// Every raw signal available for resolving terminal dimensions, regardless of
/// mode. The resolver picks which of these to trust based on the mode.
#[derive(Clone, Copy)]
pub(crate) struct SizeInputs {
    /// Width queried from `terminal_size` (the ioctl), when a TTY is present.
    pub tty_width: Option<usize>,
    /// Height queried from `terminal_size` (the ioctl), when a TTY is present.
    pub tty_height: Option<usize>,
    /// `COLUMNS` env var, exported by watch-like wrappers (viddy).
    pub columns_env: Option<usize>,
    /// `LINES` env var, exported by watch-like wrappers (viddy).
    pub lines_env: Option<usize>,
    /// Whether stdout is a direct TTY.
    pub stdout_is_tty: bool,
    /// User-requested columns to subtract from the detected width.
    pub width_offset: usize,
}

/// Resolve the terminal dimensions `gsw` should render for, keyed off the mode.
///
/// - [`Mode::OneShot`] preserves the existing viddy-aware behavior: width and
///   height come from the `COLUMNS`/`LINES` env vars when stdout is captured by
///   a wrapper, reserving rows for the wrapper's chrome. This keeps `gsw | …`
///   and `viddy gsw` byte-identical to before.
/// - [`Mode::Watch`] owns the entire pane, so it takes width and height
///   straight from `terminal_size`, ignores `COLUMNS`/`LINES`, and reserves
///   **no** wrapper chrome rows. The one-cell width safety margin (DECAWM) and
///   the user's `width_offset` still apply.
pub(crate) fn resolve_dimensions(mode: Mode, inputs: &SizeInputs) -> Dimensions {
    match mode {
        Mode::OneShot => Dimensions {
            width: effective_terminal_width(
                inputs.tty_width,
                inputs.columns_env,
                inputs.stdout_is_tty,
                inputs.width_offset,
            ),
            height: effective_terminal_height(
                inputs.tty_height,
                inputs.lines_env,
                inputs.stdout_is_tty,
            ),
        },
        Mode::Watch => Dimensions {
            // Watch owns the whole pane: ignore COLUMNS/LINES, take the size
            // from terminal_size, and reserve no wrapper chrome. The one-cell
            // DECAWM safety margin and the user's width_offset still apply to
            // width, matching the one-shot path's right-edge behavior.
            width: inputs
                .tty_width
                .unwrap_or(DEFAULT_TERMINAL_WIDTH)
                .saturating_sub(1)
                .saturating_sub(inputs.width_offset)
                .max(1),
            height: inputs.tty_height.unwrap_or(DEFAULT_TERMINAL_HEIGHT).max(1),
        },
    }
}

/// Whether a filesystem event at `path` should wake the render loop.
///
/// `gsw` watches the worktree root *and* the git directory recursively — a
/// linked worktree splits the two (its `.git` is a file pointing at
/// `<common>/.git/worktrees/<name>`, outside the worktree), so events for
/// commits arrive from a path the worktree subtree never covers. Both sources
/// feed this classifier:
///
/// - **Git-dir paths are accepted wholesale.** Anything under `git_dirs` (just
///   `<workdir>/.git` for a normal repo; the worktree git dir *and* the shared
///   common dir for a linked worktree) reflects a ref / HEAD / index / commit
///   change that can move the rendered view. The noisy object/pack/log churn
///   riding along is absorbed downstream by the debounce window and
///   byte-identical suppression, never by a curated allowlist here — so the
///   watch filter and `gix status` agree by construction.
/// - **Ignored worktree paths are dropped.** A change under a path matched by
///   the repo's ignore set (`target/`, `node_modules/`, …) can never alter what
///   `gix status` renders, so reacting would only burn a status walk.
/// - **Every other worktree path is accepted** (tracked, or untracked but not
///   ignored).
/// - A path under neither the worktree nor a git dir is accepted defensively;
///   suppression makes a spurious wake-up free.
///
/// `workdir` roots the ignore matcher. [`Gitignore::matched_path_or_any_parents`]
/// panics on a path outside its root, so the matcher is only consulted for
/// paths confirmed to be under `workdir` (git-dir paths, which may live outside
/// the worktree, are classified before it is ever called).
pub(crate) fn should_react(
    path: &Path,
    ignore: &Gitignore,
    workdir: &Path,
    git_dirs: &[PathBuf],
) -> bool {
    // Git-dir paths win first: they may live outside the worktree (linked
    // worktree) and so must never reach the worktree-rooted ignore matcher,
    // which would panic on an out-of-root path.
    if git_dirs.iter().any(|git_dir| path.starts_with(git_dir)) {
        return true;
    }

    if path.starts_with(workdir) {
        // `matched_path_or_any_parents` walks up to the root, so a write deep
        // inside an ignored directory (`target/debug/app`) is matched by the
        // `target/` rule on the parent. Drop the event only when the ignore
        // set actually claims the path.
        return !ignore
            .matched_path_or_any_parents(path, path.is_dir())
            .is_ignore();
    }

    // Outside both the worktree and every git dir: unexpected, but cheap to
    // honor — a redundant wake-up is swallowed by suppression.
    true
}

/// Whether freshly-computed output warrants a repaint, i.e. it differs from
/// what is already on screen.
///
/// Byte-identical output is suppressed. This is what makes watching all of
/// `.git/` (and reacting to any accepted event) cheap: object/pack/log churn
/// that doesn't change the visible state costs at most one status walk — never
/// a repaint, never a flicker.
fn should_repaint(new: &str, displayed: &str) -> bool {
    new != displayed
}

/// Adaptive decay-timer cadence as a pure function of the freshest displayed
/// item's age (newest commit or working-tree change). Returns how long to wait
/// before the next time-driven re-render, or `None` when the timer should be
/// disabled entirely (the freshest item is old enough that nothing visible
/// changes with the passage of time).
///
/// The cadence mirrors the [`crate::age`] fade model — a linear ramp from age 0
/// to [`FADE_DARKEST_AT`] (2 h), then frozen at the floor — so the timer stops
/// ticking exactly when the fade stops moving:
///
/// | Freshest item age | Tick interval | Why |
/// | --- | --- | --- |
/// | `< 1 min` | 1 s | live seconds in the age text; fade moving fast |
/// | `1 min – 2 h` | 60 s | minute text ticks over; fade moves ~1 RGB unit/min |
/// | `≥ 2 h` | `None` | fade frozen at the floor — FS events only, idle ≈ 0 |
///
/// [`FADE_DARKEST_AT`]: crate::age::FADE_DARKEST_AT
pub(crate) fn next_tick(freshest_age: Duration) -> Option<Duration> {
    if freshest_age < Duration::from_secs(60) {
        Some(Duration::from_secs(1))
    } else if freshest_age < crate::age::FADE_DARKEST_AT {
        Some(Duration::from_secs(60))
    } else {
        None
    }
}

/// How long the loop keeps draining the channel after the first event before
/// it renders — the debounce / coalescing window. A burst of writes (a `git
/// commit` touching many `.git/` files, an editor's save-and-rename dance)
/// arrives inside this window and collapses into a single repaint.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Events the watch loop reacts to. The main thread owns all rendering and
/// blocks on a single channel carrying these.
///
/// There is deliberately no `Tick` variant: the decay timer is driven by the
/// loop's own `recv_timeout` window — a timeout *is* a tick — so the cadence is
/// recomputed after every render with no extra thread to reconfigure (see
/// [`event_loop`] and [`next_tick`]).
enum Event {
    /// A non-ignored filesystem path under the worktree or git dir changed.
    /// The path was already classified by [`should_react`] before the event
    /// was sent, so the loop only needs to know that *something* relevant
    /// moved — it recomputes the whole render regardless of which path it was.
    FsChanged,
    /// The terminal was resized — repaint at the new dimensions.
    Resize,
    /// The user asked to quit (`q` or Ctrl-C).
    Quit,
}

/// Run the live watch loop: take over the alternate screen, seed the snapshot
/// cache with one git walk, paint the first frame, then re-render on filesystem
/// changes, terminal resizes, and decay-timer ticks until the user quits with
/// `q` or Ctrl-C.
///
/// Filesystem changes walk git and re-seed the cache; decay ticks and resizes
/// re-render the cached snapshot with no git work (Part A). The [`TerminalGuard`]
/// restores the main screen and cursor on every exit path.
pub(crate) fn run(repo: &gix::Repository, cfg: &RenderConfig) -> Result<()> {
    let _guard = TerminalGuard::enter()?;

    // Seed the cache with one git walk and paint the first frame at offset 0,
    // byte-identical to a one-shot render of the same state. That frame's
    // freshest age seeds the decay-timer cadence.
    let dims = current_dimensions(cfg.width_offset);
    let collected_at = Instant::now();
    let snapshot = collect_snapshot(repo, cfg)?;
    let first = render_frame(&snapshot, cfg, dims, Duration::ZERO);
    paint_output(&first.output)?;
    let mut displayed = first.output;
    let initial_freshest = first.freshest_age;

    let cache = SnapshotCache {
        snapshot,
        collected_at,
        dims,
    };

    let (tx, rx) = mpsc::channel();
    spawn_event_reader(tx.clone());

    // The filesystem watcher must outlive the loop — dropping it stops watching.
    let _watcher = spawn_fs_watcher(repo, tx)?;

    event_loop(
        &rx,
        DEBOUNCE,
        &mut displayed,
        cache,
        initial_freshest,
        LoopHooks {
            collect: || collect_snapshot(repo, cfg),
            render: |snap: &Snapshot, dims: Dimensions, offset: Duration| {
                render_frame(snap, cfg, dims, offset)
            },
            dimensions: || current_dimensions(cfg.width_offset),
            paint: |output: &str| paint_output(output),
            clock: Instant::now,
            next_tick: |freshest: Option<Duration>| freshest.and_then(next_tick),
        },
    )
}

/// Start the recursive filesystem watcher that feeds [`Event::FsChanged`] into
/// the loop. Returns the live watcher, which the caller must keep in scope: a
/// dropped watcher stops delivering events.
///
/// The watcher covers the worktree root and — for a linked worktree, whose
/// `.git` lives outside the worktree — the git dir and shared common dir too,
/// so commits (which write only under those) still register. Every event path
/// is run through [`should_react`] *before* a wake-up is sent, so ignored
/// build churn (`target/`, `node_modules/`) never even reaches the channel.
fn spawn_fs_watcher(
    repo: &gix::Repository,
    tx: Sender<Event>,
) -> Result<Option<RecommendedWatcher>> {
    let Some(workdir) = repo.workdir().map(Path::to_path_buf) else {
        return Ok(None);
    };

    // `git_dir()` is the per-worktree dir; `common_dir()` is the shared store
    // (they're equal for a normal repo). Both carry state we render.
    let mut git_dirs = vec![repo.git_dir().to_path_buf()];
    let common = repo.common_dir().to_path_buf();
    if !git_dirs.contains(&common) {
        git_dirs.push(common);
    }

    let ignore = build_ignore_matcher(repo, &workdir);

    let filter_workdir = workdir.clone();
    let filter_git_dirs = git_dirs.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else {
            return;
        };
        // One wake-up per relevant event; the loop coalesces bursts anyway, so
        // there's no value in sending once per path. A send error means the
        // receiver is gone (loop ended) — nothing left to do.
        let relevant = event
            .paths
            .iter()
            .any(|path| should_react(path, &ignore, &filter_workdir, &filter_git_dirs));
        if relevant {
            let _ = tx.send(Event::FsChanged);
        }
    })?;

    // Watch the worktree, plus any git dir that isn't already inside it (a
    // normal repo's `.git` is covered by the recursive worktree watch; a linked
    // worktree's dirs are not). A failed watch on one root is non-fatal — the
    // others still drive refreshes.
    let _ = watcher.watch(&workdir, RecursiveMode::Recursive);
    for git_dir in &git_dirs {
        if !git_dir.starts_with(&workdir) {
            let _ = watcher.watch(git_dir, RecursiveMode::Recursive);
        }
    }

    Ok(Some(watcher))
}

/// Build the ignore matcher the watcher uses to drop build/dependency churn,
/// assembled from the repo's ignore sources the way `gix status` honors them:
/// the worktree-root `.gitignore`, `$GIT_COMMON_DIR/info/exclude`, and the
/// user's global excludes (`core.excludesFile`, else `~/.config/git/ignore`).
///
/// Nested `.gitignore` files deeper in the tree are deliberately *not*
/// enumerated here: anything they would newly ignore still triggers at most one
/// *suppressed* status walk, so the byte-identical-output backstop keeps the
/// rendered view correct, while the high-volume top-level churn this is meant
/// to filter (`target/`, `node_modules/`) is matched up front.
fn build_ignore_matcher(repo: &gix::Repository, workdir: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(workdir);
    // `add` returns `Some(err)` when a file is missing or unreadable; a repo
    // without a `.gitignore` is normal, so these are intentionally ignored.
    let _ = builder.add(workdir.join(".gitignore"));
    let _ = builder.add(repo.common_dir().join("info").join("exclude"));
    if let Some(global) = global_excludes_path(repo) {
        let _ = builder.add(global);
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

/// Resolve git's global excludes file: an explicit `core.excludesFile` config
/// value wins, otherwise git's default of `$XDG_CONFIG_HOME/git/ignore`
/// (falling back to `~/.config/git/ignore`). `None` when neither is locatable.
fn global_excludes_path(repo: &gix::Repository) -> Option<PathBuf> {
    if let Some(Ok(path)) = repo.config_snapshot().trusted_path("core.excludesFile") {
        return Some(path.into_owned());
    }
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config_home.join("git").join("ignore"))
}

/// The cached repository [`Snapshot`] plus the metadata Part A needs to
/// re-render it without re-walking git. A decay tick or resize repaints from
/// this cache, advancing every displayed age by `now - collected_at`; only a
/// filesystem change re-collects and re-seeds it.
struct SnapshotCache {
    /// The most recently collected repository state.
    snapshot: Snapshot,
    /// When `snapshot` was collected, against the loop's injected clock. The age
    /// offset for a no-git re-render is `clock() - collected_at`.
    collected_at: Instant,
    /// The dimensions `snapshot` was last rendered at, so a resize can re-render
    /// the cached snapshot at the new size without collecting.
    dims: Dimensions,
}

/// The side-effecting hooks the watch loop drives, bundled so the loop stays one
/// testable function instead of taking a fistful of closures. Production wires
/// these to the real git collect, render, terminal-size query, painter, and
/// clock; tests inject counters and a controllable clock to assert which hooks
/// ran — and with what age offset — without a TTY or real time.
struct LoopHooks<Collect, RenderFn, Dims, Paint, Clock, Tick> {
    /// Walk the repo into a fresh [`Snapshot`] (the expensive git work).
    collect: Collect,
    /// Render a snapshot at the given dimensions, advancing ages by the offset.
    render: RenderFn,
    /// Query the current terminal dimensions (re-evaluated on resize).
    dimensions: Dims,
    /// Paint a finished frame.
    paint: Paint,
    /// Read the current instant (real `Instant::now` in production).
    clock: Clock,
    /// Map the freshest displayed age to the decay-tick interval (`None` = off).
    next_tick: Tick,
}

/// The render loop's terminal-free core: wait for a filesystem event, a resize,
/// or a decay-timer tick, then update the screen. Only a filesystem change walks
/// git; a tick or resize re-renders the *cached* [`Snapshot`] (Part A), so the
/// idle-after-commit decay tick no longer pins a core re-walking an unchanged
/// repo.
///
/// The decay timer needs no thread of its own: `next_tick` (in `hooks`) turns
/// the freshest displayed-item age into the `recv_timeout` window, so a timeout
/// *is* a tick and the cadence is recomputed after every render. `None` disables
/// the timer — the loop blocks indefinitely on events.
///
/// `hooks` bundles the side effects (collect, render, terminal-size query, paint,
/// clock, tick cadence) so the loop is one function testable without a TTY or
/// real time: a test feeds a pre-loaded channel, a controllable clock, and
/// counters, then asserts which hooks ran and with what age offset. The
/// contracts verified there:
///
/// - a burst of filesystem events between renders collapses into **one** collect
///   and at most one paint (coalescing);
/// - a decay tick re-renders from cache with **no** collect, advancing every age
///   by `clock() - collected_at`, and repaints only if the frame changed;
/// - a recompute whose output is byte-identical to what's displayed paints
///   nothing (suppression);
/// - [`Event::Quit`] ends the loop, as does every sender hanging up.
fn event_loop<Collect, RenderFn, Dims, Paint, Clock, Tick>(
    rx: &Receiver<Event>,
    debounce: Duration,
    displayed: &mut String,
    mut cache: SnapshotCache,
    initial_freshest: Option<Duration>,
    mut hooks: LoopHooks<Collect, RenderFn, Dims, Paint, Clock, Tick>,
) -> Result<()>
where
    Collect: FnMut() -> Result<Snapshot>,
    RenderFn: FnMut(&Snapshot, Dimensions, Duration) -> Render,
    Dims: Fn() -> Dimensions,
    Paint: FnMut(&str) -> Result<()>,
    Clock: Fn() -> Instant,
    Tick: Fn(Option<Duration>) -> Option<Duration>,
{
    let mut freshest = initial_freshest;
    loop {
        // Wait for the first event, or — when the decay timer is enabled — wake
        // after `interval` of quiet for a tick.
        let woke_for_tick = match (hooks.next_tick)(freshest) {
            Some(interval) => match rx.recv_timeout(interval) {
                Ok(Event::Quit) => break,
                Ok(_) => false,
                Err(RecvTimeoutError::Timeout) => true,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(Event::Quit) => break,
                Ok(_) => false,
                Err(_) => break,
            },
        };

        // Coalesce a filesystem burst: keep draining until the channel stays
        // quiet for a full `debounce`. A tick has no burst behind it.
        let mut quitting = false;
        if !woke_for_tick {
            loop {
                match rx.recv_timeout(debounce) {
                    Ok(Event::Quit) => {
                        quitting = true;
                        break;
                    }
                    Ok(_) => {}
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => {
                        quitting = true;
                        break;
                    }
                }
            }
        }

        // Only a filesystem change walks git. A decay tick re-renders the cached
        // snapshot with no git work, advancing every displayed age by the
        // wall-clock elapsed since the snapshot was collected (Part A).
        let render = if woke_for_tick {
            let age_offset = (hooks.clock)().saturating_duration_since(cache.collected_at);
            (hooks.render)(&cache.snapshot, cache.dims, age_offset)
        } else {
            cache.dims = (hooks.dimensions)();
            cache.snapshot = (hooks.collect)()?;
            (hooks.render)(&cache.snapshot, cache.dims, Duration::ZERO)
        };

        if should_repaint(&render.output, displayed) {
            (hooks.paint)(&render.output)?;
            *displayed = render.output;
        }
        freshest = render.freshest_age;

        if quitting {
            break;
        }
    }
    Ok(())
}

/// Paint `output` into the alternate screen, replacing whatever frame is there.
fn paint_output(output: &str) -> Result<()> {
    let mut out = io::stdout();
    // In raw mode a bare '\n' moves down without returning to column 0, which
    // would stair-step the output; translate to CRLF. Clear first so a shorter
    // render can't leave stale glyphs from a taller previous frame.
    let painted = output.replace('\n', "\r\n");
    execute!(out, MoveTo(0, 0), Clear(ClearType::All))?;
    write!(out, "{painted}")?;
    out.flush()?;
    Ok(())
}

/// Query the live terminal size and resolve watch-mode dimensions from it.
fn current_dimensions(width_offset: usize) -> Dimensions {
    let tty = terminal_size::terminal_size().map(|(w, h)| (usize::from(w.0), usize::from(h.0)));
    resolve_dimensions(
        Mode::Watch,
        &SizeInputs {
            tty_width: tty.map(|(w, _)| w),
            tty_height: tty.map(|(_, h)| h),
            columns_env: None,
            lines_env: None,
            stdout_is_tty: true,
            width_offset,
        },
    )
}

/// Spawn the crossterm event-reader thread. It blocks on `event::read`,
/// translating `q`/Ctrl-C into [`Event::Quit`] and terminal resizes into
/// [`Event::Resize`], and exits when the receiver is gone or reading fails.
fn spawn_event_reader(tx: Sender<Event>) {
    thread::spawn(move || loop {
        match event::read() {
            Ok(CtEvent::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            })) => {
                // Ignore key-release events (kitty/Windows report them); only
                // a press should quit.
                if kind == KeyEventKind::Release {
                    continue;
                }
                let quit = matches!(code, KeyCode::Char('q'))
                    || (modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(code, KeyCode::Char('c')));
                if quit && tx.send(Event::Quit).is_err() {
                    break;
                }
            }
            Ok(CtEvent::Resize(_, _)) => {
                if tx.send(Event::Resize).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            // The terminal closed or reading failed: nothing more to read.
            Err(_) => break,
        }
    });
}

/// A panic hook, matching what [`std::panic::take_hook`] returns. Held in an
/// [`Arc`] so the installed wrapper and [`TerminalGuard::drop`] can both reach
/// the same pre-watch hook — the wrapper to chain to it, `Drop` to reinstate it.
type PanicHook = Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send>;

/// RAII guard for the alternate screen, hidden cursor, and raw mode. Restores
/// the main screen and cursor on drop *and* via a panic hook, so no exit path
/// — normal return, propagated error, or panic — can leave the terminal in a
/// wedged state. The panic hook restores *before* the default handler prints,
/// so the panic message lands on the main screen rather than the torn-down
/// alternate one. On drop the pre-watch panic hook is reinstated, so our
/// terminal-restoring wrapper never lingers as global process state once the
/// guard is gone.
struct TerminalGuard {
    /// The panic hook in effect before [`TerminalGuard::enter`] wrapped it,
    /// reinstated on drop. `Option` only so `Drop` can move it back out.
    previous_hook: Option<PanicHook>,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;

        let previous: PanicHook = Arc::from(std::panic::take_hook());
        let chained = Arc::clone(&previous);
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            (*chained)(info);
        }));

        Ok(TerminalGuard {
            previous_hook: Some(previous),
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
        // Reinstate the pre-watch panic hook so our terminal-restoring wrapper
        // doesn't outlive the guard as global process state.
        if let Some(previous) = self.previous_hook.take() {
            std::panic::set_hook(Box::new(move |info| (*previous)(info)));
        }
    }
}

/// Best-effort restore of the terminal to its pre-watch state. Idempotent and
/// failure-tolerant: both the panic hook and `Drop` may call it (a panic runs
/// the hook, then unwinding runs `Drop`), and a partially-entered terminal
/// must still be cleaned up, so every step is independently ignored on error.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WRAPPER_CHROME_ROWS;
    use ignore::gitignore::GitignoreBuilder;

    /// Build an ignore matcher rooted at `root` from raw gitignore lines, the
    /// way the production matcher is assembled from the repo's ignore files.
    fn matcher(root: &str, patterns: &[&str]) -> Gitignore {
        let mut builder = GitignoreBuilder::new(root);
        for pattern in patterns {
            builder.add_line(None, pattern).expect("valid glob");
        }
        builder.build().expect("build matcher")
    }

    #[test]
    fn next_tick_boundaries_follow_the_fade_model() {
        use crate::age::FADE_DARKEST_AT;

        // `< 1 min`: tick every second so the live seconds in the age text and
        // the fast early fade both stay current.
        assert_eq!(next_tick(Duration::ZERO), Some(Duration::from_secs(1)));
        assert_eq!(
            next_tick(Duration::from_secs(59)),
            Some(Duration::from_secs(1)),
            "just under a minute is still in the 1 s band",
        );

        // At and past 1 min, drop to the ~60 s cadence: the minute text changes
        // only once a minute and the fade moves ~1 RGB unit/min.
        assert_eq!(
            next_tick(Duration::from_secs(60)),
            Some(Duration::from_secs(60)),
            "exactly one minute crosses into the 60 s band",
        );
        assert_eq!(
            next_tick(Duration::from_secs(60 * 60)),
            Some(Duration::from_secs(60)),
            "an hour old still ticks every 60 s",
        );
        assert_eq!(
            next_tick(FADE_DARKEST_AT - Duration::from_secs(1)),
            Some(Duration::from_secs(60)),
            "just under 2 h is still in the 60 s band",
        );

        // At [`FADE_DARKEST_AT`] (2 h) and beyond the fade is frozen at the
        // floor: nothing visible changes with time, so the timer is disabled.
        assert_eq!(
            next_tick(FADE_DARKEST_AT),
            None,
            "the fade-floor boundary disables the timer",
        );
        assert_eq!(
            next_tick(FADE_DARKEST_AT + Duration::from_secs(1)),
            None,
            "past the floor the timer stays disabled",
        );
        assert_eq!(
            next_tick(Duration::from_secs(60 * 60 * 24 * 30)),
            None,
            "a month-old freshest item produces no ticks",
        );
    }

    #[test]
    fn should_react_accepts_a_tracked_or_untracked_non_ignored_worktree_path() {
        // An edit to a normal source file under the worktree must wake the
        // loop — it's exactly what gsw exists to show.
        let ignore = matcher("/repo", &["target/", "*.log"]);
        let git_dirs = [PathBuf::from("/repo/.git")];
        assert!(should_react(
            Path::new("/repo/src/main.rs"),
            &ignore,
            Path::new("/repo"),
            &git_dirs,
        ));
    }

    #[test]
    fn should_react_drops_an_ignored_worktree_file() {
        // A path matched directly by the ignore set can't change gix status,
        // so reacting would only burn a status walk.
        let ignore = matcher("/repo", &["*.log"]);
        let git_dirs = [PathBuf::from("/repo/.git")];
        assert!(!should_react(
            Path::new("/repo/build.log"),
            &ignore,
            Path::new("/repo"),
            &git_dirs,
        ));
    }

    #[test]
    fn should_react_drops_paths_under_an_ignored_directory() {
        // `target/` ignores the whole subtree: a write to target/debug/app is
        // build churn gsw must not chase (the cargo-build storm this avoids is
        // the whole point of the filter).
        let ignore = matcher("/repo", &["target/"]);
        let git_dirs = [PathBuf::from("/repo/.git")];
        assert!(!should_react(
            Path::new("/repo/target/debug/app"),
            &ignore,
            Path::new("/repo"),
            &git_dirs,
        ));
    }

    #[test]
    fn should_react_accepts_git_head_writes() {
        // `.git/HEAD` moves on checkout/commit — always visible state.
        let ignore = matcher("/repo", &["target/"]);
        let git_dirs = [PathBuf::from("/repo/.git")];
        assert!(should_react(
            Path::new("/repo/.git/HEAD"),
            &ignore,
            Path::new("/repo"),
            &git_dirs,
        ));
    }

    #[test]
    fn should_react_accepts_git_object_writes_for_suppression_to_filter() {
        // `.git/objects/...` churn is accepted at classification time even
        // though it usually changes nothing visible; byte-identical
        // suppression — a separate concern — absorbs it downstream.
        let ignore = matcher("/repo", &["target/"]);
        let git_dirs = [PathBuf::from("/repo/.git")];
        assert!(should_react(
            Path::new("/repo/.git/objects/ab/cdef0123456789"),
            &ignore,
            Path::new("/repo"),
            &git_dirs,
        ));
    }

    #[test]
    fn should_react_accepts_linked_worktree_git_dir_and_common_dir_paths() {
        // gsw runs inside worktrees: a commit there writes under the worktree
        // git dir (HEAD/logs) and the shared common dir (objects/refs), both
        // *outside* the worktree subtree. The ignore matcher must never be
        // consulted for them (it would panic on an out-of-root path), so they
        // are accepted purely by git-dir containment.
        let ignore = matcher("/main/wt", &["target/"]);
        let git_dirs = [
            PathBuf::from("/main/.git/worktrees/wt"),
            PathBuf::from("/main/.git"),
        ];
        assert!(should_react(
            Path::new("/main/.git/worktrees/wt/HEAD"),
            &ignore,
            Path::new("/main/wt"),
            &git_dirs,
        ));
        assert!(should_react(
            Path::new("/main/.git/refs/heads/main"),
            &ignore,
            Path::new("/main/wt"),
            &git_dirs,
        ));
    }

    /// A short debounce keeps the loop tests fast. The events are pre-queued
    /// before the loop runs, so they drain immediately and never actually wait
    /// out the window — only the final disconnect costs nothing — which makes
    /// these tests deterministic regardless of the exact value here.
    const TEST_DEBOUNCE: Duration = Duration::from_millis(20);

    /// A `next_tick` that always disables the timer, so the loop blocks purely
    /// on channel events. The event-driven tests use this to stay independent
    /// of the decay-timer behavior, which has its own dedicated tests.
    fn timer_off(_freshest: Option<Duration>) -> Option<Duration> {
        None
    }

    /// Build a [`Render`] with the given frame and no freshest age — enough for
    /// the event-driven loop tests, which don't exercise the cadence.
    fn frame(output: &str) -> Render {
        Render {
            output: output.to_string(),
            freshest_age: None,
        }
    }

    /// A minimal [`Snapshot`] for loop tests that don't inspect snapshot contents
    /// (the injected render hook returns a canned frame regardless).
    fn empty_snapshot() -> Snapshot {
        Snapshot {
            branch: "b".into(),
            base: "main".into(),
            commits_ahead: 0,
            commits_behind: 0,
            last_commit_age: None,
            files: Vec::new(),
            log: Vec::new(),
            upstream: None,
        }
    }

    /// Dimensions used by loop tests that don't exercise resize.
    const TEST_DIMS: Dimensions = Dimensions {
        width: 80,
        height: 24,
    };

    /// A [`SnapshotCache`] seeded at `collected_at` with an empty snapshot and
    /// [`TEST_DIMS`].
    fn seeded_cache(collected_at: Instant) -> SnapshotCache {
        SnapshotCache {
            snapshot: empty_snapshot(),
            collected_at,
            dims: TEST_DIMS,
        }
    }

    #[test]
    fn event_loop_coalesces_a_burst_into_one_repaint() {
        // A `git commit` is a storm of `.git/` writes; an editor save is a
        // write+rename. Either way the burst must collapse into a single
        // collect and a single repaint, not one per event.
        let (tx, rx) = mpsc::channel();
        for _ in 0..5 {
            tx.send(Event::FsChanged).expect("queue event");
        }
        drop(tx);

        let mut displayed = String::new();
        let mut collects = 0_usize;
        let mut paints = 0_usize;
        let now = Instant::now();
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(now),
            None,
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, _offset: Duration| frame("frame"),
                dimensions: || TEST_DIMS,
                paint: |_output: &str| {
                    paints += 1;
                    Ok(())
                },
                clock: || now,
                next_tick: timer_off,
            },
        )
        .expect("loop");

        assert_eq!(collects, 1, "a coalesced burst must walk status once");
        assert_eq!(paints, 1, "a coalesced burst must repaint once");
        assert_eq!(displayed, "frame");
    }

    #[test]
    fn event_loop_suppresses_when_recompute_is_unchanged() {
        // FS churn that doesn't change the visible state must still collect but
        // produce no repaint.
        let (tx, rx) = mpsc::channel();
        tx.send(Event::FsChanged).expect("queue event");
        drop(tx);

        let mut displayed = "unchanged".to_string();
        let mut collects = 0_usize;
        let mut paints = 0_usize;
        let now = Instant::now();
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(now),
            None,
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, _offset: Duration| frame("unchanged"),
                dimensions: || TEST_DIMS,
                paint: |_output: &str| {
                    paints += 1;
                    Ok(())
                },
                clock: || now,
                next_tick: timer_off,
            },
        )
        .expect("loop");

        assert_eq!(collects, 1, "a wake-up still does the status walk");
        assert_eq!(paints, 0, "byte-identical output must not repaint");
    }

    #[test]
    fn event_loop_quit_as_first_event_exits_without_rendering() {
        // `q` / Ctrl-C before anything else changes must exit cleanly without a
        // stray collect or repaint.
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Quit).expect("queue quit");
        drop(tx);

        let mut displayed = String::new();
        let mut collects = 0_usize;
        let mut paints = 0_usize;
        let now = Instant::now();
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(now),
            None,
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, _offset: Duration| frame("frame"),
                dimensions: || TEST_DIMS,
                paint: |_output: &str| {
                    paints += 1;
                    Ok(())
                },
                clock: || now,
                next_tick: timer_off,
            },
        )
        .expect("loop");

        assert_eq!(collects, 0, "Quit must not trigger a collect");
        assert_eq!(paints, 0, "Quit must not trigger a repaint");
    }

    #[test]
    fn event_loop_tick_triggers_a_render() {
        // With no filesystem events, the decay timer must still wake the loop and
        // re-render so the age text and color fade stay current. The render hook
        // queues a Quit so the loop ends right after the tick-driven render.
        let (tx, rx) = mpsc::channel();
        let mut displayed = String::new();
        let mut renders = 0_usize;
        let mut paints = 0_usize;
        let now = Instant::now();
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(now),
            Some(Duration::ZERO),
            LoopHooks {
                collect: || Ok(empty_snapshot()),
                render: |_snap: &Snapshot, _dims: Dimensions, _offset: Duration| {
                    renders += 1;
                    // End the loop right after this first tick-driven render.
                    let _ = tx.send(Event::Quit);
                    frame(&format!("tick {renders}"))
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| {
                    paints += 1;
                    Ok(())
                },
                clock: || now,
                // Tiny interval so the tick fires fast; the cadence-vs-age
                // mapping is covered by the next_tick tests.
                next_tick: |_freshest| Some(Duration::from_millis(5)),
            },
        )
        .expect("loop");

        assert_eq!(renders, 1, "a decay tick must trigger exactly one render");
        assert_eq!(
            paints, 1,
            "the tick-driven render must repaint the new frame"
        );
    }

    #[test]
    fn event_loop_tick_with_unchanged_render_does_not_repaint() {
        // A decay tick re-renders, but if the frame is byte-identical to what's
        // displayed it must skip the repaint — the same suppression that absorbs
        // no-op filesystem churn.
        let (tx, rx) = mpsc::channel();
        let mut displayed = "steady".to_string();
        let mut renders = 0_usize;
        let mut paints = 0_usize;
        let now = Instant::now();
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(now),
            Some(Duration::from_secs(30)),
            LoopHooks {
                collect: || Ok(empty_snapshot()),
                render: |_snap: &Snapshot, _dims: Dimensions, _offset: Duration| {
                    renders += 1;
                    let _ = tx.send(Event::Quit);
                    frame("steady")
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| {
                    paints += 1;
                    Ok(())
                },
                clock: || now,
                next_tick: |_freshest| Some(Duration::from_millis(5)),
            },
        )
        .expect("loop");

        assert_eq!(renders, 1, "the tick still renders");
        assert_eq!(paints, 0, "an unchanged tick render must not repaint");
    }

    #[test]
    fn event_loop_tick_renders_cached_snapshot_without_collecting() {
        // A decay tick must NOT re-walk git: it re-renders the CACHED snapshot,
        // advancing every displayed age by `now - collected_at` (Part A). With a
        // clock 50s past collection, the render hook must see a 50s offset and
        // the git-collect hook must never run.
        let (tx, rx) = mpsc::channel();
        let mut displayed = String::new();
        let mut collects = 0_usize;
        let mut renders = 0_usize;
        let mut seen_offset: Option<Duration> = None;
        let collected_at = Instant::now();
        let clock_at = collected_at + Duration::from_secs(50);
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(collected_at),
            Some(Duration::ZERO),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, offset: Duration| {
                    renders += 1;
                    seen_offset = Some(offset);
                    let _ = tx.send(Event::Quit);
                    frame(&format!("tick {renders}"))
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| Ok(()),
                clock: || clock_at,
                next_tick: |_freshest| Some(Duration::from_millis(5)),
            },
        )
        .expect("loop");

        assert_eq!(collects, 0, "a decay tick must not walk git");
        assert_eq!(
            seen_offset,
            Some(Duration::from_secs(50)),
            "the no-git re-render must advance ages by now - collected_at",
        );
    }

    #[test]
    fn event_loop_resize_renders_cached_snapshot_at_new_dims_without_collecting() {
        // A terminal resize must re-render the CACHED snapshot at the new
        // dimensions without walking git (Part A): the collect hook never runs
        // and the render hook is handed the freshly-queried dimensions.
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Resize).expect("queue resize");
        drop(tx);

        let new_dims = Dimensions {
            width: 123,
            height: 45,
        };
        let mut displayed = String::new();
        let mut collects = 0_usize;
        let mut seen_dims: Option<Dimensions> = None;
        let now = Instant::now();
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(now),
            None,
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, dims: Dimensions, _offset: Duration| {
                    seen_dims = Some(dims);
                    frame("resized")
                },
                dimensions: || new_dims,
                paint: |_output: &str| Ok(()),
                clock: || now,
                next_tick: timer_off,
            },
        )
        .expect("loop");

        assert_eq!(collects, 0, "a resize must not walk git");
        assert_eq!(
            seen_dims,
            Some(new_dims),
            "a resize must re-render at the freshly-queried dimensions",
        );
    }

    #[test]
    fn should_repaint_suppresses_byte_identical_output() {
        // The suppression backstop: an unchanged snapshot must not trigger a
        // repaint, no matter how many accepted events drove the recompute.
        assert!(
            !should_repaint("branch • 0 commits", "branch • 0 commits"),
            "identical output must be suppressed",
        );
        // A genuine change must still paint.
        assert!(
            should_repaint("branch • 1 commit", "branch • 0 commits"),
            "changed output must repaint",
        );
    }

    #[test]
    fn one_shot_uses_env_dimensions_watch_uses_terminal_size() {
        // Deliberately make terminal_size (200x50) disagree with the env
        // (COLUMNS=120, LINES=40) so the *source* each mode picks is
        // unambiguous from the resulting numbers.
        let inputs = SizeInputs {
            tty_width: Some(200),
            tty_height: Some(50),
            columns_env: Some(120),
            lines_env: Some(40),
            stdout_is_tty: false, // viddy-like capture for the one-shot case
            width_offset: 0,
        };

        // One-shot trusts the env: COLUMNS-1 for width, LINES minus wrapper
        // chrome for height.
        let one_shot = resolve_dimensions(Mode::OneShot, &inputs);
        assert_eq!(one_shot.width, 119, "one-shot width must come from COLUMNS");
        assert_eq!(
            one_shot.height,
            40 - WRAPPER_CHROME_ROWS,
            "one-shot height must come from LINES minus wrapper chrome",
        );

        // Watch ignores the env entirely and takes terminal_size directly,
        // reserving no chrome: 200-1 wide, full 50 tall.
        let watch_inputs = SizeInputs {
            stdout_is_tty: true,
            ..inputs
        };
        let watch = resolve_dimensions(Mode::Watch, &watch_inputs);
        assert_eq!(
            watch.width, 199,
            "watch width must come from terminal_size, not COLUMNS",
        );
        assert_eq!(
            watch.height, 50,
            "watch height must come from terminal_size with no chrome reserved",
        );
    }
}

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
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
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
use crate::repo::RepoHandle;
use crate::{
    collect_snapshot, effective_terminal_height, effective_terminal_width, render_frame,
    FrameTiming, Render, RenderConfig, DEFAULT_TERMINAL_HEIGHT, DEFAULT_TERMINAL_WIDTH,
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

/// The watcher's ignore matcher, shared between the render loop — which
/// rebuilds it from the repository's ignore sources on every git walk — and the
/// watcher callback thread, which reads it on every filesystem event.
///
/// The sharing exists because the matcher must be *live*. Built once at watcher
/// spawn and never rebuilt, it renders whatever the ignore files said at
/// startup, and both directions of a later edit are wrong:
///
/// - **A rule added** (`echo 'build/' >> .gitignore`) never takes effect, so the
///   watcher keeps waking on churn that can no longer change anything — wasteful.
/// - **A rule removed** never takes effect either, and that one is a correctness
///   bug: the callback goes on silently *dropping* events for paths that are now
///   rendered, so the view freezes until gsw is restarted.
///
/// `core.excludesFile` is the same failure one level up — it lives in
/// `.git/config`, which a long-lived [`gix::Repository`] also caches — which is
/// why [`refresh`](Self::refresh) takes a repository rather than closing over
/// the one held at spawn. Handed the freshly re-opened handle from [`walk`], a
/// changed excludes path flows straight through, so the two halves of the
/// staleness fix compose instead of each needing its own special case.
///
/// The [`RwLock`] is what lets the two threads share one matcher: the render
/// loop takes the write side once per walk, while the callback takes the read
/// side once per filesystem event. Events outnumber walks by orders of
/// magnitude, so the read side must not serialize them — hence a reader-writer
/// lock rather than a `Mutex`.
#[derive(Clone)]
pub(crate) struct LiveIgnore(Arc<RwLock<Gitignore>>);

impl LiveIgnore {
    /// Build the matcher from the repository's ignore sources as they are on
    /// disk right now. See [`build_ignore_matcher`] for which sources those are.
    pub(crate) fn new(repo: &gix::Repository) -> Self {
        Self(Arc::new(RwLock::new(build_ignore_matcher(repo))))
    }

    /// Re-read the repository's ignore sources so a rule added or removed since
    /// the last call takes effect on the very next filesystem event, with no
    /// restart.
    ///
    /// Called once per git walk, unconditionally. That is deliberate: rebuilding
    /// reads at most three small files and recompiles a handful of globs, which
    /// is negligible against the status traversal it rides along with — and
    /// watch-mode walks are already gated to a ~1% duty cycle by [`WalkSchedule`], so
    /// the rebuild rate is bounded by the same budget. Do **not** "optimize" this
    /// into a build-once cache or an mtime check: building it exactly once is the
    /// staleness this method exists to fix.
    pub(crate) fn refresh(&self, repo: &gix::Repository) {
        *self.write() = build_ignore_matcher(repo);
    }

    /// Whether the ignore set claims `path` — directly, or via a rule on any of
    /// its parents, so a write deep inside an ignored directory
    /// (`target/debug/app`) is matched by the `target/` rule above it.
    ///
    /// `is_dir` tells the matcher whether `path` itself is a directory, which
    /// decides whether directory-only rules (`build/`) can match it directly.
    ///
    /// # Panics
    ///
    /// Panics if `path` is not under the work-tree root the matcher was built
    /// against — the underlying [`Gitignore::matched_path_or_any_parents`]
    /// contract. [`should_react`] is the only caller and it classifies out-of-
    /// worktree paths before reaching here.
    fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        self.read()
            .matched_path_or_any_parents(path, is_dir)
            .is_ignore()
    }

    /// The read side of the shared matcher, recovering from lock poisoning.
    ///
    /// A [`Gitignore`] is an immutable compiled glob set with no cross-field
    /// invariant a panic could leave half-written: whatever is behind the lock is
    /// always a complete matcher. Propagating poisoning instead would let an
    /// unrelated thread's panic wedge the monitor permanently — every subsequent
    /// event unwrapping on a poisoned lock — which is strictly worse than reading
    /// a perfectly valid matcher, so recover the inner value.
    fn read(&self) -> RwLockReadGuard<'_, Gitignore> {
        self.0.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// The write side of the shared matcher, recovering from lock poisoning for
    /// the same reason as [`read`](Self::read) — and with even less at stake
    /// here, since the write replaces the matcher wholesale.
    fn write(&self) -> RwLockWriteGuard<'_, Gitignore> {
        self.0.write().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
impl From<Gitignore> for LiveIgnore {
    /// Share an already-built matcher — **in test builds only**, and the
    /// `#[cfg(test)]` above is load-bearing rather than tidiness.
    ///
    /// A [`LiveIgnore`] built from a bare [`Gitignore`] is one nobody refreshes.
    /// It is assembled from a glob set the caller already had in hand, with no
    /// repository behind it to re-read, so it is frozen at whatever those globs
    /// said the moment it was handed over — the exact staleness this type was
    /// introduced to prevent, wearing the type that promises the opposite. Left
    /// ungated, that construction is reachable from anywhere in the crate, and
    /// the liveness invariant degrades from something the compiler holds into
    /// something a future caller is trusted to remember.
    ///
    /// Gating it leaves [`LiveIgnore::new`] as the only entrance that survives
    /// into a production build, and `new` reads from a repository — so every
    /// `LiveIgnore` that ships is one [`refresh`](LiveIgnore::refresh) can keep
    /// current. Under `cfg(test)` the impl remains what it always was: the seam
    /// the pure [`should_react`] tests use to hand in a matcher assembled from
    /// raw gitignore lines instead of from a repository on disk.
    fn from(matcher: Gitignore) -> Self {
        Self(Arc::new(RwLock::new(matcher)))
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
/// `workdir` roots the ignore matcher. [`LiveIgnore::is_ignored`] panics on a
/// path outside that root, so the matcher is only consulted for paths confirmed
/// to be under `workdir` (git-dir paths, which may live outside the worktree,
/// are classified before it is ever called).
///
/// The matcher arrives as a [`LiveIgnore`] rather than a bare [`Gitignore`]
/// because the render loop rebuilds it from disk on every walk while this
/// classifier is running on the watcher thread: an ignore rule added or removed
/// mid-session must change the answer here without a restart. This function
/// stays pure — it reads the matcher, never rebuilds it.
pub(crate) fn should_react(
    path: &Path,
    ignore: &LiveIgnore,
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
        // `is_ignored` walks up to the root, so a write deep inside an ignored
        // directory (`target/debug/app`) is matched by the `target/` rule on the
        // parent. Drop the event only when the ignore set actually claims the
        // path — as of the last rebuild, which is the last git walk.
        return !ignore.is_ignored(path, path.is_dir());
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
/// This is only one of the loop's deadline sources, and the least demanding of
/// them: while a refresh countdown is on screen, [`CLOCK_CADENCE`] wakes the
/// loop every second regardless of what this returns. `None` here therefore
/// means "the fade needs no tick", not "the loop will sleep" — it sleeps only
/// when `--refresh-interval 0` takes the countdown away too.
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

/// Whether a filesystem change may walk git right now, or must wait out the
/// adaptive cooldown. Returned by [`WalkSchedule::on_change`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Walk {
    /// The cooldown has expired (or none is armed): walk git now.
    Now,
    /// A walk is still gated by the cooldown: skip the git work this time.
    Defer,
}

/// Fraction of one core git walks may occupy under sustained churn (1%). A walk
/// costing `D` is followed by a cooldown of `D / BUDGET`, so the duty cycle
/// settles at `BUDGET`. Hard-coded, not a user dial.
#[allow(
    dead_code,
    reason = "Names the 1% duty-cycle target for the docs and spec; `cooldown` \
              applies the exact integer reciprocal (100) to avoid float drift, so \
              the constant itself is never read."
)]
const BUDGET: f64 = 0.01;

/// Minimum cooldown, equal to today's [`DEBOUNCE`] window (150 ms). When a walk
/// is cheap enough that 100·`cost` falls below this, [`cooldown`] clamps the
/// result UP to `FLOOR` — so adaptive throttling can only ever make watch-mode
/// updates *slower* (for an expensive repo), never faster than they already are
/// under today's debounce. A nearly-free walk therefore still settles at 150 ms.
const FLOOR: Duration = Duration::from_millis(150);

/// Pure, time-injected throttle that gates git walks to the [`BUDGET`] duty
/// cycle. After a walk costing `D`, the next walk is held off for `D / BUDGET`
/// (= 100·`D`), so an expensive repo automatically backs off and a cheap one
/// stays responsive — all decided here with injected instants, no clock of its
/// own.
struct WalkSchedule {
    /// Earliest instant the next walk may start. `None` = a walk is allowed now.
    next_allowed_at: Option<Instant>,
    /// Set when a change arrives during an active cooldown: a walk has been
    /// deferred and exactly one coalesced walk is now owed at the cooldown's
    /// expiry. Cleared by [`Self::record`] once that owed walk is performed.
    dirty: bool,
    /// How often a walk runs with no filesystem event to prompt it. `None`
    /// disables the timed walk, leaving gsw purely event-driven.
    interval: Option<Duration>,
    /// When the next timed walk falls due. `None` while no interval is set.
    next_timed_at: Option<Instant>,
}

impl WalkSchedule {
    /// A schedule that runs no timed walk: gsw stays purely event-driven and
    /// the duty-cycle gate is the only timing policy. Takes no instant on
    /// purpose — with no interval there is nothing to count from, and this type
    /// reads no clock of its own.
    #[cfg(test)]
    fn unscheduled() -> Self {
        Self {
            next_allowed_at: None,
            dirty: false,
            interval: None,
            next_timed_at: None,
        }
    }

    /// Build a schedule whose timed walks run every `interval`, counting from
    /// `last_walk_at` — the start of the walk that seeded the first frame —
    /// and gated by what that walk cost, exactly as [`Self::record`] gates
    /// every walk after it.
    fn new(interval: Option<Duration>, last_walk_at: Instant, last_walk_cost: Duration) -> Self {
        let cooldown = cooldown(last_walk_cost);
        Self {
            // The seed walk arms no cooldown gate: a filesystem change landing
            // right after startup still walks immediately, as it always has.
            // Only the *timed* walk is held to the budget here — the gate is
            // record()'s job, and the seed walk is over before this is built.
            next_allowed_at: None,
            dirty: false,
            interval,
            next_timed_at: interval.map(|i| last_walk_at + i.max(cooldown)),
        }
    }

    /// When the next walk this schedule *owes* falls due, or `None` when it owes
    /// none. Both a deferred filesystem change and a timed walk are walks that
    /// will happen with no further input, so the sooner of the two wins.
    ///
    /// This is what the refresh clock counts down to. A filesystem event can
    /// still walk earlier, which is why the clock says "scheduled".
    fn next_walk_at(&self) -> Option<Instant> {
        match (self.next_allowed(), self.next_timed_at) {
            (Some(owed), Some(timed)) => Some(owed.min(timed)),
            (owed, None) => owed,
            (None, timed) => timed,
        }
    }

    /// The countdown the refresh clock prints at `now`, or `None` when this
    /// schedule runs no timed walks and therefore shows no clock.
    ///
    /// Deliberately not just "the next walk owed": a change deferred through a
    /// cooldown is owed under `--refresh-interval 0` too, and counting down to
    /// it would put the clock back on screen under the flag that took it away.
    /// The interval is what decides whether a clock exists at all; once it does,
    /// the countdown tracks whichever walk lands first.
    fn countdown(&self, now: Instant) -> Option<Duration> {
        self.interval?;
        self.next_walk_at()
            .map(|at| ceil_secs(at.saturating_duration_since(now)))
    }

    /// Decide whether a change arriving at `now` may walk git: [`Walk::Now`] once
    /// the armed cooldown has elapsed (or none is armed), otherwise [`Walk::Defer`].
    fn on_change(&mut self, now: Instant) -> Walk {
        match self.next_allowed_at {
            Some(allowed_at) if now < allowed_at => {
                // A change landed mid-cooldown: don't walk now, but remember that
                // a change has happened which no walk has yet reflected, so one
                // coalesced walk is owed when the cooldown expires.
                self.dirty = true;
                Walk::Defer
            }
            _ => Walk::Now,
        }
    }

    /// Arm the next cooldown from a walk that started at `walk_start` and took
    /// `cost`: the next walk is gated until `walk_start + cost / BUDGET`. Purely
    /// last-write-wins — each call replaces any prior cooldown, no averaging.
    /// Clears any pending deferred walk: a freshly-recorded walk reflects the
    /// latest coalesced state, so no walk is owed afterward (see [`Self::dirty`]).
    ///
    /// The timed walk is re-armed from the same `walk_start`, at whichever is
    /// later: one `interval`, or the cooldown this walk just earned. The budget
    /// outranks the interval on purpose — a repo whose walk costs two seconds
    /// owes a 200-second cooldown, and a 60-second timed walk into that window
    /// would either violate the duty cycle or promise a refresh the gate refuses
    /// to admit. Pushing the timed walk out keeps the countdown honest.
    fn record(&mut self, walk_start: Instant, cost: Duration) {
        let cooldown = cooldown(cost);
        self.next_allowed_at = Some(walk_start + cooldown);
        self.dirty = false;
        self.next_timed_at = self
            .interval
            .map(|interval| walk_start + interval.max(cooldown));
    }

    /// The instant a pending deferred walk should fire — the cooldown's expiry —
    /// or `None` when no walk is owed. A change that arrives mid-cooldown is
    /// deferred (it sets the dirty flag) and registers exactly one coalesced
    /// walk at expiry; the Phase-4 loop reads this to arm a throttle wakeup only
    /// when one is actually owed, and stays asleep otherwise.
    fn next_allowed(&self) -> Option<Instant> {
        if self.dirty {
            self.next_allowed_at
        } else {
            None
        }
    }

    /// Force the next walk to be allowed immediately, lifting any active
    /// cooldown gate — the manual-refresh escape hatch (Phase 5's `r` key) for
    /// a long cooldown the user doesn't want to wait out. Leaves [`Self::dirty`]
    /// untouched (the forced walk's subsequent [`Self::record`] clears it); this
    /// only opens the gate so the next [`Self::on_change`] returns [`Walk::Now`].
    fn force(&mut self) {
        // Clearing the gate is exactly the "a walk is allowed now" state, so the
        // next on_change short-circuits to Walk::Now regardless of how much
        // cooldown remained. dirty is left as-is — the forced walk's record clears it.
        self.next_allowed_at = None;
    }
}

/// Cooldown for a walk costing `cost`: `max(FLOOR, cost / BUDGET)` (= `max(FLOOR,
/// 100·cost)`), so the sustained git duty cycle settles at [`BUDGET`] while the
/// [`FLOOR`] keeps a nearly-free walk from updating faster than today's debounce.
/// The integer multiply by the reciprocal (not `Duration::mul_f64`) keeps it
/// nanosecond-exact, so the [`Walk::Defer`]/[`Walk::Now`] boundary lands precisely
/// at `walk_start + cost / BUDGET` once that exceeds the floor; a cost large enough
/// to overflow saturates at [`Duration::MAX`].
fn cooldown(cost: Duration) -> Duration {
    // = 1 / BUDGET (0.01); an exact integer scale avoids the nanosecond drift
    // `Duration::mul_f64` would introduce at the on_change boundary.
    const COOLDOWN_MULTIPLIER: u32 = 100;
    // Clamp UP to FLOOR: a sub-1.5 ms walk's 100·cost is under 150 ms, so it
    // settles at the floor; anything ≥ 1.5 ms already clears it and is unaffected.
    cost.checked_mul(COOLDOWN_MULTIPLIER)
        .unwrap_or(Duration::MAX)
        .max(FLOOR)
}

/// How often the loop repaints purely to move the refresh clock along. The
/// clock prints whole seconds, so a second is exactly what it needs — waking
/// more often would repaint an identical frame, and less often would leave a
/// countdown visibly stuck.
///
/// This cadence applies only while a countdown is on screen. With
/// `--refresh-interval 0` there is no countdown, no clock tick, and the
/// adaptive decay cadence is once again the only timer — which is what keeps
/// today's idle-at-zero behavior available to anyone who wants it back.
const CLOCK_CADENCE: Duration = Duration::from_secs(1);

/// Place a frame in time: how stale its snapshot is, and how long until the
/// walk `schedule` next owes — the countdown the refresh clock prints.
///
/// Called after [`WalkSchedule::record`] on a walking wake, so a frame painted
/// by a walk shows the interval it just re-armed rather than the one it spent.
fn timing(age_offset: Duration, schedule: &WalkSchedule, now: Instant) -> FrameTiming {
    FrameTiming {
        age_offset,
        next_refresh_in: schedule.countdown(now),
    }
}

/// Round a duration up to the next whole second.
///
/// The clock prints whole seconds, and its two halves round in opposite
/// directions on purpose: an elapsed time floors (0.9 s ago really is "0s ago"
/// so far), while a countdown ceils (0.1 s left must not read as "0s"). Rounding
/// both the same way loses a second between them, and the pair stops adding up
/// to the interval it is measuring.
fn ceil_secs(remaining: Duration) -> Duration {
    let secs = remaining.as_secs();
    if remaining.subsec_nanos() > 0 {
        Duration::from_secs(secs.saturating_add(1))
    } else {
        Duration::from_secs(secs)
    }
}

/// The loop's wait window: the soonest deadline any source imposes, or `None`
/// to block until an event arrives.
///
/// Every input is already expressed as a duration from now, and a `None` from a
/// source means that source imposes no deadline. Taking a slice rather than a
/// fixed pair is what lets a new source (the timed refresh, the refresh clock's
/// own cadence) join without every caller and test changing shape.
fn wait_window(deadlines: &[Option<Duration>]) -> Option<Duration> {
    deadlines.iter().flatten().min().copied()
}

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
    /// The user asked to force an immediate refresh (`r`), bypassing the
    /// throttle cooldown.
    ForceRefresh,
}

/// The git work one watch-mode refresh performs: re-open the repository so
/// configuration written since the last refresh takes effect, rebuild the
/// watcher's ignore matcher from that fresh handle, then collect the snapshot.
///
/// This is watch mode's whole re-derivation of on-disk state, named so it has
/// one place to grow. The re-open is the load-bearing half. A
/// [`gix::Repository`] snapshots `.git/config` at open time and never reloads
/// it, so a process-lifetime handle renders whatever the config said at
/// startup: `git push -u origin <branch>` in another pane writes
/// `branch.<name>.remote`/`.merge` and the header's `↑0 ↓0 origin/<branch>`
/// segment still never appears, `git branch --unset-upstream` leaves stale
/// arrows on screen, and a renamed remote or a changed `core.excludesFile` are
/// equally invisible. Re-opening per refresh fixes the class rather than
/// special-casing `branch.*`.
///
/// The ignore rebuild is the same bug one layer out. The matcher the watcher
/// thread classifies events against was built once at spawn, so a rule added to
/// `.gitignore` mid-session never starts filtering (the watcher keeps chasing
/// build churn) and — worse — a rule *removed* never stops filtering, leaving
/// the callback silently dropping events for paths that are once again
/// rendered. Refreshing it here, from the handle re-opened one line above, is
/// what makes the two fixes compose: `core.excludesFile` is read out of
/// `.git/config`, so the fresh config feeds straight into the fresh matcher and
/// a user who repoints their global excludes sees it on the next walk.
///
/// The cost is one config parse plus at most three small ignore-file reads per
/// walk, which only happens when the throttle admits a walk in the first place —
/// and it rides along with a full status traversal that dwarfs both.
/// [`RepoHandle::reopened`] keeps the previous handle if the re-open fails, so
/// catching git mid-write costs a tick of stale configuration rather than a
/// failed walk. That fallback is only half of "never a blank screen": it keeps
/// the *handle*, while [`event_loop`] keeps the *frame* when the status walk on
/// that handle fails anyway. Both halves are required — see the `# Errors`
/// section.
///
/// # Errors
///
/// Propagates a [`collect_snapshot`] failure (the status walk). Neither a failed
/// *re-open* nor an unreadable ignore file is an error: the first degrades to the
/// handle already in hand, the second to a matcher without that source.
///
/// The one production caller, [`event_loop`], deliberately does **not** let that
/// error out of watch mode: it keeps the last good snapshot, re-renders it at its
/// true (still-advancing) age, arms the throttle from the failed walk's cost, and
/// retries on the next event. So this signature says "this walk did not produce a
/// snapshot", not "the monitor should stop" — a distinction worth preserving if a
/// second caller ever appears.
pub(crate) fn walk(
    handle: &mut RepoHandle,
    ignore: &LiveIgnore,
    cfg: &RenderConfig,
) -> Result<Snapshot> {
    let repo = handle.reopened();
    ignore.refresh(repo);
    collect_snapshot(repo, cfg)
}

/// Run the live watch loop: take over the alternate screen, seed the snapshot
/// cache with one git walk, paint the first frame, then re-render on filesystem
/// changes, terminal resizes, timed refreshes, and decay-timer ticks until the
/// user quits with `q` or Ctrl-C.
///
/// Filesystem changes and timed refreshes [`walk`] git — re-opening the
/// repository so config changed in another pane takes effect — and re-seed the
/// cache; decay ticks and resizes re-render the cached snapshot with no git work
/// (Part A). The [`TerminalGuard`] restores the main screen and cursor on every
/// exit path.
///
/// Takes the [`RepoHandle`] **by value**: watch mode owns the repository for
/// the rest of the process, and each refresh mutates the handle in place by
/// re-opening it. Borrowing instead would make the caller hold a mutable borrow
/// across a call that never returns until the user quits, for no gain — nothing
/// is left for it to do with the handle afterward.
pub(crate) fn run(mut handle: RepoHandle, cfg: &RenderConfig) -> Result<()> {
    let _guard = TerminalGuard::enter()?;

    // Seed the cache with one git walk and paint the first frame at offset 0,
    // byte-identical to a one-shot render of the same state. That frame's
    // freshest age seeds the decay-timer cadence.
    //
    // Deliberately NOT `walk`: the handle was opened microseconds ago in
    // `main`, so nothing can have changed the config since, and a re-open here
    // would only pay for a config parse to read back what we already hold. The
    // ignore matcher is equally fresh — `LiveIgnore::new` below builds it from
    // that same just-opened handle — so skipping `walk`'s rebuild costs nothing
    // either. Every *subsequent* refresh goes through `walk`, which re-opens the
    // handle and rebuilds the matcher.
    let dims = current_dimensions(cfg.width_offset);
    let collected_at = Instant::now();
    let snapshot = collect_snapshot(handle.repo(), cfg)?;
    // The seed walk pays into the duty-cycle budget like every walk after it,
    // so its cost is what the schedule's first timed walk is gated on. The seed
    // frame then counts down to that same schedule rather than to the raw
    // interval — one deadline, quoted once, so the opening frame cannot promise
    // a refresh the loop will not make.
    let schedule = WalkSchedule::new(
        cfg.refresh_interval,
        collected_at,
        Instant::now().saturating_duration_since(collected_at),
    );
    let first = render_frame(
        &snapshot,
        cfg,
        dims,
        timing(Duration::ZERO, &schedule, collected_at),
    );
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

    // The one ignore matcher both threads share: the watcher callback reads it
    // per event, and every `walk` below rebuilds it from disk so a `.gitignore`
    // edited in another pane takes effect without a restart.
    let ignore = LiveIgnore::new(handle.repo());

    // The filesystem watcher must outlive the loop — dropping it stops watching.
    // Started before the collect closure below takes its mutable borrow of the
    // handle; the watcher clones everything it needs, so this borrow ends here.
    let _watcher = spawn_fs_watcher(handle.repo(), ignore.clone(), tx)?;

    event_loop(
        &rx,
        DEBOUNCE,
        &mut displayed,
        cache,
        initial_freshest,
        schedule,
        LoopHooks {
            collect: || walk(&mut handle, &ignore, cfg),
            render: |snap: &Snapshot, dims: Dimensions, timing: FrameTiming| {
                render_frame(snap, cfg, dims, timing)
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
///
/// `ignore` is the caller's [`LiveIgnore`], not one built here: the callback
/// thread only ever *reads* the matcher, while the render loop rebuilds it on
/// every walk. Owning it here would pin the ignore set to whatever was on disk
/// at spawn — precisely the staleness [`LiveIgnore`] exists to prevent.
fn spawn_fs_watcher(
    repo: &gix::Repository,
    ignore: LiveIgnore,
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
///
/// Every source is re-read on each call — this is what [`LiveIgnore::refresh`]
/// runs per walk — so the work-tree root is taken from the repository rather
/// than passed in, keeping the ignore set and the handle it was derived from
/// impossible to get out of step.
///
/// A repository with no work tree yields an empty matcher instead of a panic.
/// [`RepoHandle`] already rejects bare repos on the way in, so this should be
/// unreachable, but `workdir()` is still an `Option` and "nothing is ignored"
/// (every event wakes the loop — merely wasteful) is the right way for a monitor
/// to be wrong.
fn build_ignore_matcher(repo: &gix::Repository) -> Gitignore {
    let Some(workdir) = repo.workdir() else {
        return Gitignore::empty();
    };
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
    /// Render a snapshot at the given dimensions and timing.
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
/// or a timeout, then update the screen. A filesystem change walks git, and so
/// does a timeout at which [`WalkSchedule`] owes a walk — a timed refresh, or a
/// change deferred through a cooldown. Every other timeout re-renders the
/// *cached* [`Snapshot`] (Part A), so a decay tick on an unchanged repo still
/// costs no git work at all.
///
/// `schedule` arrives already anchored to the walk that filled `cache`, so the
/// caller's seed frame and this loop count down to the same deadline.
///
/// No timer needs a thread of its own: every deadline is folded into the
/// `recv_timeout` window by [`wait_window`], so a timeout *is* the tick, and the
/// window is recomputed after every render. Three sources feed it — the decay
/// cadence from `next_tick` (in `hooks`), the walk the schedule owes, and, while
/// a countdown is on screen, [`CLOCK_CADENCE`]. With all three absent the loop
/// blocks indefinitely on events.
///
/// `hooks` bundles the side effects (collect, render, terminal-size query, paint,
/// clock, tick cadence) so the loop is one function testable without a TTY or
/// real time: a test feeds a pre-loaded channel, a controllable clock, and
/// counters, then asserts which hooks ran and with what age offset. The
/// contracts verified there:
///
/// - a burst of filesystem events between renders collapses into **one** collect
///   (re-seeding the cache) and at most one paint (coalescing);
/// - a decay tick re-renders from cache with **no** collect, advancing every age
///   by `clock() - collected_at`, and repaints only if the frame changed;
/// - a resize re-renders the cached snapshot at the new dimensions with **no**
///   collect;
/// - a recompute whose output is byte-identical to what's displayed paints
///   nothing (suppression);
/// - a walk that *fails* does not end the loop: the last good snapshot is
///   re-rendered at its true age and the next event retries (see below);
/// - [`Event::Quit`] ends the loop, as does every sender hanging up.
///
/// A failed collect is absorbed rather than propagated because the failures are
/// overwhelmingly transient and none of the user's doing — `git gc` swapping the
/// ref store out from under the walk, a worktree being pruned, `.git` renamed
/// mid-operation. This is the other half of [`RepoHandle::reopened`]'s "never a
/// blank screen" guarantee: that fallback keeps a *handle* when the re-open
/// fails, and this keeps a *frame* when the status walk on it fails. Either half
/// alone leaves the monitor dying on a repository that is momentarily
/// unreadable. The failed walk still arms the throttle from its measured cost —
/// so a repo that fails every walk backs off on the same duty cycle instead of
/// hot-looping — and deliberately does *not* advance `collected_at`, so the
/// stale frame goes on aging honestly rather than resetting every displayed age
/// to zero. The accepted cost: a repository deleted for good leaves a frozen
/// (but visibly aging) frame until the user quits. That is the right failure for
/// a monitor — a wrong-but-labeled-old screen beats no screen.
fn event_loop<Collect, RenderFn, Dims, Paint, Clock, Tick>(
    rx: &Receiver<Event>,
    debounce: Duration,
    displayed: &mut String,
    mut cache: SnapshotCache,
    initial_freshest: Option<Duration>,
    mut schedule: WalkSchedule,
    mut hooks: LoopHooks<Collect, RenderFn, Dims, Paint, Clock, Tick>,
) -> Result<()>
where
    Collect: FnMut() -> Result<Snapshot>,
    RenderFn: FnMut(&Snapshot, Dimensions, FrameTiming) -> Render,
    Dims: Fn() -> Dimensions,
    Paint: FnMut(&str) -> Result<()>,
    Clock: Fn() -> Instant,
    Tick: Fn(Option<Duration>) -> Option<Duration>,
{
    let mut freshest = initial_freshest;
    loop {
        // Wait for the first event, or — when the decay timer is enabled — wake
        // after `interval` of quiet for a tick.
        // Track *which* triggers arrived so the render below can route them: a
        // filesystem change walks git, a resize re-renders the cache at the new
        // size, a bare timeout is a decay tick.
        let mut saw_fs = false;
        let mut saw_resize = false;
        let mut saw_force = false;
        // Wait window: the soonest of the decay-tick cadence, the next walk this
        // schedule owes (a timed refresh, or a walk deferred during a cooldown),
        // and — while a countdown is on screen — the cadence that countdown
        // needs to keep moving. The clock is read only when a walk is actually
        // owed.
        let walk_wait = schedule
            .next_walk_at()
            .map(|at| at.saturating_duration_since((hooks.clock)()));
        let wait = wait_window(&[
            (hooks.next_tick)(freshest),
            walk_wait,
            schedule.interval.map(|_| CLOCK_CADENCE),
        ]);
        let woke_for_timeout = match wait {
            Some(interval) => match rx.recv_timeout(interval) {
                Ok(Event::Quit) => break,
                Ok(Event::FsChanged) => {
                    saw_fs = true;
                    false
                }
                Ok(Event::Resize) => {
                    saw_resize = true;
                    false
                }
                Ok(Event::ForceRefresh) => {
                    saw_force = true;
                    false
                }
                Err(RecvTimeoutError::Timeout) => true,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(Event::Quit) => break,
                Ok(Event::FsChanged) => {
                    saw_fs = true;
                    false
                }
                Ok(Event::Resize) => {
                    saw_resize = true;
                    false
                }
                Ok(Event::ForceRefresh) => {
                    saw_force = true;
                    false
                }
                Err(_) => break,
            },
        };

        // Coalesce a filesystem burst: keep draining until the channel stays
        // quiet for a full `debounce`. A tick has no burst behind it.
        let mut quitting = false;
        if !woke_for_timeout {
            loop {
                match rx.recv_timeout(debounce) {
                    Ok(Event::Quit) => {
                        quitting = true;
                        break;
                    }
                    Ok(Event::FsChanged) => saw_fs = true,
                    Ok(Event::Resize) => saw_resize = true,
                    Ok(Event::ForceRefresh) => saw_force = true,
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => {
                        quitting = true;
                        break;
                    }
                }
            }
        }

        // Read the clock once for this wake: the throttle decision, a walk's
        // start, and any age offset all key off the same instant.
        let now = (hooks.clock)();

        // Does this wake walk git? A manual refresh (`r`) forces one
        // unconditionally, bypassing the cooldown. Otherwise an FS change the
        // throttle admits, or — when a change was deferred during a cooldown —
        // the single owed walk, fired by the timeout the loop armed at the
        // cooldown's expiry. `on_change` defers a mid-cooldown FS change (setting
        // the throttle's dirty flag) and we fall through to a cheap cached
        // re-render (Part A) instead of walking. A resize or a plain decay tick
        // never walks. A filesystem walk in a coalesced burst wins over a
        // co-arriving resize: the fresh walk already renders at the current
        // dimensions.
        let walk_now = if saw_force {
            // Manual refresh (`r`): lift the cooldown gate and walk now. The walk
            // branch re-measures cost and re-arms the throttle from it.
            schedule.force();
            true
        } else if saw_fs {
            matches!(schedule.on_change(now), Walk::Now)
        } else if woke_for_timeout {
            // Only a walk the schedule OWES fires here — a deferred change's
            // coalesced walk, or a timed refresh — and only once it has actually
            // fallen due: a decay tick or a clock tick that fires ahead of it (a
            // shorter wait than the walk deadline) re-renders from cache without
            // walking, so Part A and Part B compose.
            matches!(schedule.next_walk_at(), Some(due) if now >= due)
        } else {
            false
        };

        let render = if walk_now {
            cache.dims = (hooks.dimensions)();
            let collected = (hooks.collect)();
            // Measure the walk's wall-clock cost around collect and feed it to
            // the throttle, which arms the next cooldown (= 100·cost) from it.
            // Deliberately outside the match: a *failed* walk still paid for a
            // status traversal, and a repo that is unreadable for a while fails
            // every walk, so gating the retries on the same duty-cycle budget is
            // what keeps a permanently-deleted repo from pinning a core.
            let cost = (hooks.clock)().saturating_duration_since(now);
            schedule.record(now, cost);
            match collected {
                Ok(snapshot) => {
                    // Re-seed the collection time to the walk's start so a later
                    // decay tick or resize advances ages from *this* walk, not
                    // the previous one.
                    cache.collected_at = now;
                    cache.snapshot = snapshot;
                    (hooks.render)(
                        &cache.snapshot,
                        cache.dims,
                        timing(Duration::ZERO, &schedule, now),
                    )
                }
                // A walk can fail for reasons that are none of the user's
                // business and usually transient: `git gc` swapping the ref
                // store, a worktree being pruned, `.git` renamed mid-operation.
                // Ending watch mode over that would make the whole
                // stale-configuration fallback in [`RepoHandle::reopened`]
                // pointless, so absorb it: keep the last good snapshot and let
                // the next event retry. `collected_at` is pointedly NOT advanced
                // — a collection that never happened must not reset every
                // displayed age to "just now", or the monitor would claim
                // freshness exactly when it has none. The frame therefore keeps
                // aging truthfully while the repository is unreadable.
                Err(_) => {
                    let age_offset = now.saturating_duration_since(cache.collected_at);
                    (hooks.render)(
                        &cache.snapshot,
                        cache.dims,
                        timing(age_offset, &schedule, now),
                    )
                }
            }
        } else if saw_resize {
            cache.dims = (hooks.dimensions)();
            let age_offset = now.saturating_duration_since(cache.collected_at);
            (hooks.render)(
                &cache.snapshot,
                cache.dims,
                timing(age_offset, &schedule, now),
            )
        } else {
            // Decay tick, or an FS change the throttle deferred: re-render the
            // cached snapshot, advancing every displayed age by the elapsed time.
            let age_offset = now.saturating_duration_since(cache.collected_at);
            (hooks.render)(
                &cache.snapshot,
                cache.dims,
                timing(age_offset, &schedule, now),
            )
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

/// The pure, unit-testable core of [`spawn_event_reader`]: map one crossterm
/// terminal event to the [`Event`] the watch loop should react to, or `None`
/// when the event is irrelevant. Keeping the key→event decision here —
/// terminal-free and side-effect-free — lets it be tested without a pty while
/// the reader thread stays a thin `event::read` → `classify_input` → `tx.send`
/// loop.
///
/// - A key *release* is ignored (kitty/Windows report them; only a press acts).
/// - `q`, or Ctrl-C, requests a [`Event::Quit`].
/// - `r` forces an immediate refresh ([`Event::ForceRefresh`]), bypassing the
///   throttle cooldown.
/// - A terminal resize becomes [`Event::Resize`].
/// - Everything else is ignored.
fn classify_input(event: CtEvent) -> Option<Event> {
    match event {
        CtEvent::Key(KeyEvent {
            code,
            modifiers,
            kind,
            ..
        }) => {
            if kind == KeyEventKind::Release {
                // Ignore key releases (kitty/Windows report them); only a press acts.
                None
            } else if code == KeyCode::Char('q')
                || (modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c'))
            {
                Some(Event::Quit)
            } else if code == KeyCode::Char('r') {
                Some(Event::ForceRefresh)
            } else {
                None
            }
        }
        CtEvent::Resize(_, _) => Some(Event::Resize),
        _ => None,
    }
}

/// Spawn the crossterm event-reader thread. It blocks on `event::read`, routes
/// each event through [`classify_input`] (which maps `q`/Ctrl-C to
/// [`Event::Quit`], `r` to [`Event::ForceRefresh`], and terminal resizes to
/// [`Event::Resize`]), forwards any resulting [`Event`], and exits when the
/// receiver is gone or reading fails.
fn spawn_event_reader(tx: Sender<Event>) {
    thread::spawn(move || {
        // Loop until reading fails (terminal closed) — the `while let` exits on
        // `Err` — or a forwarded send fails because the receiver is gone.
        while let Ok(ct_event) = event::read() {
            if let Some(event) = classify_input(ct_event) {
                if tx.send(event).is_err() {
                    break;
                }
            }
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
    use crate::testrepo;
    use crate::WRAPPER_CHROME_ROWS;
    use ignore::gitignore::GitignoreBuilder;

    /// A [`RenderConfig`] for the fixture-backed walk tests: no explicit base,
    /// no caps, no log rows, no color. Only the git work matters here — the
    /// rendering knobs are exercised by the render tests.
    fn walk_config() -> RenderConfig {
        RenderConfig {
            base: None,
            max_files: None,
            bar_width: 20,
            log_lines: 0,
            truecolor: false,
            width_offset: 0,
            refresh_interval: None,
        }
    }

    #[test]
    fn walk_sees_an_upstream_configured_after_watch_started() {
        // The reported bug (#334), at the snapshot level: gsw is already
        // watching a local-only branch when the user runs `git push -u origin
        // <branch>` in another pane. That writes `branch.feature.remote` and
        // `branch.feature.merge` into `.git/config`, which the gix handle
        // opened at startup has cached and never re-reads — so the header's
        // `↑0 ↓0 origin/feature` segment stays missing until gsw is restarted.
        // A refresh must pick it up on the very next walk.
        let (_origin, clone) = testrepo::init_repo_with_upstream();
        let p = clone.path();
        testrepo::git(p, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(p.join("feature.txt"), "x\n").expect("write feature.txt");
        testrepo::git(p, &["add", "feature.txt"]);
        testrepo::git(p, &["commit", "-q", "-m", "feature work"]);

        // Opened BEFORE the push and held across it, exactly like watch mode.
        let mut handle = RepoHandle::discover(p).expect("clone is a worktree repo");
        let ignore = LiveIgnore::new(handle.repo());
        let cfg = walk_config();

        let before = walk(&mut handle, &ignore, &cfg).expect("first walk");
        assert!(
            before.upstream.is_none(),
            "a local-only branch has no upstream yet",
        );

        // What `git push -u origin feature` in another pane does while gsw runs.
        testrepo::git(p, &["push", "-q", "-u", "origin", "feature"]);

        let after = walk(&mut handle, &ignore, &cfg).expect("second walk");
        let up = after
            .upstream
            .as_ref()
            .expect("the upstream segment must appear without restarting gsw");
        assert_eq!(up.name, "origin/feature");
        assert_eq!(
            (up.ahead, up.behind),
            (0, 0),
            "the push left the branch level with its brand-new upstream",
        );
    }

    #[test]
    fn walk_sees_an_upstream_unset_after_watch_started() {
        // The mirror of #334, and the direction a narrow fix would miss: a fix
        // that only ever *adds* the upstream segment leaves the opposite case
        // broken. The user is watching a branch that tracks `origin/main` and
        // runs `git branch --unset-upstream` in another pane — deleting
        // `branch.main.remote`/`.merge` from `.git/config`. With a handle that
        // never re-reads that config, the header keeps painting `↑0 ↓0
        // origin/main` for a branch that no longer tracks anything: arrows that
        // are not merely stale but describe a relationship that has ceased to
        // exist. The segment must disappear on the very next walk.
        let (_origin, clone) = testrepo::init_repo_with_upstream();
        let p = clone.path();

        // Opened while the branch still tracks, and held across the unset.
        let mut handle = RepoHandle::discover(p).expect("clone is a worktree repo");
        let ignore = LiveIgnore::new(handle.repo());
        let cfg = walk_config();

        let before = walk(&mut handle, &ignore, &cfg).expect("first walk");
        let up = before
            .upstream
            .as_ref()
            .expect("a fresh clone's branch tracks origin/main");
        assert_eq!(up.name, "origin/main");

        // What `git branch --unset-upstream` in another pane does while gsw runs.
        testrepo::git(p, &["branch", "--unset-upstream"]);

        let after = walk(&mut handle, &ignore, &cfg).expect("second walk");
        assert!(
            after.upstream.is_none(),
            "the upstream segment must vanish without restarting gsw; instead \
             the header still claims {:?}",
            after.upstream.as_ref().map(|u| &u.name),
        );
    }

    #[test]
    fn walk_tracks_the_counts_when_the_remote_moves_under_it() {
        // End-to-end cover for the *counts* half of the upstream segment: a
        // teammate pushes and the user runs `git fetch` in another pane, which
        // moves `refs/remotes/origin/main` forward. The header's `↓` must follow
        // on the next walk — a monitor that keeps reporting `↓0` while the
        // branch is a commit behind is telling the user their branch is current
        // when it is not.
        //
        // This covers the ref-driven half of the refresh rather than the
        // config-cached half: tracking refs are read from disk on every
        // `upstream_status` call, so this direction was already live before the
        // re-open landed. It stays as a cheap guard that the whole path — walk,
        // snapshot, counts — still moves with the remote.
        let (origin, clone) = testrepo::init_repo_with_upstream();
        let p = clone.path();
        std::fs::write(p.join("local.txt"), "x\n").expect("write local.txt");
        testrepo::git(p, &["add", "local.txt"]);
        testrepo::git(p, &["commit", "-q", "-m", "local only"]);

        let mut handle = RepoHandle::discover(p).expect("clone is a worktree repo");
        let ignore = LiveIgnore::new(handle.repo());
        let cfg = walk_config();

        let before = walk(&mut handle, &ignore, &cfg).expect("first walk");
        let up = before
            .upstream
            .as_ref()
            .expect("the clone tracks origin/main");
        assert_eq!(
            (up.ahead, up.behind),
            (1, 0),
            "one local commit, and the remote has not moved yet",
        );

        // What a teammate's push plus `git fetch` in another pane amounts to.
        let op = origin.path();
        std::fs::write(op.join("remote.txt"), "y\n").expect("write remote.txt");
        testrepo::git(op, &["add", "remote.txt"]);
        testrepo::git(op, &["commit", "-q", "-m", "remote moved on"]);
        testrepo::git(p, &["fetch", "-q"]);

        let after = walk(&mut handle, &ignore, &cfg).expect("second walk");
        let up = after
            .upstream
            .as_ref()
            .expect("fetching does not remove the upstream");
        assert_eq!(
            (up.ahead, up.behind),
            (1, 1),
            "the remote advanced one commit past the branch, so the header must \
             show it as behind without restarting gsw",
        );
    }

    #[test]
    fn walk_follows_a_remote_renamed_after_watch_started() {
        // `git remote rename origin upstream` is the same staleness as #334
        // wearing a different hat: it rewrites `branch.main.remote` *and* moves
        // every `refs/remotes/origin/*` ref to `refs/remotes/upstream/*`. A
        // handle holding the old config resolves the tracking ref to
        // `refs/remotes/origin/main`, which no longer exists — so the segment
        // doesn't just show the wrong name, it drops out of the header entirely
        // until gsw restarts. The re-open makes the rename land on the next walk.
        let (_origin, clone) = testrepo::init_repo_with_upstream();
        let p = clone.path();

        // Opened while the remote is still called `origin`, held across the rename.
        let mut handle = RepoHandle::discover(p).expect("clone is a worktree repo");
        let ignore = LiveIgnore::new(handle.repo());
        let cfg = walk_config();

        let before = walk(&mut handle, &ignore, &cfg).expect("first walk");
        assert_eq!(
            before.upstream.as_ref().map(|u| u.name.as_str()),
            Some("origin/main"),
            "the clone starts out tracking origin/main",
        );

        // What `git remote rename origin upstream` in another pane does.
        testrepo::git(p, &["remote", "rename", "origin", "upstream"]);

        let after = walk(&mut handle, &ignore, &cfg).expect("second walk");
        assert_eq!(
            after.upstream.as_ref().map(|u| u.name.as_str()),
            Some("upstream/main"),
            "a remote renamed after watch started must be reflected without a \
             restart, not blank the upstream segment",
        );
    }

    /// Render `snapshot` exactly the way a watch-mode repaint does — the same
    /// [`render_frame`] call [`run`] makes, at a zero age offset — and hand back
    /// the header, which is the frame's first line, with ANSI stripped.
    ///
    /// The dimensions are deliberately generous: [`crate::render`] degrades the
    /// header through a ladder (full upstream → counts only → shaved names →
    /// omitted) as the terminal narrows, so a cramped width would hide the
    /// upstream name for reasons that have nothing to do with what is being
    /// asserted.
    ///
    /// Stripping is not optional. `colored` decides whether to emit escapes from
    /// a *process-global* override that other tests in this parallel suite
    /// toggle, so a byte-level `contains` would pass or fail depending on which
    /// test ran last. Comparing visible glyphs is stable either way.
    fn header_line(snapshot: &Snapshot, cfg: &RenderConfig) -> String {
        let dims = Dimensions {
            width: 200,
            height: 40,
        };
        let frame = render_frame(snapshot, cfg, dims, FrameTiming::at_walk(None));
        crate::render::strip_ansi(frame.output.lines().next().unwrap_or_default())
    }

    #[test]
    fn the_rendered_header_gains_the_upstream_segment_after_a_push() {
        // #334 stated as what the user actually sees. Every other guard on this
        // branch asserts on a `Snapshot` field; this one runs the snapshot
        // through the same `render_frame` a repaint uses and reads the header
        // line, so a refresh that collected the upstream correctly but failed to
        // surface it in the header would still be caught.
        //
        // The scenario is the bug report verbatim: gsw is watching a local-only
        // `feature` branch, the user runs `git push -u origin feature` in
        // another pane, and the `↑0 ↓0 origin/feature` segment has to appear in
        // the header on the next refresh instead of after a restart.
        let (_origin, clone) = testrepo::init_repo_with_upstream();
        let p = clone.path();
        testrepo::git(p, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(p.join("feature.txt"), "x\n").expect("write feature.txt");
        testrepo::git(p, &["add", "feature.txt"]);
        testrepo::git(p, &["commit", "-q", "-m", "feature work"]);

        // Opened BEFORE the push and held across it, exactly like watch mode.
        let mut handle = RepoHandle::discover(p).expect("clone is a worktree repo");
        let ignore = LiveIgnore::new(handle.repo());
        let cfg = walk_config();

        let before = header_line(&walk(&mut handle, &ignore, &cfg).expect("first walk"), &cfg);
        assert!(
            !before.contains("origin/"),
            "a local-only branch must not advertise any upstream: {before:?}",
        );

        // What `git push -u origin feature` in another pane does while gsw runs.
        testrepo::git(p, &["push", "-q", "-u", "origin", "feature"]);

        let after = header_line(
            &walk(&mut handle, &ignore, &cfg).expect("second walk"),
            &cfg,
        );
        assert!(
            after.contains("origin/feature"),
            "the header must name the brand-new upstream without restarting \
             gsw: {after:?}",
        );
        assert!(
            after.contains('↑') && after.contains('↓'),
            "and it must carry the ahead/behind arrows alongside it: {after:?}",
        );
    }

    /// Build an ignore matcher rooted at `root` from raw gitignore lines, the
    /// way the production matcher is assembled from the repo's ignore files.
    /// Handed back as a [`LiveIgnore`] so the pure [`should_react`] tests use
    /// the same type production does, without needing a repository on disk.
    fn matcher(root: &str, patterns: &[&str]) -> LiveIgnore {
        let mut builder = GitignoreBuilder::new(root);
        for pattern in patterns {
            builder.add_line(None, pattern).expect("valid glob");
        }
        builder.build().expect("build matcher").into()
    }

    /// Open `path` the way watch mode does, and hand back everything the
    /// live-ignore tests need: the handle each walk re-opens, the matcher built
    /// from the repo's ignore sources as they stood at "startup", and the
    /// work-tree root plus git dirs [`should_react`] classifies against.
    ///
    /// The work-tree root comes from the *repository*, never from the tempdir
    /// path the caller created. On macOS `tempfile::tempdir()` hands back
    /// `/var/folders/…`, a symlink to the real `/private/var/folders/…`, and the
    /// matcher is rooted at whichever form gix reports. Since
    /// `matched_path_or_any_parents` panics on a path outside its root, every
    /// path fed to `should_react` must be built from this same root.
    fn watching(path: &Path) -> (RepoHandle, LiveIgnore, PathBuf, Vec<PathBuf>) {
        let handle = RepoHandle::discover(path).expect("fixture is a worktree repo");
        let workdir = handle
            .repo()
            .workdir()
            .expect("a discovered worktree repo has a work tree")
            .to_path_buf();
        let git_dirs = vec![handle.repo().git_dir().to_path_buf()];
        let ignore = LiveIgnore::new(handle.repo());
        (handle, ignore, workdir, git_dirs)
    }

    #[test]
    fn walk_picks_up_a_gitignore_rule_added_after_watch_started() {
        // The second half of #334: the watcher's ignore matcher was built once
        // at spawn and moved into the notify callback, so an ignore rule the
        // user adds while gsw is running never takes effect. `echo 'build/' >>
        // .gitignore` in another pane leaves the watcher waking the render loop
        // on every object file the next build writes, forever — a status walk
        // burned per event, for churn that can no longer change the view. A
        // refresh must rebuild the matcher so the rule takes hold immediately.
        let dir = testrepo::init_repo();
        let p = dir.path();
        std::fs::create_dir(p.join("build")).expect("create build/");
        std::fs::write(p.join("build").join("out.o"), "obj\n").expect("write build/out.o");

        // Opened BEFORE the .gitignore edit and held across it, like watch mode.
        let (mut handle, ignore, workdir, git_dirs) = watching(p);
        let churn = workdir.join("build").join("out.o");
        let cfg = walk_config();

        assert!(
            should_react(&churn, &ignore, &workdir, &git_dirs),
            "nothing is ignored yet, so build churn still has to wake the loop",
        );

        // What `echo 'build/' >> .gitignore` in another pane does while gsw runs.
        std::fs::write(p.join(".gitignore"), "build/\n").expect("write .gitignore");
        walk(&mut handle, &ignore, &cfg).expect("walk");

        assert!(
            !should_react(&churn, &ignore, &workdir, &git_dirs),
            "a .gitignore rule written after watch started must take effect \
             without restarting gsw",
        );
    }

    #[test]
    fn walk_picks_up_a_gitignore_rule_removed_after_watch_started() {
        // The mirror direction, and the one that is a correctness bug rather
        // than wasted work: with a stale matcher the watcher keeps *dropping*
        // events for a path that is no longer ignored. Delete `build/` from
        // `.gitignore` and everything under it becomes untracked — rows gsw is
        // supposed to render — yet the callback filters those events out, so the
        // view sits frozen until gsw is restarted. A refresh must let them
        // through again.
        let dir = testrepo::init_repo();
        let p = dir.path();
        std::fs::write(p.join(".gitignore"), "build/\n").expect("write .gitignore");
        std::fs::create_dir(p.join("build")).expect("create build/");
        std::fs::write(p.join("build").join("out.o"), "obj\n").expect("write build/out.o");

        // Opened while the rule is still in force, and held across its removal.
        let (mut handle, ignore, workdir, git_dirs) = watching(p);
        let churn = workdir.join("build").join("out.o");
        let cfg = walk_config();

        assert!(
            !should_react(&churn, &ignore, &workdir, &git_dirs),
            "the rule is in force at startup, so build churn is filtered out",
        );

        // What deleting the `build/` line in another pane does while gsw runs.
        std::fs::write(p.join(".gitignore"), "").expect("truncate .gitignore");
        walk(&mut handle, &ignore, &cfg).expect("walk");

        assert!(
            should_react(&churn, &ignore, &workdir, &git_dirs),
            "a .gitignore rule removed after watch started must stop filtering \
             without restarting gsw — those paths are rendered again",
        );
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
    fn wait_window_picks_the_earliest_deadline() {
        // The loop waits on the SOONEST deadline any source imposes. A `None`
        // from a source means it imposes no deadline; all `None` means block
        // until an event arrives.
        let short = Duration::from_secs(5);
        let long = Duration::from_secs(60);

        // Earliest of the present deadlines wins, regardless of argument order.
        assert_eq!(wait_window(&[Some(long), Some(short)]), Some(short));
        assert_eq!(wait_window(&[Some(short), Some(long)]), Some(short));
        assert_eq!(wait_window(&[Some(short), Some(short)]), Some(short));

        // One source absent: the other's deadline stands.
        assert_eq!(wait_window(&[Some(short), None]), Some(short));
        assert_eq!(wait_window(&[None, Some(short)]), Some(short));

        // Any number of sources, not just two.
        let mid = Duration::from_secs(30);
        assert_eq!(
            wait_window(&[Some(long), Some(short), Some(mid)]),
            Some(short)
        );
        assert_eq!(wait_window(&[None, Some(mid), None, Some(long)]), Some(mid));

        // No source imposes a deadline: block until an event arrives.
        assert_eq!(wait_window(&[None, None]), None);
        assert_eq!(wait_window(&[]), None);
    }

    #[test]
    fn cooldown_gates_the_next_walk_at_one_hundred_times_the_latest_cost() {
        // BUDGET is a 1% duty cycle, so a walk costing D must be followed by a
        // cooldown of D / 0.01 = 100·D before the next walk is allowed:
        // on_change returns Defer until that instant and Now at/after it. The
        // cooldown is recomputed PURELY from the latest record (last-write-wins,
        // no smoothing) and has NO ceiling. All instants are derived from one
        // base so the test is deterministic and parallel-safe — no real sleeping.
        let t0 = Instant::now();

        // Representative cost: a 150 ms walk gates the next for 100·150 ms = 15 s.
        let mut representative = WalkSchedule::unscheduled();
        representative.record(t0, Duration::from_millis(150));
        assert_eq!(
            representative.on_change(t0 + Duration::from_secs(15) - Duration::from_nanos(1)),
            Walk::Defer,
            "still gated one nanosecond before 100× the 150 ms cost",
        );
        assert_eq!(
            representative.on_change(t0 + Duration::from_secs(15)),
            Walk::Now,
            "allowed exactly at 100× the 150 ms cost",
        );

        // No ceiling: a 5 s walk gates the next for 100·5 s = 500 s, uncapped.
        let mut costly = WalkSchedule::unscheduled();
        costly.record(t0, Duration::from_secs(5));
        assert_eq!(
            costly.on_change(t0 + Duration::from_secs(500) - Duration::from_nanos(1)),
            Walk::Defer,
            "an expensive walk yields a proportionally long, uncapped cooldown",
        );
        assert_eq!(
            costly.on_change(t0 + Duration::from_secs(500)),
            Walk::Now,
            "allowed exactly at 100× the 5 s cost — no ceiling clamps it",
        );

        // Recompute-from-latest: a later record fully replaces the earlier one,
        // gating from the LATEST walk start at 100× the LATEST cost.
        let mut last_write_wins = WalkSchedule::unscheduled();
        let t1 = t0 + Duration::from_secs(1);
        last_write_wins.record(t0, Duration::from_millis(500)); // would gate until t0 + 50 s
        last_write_wins.record(t1, Duration::from_millis(30)); // replaced: gate until t1 + 3 s
        assert_eq!(
            last_write_wins.on_change(t1 + Duration::from_secs(3) - Duration::from_nanos(1)),
            Walk::Defer,
            "the gate follows the latest 30 ms cost (3 s from t1), not the prior 500 ms",
        );
        assert_eq!(
            last_write_wins.on_change(t1 + Duration::from_secs(3)),
            Walk::Now,
            "allowed exactly at 100× the latest cost, measured from the latest walk start",
        );

        // A fresh throttle that has never recorded imposes no cooldown.
        let mut fresh = WalkSchedule::unscheduled();
        assert_eq!(
            fresh.on_change(t0),
            Walk::Now,
            "a throttle that has never walked allows a walk immediately",
        );
    }

    /// The default timed-refresh cadence used across the schedule tests.
    const TEST_INTERVAL: Duration = Duration::from_secs(60);

    /// A walk cheap enough that its duty-cycle cooldown (100× cost, floored at
    /// 150 ms) stays far inside `TEST_INTERVAL` — so the interval, not the
    /// budget, decides when the timed walk falls due.
    const CHEAP: Duration = Duration::from_millis(150);

    #[test]
    fn the_two_clock_numbers_always_sum_to_the_interval() {
        // "last refresh: 1s ago, next refresh: 58s" on a 60-second interval
        // makes a reader check their arithmetic. Both numbers are printed as
        // whole seconds, so the elapsed half rounds down and the remaining half
        // must round up — then the pair reads as one interval, and the countdown
        // never claims less time than is actually left.
        let t0 = Instant::now();
        let schedule = WalkSchedule::new(Some(TEST_INTERVAL), t0, Duration::ZERO);
        for millis in [0, 1, 400, 999, 1000, 1400, 30_500, 58_999, 59_999] {
            let now = t0 + Duration::from_millis(millis);
            let frame = timing(now.saturating_duration_since(t0), &schedule, now);
            let elapsed = frame.age_offset.as_secs();
            let remaining = frame
                .next_refresh_in
                .expect("a scheduled walk has a countdown")
                .as_secs();
            assert_eq!(
                elapsed + remaining,
                TEST_INTERVAL.as_secs(),
                "at {millis}ms the clock reads {elapsed}s ago / {remaining}s left, \
                 which does not add up to one {TEST_INTERVAL:?} interval",
            );
        }
    }

    #[test]
    fn the_seed_walks_cost_gates_the_first_timed_walk() {
        // The walk that seeds the first frame is a walk like any other, so its
        // cost has to buy the same duty-cycle cooldown. Without that, the first
        // timed refresh spends a budget nothing paid for: on a repository whose
        // walk costs 2 s, gsw would re-walk at 60 s instead of the 200 s the
        // budget owes, running that first cycle at ~3.3% against a stated 1%.
        let t0 = Instant::now();
        let costly = Duration::from_secs(2);
        let schedule = WalkSchedule::new(Some(TEST_INTERVAL), t0, costly);
        assert_eq!(
            schedule.next_walk_at(),
            Some(t0 + Duration::from_secs(200)),
            "the seed walk's cost must gate the first timed walk, like every later walk",
        );
    }

    #[test]
    fn the_seed_frame_counts_down_to_the_schedule_the_loop_runs_on() {
        // The frame the seed walk paints and the schedule handed to the loop
        // have to quote the same deadline. A frame opening with "next refresh:
        // 60s" over a schedule that will not walk for 200 s promises a refresh
        // the gate has no intention of admitting — the exact dishonesty the
        // budget-outranks-the-interval rule exists to prevent.
        let t0 = Instant::now();
        let costly = Duration::from_secs(2);
        let schedule = WalkSchedule::new(Some(TEST_INTERVAL), t0, costly);
        assert_eq!(
            timing(Duration::ZERO, &schedule, t0).next_refresh_in,
            Some(Duration::from_secs(200)),
            "the seed frame must count down to the schedule's own first walk",
        );
    }

    #[test]
    fn timed_walk_falls_due_one_interval_after_the_last_walk() {
        // The countdown the refresh clock shows: with no filesystem event at
        // all, gsw still re-walks every interval. Instants derive from one base,
        // so the test is deterministic and parallel-safe — no real sleeping.
        let t0 = Instant::now();
        let mut schedule = WalkSchedule::new(Some(TEST_INTERVAL), t0, Duration::ZERO);
        assert_eq!(
            schedule.next_walk_at(),
            Some(t0 + TEST_INTERVAL),
            "the first timed walk is due one interval after the seed walk",
        );

        // Each walk re-arms the schedule from its own start.
        let t1 = t0 + Duration::from_secs(90);
        schedule.record(t1, CHEAP);
        assert_eq!(
            schedule.next_walk_at(),
            Some(t1 + TEST_INTERVAL),
            "a walk re-arms the timed walk one interval from that walk's start",
        );
    }

    #[test]
    fn timed_walk_never_outruns_the_duty_cycle_budget() {
        // On an expensive repo the 1% budget outranks the interval: a 2 s walk
        // earns a 200 s cooldown, so the timed walk waits 200 s, not 60 s.
        // Otherwise the "next refresh" countdown would promise a walk the gate
        // has no intention of admitting.
        let t0 = Instant::now();
        let mut schedule = WalkSchedule::new(Some(TEST_INTERVAL), t0, Duration::ZERO);
        let costly = Duration::from_secs(2);
        schedule.record(t0, costly);
        assert_eq!(
            schedule.next_walk_at(),
            Some(t0 + Duration::from_secs(200)),
            "the timed walk must wait out the duty-cycle cooldown",
        );
    }

    #[test]
    fn a_deferred_change_pulls_the_next_walk_in_ahead_of_the_interval() {
        // A filesystem change deferred mid-cooldown owes a walk at the
        // cooldown's expiry, which is sooner than the interval. The clock must
        // count down to the sooner of the two, or it would over-promise the wait.
        let t0 = Instant::now();
        let mut schedule = WalkSchedule::new(Some(TEST_INTERVAL), t0, Duration::ZERO);
        schedule.record(t0, CHEAP); // cooldown expires at t0 + 15 s
        assert_eq!(schedule.on_change(t0 + Duration::from_secs(1)), Walk::Defer);
        assert_eq!(
            schedule.next_walk_at(),
            Some(t0 + Duration::from_secs(15)),
            "an owed walk at 15 s beats the timed walk at 60 s",
        );
    }

    #[test]
    fn a_disabled_interval_shows_no_countdown_even_with_a_walk_owed() {
        // `--refresh-interval 0` takes the clock away. A change deferred through
        // a cooldown still owes a walk — the loop must fire it — but printing a
        // countdown for it would contradict the flag that removed the clock, and
        // on an expensive repo that stray countdown would sit there for minutes.
        let t0 = Instant::now();
        let mut schedule = WalkSchedule::unscheduled();
        schedule.record(t0, CHEAP);
        let during_cooldown = t0 + Duration::from_secs(1);
        assert_eq!(schedule.on_change(during_cooldown), Walk::Defer);
        assert!(
            schedule.next_walk_at().is_some(),
            "a deferred change still owes a walk the loop has to fire",
        );
        assert_eq!(
            schedule.countdown(during_cooldown),
            None,
            "a schedule with no interval must show no countdown, owed walk or not",
        );
    }

    #[test]
    fn the_countdown_tracks_the_next_scheduled_walk() {
        let t0 = Instant::now();
        let schedule = WalkSchedule::new(Some(TEST_INTERVAL), t0, Duration::ZERO);
        assert_eq!(
            schedule.countdown(t0 + Duration::from_secs(15)),
            Some(Duration::from_secs(45)),
        );
        // Overdue reads as due now rather than underflowing.
        assert_eq!(
            schedule.countdown(t0 + Duration::from_secs(120)),
            Some(Duration::ZERO),
        );
    }

    #[test]
    fn a_disabled_interval_owes_no_timed_walk() {
        // `--refresh-interval 0` restores today's purely event-driven gsw: the
        // gate still applies, but nothing falls due on its own.
        let t0 = Instant::now();
        let mut schedule = WalkSchedule::unscheduled();
        assert_eq!(
            schedule.next_walk_at(),
            None,
            "an unscheduled walk schedule owes nothing on its own",
        );
        schedule.record(t0, CHEAP);
        assert_eq!(
            schedule.next_walk_at(),
            None,
            "recording a walk must not invent a timed walk",
        );
    }

    #[test]
    fn floor_clamps_a_fast_walk_to_the_minimum_cooldown() {
        // A nearly-free walk has a tiny 100·cost cooldown, which would let the
        // throttle update FASTER than today's 150 ms debounce window. The FLOOR
        // clamps it: watch-mode updates can never be quicker than today even
        // when a walk costs almost nothing. A 1 ms walk's un-floored cooldown is
        // 100·1 ms = 100 ms; the floor must extend it out to 150 ms. Instants are
        // derived from one base, so the test is deterministic and parallel-safe.
        let t0 = Instant::now();

        let mut schedule = WalkSchedule::unscheduled();
        schedule.record(t0, Duration::from_millis(1));
        assert_eq!(
            schedule.on_change(t0 + Duration::from_millis(100)),
            Walk::Defer,
            "still gated past the un-floored 100 ms cooldown — the floor extends it",
        );
        assert_eq!(
            schedule.on_change(t0 + Duration::from_millis(150)),
            Walk::Now,
            "allowed exactly at the 150 ms floor, never faster than today's debounce",
        );
    }

    #[test]
    fn a_change_during_an_active_cooldown_pends_a_deferred_walk() {
        // Deferring a mid-cooldown change must NOT walk immediately — instead it
        // registers exactly one pending walk at the cooldown's expiry, so a burst
        // of changes coalesces into a single owed walk. `next_allowed()` exposes
        // WHEN that owed walk should fire (so the Phase-4 loop can arm a wakeup),
        // and is `None` until a deferral actually owes one. A 150 ms walk gates
        // the next for 100·150 ms = 15 s, so the owed walk lands at t0 + 15 s.
        // Instants derive from one base — deterministic and parallel-safe.
        let t0 = Instant::now();

        let mut schedule = WalkSchedule::unscheduled();
        schedule.record(t0, Duration::from_millis(150));
        assert_eq!(
            schedule.next_allowed(),
            None,
            "a recorded-but-unchanged throttle owes no walk yet — nothing is pending",
        );

        assert_eq!(
            schedule.on_change(t0 + Duration::from_secs(1)),
            Walk::Defer,
            "a change 1 s into the 15 s cooldown is deferred, not walked",
        );
        assert_eq!(
            schedule.next_allowed(),
            Some(t0 + Duration::from_secs(15)),
            "that deferred change now owes one coalesced walk at the cooldown's expiry",
        );
    }

    #[test]
    fn recording_a_walk_consumes_the_pending_deferred_walk() {
        // A completed walk reflects the LATEST coalesced state, so recording it
        // must consume the single owed walk and reset the deferral — otherwise
        // the throttle would believe a walk is owed forever. Any number of
        // mid-cooldown changes collapse to exactly one owed walk at the original
        // expiry (they neither double up nor move it), and the next `record`
        // clears it. A 150 ms walk gates the next for 100·150 ms = 15 s. Instants
        // derive from one base — deterministic and parallel-safe, no sleeping.
        let t0 = Instant::now();

        let mut schedule = WalkSchedule::unscheduled();
        schedule.record(t0, Duration::from_millis(150)); // next_allowed_at = t0 + 15 s
        assert_eq!(
            schedule.on_change(t0 + Duration::from_secs(1)),
            Walk::Defer,
            "a change 1 s into the 15 s cooldown is deferred, not walked",
        );
        assert_eq!(
            schedule.on_change(t0 + Duration::from_secs(2)),
            Walk::Defer,
            "a second mid-cooldown change coalesces into the same owed walk",
        );
        assert_eq!(
            schedule.next_allowed(),
            Some(t0 + Duration::from_secs(15)),
            "still exactly one walk owed at the original expiry — coalesced, not doubled or moved",
        );

        // The owed walk runs at expiry and is recorded: that walk reflects the
        // latest coalesced state, so the single owed walk is consumed and the
        // deferral resets — nothing is pending afterward.
        schedule.record(t0 + Duration::from_secs(15), Duration::from_millis(150));
        assert_eq!(
            schedule.next_allowed(),
            None,
            "the owed walk is consumed by the record; no walk is owed afterward",
        );
    }

    #[test]
    fn force_allows_an_immediate_walk_mid_cooldown() {
        // `force` is the manual-refresh escape hatch (Phase 5's `r` key): when a
        // long cooldown is still gating walks, the user can demand an immediate
        // one and bypass the unexpired cooldown. A 150 ms walk gates the next for
        // 100·150 ms = 15 s, so a change 1 s in is normally deferred — but after
        // `force` that SAME mid-cooldown instant must walk now. Instants derive
        // from one base — deterministic and parallel-safe, no sleeping.
        let t0 = Instant::now();

        let mut schedule = WalkSchedule::unscheduled();
        schedule.record(t0, Duration::from_millis(150)); // cooldown until t0 + 15 s
        assert_eq!(
            schedule.on_change(t0 + Duration::from_secs(1)),
            Walk::Defer,
            "a change 1 s into the 15 s cooldown is deferred — we're genuinely mid-cooldown",
        );

        schedule.force();
        assert_eq!(
            schedule.on_change(t0 + Duration::from_secs(1)),
            Walk::Now,
            "after force, the same mid-cooldown instant walks immediately — the gate is lifted",
        );
    }

    #[test]
    fn classify_input_maps_the_r_key_to_force_refresh() {
        // Pressing `r` is the manual-refresh escape hatch: the input classifier
        // must turn an `r` key PRESS into Event::ForceRefresh.
        let r_press = CtEvent::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert!(matches!(classify_input(r_press), Some(Event::ForceRefresh)));
    }

    #[test]
    fn classify_input_handles_keys_and_ignores_releases() {
        // Regression guard for the rest of the classifier's contract once `r`
        // joined it: a key RELEASE is dropped (kitty/Windows emit them and only
        // a press should act), `q` and Ctrl-C still quit, a resize still maps to
        // a repaint, and an unrelated key is ignored.

        // A key release — even of a key we act on — is ignored.
        let r_release = CtEvent::Key(KeyEvent {
            kind: KeyEventKind::Release,
            ..KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)
        });
        assert!(
            classify_input(r_release).is_none(),
            "a key release must be ignored — only a press acts",
        );

        // `q` and Ctrl-C both request a quit.
        let q_press = CtEvent::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(matches!(classify_input(q_press), Some(Event::Quit)));
        let ctrl_c = CtEvent::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(matches!(classify_input(ctrl_c), Some(Event::Quit)));

        // A resize becomes a repaint at the new dimensions.
        assert!(matches!(
            classify_input(CtEvent::Resize(80, 24)),
            Some(Event::Resize)
        ));

        // An unrelated key press is ignored.
        let x_press = CtEvent::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(
            classify_input(x_press).is_none(),
            "an unrelated key must be ignored",
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

    /// No timed refresh: the loop under test is purely event-driven, so a walk
    /// can only come from a filesystem change. Every test that predates the
    /// timed refresh passes this, keeping its subject isolated from it.
    fn no_timed_refresh() -> WalkSchedule {
        WalkSchedule::unscheduled()
    }

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
            files: Vec::new(),
            log: Vec::new(),
            upstream: None,
            operation: None,
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

    /// A clock that steps forward by `step` on every read, from `base`. The loop
    /// reads the clock several times per iteration, so a stepping clock is what
    /// lets a test cross a scheduled deadline without sleeping — deterministic
    /// and parallel-safe, unlike a real timer.
    fn stepping_clock(base: Instant, step: Duration) -> impl Fn() -> Instant {
        let reads = std::cell::Cell::new(0_u32);
        move || {
            let n = reads.get();
            reads.set(n + 1);
            base + step * n
        }
    }

    #[test]
    fn event_loop_walks_on_the_timed_deadline_with_no_filesystem_event() {
        // The timed refresh: with the decay timer off and not one filesystem
        // event, the loop must still re-walk git once the interval elapses.
        // Without this, "next refresh" counts down to nothing.
        let (tx, rx) = mpsc::channel();
        let mut displayed = String::new();
        let mut collects = 0_usize;
        let base = Instant::now();
        // Above FLOOR, so the interval is what sets the deadline: every walk's
        // cooldown is floored at 150 ms, and a sub-floor interval would be
        // stretched to it. The CLI takes whole seconds, so it cannot ask for one.
        let interval = Duration::from_millis(200);
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(base),
            Some(Duration::ZERO),
            WalkSchedule::new(Some(interval), base, Duration::ZERO),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| {
                    // One wake is enough to decide: a decay tick alone never
                    // walks, so any collect at all came from the timed deadline.
                    let _ = tx.send(Event::Quit);
                    frame("timed")
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| Ok(()),
                // Steps past the deadline by the time the loop re-reads it.
                clock: stepping_clock(base, interval * 5),
                // A decay tick on the same cadence, so the loop always wakes:
                // the test must fail when no walk is scheduled, not block.
                next_tick: |_freshest| Some(Duration::from_millis(5)),
            },
        )
        .expect("loop");

        assert_eq!(
            collects, 1,
            "the timed deadline must walk git with no filesystem event",
        );
    }

    #[test]
    fn event_loop_hands_the_render_a_countdown_to_the_next_walk() {
        // The clock in the separator: the render hook must be told how long is
        // left until the next scheduled walk, alongside how stale the snapshot
        // already is. A tick 50s after collection, on a 60s interval, leaves 10s.
        let (tx, rx) = mpsc::channel();
        let mut displayed = String::new();
        let mut seen: Option<FrameTiming> = None;
        let collected_at = Instant::now();
        let clock_at = collected_at + Duration::from_secs(50);
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(collected_at),
            Some(Duration::ZERO),
            WalkSchedule::new(Some(Duration::from_secs(60)), collected_at, Duration::ZERO),
            LoopHooks {
                collect: || Ok(empty_snapshot()),
                render: |_snap: &Snapshot, _dims: Dimensions, timing: FrameTiming| {
                    seen = Some(timing);
                    let _ = tx.send(Event::Quit);
                    frame("tick")
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| Ok(()),
                clock: || clock_at,
                next_tick: |_freshest| Some(Duration::from_millis(5)),
            },
        )
        .expect("loop");

        assert_eq!(
            seen,
            Some(FrameTiming {
                age_offset: Duration::from_secs(50),
                next_refresh_in: Some(Duration::from_secs(10)),
            }),
            "the frame must carry both how stale it is and how long until the next walk",
        );
    }

    #[test]
    fn event_loop_without_an_interval_never_walks_on_a_timeout() {
        // `--refresh-interval 0` keeps today's purely event-driven gsw: a decay
        // tick re-renders from cache and walks nothing, and the frame carries no
        // countdown because no walk is scheduled.
        let (tx, rx) = mpsc::channel();
        let mut displayed = String::new();
        let mut collects = 0_usize;
        let mut seen: Option<FrameTiming> = None;
        let base = Instant::now();
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(base),
            Some(Duration::ZERO),
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, timing: FrameTiming| {
                    seen = Some(timing);
                    let _ = tx.send(Event::Quit);
                    frame("tick")
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| Ok(()),
                clock: stepping_clock(base, Duration::from_secs(60)),
                next_tick: |_freshest| Some(Duration::from_millis(5)),
            },
        )
        .expect("loop");

        assert_eq!(collects, 0, "no interval means no timed walk, ever");
        assert_eq!(
            seen.and_then(|t| t.next_refresh_in),
            None,
            "with nothing scheduled there is no countdown to show",
        );
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
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| frame("frame"),
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
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| {
                    frame("unchanged")
                },
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
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| frame("frame"),
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
            no_timed_refresh(),
            LoopHooks {
                collect: || Ok(empty_snapshot()),
                render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| {
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
            no_timed_refresh(),
            LoopHooks {
                collect: || Ok(empty_snapshot()),
                render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| {
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
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, timing: FrameTiming| {
                    renders += 1;
                    seen_offset = Some(timing.age_offset);
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
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, dims: Dimensions, _timing: FrameTiming| {
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
    fn event_loop_fs_change_reseeds_collected_at() {
        // After a filesystem change re-collects the snapshot, a later decay tick
        // must measure its age offset from the NEW collection time, not the stale
        // seed (Part A). We drive: FsChanged (collected at t+10s), then a tick
        // (clock at t+15s) whose render must see a 5s offset — not 15s.
        let (tx, rx) = mpsc::channel();
        tx.send(Event::FsChanged).expect("queue fs change");

        let base = Instant::now();
        // Clock returns t+10s for the FS collect, then t+15s for the tick.
        let times = [
            base + Duration::from_secs(10),
            base + Duration::from_secs(15),
        ];
        let clock_calls = std::cell::Cell::new(0_usize);

        let mut displayed = String::new();
        let mut offsets: Vec<Duration> = Vec::new();
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(base),
            Some(Duration::ZERO),
            no_timed_refresh(),
            LoopHooks {
                collect: || Ok(empty_snapshot()),
                render: |_snap: &Snapshot, _dims: Dimensions, timing: FrameTiming| {
                    offsets.push(timing.age_offset);
                    // First render is the FS walk (offset 0); the next wake is a
                    // decay tick. End the loop once the tick render has happened.
                    if offsets.len() >= 2 {
                        let _ = tx.send(Event::Quit);
                    }
                    frame(&format!("frame {}", offsets.len()))
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| Ok(()),
                clock: || {
                    let i = clock_calls.get();
                    clock_calls.set(i + 1);
                    times[i.min(times.len() - 1)]
                },
                next_tick: |_freshest| Some(Duration::from_millis(5)),
            },
        )
        .expect("loop");

        assert_eq!(offsets.len(), 2, "expected an FS render then a tick render");
        assert_eq!(
            offsets[0],
            Duration::ZERO,
            "the FS render is always at offset 0"
        );
        assert_eq!(
            offsets[1],
            Duration::from_secs(5),
            "the tick after an FS change must measure its offset from the re-collected \
             time (t+15 - t+10 = 5s), not from the stale seed",
        );
    }

    #[test]
    fn event_loop_throttles_walks_after_an_idle_change() {
        // Part B: the FIRST change after idle walks immediately — the throttle
        // imposes no cooldown until a walk is recorded. That walk costs D = 10 ms,
        // arming a cooldown of 100·D = 1 s (the walk occupies 10 ms of every 1 s
        // it gates — a 1% duty cycle). A second change 100 ms in is DURING the
        // cooldown, so it must be deferred (no git walk). A third change at
        // base + 2 s is past the cooldown, so it walks again. Three FS changes
        // across the cooldown boundary therefore collapse to exactly TWO walks.
        //
        // The injected clock is a short clamped sequence — once exhausted, every
        // further read saturates at base + 2 s (well past expiry) — so the test
        // is fully deterministic and never sleeps on the clock. The three changes
        // are delivered one per loop iteration (via the render hook) so they land
        // in separate debounce windows instead of coalescing into one walk.
        let base = Instant::now();
        let times = [
            base,                              // first walk start
            base + Duration::from_millis(10),  // first walk end → D = 10 ms → 1 s cooldown
            base + Duration::from_millis(100), // second change: mid-cooldown → deferred
            base + Duration::from_secs(2), // third change: past expiry → walks; trailing reads clamp here
        ];
        let clock_calls = std::cell::Cell::new(0_usize);

        let (tx, rx) = mpsc::channel();
        tx.send(Event::FsChanged).expect("queue first change");

        let mut displayed = String::new();
        let mut collects = 0_usize;
        let mut changes_sent = 1_usize;
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(base),
            None, // decay timer off: isolate the throttle from tick behavior
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| {
                    // Deliver the next change in its own iteration so the three
                    // never coalesce; quit once all three have been processed.
                    if changes_sent < 3 {
                        changes_sent += 1;
                        let _ = tx.send(Event::FsChanged);
                    } else {
                        let _ = tx.send(Event::Quit);
                    }
                    frame(&format!("frame {changes_sent}"))
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| Ok(()),
                clock: || {
                    let i = clock_calls.get();
                    clock_calls.set(i + 1);
                    times[i.min(times.len() - 1)]
                },
                next_tick: timer_off,
            },
        )
        .expect("loop");

        assert_eq!(
            collects, 2,
            "three FS changes across the cooldown boundary must walk exactly \
             twice — the mid-cooldown change is deferred",
        );
    }

    #[test]
    fn event_loop_defers_a_cooldown_burst_into_one_walk_at_expiry() {
        // Part B coalescing: an arming walk starts a cooldown; a BURST of FS
        // changes that all land during that cooldown must not each walk — they
        // collapse into exactly ONE deferred walk that the loop runs when the
        // cooldown expires. Across the whole sequence git is walked twice: the
        // arming walk, then the single owed walk. The collect counter proves the
        // burst added exactly one walk, not one per event.
        //
        // The arming walk costs D = 10 ms → a 1 s cooldown (expiry = base + 1 s).
        // The burst lands at base + 100 ms (mid-cooldown → deferred). The owed
        // walk fires at base + 2 s, past expiry, on a zero-length timeout (the
        // injected clock reports "now" already past expiry, so the loop never
        // sleeps it out). A 5 ms decay timer is enabled so the loop still wakes
        // periodically — proving the owed walk runs at the cooldown's expiry, not
        // merely on the next tick.
        let base = Instant::now();
        let times = [
            base,                              // arming walk start
            base + Duration::from_millis(10),  // arming walk end → D = 10 ms → 1 s cooldown
            base + Duration::from_millis(100), // burst: mid-cooldown → deferred
            base + Duration::from_secs(2), // owed-walk wake: past expiry; trailing reads clamp here
        ];
        let clock_calls = std::cell::Cell::new(0_usize);

        let (tx, rx) = mpsc::channel();
        tx.send(Event::FsChanged).expect("queue arming change");

        let mut displayed = String::new();
        let mut collects = 0_usize;
        let mut stage = 0_usize;
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(base),
            Some(Duration::ZERO),
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| {
                    stage += 1;
                    match stage {
                        // After the arming walk: fire a burst of three changes
                        // that coalesce inside one debounce window.
                        1 => {
                            for _ in 0..3 {
                                let _ = tx.send(Event::FsChanged);
                            }
                        }
                        // The deferred re-render of the coalesced burst: do
                        // nothing, let the cooldown expire so the owed walk fires.
                        2 => {}
                        // The owed walk at expiry (or, if throttling were absent,
                        // a tick): end the loop.
                        _ => {
                            let _ = tx.send(Event::Quit);
                        }
                    }
                    frame(&format!("frame {stage}"))
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| Ok(()),
                clock: || {
                    let i = clock_calls.get();
                    clock_calls.set(i + 1);
                    times[i.min(times.len() - 1)]
                },
                next_tick: |_freshest| Some(Duration::from_millis(5)),
            },
        )
        .expect("loop");

        assert_eq!(
            collects, 2,
            "an arming walk plus a mid-cooldown burst must walk exactly twice: \
             the arming walk and one coalesced owed walk at the cooldown's expiry",
        );
    }

    #[test]
    fn event_loop_decay_tick_during_cooldown_does_not_walk() {
        // Part A and Part B compose: while a cooldown is active (a deferred walk
        // is owed), a plain decay tick — which fires on the SHORTER decay cadence,
        // before the cooldown expires — must re-render the cached snapshot WITHOUT
        // walking git. Only the owed walk, once its cooldown has actually expired,
        // walks. A constant injected clock pins "now" at `base`, so the 150 ms
        // FLOOR cooldown (the arming walk costs 0) is never reached: the arming
        // walk is the ONLY walk; the deferred change and the decay tick during the
        // cooldown add none.
        let base = Instant::now();

        let (tx, rx) = mpsc::channel();
        tx.send(Event::FsChanged).expect("queue arming change");

        let mut displayed = String::new();
        let mut collects = 0_usize;
        let mut stage = 0_usize;
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(base),
            Some(Duration::ZERO),
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| {
                    stage += 1;
                    match stage {
                        // After the arming walk: one FS change lands mid-cooldown
                        // (it is deferred, setting the dirty flag).
                        1 => {
                            let _ = tx.send(Event::FsChanged);
                        }
                        // The deferred re-render: do nothing, let a decay tick fire
                        // while the cooldown is still active.
                        2 => {}
                        // The decay tick during the cooldown (or, if the guard were
                        // missing, a wrongful owed walk): end the loop.
                        _ => {
                            let _ = tx.send(Event::Quit);
                        }
                    }
                    frame(&format!("frame {stage}"))
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| Ok(()),
                clock: || base,
                next_tick: |_freshest| Some(Duration::from_millis(5)),
            },
        )
        .expect("loop");

        assert_eq!(
            collects, 1,
            "a decay tick that fires during an active cooldown must re-render \
             from cache without walking git — only the arming walk runs",
        );
    }

    #[test]
    fn event_loop_force_refresh_walks_immediately_mid_cooldown() {
        // Phase 5: pressing `r` (Event::ForceRefresh) mid-cooldown must force an
        // immediate git walk, bypassing the active cooldown. An arming FS walk costs
        // D = 10 ms, arming a 100·D = 1 s cooldown (expiry = base + 1 s). A force
        // refresh arrives at base + 100 ms — genuinely mid-cooldown — and must walk
        // anyway. Across the sequence git is therefore walked exactly TWICE: the
        // arming walk and the forced walk. The injected clock is a short clamped
        // sequence (trailing reads saturate at the last entry), so the test is
        // deterministic and never sleeps on the clock.
        let base = Instant::now();
        let times = [
            base,                              // arming walk start
            base + Duration::from_millis(10),  // arming walk end → D = 10 ms → 1 s cooldown
            base + Duration::from_millis(100), // force-refresh wake: mid-cooldown
            base + Duration::from_millis(110), // forced walk end
        ];
        let clock_calls = std::cell::Cell::new(0_usize);

        let (tx, rx) = mpsc::channel();
        tx.send(Event::FsChanged).expect("queue arming change");

        let mut displayed = String::new();
        let mut collects = 0_usize;
        let mut stage = 0_usize;
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(base),
            None, // decay timer off: isolate the throttle from tick behavior
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| {
                    stage += 1;
                    match stage {
                        // After the arming walk: a manual refresh lands mid-cooldown.
                        1 => {
                            let _ = tx.send(Event::ForceRefresh);
                        }
                        // The forced walk's render (or, if force were unwired, the
                        // deferred re-render): end the loop.
                        _ => {
                            let _ = tx.send(Event::Quit);
                        }
                    }
                    frame(&format!("frame {stage}"))
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| Ok(()),
                clock: || {
                    let i = clock_calls.get();
                    clock_calls.set(i + 1);
                    times[i.min(times.len() - 1)]
                },
                next_tick: timer_off,
            },
        )
        .expect("loop");

        assert_eq!(
            collects, 2,
            "a force refresh mid-cooldown must walk despite the unexpired cooldown: \
             the arming walk plus the forced walk",
        );
    }

    #[test]
    fn event_loop_force_refresh_rearms_throttle_from_fresh_measurement() {
        // Phase 5 acceptance: a forced walk must re-measure its cost and re-arm the
        // cooldown from it, exactly like an ordinary FS walk — so a *subsequent* FS
        // change is throttled against the FRESH measurement. The forced walk costs
        // D = 10 ms, arming a 100·D = 1 s cooldown. An FS change then lands at
        // base + 100 ms, genuinely mid-cooldown, and must be DEFERRED — proving the
        // forced walk re-armed the throttle. Across the sequence git is walked
        // exactly ONCE (the forced walk); if `force` had failed to re-arm, the FS
        // change would have walked too and collects would be 2. The injected clock
        // is a short clamped sequence, so the test is deterministic and never sleeps.
        let base = Instant::now();
        let times = [
            base,                              // forced walk start
            base + Duration::from_millis(10),  // forced walk end → D = 10 ms → 1 s cooldown
            base + Duration::from_millis(100), // FS-change wake: mid-cooldown → deferred
        ];
        let clock_calls = std::cell::Cell::new(0_usize);

        let (tx, rx) = mpsc::channel();
        tx.send(Event::ForceRefresh).expect("queue forced refresh");

        let mut displayed = String::new();
        let mut collects = 0_usize;
        let mut stage = 0_usize;
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(base),
            None, // decay timer off: isolate the throttle from tick behavior
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| {
                    stage += 1;
                    match stage {
                        // After the forced walk: an FS change lands mid-cooldown.
                        1 => {
                            let _ = tx.send(Event::FsChanged);
                        }
                        // The deferred re-render of that FS change: end the loop.
                        _ => {
                            let _ = tx.send(Event::Quit);
                        }
                    }
                    frame(&format!("frame {stage}"))
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| Ok(()),
                clock: || {
                    let i = clock_calls.get();
                    clock_calls.set(i + 1);
                    times[i.min(times.len() - 1)]
                },
                next_tick: timer_off,
            },
        )
        .expect("loop");

        assert_eq!(
            collects, 1,
            "the forced walk re-arms the cooldown from its fresh cost, so a later \
             mid-cooldown FS change is deferred: only the forced walk runs. If force \
             failed to re-arm, the FS change would walk too and collects would be 2.",
        );
    }

    #[test]
    fn event_loop_force_refresh_on_idle_walks_once() {
        // Phase 5 acceptance: pressing `r` on a clean/idle loop simply walks once
        // and repaints once — no error, no double walk. With a constant clock and
        // the decay timer off, the single queued ForceRefresh drives exactly one
        // collect and one paint before the render hook quits.
        let base = Instant::now();

        let (tx, rx) = mpsc::channel();
        tx.send(Event::ForceRefresh).expect("queue forced refresh");

        let mut displayed = String::new();
        let mut collects = 0_usize;
        let mut paints = 0_usize;
        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(base),
            None, // decay timer off: isolate the forced walk from tick behavior
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Ok(empty_snapshot())
                },
                render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| {
                    // End the loop right after the forced walk's render.
                    let _ = tx.send(Event::Quit);
                    frame("forced")
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| {
                    paints += 1;
                    Ok(())
                },
                clock: || base,
                next_tick: timer_off,
            },
        )
        .expect("loop");

        assert_eq!(
            collects, 1,
            "a forced refresh on an idle loop walks git once"
        );
        assert_eq!(paints, 1, "the forced walk repaints exactly once");
    }

    #[test]
    fn event_loop_absorbs_a_failed_walk_and_keeps_the_last_good_frame() {
        // A walk can fail for reasons that have nothing to do with the user:
        // `git gc` swapping the ref store out from under us, a worktree being
        // pruned, `.git` momentarily renamed by a tool. `RepoHandle::reopened`
        // already degrades to the handle in hand rather than blanking the
        // screen — but that fallback only matters if the loop survives the
        // *status walk* failing too. So a failed collect must not end watch
        // mode; it must re-render the LAST GOOD snapshot, and it must render it
        // at its TRUE age: `collected_at` may not advance for a collection that
        // never happened, or the monitor would show a stale repo with every
        // file age reset to "just now" — lying about freshness precisely when
        // it is least fresh. With the cache collected at `base` and the clock
        // 50 s later, the render hook must see the cached snapshot and a 50 s
        // offset, and the loop must return `Ok`.
        let base = Instant::now();
        let clock_at = base + Duration::from_secs(50);

        let (tx, rx) = mpsc::channel();
        tx.send(Event::FsChanged).expect("queue fs change");
        drop(tx);

        let mut cache = seeded_cache(base);
        cache.snapshot.branch = "last-good".to_string();

        let mut displayed = String::new();
        let mut collects = 0_usize;
        let mut rendered: Vec<(String, Duration)> = Vec::new();
        let result = event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            cache,
            None, // decay timer off: isolate the failed walk from tick behavior
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    // The exact shape `collect_snapshot` produces when the ref
                    // store has gone missing mid-walk.
                    Err(anyhow::anyhow!(
                        "status iter: The reference 'HEAD' did not exist"
                    ))
                },
                render: |snap: &Snapshot, _dims: Dimensions, timing: FrameTiming| {
                    rendered.push((snap.branch.clone(), timing.age_offset));
                    frame("last good frame")
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| Ok(()),
                clock: || clock_at,
                next_tick: timer_off,
            },
        );

        assert!(
            result.is_ok(),
            "a failed walk must be absorbed, not propagated out of watch mode: {result:?}",
        );
        assert_eq!(collects, 1, "the failed walk still ran exactly once");
        assert_eq!(
            rendered.len(),
            1,
            "a failed walk must still produce a frame — the screen never blanks",
        );
        assert_eq!(
            rendered[0].0, "last-good",
            "the frame after a failed walk must come from the CACHED snapshot",
        );
        assert_eq!(
            rendered[0].1,
            Duration::from_secs(50),
            "a failed walk must not advance collected_at: the cached snapshot has \
             to keep aging truthfully (now - collected_at = 50s), not reset to 0",
        );
        assert_eq!(displayed, "last good frame");
    }

    #[test]
    fn event_loop_keeps_throttling_when_every_walk_fails() {
        // Absorbing a failed walk must not turn the loop into a hot spin. A
        // repository that is unreadable for a while (mid-`gc`, mid-checkout)
        // will fail *every* walk, and each failure still costs a real status
        // traversal — so the failure path has to feed the throttle exactly like
        // the success path does, or a deleted repo would burn a core retrying.
        //
        // Same shape as `event_loop_throttles_walks_after_an_idle_change`, but
        // every collect fails: three FS changes across the cooldown boundary
        // must still collapse to exactly TWO walks. The first walk costs
        // D = 10 ms, arming a 100·D = 1 s cooldown; the change at base + 100 ms
        // lands mid-cooldown and is deferred; the change at base + 2 s is past
        // expiry and walks. The injected clock is a short clamped sequence, so
        // the test never sleeps and stays deterministic.
        let base = Instant::now();
        let times = [
            base,                              // first (failing) walk start
            base + Duration::from_millis(10),  // first walk end → D = 10 ms → 1 s cooldown
            base + Duration::from_millis(100), // second change: mid-cooldown → deferred
            base + Duration::from_secs(2), // third change: past expiry → walks; trailing reads clamp here
        ];
        let clock_calls = std::cell::Cell::new(0_usize);

        let (tx, rx) = mpsc::channel();
        tx.send(Event::FsChanged).expect("queue first change");

        let mut displayed = String::new();
        let mut collects = 0_usize;
        let mut changes_sent = 1_usize;
        let result = event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            seeded_cache(base),
            None, // decay timer off: isolate the throttle from tick behavior
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    Err(anyhow::anyhow!("status platform: repository is gone"))
                },
                render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| {
                    // Deliver the next change in its own iteration so the three
                    // never coalesce; quit once all three have been processed.
                    if changes_sent < 3 {
                        changes_sent += 1;
                        let _ = tx.send(Event::FsChanged);
                    } else {
                        let _ = tx.send(Event::Quit);
                    }
                    frame(&format!("frame {changes_sent}"))
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| Ok(()),
                clock: || {
                    let i = clock_calls.get();
                    clock_calls.set(i + 1);
                    times[i.min(times.len() - 1)]
                },
                next_tick: timer_off,
            },
        );

        assert!(
            result.is_ok(),
            "failing walks must keep the loop alive: {result:?}",
        );
        assert_eq!(
            collects, 2,
            "a failing walk must arm the cooldown exactly like a successful one: \
             three FS changes across the boundary walk twice, not three times",
        );
    }

    #[test]
    fn event_loop_recovers_and_reseeds_age_after_a_failed_walk() {
        // Absorbing a failed walk is only half the promise; the other half is
        // that "the next event retries" and the monitor visibly RECOVERS. The
        // two failure tests above never let a walk succeed afterward, so
        // neither can tell a loop that retries from one that has quietly
        // wedged itself on the last good frame forever. This drives the whole
        // arc: fail, then succeed.
        //
        // The recovery that matters is the age display. While the repo is
        // unreadable the cached frame keeps aging truthfully (50 s here); the
        // moment a walk succeeds, the fresh snapshot must render at age zero
        // AND `collected_at` must be re-seeded to that walk's start, so every
        // later re-render ages from the new walk rather than from the
        // long-stale seed. The final resize is what makes the re-seed directly
        // observable: it re-renders the cache with no walk, and its offset is
        // `now - collected_at` — 3 s off the second walk, not 55 s off the
        // original seed. Without that third frame a "do not re-seed after a
        // failure" regression would leave the loop stuck reporting the stale
        // seed's age while every assertion still passed.
        //
        // Clock reads, in order (a clamped sequence, so the test never sleeps
        // on real time): iteration 1's `now` and the failed walk's cost end;
        // iteration 2's `now` and the successful walk's cost end; iteration
        // 3's `now`. No read for a deferred deadline — nothing lands
        // mid-cooldown, so the throttle is never dirty.
        let base = Instant::now();
        let times = [
            base + Duration::from_secs(50), // failed walk start (cache is 50 s stale)
            base + Duration::from_millis(50_010), // failed walk end → D = 10 ms → 1 s cooldown
            base + Duration::from_secs(52), // retry: past expiry → walks, and succeeds
            base + Duration::from_millis(52_010), // successful walk end
            base + Duration::from_secs(55), // resize re-render; trailing reads clamp here
        ];
        let clock_calls = std::cell::Cell::new(0_usize);

        let (tx, rx) = mpsc::channel();
        tx.send(Event::FsChanged).expect("queue first change");

        let mut cache = seeded_cache(base);
        cache.snapshot.branch = "last-good".to_string();

        let mut displayed = String::new();
        let mut collects = 0_usize;
        let mut rendered: Vec<(String, Duration)> = Vec::new();
        let result = event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            cache,
            None, // decay timer off: isolate recovery from tick behavior
            no_timed_refresh(),
            LoopHooks {
                collect: || {
                    collects += 1;
                    if collects == 1 {
                        // The repo is momentarily unreadable — mid-`gc`, say.
                        Err(anyhow::anyhow!(
                            "status iter: The reference 'HEAD' did not exist"
                        ))
                    } else {
                        // ...and then it isn't. A distinguishable branch name
                        // proves the frame came from THIS walk, not the cache.
                        let mut fresh = empty_snapshot();
                        fresh.branch = "recovered".to_string();
                        Ok(fresh)
                    }
                },
                render: |snap: &Snapshot, _dims: Dimensions, timing: FrameTiming| {
                    rendered.push((snap.branch.clone(), timing.age_offset));
                    match rendered.len() {
                        // Deliver the retry in its own iteration so it lands in
                        // a separate debounce window instead of coalescing.
                        1 => {
                            let _ = tx.send(Event::FsChanged);
                            frame("stale")
                        }
                        // A resize forces one more cached re-render (no walk),
                        // whose age offset is read straight off `collected_at`.
                        // The quit rides the same debounce window, so the loop
                        // renders that frame and then stops.
                        2 => {
                            let _ = tx.send(Event::Resize);
                            let _ = tx.send(Event::Quit);
                            frame("fresh")
                        }
                        _ => frame("aged"),
                    }
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| Ok(()),
                clock: || {
                    let i = clock_calls.get();
                    clock_calls.set(i + 1);
                    times[i.min(times.len() - 1)]
                },
                next_tick: timer_off,
            },
        );

        assert!(
            result.is_ok(),
            "a failure followed by a success must run to a clean stop: {result:?}",
        );
        assert_eq!(collects, 2, "the failed walk must be retried exactly once");
        assert_eq!(
            rendered.len(),
            3,
            "expected three frames: the absorbed failure, the recovery, and the \
             cached re-render that exposes the re-seeded age",
        );
        assert_eq!(
            rendered[0],
            ("last-good".to_string(), Duration::from_secs(50)),
            "while the walk is failing the CACHED snapshot keeps aging truthfully",
        );
        assert_eq!(
            rendered[1],
            ("recovered".to_string(), Duration::ZERO),
            "the walk that succeeds after a failure must render its FRESH snapshot \
             at age zero — recovery, not a permanently frozen last-good frame",
        );
        assert_eq!(
            rendered[2],
            ("recovered".to_string(), Duration::from_secs(3)),
            "the successful walk must re-seed collected_at to its own start: the \
             re-render 3s later ages from that walk (55s - 52s), not from the \
             50s-stale seed the failure left in place",
        );
        assert_eq!(displayed, "aged");
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

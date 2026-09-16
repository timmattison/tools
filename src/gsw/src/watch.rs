//! Watch mode: event loop, terminal lifecycle, and the pure helpers that drive
//! refresh decisions.
//!
//! The watch loop owns all rendering on a single thread and is fed by a
//! `std::sync::mpsc` channel. Every *decision* — which terminal dimensions to
//! render for, which filesystem events matter, and how fast the decay timer
//! should tick — lives in a pure, terminal-free function here
//! ([`resolve_dimensions`], [`should_react`], [`next_tick`]) so it can be
//! unit-tested without a pty.

use std::cell::RefCell;
use std::ffi::OsString;
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

use crate::conflicts::ConflictsWorker;
use crate::push::{PushCommand, PushUi};
use crate::render::Snapshot;
use crate::repo::RepoHandle;
use crate::worktrees::{
    head_label, list_worktrees, worktree_paths, WorktreeEntry, WorktreeList, WorktreePath,
};
use crate::{collect_snapshot, render_frame, render_list_frame, FrameTiming, Render, RenderConfig};
use termwindow::{
    effective_terminal_height, effective_terminal_width, DEFAULT_TERMINAL_HEIGHT,
    DEFAULT_TERMINAL_WIDTH,
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
    /// Width queried from `termsize::stdout_size` (the ioctl), when a TTY is
    /// present.
    pub tty_width: Option<usize>,
    /// Height queried from `termsize::stdout_size` (the ioctl), when a TTY is
    /// present.
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
///   straight from `termsize::stdout_size`, ignores `COLUMNS`/`LINES`, and reserves
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
            // from termsize::stdout_size, and reserve no wrapper chrome. The one-cell
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
///
/// A quiet channel is the only thing that ends the drain for every event but
/// one. [`Event::PushOutput`] is the exception, and [`PUSH_DRAIN_BUDGET`] says
/// why.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// How long the drain goes on absorbing a running push's output before it
/// leaves and paints, whatever is still queued behind it.
///
/// [`DEBOUNCE`] ends a drain on a channel that has gone quiet, which is the
/// right rule for a filesystem burst: a burst is finite, and its end is what
/// says the repository has settled. A push's output is neither. A pre-push
/// hook that builds and tests a workspace prints far faster than one line per
/// [`DEBOUNCE`], for minutes on end, so a drain with no deadline of its own
/// would absorb the whole build and paint once when it finished — the window
/// empty and the `Pushing…` age frozen throughout, which is precisely the
/// frozen screen the output window exists to answer.
///
/// The accepted cost is one repaint per 250 ms while a push is streaming, and
/// only while one is: the deadline is armed by the first
/// [`Event::PushOutput`] of a wake and the clock is not read at all on a wake
/// that sees none, so a filesystem burst still coalesces byte for byte as it
/// did before this constant existed. 250 ms is above the ~100 ms at which a
/// screen stops reading as live and far below the point at which a reader
/// would call it stuck, and it is deliberately longer than [`DEBOUNCE`]: a
/// budget shorter than the debounce window would repaint on lines a single
/// window could have carried together.
const PUSH_DRAIN_BUDGET: Duration = Duration::from_millis(250);

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
    /// A key was pressed. Deliberately **unclassified**: what a key means
    /// depends on whether a confirmation is on screen, and only the loop knows
    /// that. Sending the raw key and classifying it in the loop is what makes
    /// the mode and the key arrive in the same order the user pressed them —
    /// were the reader thread to classify against a shared mode flag instead,
    /// a fast `p` then `y` could be read while the flag still said
    /// [`InputMode::Normal`], and the `y` would be silently dropped.
    Key(KeyEvent),
    /// The user asked to quit (`q` or Ctrl-C).
    Quit,
    /// The user asked to force an immediate refresh (`r`), bypassing the
    /// throttle cooldown.
    ForceRefresh,
    /// The user asked to push (`p`) — show the confirmation, or say why there
    /// is nothing to confirm.
    PushRequested,
    /// The user confirmed the push at the prompt (`y` or Enter).
    PushConfirmed,
    /// The user declined the push at the prompt (`n`, Esc, or `q`).
    PushCancelled,
    /// A running push wrote a line. Carried one line at a time rather than as
    /// a batch at the end, because the point of it is to arrive early: a
    /// pre-push hook can hold the push for minutes, and a batch would land
    /// when the wait it explains is already over.
    ///
    /// Arriving early is only half of it — the loop must also *leave* its
    /// debounce drain to paint what arrived, and this is the only event that
    /// can go on producing for the length of the push. [`PUSH_DRAIN_BUDGET`]
    /// is what stops the drain re-batching what the runner deliberately did
    /// not.
    PushOutput(String),
    /// A push that was running has finished, either way.
    ///
    /// Always arrives after the last [`Event::PushOutput`] of the same push.
    /// The runner joins its reader threads before it reports, so every line is
    /// already on this channel by the time the outcome is sent — which is what
    /// keeps a late line from reopening a window the outcome just closed.
    PushFinished(crate::push::PushOutcome),
    /// A rebase onto the base, or a merge of the base, has finished — either
    /// way.
    ///
    /// It carries the outcome a push carries, because the row reports the two
    /// the same way: a command that either worked or wrote a reason. What it
    /// does not share is what happens next. **Every outcome of this one walks
    /// the repository**, and a failed push walks nothing: a push that failed
    /// changed nothing to re-read, and a rebase that failed stopped in the
    /// middle of rewriting the branch. The `⚠ rebase` row of the header is what
    /// says so, and only a walk puts it there.
    BaseUpdateFinished(crate::push::PushOutcome),
    /// A key press with no other meaning. Clears a status message if one is on
    /// screen and does nothing otherwise, which is what keeps a push error up
    /// until the user has actually looked at the screen.
    Dismiss,
    /// The user asked to go to the home worktree (Up): the worktree where gsw
    /// started.
    GoHome,
    /// The user asked to go to the previous worktree in path order (Left).
    GoPrevious,
    /// The user asked to go to the next worktree in path order (Right).
    GoNext,
    /// The user asked for the list of the worktrees (Down).
    ///
    /// The loop reads the list again at each press, and opens it only in a
    /// pane that has a row for it, so Enter never chooses a row that the user
    /// did not see.
    OpenList,
    /// The user moved the cursor of the open list one row up (Up).
    ListUp,
    /// The user moved the cursor of the open list one row down (Down).
    ListDown,
    /// The user chose the worktree under the cursor of the open list (Enter).
    ListGo,
    /// The user closed the open list (Esc or `q`). The watch stays on the
    /// worktree that it showed before the list opened.
    ListClose,
    /// The user asked for the issue of the branch (`G`).
    ///
    /// Only [`classify_input`] makes one, and it makes one only where the
    /// command exists — so the loop never receives a request it cannot serve.
    IssueRequested,
    /// The probe found the command, and this is its name.
    ///
    /// Sent only where the command exists. A shell that says no, a shell that
    /// cannot be started, and a shell that never answers all send nothing, so
    /// the key stays unbound and silent.
    IssueCommandFound(crate::shell::ShellCommand),
    /// A run of the issue command has finished, either way.
    IssueFinished {
        /// The generation that [`LoopHooks::start_issue`] was given for this
        /// run: the generation of the worktree where the run started.
        generation: Generation,
        /// What the run did.
        outcome: crate::issue::IssueOutcome,
    },
    /// The user asked to measure a rebase and a merge against the default
    /// branch (`m`).
    ///
    /// The loop starts a run only when no run is in flight. So a press during
    /// a run does nothing, and [`ConflictsRun`] says why.
    ConflictsRequested,
    /// A run of `m` knows the branch it measures against, and this is its
    /// name.
    ///
    /// Sent before the first replay starts. A run that is refused, or that
    /// finds HEAD on the default branch, starts no replay and sends none of
    /// these, because its outcome follows at once.
    ConflictsStarted {
        /// The generation that [`LoopHooks::start_conflicts`] was given for
        /// this run: the generation of the worktree where the run started.
        generation: Generation,
        /// The branch the run measures against.
        branch: String,
    },
    /// A run of `m` has ended, and this is what it found.
    ///
    /// Always arrives after the [`Event::ConflictsStarted`] of the same run,
    /// because one thread sends both on this one channel. A run that a quit
    /// abandoned sends none, because nobody reads it.
    ConflictsFinished {
        /// The generation that [`LoopHooks::start_conflicts`] was given for
        /// this run: the generation of the worktree where the run started.
        generation: Generation,
        /// What the run found.
        outcome: crate::conflicts::ConflictsOutcome,
    },
}

/// What keys mean right now.
///
/// The loop owns this, because the loop is the only place that knows what is on
/// screen. It is an input *mode* rather than a set of booleans so the key table
/// is total: every mode answers every key, and a mode added later cannot
/// silently inherit another's bindings.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum InputMode {
    /// Nothing is being asked. The monitor's ordinary keys apply. Up, Left,
    /// and Right move the watch between the worktrees, and Down opens the list
    /// of the worktrees.
    Normal,
    /// A push confirmation is on screen and is waiting for an answer. The
    /// arrow keys never answer it.
    Confirm,
    /// A push is running. `p` is inert here, so two pushes cannot overlap. The
    /// arrow keys are inert too, because the window under the frame belongs to
    /// the worktree that pushes, and a push that succeeds walks that worktree
    /// again.
    Pushing,
    /// The list of the worktrees is open. Up and Down move its cursor, Enter
    /// goes to the worktree under the cursor, and Esc and `q` close it. Every
    /// other key does nothing at all, because the list takes the pane, and a
    /// key that acts on the frame acts on a frame that the user cannot see.
    List,
}

/// Whether the `G` key has a command behind it.
///
/// A separate value from [`InputMode`], because it is not a mode: it changes
/// what one key does and it changes no other key. It is a parameter of
/// [`classify_input`] rather than a flag that function reads, so the absent
/// case is testable with no shell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum IssueKey {
    /// The command exists. `G` asks for the issue.
    Bound,
    /// The command does not exist, or the probe has not answered yet. `G`
    /// does nothing, the way an unbound key does.
    Unbound,
}

/// What one press of `G` does.
///
/// One value rather than an `Option` beside a flag, because the loop does one
/// thing for each answer. A press that runs and a press that asks are then two
/// arms of one match, and no second question decides between them.
#[derive(Debug, PartialEq, Eq)]
enum IssuePress {
    /// Run this command.
    Run(crate::shell::ShellCommand),
    /// Put this under the frame, and wait for a second press.
    Ask(String),
    /// Nothing at all.
    Nothing,
}

/// The `G` key's own state, for the life of one watch-mode run.
///
/// Four facts, and they are one value because one key reads all of them: the
/// command the probe found, whether a run of it is in flight, where the person
/// who reads this screen sits, and whether a message that asks for a second
/// press stands. None of them is an [`InputMode`], because each changes what
/// this one key does and changes no other key.
struct IssueRun {
    /// The command the probe found. `None` until the probe answers, and
    /// forever where it found none or where the feature is off.
    command: Option<crate::shell::ShellCommand>,
    /// Whether a run is in flight. One run at a time: a browser opening twice
    /// is two tabs nobody asked for.
    running: bool,
    /// Where the person who reads this screen sits. Decided once, at start.
    session: crate::remote::Session,
    /// When the message that asks for a second press took the row, or `None`
    /// when no such message stands.
    ///
    /// The message on screen is the armed state, so this holds an instant and
    /// not a flag: the message goes away by itself after
    /// [`crate::push::STATUS_LIFETIME`], and the arming goes with it.
    ///
    /// **The instant is the one the message took the row at, and not the one
    /// the key was pressed at.** The row is not always free — a question and a
    /// push in flight own it, and [`crate::push::PushUi::post_notice`] holds a
    /// message that arrives then. A held message is a message nobody has read,
    /// so an arming that started at the press would offer a second press
    /// against a warning still sitting in the queue. [`IssueRun::arm`] is
    /// therefore called by [`absorb`] and only for the answer that says the
    /// words reached the screen.
    armed: Option<Instant>,
}

impl IssueRun {
    /// Nothing found yet, nothing running, and nothing asked.
    fn new(session: crate::remote::Session) -> Self {
        Self {
            command: None,
            running: false,
            session,
            armed: None,
        }
    }

    /// Whether `G` has a command behind it.
    fn key(&self) -> IssueKey {
        if self.command.is_some() {
            IssueKey::Bound
        } else {
            IssueKey::Unbound
        }
    }

    /// Keep the command the probe found.
    fn found(&mut self, command: crate::shell::ShellCommand) {
        self.command = Some(command);
    }

    /// What one press of `G` does now.
    ///
    /// The questions come in the order of the rules, and each one settles the
    /// press on its own.
    ///
    /// A run in flight answers nothing and changes nothing else, because one
    /// run at a time is the rule and a message that asked for a second press
    /// would ask for a press that runs nothing. No command answers nothing
    /// too, which is the silence of an unbound key.
    ///
    /// A local shell runs the command, because the browser opens where the
    /// person sits. So does a remote shell whose message still stands: that
    /// message is what the user read, and this press is the answer to it.
    ///
    /// Everything else is a first press on a remote shell. It asks, and
    /// [`absorb`] puts the words it returns under the frame.
    ///
    /// **The asking and the arming are two halves, and this half only asks.**
    /// The message is the armed state, and whether the message reaches the
    /// screen is the row's answer rather than this one's — see
    /// [`IssueRun::armed`]. So [`absorb`] posts the words, reads that answer,
    /// and calls [`IssueRun::arm`] for the answer that says the row took them.
    fn press(&mut self, now: Instant) -> IssuePress {
        if self.running {
            return IssuePress::Nothing;
        }
        let Some(command) = self.command.clone() else {
            return IssuePress::Nothing;
        };
        if self.session == crate::remote::Session::Local || self.is_armed(now) {
            // The arming goes as the run starts. An arming that outlived the
            // run it was given for would let the next press open a browser on
            // the wrong machine, with nobody asked a second time for it.
            self.armed = None;
            self.running = true;
            return IssuePress::Run(command);
        }
        IssuePress::Ask(format!(
            "remote shell — press G again to run {}",
            command.name()
        ))
    }

    /// The message that asks for a second press reached the row at `now`.
    ///
    /// The one door into the armed state, and [`absorb`] is its one caller:
    /// the row answers whether the words are on the screen, and only that
    /// answer arms the key. [`IssueRun::armed`] says why the instant is this
    /// one and not the instant of the press.
    fn arm(&mut self, now: Instant) {
        self.armed = Some(now);
    }

    /// Whether the message that asks for a second press still stands.
    ///
    /// The message goes off the screen one [`crate::push::STATUS_LIFETIME`]
    /// after it took the row, and [`IssueRun::armed`] holds that same instant,
    /// so the arming ends at that same moment. The screen and the key then say
    /// one thing: a `G` a minute later asks again.
    ///
    /// Saturating for the reason the age of a status message in
    /// [`crate::push`] is: `now` comes from the loop's injected clock, and a
    /// clock a test drives backwards reports the zero age it plainly has
    /// rather than underflowing.
    fn is_armed(&self, now: Instant) -> bool {
        self.armed.is_some_and(|posted_at| {
            now.saturating_duration_since(posted_at) < crate::push::STATUS_LIFETIME
        })
    }

    /// A key other than `G` takes the arming away.
    ///
    /// That key also takes the message off the screen, and the message is the
    /// armed state. The two go together, so a `G` after such a key is a first
    /// press again.
    fn disarm(&mut self) {
        self.armed = None;
    }

    /// A run has ended, so `G` means something again.
    fn finished(&mut self) {
        self.running = false;
    }
}

/// What one press of `m` does.
///
/// An enum and not a `bool`, for the reason [`IssuePress`] gives: the loop does
/// one thing for each answer, and each answer is one arm of one match.
#[derive(Debug, PartialEq, Eq)]
enum ConflictsPress {
    /// Start a run.
    Start,
    /// Nothing at all, because a run is in flight.
    Nothing,
}

/// The `m` key's own state, for the life of one watch-mode run.
///
/// Only one measurement can be in flight in one gsw process, so a user who
/// presses `m` ten times starts one run. The loop owns this value, and the
/// thread that measures never reads it or writes it. The single thread of the
/// loop is what makes the check and the set in [`ConflictsRun::press`] one
/// operation, so no lock is necessary.
///
/// The loop does not ask the worker whether a thread is alive. A run hands over
/// its outcome a moment before its thread ends, so that answer can say a run
/// is in flight just after the outcome arrived, and a press then would do
/// nothing for no reason the user can see.
struct ConflictsRun {
    /// Whether a run is in flight. Set by the press that starts the run, and
    /// cleared by the outcome of that run.
    running: bool,
}

impl ConflictsRun {
    /// No run in flight.
    fn new() -> Self {
        Self { running: false }
    }

    /// What one press of `m` does now.
    ///
    /// The press that starts a run marks the run in flight in the same step,
    /// so every press after it answers nothing until
    /// [`ConflictsRun::finished`].
    fn press(&mut self) -> ConflictsPress {
        if self.running {
            return ConflictsPress::Nothing;
        }
        self.running = true;
        ConflictsPress::Start
    }

    /// The outcome of the run arrived, so `m` starts a run again.
    fn finished(&mut self) {
        self.running = false;
    }
}

/// How many times the loop has switched the worktree it watches: a stamp on
/// the work that the loop starts.
///
/// A run of `G` or `m` continues after a switch, in the worktree where it
/// started, so its outcome can arrive when the frame shows another worktree.
/// A line under the frame must describe the worktree in the frame. So each run
/// keeps the generation of its press, its events carry that generation back,
/// and the loop compares it with its own. An outcome with an old generation
/// frees its key and posts nothing.
///
/// A push carries no generation, because no switch happens while a push runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Generation(u64);

impl Generation {
    /// The generation after one more switch.
    ///
    /// A plain addition. A `u64` does not overflow in the life of a process:
    /// at one switch per microsecond, that takes more than 500 000 years.
    fn next(self) -> Self {
        Self(self.0 + 1)
    }
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
/// Production calls it through [`Watched::walk`], from two places, and neither
/// lets that error out of watch mode. The `collect` hook of [`event_loop`]
/// keeps the last good snapshot, re-renders it at its true (still-advancing)
/// age, arms the throttle from the failed walk's cost, and retries on the next
/// event. [`switch_watched`] refuses a worktree whose first walk fails, with a
/// reason, and stays on the worktree it watched. So this signature says "this
/// walk did not produce a snapshot", not "the monitor should stop" — a
/// distinction worth preserving if another caller ever appears.
pub(crate) fn walk(
    handle: &mut RepoHandle,
    ignore: &LiveIgnore,
    cfg: &RenderConfig,
) -> Result<Snapshot> {
    let repo = handle.reopened();
    ignore.refresh(repo);
    collect_snapshot(repo, cfg)
}

/// Everything that is tied to the worktree that the loop watches: its path,
/// the repository handle that each walk re-opens, the ignore matcher that each
/// walk rebuilds, and the filesystem watcher that wakes the loop.
///
/// The four parts are one value, so a switch replaces all of them in one step,
/// and no part stays on the old worktree. The drop of a `Watched` drops its
/// watcher, and a watcher that is dropped sends no more events. So after a
/// switch, a change in the old worktree wakes nothing.
///
/// [`run`] shares the one `Watched` between the hooks of the loop through a
/// [`RefCell`]. The loop runs on one thread and calls one hook at a time, so no
/// borrow ever meets another borrow.
struct Watched {
    /// The root of the worktree, in the one spelling that the loop compares.
    path: WorktreePath,
    /// The repository of the worktree. Each walk re-opens it, so configuration
    /// that another pane writes takes effect. See [`walk`].
    handle: RepoHandle,
    /// The ignore matcher that the watcher reads at each event, and that each
    /// walk rebuilds from disk. See [`LiveIgnore`].
    ignore: LiveIgnore,
    /// The watcher of the worktree and of its git directories. Nothing reads
    /// it. It is here for its drop, which stops the events of this worktree.
    _watcher: RecommendedWatcher,
}

impl Watched {
    /// Watch the worktree at `path` through `handle`, which is open on it
    /// already. The watcher sends its events on `tx`.
    ///
    /// [`run`] builds the first `Watched` here, from the handle that `main`
    /// opened and from the home worktree that [`resolve_home`] resolved from
    /// that same handle. So the handle is open on `path` by construction, and
    /// no second discovery is necessary.
    ///
    /// # Errors
    ///
    /// Refuses with [`WATCHER_FAILED`] when the filesystem watcher does not
    /// start, and with [`NOT_A_WORK_TREE`] when the repository has no work
    /// tree to watch. Each reason names `path`.
    fn from_handle(
        handle: RepoHandle,
        path: WorktreePath,
        tx: Sender<Event>,
    ) -> Result<Self, String> {
        let ignore = LiveIgnore::new(handle.repo());
        match spawn_fs_watcher(handle.repo(), ignore.clone(), tx) {
            Ok(Some(watcher)) => Ok(Self {
                path,
                handle,
                ignore,
                _watcher: watcher,
            }),
            Ok(None) => Err(format!("{NOT_A_WORK_TREE}: {}", path.as_path().display())),
            Err(error) => Err(format!(
                "{WATCHER_FAILED}: {}: {error}",
                path.as_path().display()
            )),
        }
    }

    /// Open the worktree at `path`, and watch it. The watcher sends its events
    /// on `tx`. A switch opens its target here.
    ///
    /// [`RepoHandle::discover`] walks up from `path`, as git does. So a
    /// directory of the list that lost its `.git` file opens the repository
    /// around it: a linked worktree inside the main worktree opens the main
    /// worktree. The open therefore makes sure that the work tree it opened
    /// resolves to `path` itself. Without that check, the frame would show the
    /// status of one worktree under the name of another.
    ///
    /// # Errors
    ///
    /// Refuses with [`DIRECTORY_GONE`] when no directory is at `path`. Refuses
    /// with [`NOT_A_WORK_TREE`] when no git work tree opens at `path`, or when
    /// the work tree that opens is not `path`. Refuses with the reasons of
    /// [`from_handle`](Self::from_handle) too. Each reason names `path`.
    fn open(path: &WorktreePath, tx: Sender<Event>) -> Result<Self, String> {
        let shown = path.as_path().display();
        if WorktreePath::resolve(path.as_path()).is_none() {
            return Err(format!("{DIRECTORY_GONE}: {shown}"));
        }
        let handle = RepoHandle::discover(path.as_path())
            .filter(|handle| {
                handle
                    .repo()
                    .workdir()
                    .and_then(WorktreePath::resolve)
                    .as_ref()
                    == Some(path)
            })
            .ok_or_else(|| format!("{NOT_A_WORK_TREE}: {shown}"))?;
        Self::from_handle(handle, path.clone(), tx)
    }

    /// Walk the worktree, and put its badge on the snapshot. [`walk`] re-opens
    /// the repository, rebuilds the ignore matcher, and collects the snapshot.
    /// [`badged`] then reads the badge from the repository that the walk
    /// re-opened. `home` is the worktree where the user started gsw.
    ///
    /// # Errors
    ///
    /// Gives the error of [`walk`], which is the error of the status walk.
    fn walk(&mut self, cfg: &RenderConfig, home: &WorktreePath) -> Result<Snapshot> {
        let snapshot = walk(&mut self.handle, &self.ignore, cfg)?;
        Ok(badged(snapshot, self.handle.repo(), &self.path, home))
    }
}

/// Put the badge of the worktree at `path` on `snapshot`, which a walk of
/// `repo` collected. `home` is the worktree where the user started gsw.
///
/// The seed walk of [`run`] and every walk of a [`Watched`] go through here, so
/// the first frame and every later frame name the worktree in the same way.
///
/// The paths come from [`worktree_paths`], which reads no HEAD and opens no
/// linked worktree. Every walk pays for this call, and the cooldown after a
/// walk is 100 times its cost, so the expensive [`list_worktrees`] is wrong
/// here. The label comes from [`head_label`] of `repo`, which is the
/// repository that the walk read, so the label and the snapshot agree.
///
/// [`list_worktrees`]: crate::worktrees::list_worktrees
fn badged(
    mut snapshot: Snapshot,
    repo: &gix::Repository,
    path: &WorktreePath,
    home: &WorktreePath,
) -> Snapshot {
    snapshot.worktree =
        crate::worktrees::badge(&worktree_paths(repo), path, home, head_label(repo));
    snapshot
}

/// Switch the watch to the worktree at `target` as one step, and give the first
/// frame of that worktree. The watcher of the new worktree sends its events on
/// `tx`, and `home` is the worktree where the user started gsw.
///
/// 1. Open a candidate [`Watched`] on `target`. That starts its watcher.
/// 2. Walk the candidate, which puts its badge on the snapshot.
/// 3. Only when both work, the candidate takes the place of the old
///    `Watched`. The drop of the old one stops its watcher, so a change in the
///    old worktree wakes the loop no more.
///
/// The `switch` hook of [`run`] is this function, so the tests call exactly
/// what production calls. It borrows `watched` only for the replacement, after
/// the open and the walk, so no borrow meets another.
///
/// # Errors
///
/// Gives the reason of [`Watched::open`] when the open fails, and
/// [`WALK_FAILED`] with the path and the error of the walk when the walk fails.
/// Either way, `watched` stays on the worktree it watched, and the candidate
/// goes away with its watcher.
fn switch_watched(
    watched: &RefCell<Watched>,
    target: &WorktreePath,
    tx: Sender<Event>,
    cfg: &RenderConfig,
    home: &WorktreePath,
) -> Result<Snapshot, String> {
    let mut candidate = Watched::open(target, tx)?;
    let snapshot = candidate
        .walk(cfg, home)
        .map_err(|error| format!("{WALK_FAILED}: {}: {error:#}", target.as_path().display()))?;
    // The old worktree goes here, and the drop of its watcher stops its events.
    drop(watched.replace(candidate));
    Ok(snapshot)
}

/// The worktrees that Down reads at each press: every worktree of the
/// repository of the watched worktree, sorted by path, each with its label.
///
/// Every worktree of a repository shares one common directory, so the list is
/// the same from each of them. gix reads the admin directories again at each
/// call, so a worktree that `nwt` or `swt` added or removed since the last
/// press is in the list or out of it. [`list_worktrees`] opens each linked
/// worktree for its label, which costs more than the paths alone. Only Down
/// shows the labels, so only Down pays for them. Left, Right, and a failed
/// walk read [`listed_paths`].
fn listed(watched: &RefCell<Watched>) -> Vec<WorktreeEntry> {
    list_worktrees(watched.borrow().handle.repo())
}

/// The paths of the worktrees that [`listed`] gives, in the same order. They
/// come from [`worktree_paths`], which reads no HEAD and opens no linked
/// worktree.
///
/// Left and Right read it at each press, because they need the paths alone.
/// A walk that fails reads it to learn whether the worktree on the screen
/// still exists. The cooldown after that walk holds the cost of this read.
fn listed_paths(watched: &RefCell<Watched>) -> Vec<WorktreePath> {
    worktree_paths(watched.borrow().handle.repo())
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
/// the rest of the process. The handle moves into the [`Watched`] of the home
/// worktree, and each refresh re-opens it in place. Borrowing instead would
/// make the caller hold a mutable borrow across a call that never returns until
/// the user quits, for no gain — nothing is left for it to do with the handle
/// afterward.
///
/// The hooks of the loop share one [`Watched`] through a [`RefCell`]. The
/// `collect` hook walks it, the `worktrees` and `worktree_paths` hooks read
/// the worktrees of its repository through [`listed`] and [`listed_paths`],
/// and the `switch` hook replaces it through [`switch_watched`].
pub(crate) fn run(handle: RepoHandle, cfg: &RenderConfig) -> Result<()> {
    // Before the guard takes the screen, so a refusal prints on the screen the
    // user started from, and not on the alternate screen that the guard
    // clears when it goes.
    let home = resolve_home(&handle)?;
    let _guard = TerminalGuard::enter()?;

    // Seed the cache with one git walk and paint the first frame at offset 0,
    // byte-identical to a one-shot render of the same state. That frame's
    // freshest age seeds the decay-timer cadence.
    //
    // Deliberately NOT `walk`: the handle was opened microseconds ago in
    // `main`, so nothing can have changed the config since, and a re-open here
    // would only pay for a config parse to read back what we already hold. The
    // ignore matcher is equally fresh — `Watched::from_handle` below builds it
    // from that same just-opened handle — so skipping `walk`'s rebuild costs
    // nothing either. Every *subsequent* refresh goes through `walk`, which
    // re-opens the handle and rebuilds the matcher.
    let dims = current_dimensions(cfg.width_offset);
    let collected_at = Instant::now();
    // The badge goes on through the same helper as on every later walk, so the
    // first frame names the worktree as every later frame does.
    let snapshot = badged(
        collect_snapshot(handle.repo(), cfg)?,
        handle.repo(),
        &home,
        &home,
    );
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

    // The push thread reports back on the loop's own channel, so its outcome
    // re-enters the loop exactly like a filesystem event — applied between
    // frames, never during one.
    let push_tx = tx.clone();

    // The shell the issue key uses, resolved once. The probe asks it whether
    // the command exists and a run asks it to run the command, and both must
    // ask the same shell.
    let shell = crate::shell::user_shell();
    let issue_tx = tx.clone();
    spawn_issue_probe(shell.clone(), tx.clone());

    // The worker that measures for `m`. It reports on the channel of the loop,
    // as the push and the issue key do. The quit below waits for it, because a
    // replay that the process abandons keeps a scratch worktree registered in
    // the repository of the user.
    let mut conflicts = ConflictsWorker::new();
    let conflicts_tx = tx.clone();

    // Where the person who reads this screen sits, read once and beside the
    // shell above. A session does not change under a running process, and the
    // read costs one `ps`.
    let session = crate::remote::Session::read();

    // The worktree on the screen, as one value: its handle, the one ignore
    // matcher that the watcher callback reads per event and that every walk
    // rebuilds from disk, and the filesystem watcher. It starts on the home
    // worktree, with the handle that `main` opened, so the start needs no
    // second discovery. The hooks below share it, and its drop at the end of
    // this function stops the watcher.
    let switch_tx = tx.clone();
    let watched =
        RefCell::new(Watched::from_handle(handle, home.clone(), tx).map_err(anyhow::Error::msg)?);

    let result = event_loop(
        &rx,
        DEBOUNCE,
        &mut displayed,
        LoopStart {
            cache,
            freshest: initial_freshest,
            schedule,
            ui: PushUi::new(cfg.truecolor),
            session,
            home: home.clone(),
        },
        LoopHooks {
            // The loop passes the worktree on the screen. The loop moves to a
            // worktree only when `switch` gives `Ok`, and `switch_watched`
            // replaces `watched` only when it gives `Ok`, so the two always
            // name the same worktree. The assertion states that in the debug
            // build, which the tests run.
            collect: |current: &WorktreePath| {
                let mut watched = watched.borrow_mut();
                debug_assert_eq!(
                    current, &watched.path,
                    "the loop and the watch must be on the same worktree",
                );
                watched.walk(cfg, &home)
            },
            render: |snap: &Snapshot, dims: Dimensions, timing: FrameTiming| {
                render_frame(snap, cfg, dims, timing)
            },
            render_list: |snap: &Snapshot,
                          dims: Dimensions,
                          timing: FrameTiming,
                          list: &WorktreeList| {
                render_list_frame(snap, dims, timing, list)
            },
            dimensions: || current_dimensions(cfg.width_offset),
            paint: |output: &str| paint_output(output),
            clock: Instant::now,
            next_tick: |freshest: Option<Duration>| freshest.and_then(next_tick),
            start_issue: |command: crate::shell::ShellCommand,
                          current: &WorktreePath,
                          generation: Generation| {
                // The run keeps the generation of its press and sends it back
                // with the outcome, so the loop knows an outcome that arrives
                // after a switch.
                let workdir = current.as_path().to_path_buf();
                let finish_tx = issue_tx.clone();
                let shell = shell.clone();
                thread::spawn(move || {
                    let outcome = crate::issue::run(&shell, &command, &workdir);
                    let _ = finish_tx.send(Event::IssueFinished {
                        generation,
                        outcome,
                    });
                });
            },
            start_base_update: |_command: crate::update::BaseUpdateCommand,
                                _current: &WorktreePath| {},
            start_push: |command: PushCommand, current: &WorktreePath| {
                // Two senders on the one channel, so a line and the outcome
                // re-enter the loop the same way every other event does —
                // applied between frames rather than during one.
                let line_tx = push_tx.clone();
                let finish_tx = push_tx.clone();
                crate::push::spawn(
                    command,
                    current.as_path().to_path_buf(),
                    move |line| {
                        let _ = line_tx.send(Event::PushOutput(line));
                    },
                    move |outcome| {
                        let _ = finish_tx.send(Event::PushFinished(outcome));
                    },
                );
            },
            start_conflicts: |current: &WorktreePath, generation: Generation| {
                // Both events of the run carry the generation of its press, as
                // the outcome of the issue key does.
                let started_tx = conflicts_tx.clone();
                let finish_tx = conflicts_tx.clone();
                conflicts.start(
                    current.as_path().to_path_buf(),
                    move |branch| {
                        let _ = started_tx.send(Event::ConflictsStarted { generation, branch });
                    },
                    move |outcome| {
                        let _ = finish_tx.send(Event::ConflictsFinished {
                            generation,
                            outcome,
                        });
                    },
                );
            },
            worktrees: || listed(&watched),
            worktree_paths: || listed_paths(&watched),
            // Each switch gives the new watcher a sender of its own on the one
            // channel of the loop.
            switch: |target: &WorktreePath| {
                switch_watched(&watched, target, switch_tx.clone(), cfg, &home)
            },
        },
    );

    // Before the guard restores the terminal, and on every way out of the
    // loop. A run in the middle of a replay finishes that replay and removes
    // its scratch worktree. The notice tells the user why the quit takes a
    // moment. The frame goes away with the quit, so the notice takes the whole
    // screen.
    conflicts.shutdown(|notice| {
        let width = current_dimensions(cfg.width_offset).width;
        let _ = paint_output(&textfit::truncate_right(notice, width));
    });

    result
}

/// Why watch mode does not start: no directory is at the root of the work tree
/// that `main` opened. The directory went away between the open and the start
/// of watch mode.
const HOME_UNRESOLVED: &str = "gsw cannot resolve the work tree it started in";

/// Why a switch refuses a worktree whose first walk fails. The line names the
/// directory and then the error of the walk after this text.
const WALK_FAILED: &str = "gsw cannot read the status of the worktree";

/// Why gsw refuses to watch a directory that is not the root of a git work
/// tree. The line names the directory after this text.
const NOT_A_WORK_TREE: &str = "gsw cannot go to a directory that is not a git work tree";

/// Why gsw refuses to go to a worktree whose directory no longer exists. The
/// line names the directory after this text.
const DIRECTORY_GONE: &str = "gsw cannot go to a worktree whose directory no longer exists";

/// Why gsw refuses to watch a worktree when its filesystem watcher does not
/// start. The line names the directory and then the error of the watcher after
/// this text.
const WATCHER_FAILED: &str = "gsw cannot watch the worktree for changes";

/// What gsw says after it went back to the home worktree, after the path of
/// the worktree that went away: `<path> no longer exists — back to the home
/// worktree`. See [`LoopState::return_home_if_gone`].
const WORKTREE_GONE: &str = "no longer exists — back to the home worktree";

/// The home worktree: the root of the work tree that `handle` opened, in the
/// one spelling that every comparison of the loop uses.
///
/// The loop compares the worktree on the screen with the paths of the list,
/// and [`WorktreePath::resolve`] is what makes two spellings of one directory
/// compare equal.
///
/// # Errors
///
/// Fails with [`HOME_UNRESOLVED`] and the path when no directory is at the root
/// any more. It fails with [`HOME_UNRESOLVED`] alone for a repository with no
/// work tree, which [`RepoHandle`] refuses at discovery, so watch mode never
/// holds one.
fn resolve_home(handle: &RepoHandle) -> Result<WorktreePath> {
    let Some(root) = handle.repo().workdir() else {
        anyhow::bail!(HOME_UNRESOLVED);
    };
    WorktreePath::resolve(root)
        .ok_or_else(|| anyhow::anyhow!("{HOME_UNRESOLVED}: {}", root.display()))
}

/// Ask the shell, once, whether the issue command exists, and report the
/// answer on the loop's own channel.
///
/// On a thread of its own, because an interactive shell reads an rc file and
/// an rc file is somebody else's code: it can take a second, and it can take
/// forever. The loop never waits for this. Until the answer arrives the key is
/// unbound, and a shell that says no sends nothing at all — so the key stays
/// unbound and silent for the life of the process.
///
/// The answer arrives once. A function added to the rc file after `gsw`
/// started needs a restart.
fn spawn_issue_probe(shell: OsString, tx: Sender<Event>) {
    // Read here rather than on the thread, so the value and the process that
    // holds it are read in one place. An absent variable gives the default
    // name, and an empty one turns the feature off.
    let named = std::env::var_os(crate::issue::ISSUE_COMMAND_ENV)
        .map(|value| value.to_string_lossy().into_owned());
    thread::spawn(move || {
        if let Some(command) = crate::shell::resolve(
            named.as_deref(),
            crate::issue::DEFAULT_ISSUE_COMMAND,
            &shell,
        ) {
            let _ = tx.send(Event::IssueCommandFound(command));
        }
    });
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

/// Everything the watch loop starts from — as opposed to [`LoopHooks`], which
/// is everything it drives.
///
/// Bundled for the same reason the hooks are: four values that are all "where
/// this run begins" read better as one argument than as four, and the next one
/// added goes here rather than onto the loop's signature. Each is built by the
/// caller because each is anchored to the seed walk that filled it — the cache
/// to the snapshot it collected, the freshest age to the frame that snapshot
/// rendered, the schedule to the cost that walk measured, and the push UI to
/// the terminal's color depth, which is resolved from the CLI and nowhere else.
struct LoopStart {
    /// The snapshot a re-render can use without walking git again.
    cache: SnapshotCache,
    /// The freshest displayed age of the frame already painted, which seeds
    /// the decay-tick cadence for the loop's first wait.
    freshest: Option<Duration>,
    /// The walk schedule, already anchored to the seed walk.
    schedule: WalkSchedule,
    /// Everything the push feature puts on screen, plus the input mode that
    /// goes with it. Owned by the loop for the rest of the run, because the
    /// loop is the only place that knows what is displayed — see
    /// [`Event::Key`] for why the reader thread must not.
    ui: PushUi,
    /// Where the person who reads this screen sits, so the `G` key knows
    /// whether a browser opened here reaches anybody.
    session: crate::remote::Session,
    /// The worktree where the user started gsw, in the one spelling that
    /// every comparison of the loop uses. The loop starts on it, and Up goes
    /// back to it.
    home: WorktreePath,
}

/// Everything the loop changes while it runs.
///
/// One value, because the loop receives events at three points and routes each
/// of them through the one function [`absorb`], and because a switch of the
/// worktree changes most of this in one step: the cache takes the snapshot of
/// the new worktree, the schedule starts the refresh clock again, the row under
/// the frame empties, and the `G` key loses its arming. An argument for each
/// part would give [`absorb`] more arguments than a reader can hold.
struct LoopState {
    /// The snapshot a re-render can use without walking git again.
    cache: SnapshotCache,
    /// The walk schedule, anchored to the last walk.
    schedule: WalkSchedule,
    /// Everything under the frame, and the input mode that goes with it.
    ui: PushUi,
    /// The state of the `G` key.
    issue: IssueRun,
    /// The state of the `m` key.
    conflicts: ConflictsRun,
    /// The worktree where the user started gsw.
    home: WorktreePath,
    /// The worktree that the frame shows. The walk, `p`, `G`, and `m` act on
    /// it. It starts at [`LoopState::home`].
    current: WorktreePath,
    /// How many switches the loop has made. See [`Generation`].
    generation: Generation,
}

impl LoopState {
    /// Switch the loop to `target` for a key the user pressed: an arrow key,
    /// or Enter in the list.
    ///
    /// The switch starts at a read of the clock, and [`LoopState::enter`]
    /// does it. On `Err`, the loop stays on the worktree it shows, and the
    /// reason takes the row on a line that fades, because it is gsw's report
    /// about a key the user pressed.
    ///
    /// [`absorb`] calls it when it reads the key, and not at the next frame. So
    /// a `p`, `G`, or `m` later in the same burst acts on the new worktree, as
    /// a `y` after a `p` in one burst reads the new mode.
    fn switch_to(
        &mut self,
        target: WorktreePath,
        clock: &impl Fn() -> Instant,
        open: &mut impl FnMut(&WorktreePath) -> Result<Snapshot, String>,
    ) {
        let now = clock();
        // The answer goes unread: no state here stands on where the line
        // landed.
        if let Err(reason) = self.enter(target, now, clock, open) {
            let _ = self.ui.post_notice(reason, now);
        }
    }

    /// Open `target` and move the loop to it. Every switch goes through here:
    /// the switch of a key, and the return to the home worktree.
    ///
    /// 1. Open `target` through `open`, and read the clock for the cost of the
    ///    open, counted from `since`.
    /// 2. On `Ok`, the snapshot of `target` goes into the cache, collected at
    ///    `since`, and the schedule records the open as a walk that started
    ///    at `since`, which starts the refresh clock again. The loop then
    ///    watches `target`, and the generation moves on, so an outcome of a
    ///    run that started before the switch is known as stale. Every message
    ///    under the frame goes, and the list closes, because each one
    ///    describes the worktree the frame showed before. The `G` key loses
    ///    its arming with the message that armed it.
    /// 3. On `Err`, nothing changes, and the reason comes back. The caller
    ///    decides whether the reason goes on the row.
    fn enter(
        &mut self,
        target: WorktreePath,
        since: Instant,
        clock: &impl Fn() -> Instant,
        open: &mut impl FnMut(&WorktreePath) -> Result<Snapshot, String>,
    ) -> Result<(), String> {
        let opened = open(&target);
        let cost = clock().saturating_duration_since(since);
        let snapshot = opened?;
        self.cache.snapshot = snapshot;
        self.cache.collected_at = since;
        self.schedule.record(since, cost);
        self.current = target;
        self.generation = self.generation.next();
        self.ui.clear();
        self.issue.disarm();
        Ok(())
    }

    /// After a walk of the current worktree failed: go back to the home
    /// worktree when the current worktree no longer exists, and say so on a
    /// line that fades.
    ///
    /// The current worktree no longer exists when the list of the paths of
    /// the worktrees no longer holds it: `git worktree remove` or `swt merge`
    /// took it. The list holds the paths alone, because the check needs no
    /// label. Three failed walks keep the rule of a failed walk, which is the
    /// last good snapshot at its true age:
    ///
    /// - a walk of the home worktree, which has no home to go back to;
    /// - a walk while a push runs, because the window under the frame belongs
    ///   to the worktree that pushes. A later failed walk goes home, after the
    ///   push;
    /// - a walk of a worktree that the list still holds. Such a walk failed
    ///   for a moment, as when `git gc` swaps the ref store under it.
    ///
    /// The switch counts from `now`, the instant of this wake. The frame of
    /// home is then placed at `now` like every frame of the wake, so its
    /// refresh clock shows the whole interval.
    ///
    /// The read of the paths and the open of home are git work of this wake,
    /// as the failed walk is. The caller records the cost of the wake after
    /// this call returns, so the duty cycle pays for all of that work on every
    /// path through this call. A return that works records the open too, and
    /// the record of the caller then replaces it, because both records start
    /// at `now`.
    ///
    /// The switch clears the row, the question, and the list, and the line
    /// goes on the row after that, so the line is on the frame that shows
    /// home. A switch that fails changes nothing and posts nothing. Every
    /// later failed walk tries again, and a line would come back on each one.
    fn return_home_if_gone(
        &mut self,
        now: Instant,
        clock: &impl Fn() -> Instant,
        worktree_paths: &mut impl FnMut() -> Vec<WorktreePath>,
        open: &mut impl FnMut(&WorktreePath) -> Result<Snapshot, String>,
    ) {
        if self.current == self.home || self.ui.mode() == InputMode::Pushing {
            return;
        }
        if worktree_paths().contains(&self.current) {
            return;
        }
        let gone = self.current.clone();
        if self.enter(self.home.clone(), now, clock, open).is_ok() {
            // The answer goes unread: no state here stands on where the line
            // landed.
            let _ = self
                .ui
                .post_notice(format!("{} {WORKTREE_GONE}", gone.as_path().display()), now);
        }
    }

    /// Whether an event with `generation` comes from a run that started before
    /// the last switch, and so describes a worktree that the frame no longer
    /// shows.
    fn is_stale(&self, generation: Generation) -> bool {
        generation != self.generation
    }
}

/// The side-effecting hooks the watch loop drives, bundled so the loop stays one
/// testable function instead of taking a fistful of closures. Production wires
/// these to the real git collect, render, terminal-size query, painter, and
/// clock; tests inject counters and a controllable clock to assert which hooks
/// ran — and with what age offset — without a TTY or real time.
struct LoopHooks<
    Collect,
    RenderFn,
    RenderList,
    Dims,
    Paint,
    Clock,
    Tick,
    StartPush,
    StartBaseUpdate,
    StartIssue,
    StartConflicts,
    Worktrees,
    Paths,
    Switch,
> {
    /// Walk the worktree that the loop passes, which is the worktree the frame
    /// shows, into a fresh [`Snapshot`] (the expensive git work).
    collect: Collect,
    /// Render a snapshot at the given dimensions and timing.
    render: RenderFn,
    /// Render the frame of the open list of the worktrees: the head of the
    /// frame of the snapshot, and the rows of the list under it. The loop
    /// calls it in place of `render` while the list is open.
    render_list: RenderList,
    /// Query the current terminal dimensions (re-evaluated on resize).
    dimensions: Dims,
    /// Paint a finished frame.
    paint: Paint,
    /// Read the current instant (real `Instant::now` in production).
    clock: Clock,
    /// Map the freshest displayed age to the decay-tick interval (`None` = off).
    next_tick: Tick,
    /// Start a confirmed push, given the [`PushCommand`] the confirmation
    /// described — the `git` arguments and the branch they were written for —
    /// and the worktree to push from, which is the worktree on the screen at
    /// the press. Production spawns a thread that runs the push and sends the
    /// outcome back as [`Event::PushFinished`]; tests record the command and
    /// decide for themselves when — or whether — the outcome arrives.
    start_push: StartPush,
    /// Start a confirmed rebase onto the base or merge of the base, given the
    /// [`crate::update::BaseUpdateCommand`] the question described — the act,
    /// the branch and the base it named, and the user's own command — and the
    /// worktree to run it in, which is the worktree on the screen at the press.
    /// Production spawns a thread that runs the command, sends each line it
    /// writes back as [`Event::PushOutput`], and sends the outcome as
    /// [`Event::BaseUpdateFinished`]; tests record the command and decide for
    /// themselves when, or whether, the outcome arrives.
    start_base_update: StartBaseUpdate,
    /// Start a run of the issue command in the worktree on the screen at the
    /// press. Production spawns a thread that runs it and sends the outcome
    /// back as [`Event::IssueFinished`], with the [`Generation`] it was given;
    /// tests record the command and decide for themselves when the outcome
    /// arrives.
    start_issue: StartIssue,
    /// Start a measurement of a rebase and a merge against the default branch,
    /// in the worktree on the screen at the press. Production starts it on a
    /// thread of its own, which sends the branch back as
    /// [`Event::ConflictsStarted`] and the outcome as
    /// [`Event::ConflictsFinished`], each with the [`Generation`] it was
    /// given. Tests count the runs and decide for themselves when, or whether,
    /// those events arrive.
    ///
    /// It takes no branch, because the thread finds the branch itself. The
    /// loop never waits for the run, because a rebase replay of a long branch
    /// can take many seconds.
    start_conflicts: StartConflicts,
    /// Read the worktrees of the repository again, sorted by path, each with
    /// its label, as [`crate::worktrees::list_worktrees`] gives them.
    ///
    /// Down calls it at each press, because `nwt` and `swt` add and remove
    /// worktrees while gsw runs, so a list read once at start is soon wrong.
    /// Only Down shows the labels, and a label costs an open of its linked
    /// worktree, so nothing else calls it.
    worktrees: Worktrees,
    /// Read the paths of the worktrees of the repository again, sorted by
    /// path, as [`crate::worktrees::worktree_paths`] gives them. No HEAD is
    /// read, and no linked worktree is opened.
    ///
    /// Left and Right call it at each press, for the same reason as Down
    /// calls `worktrees`. After a walk that fails, the loop calls it too, to
    /// learn whether the worktree on the screen still exists.
    worktree_paths: Paths,
    /// Open the worktree at the path, start its watcher, and walk it, as one
    /// step.
    ///
    /// It commits only on `Ok`, and the snapshot it gives is the first frame
    /// of that worktree. `Err` carries the reason for a fading line, and then
    /// nothing changes: the loop stays on the worktree it shows.
    switch: Switch,
}

/// The triggers one wake collected, before the render decides what to do with
/// them. A burst can carry several at once, and they are not exclusive: a walk
/// forced by `r` and a resize can arrive together.
#[derive(Default)]
struct Pending {
    /// A relevant filesystem path changed.
    fs: bool,
    /// The terminal was resized.
    resize: bool,
    /// A walk was demanded outright, bypassing the cooldown.
    force: bool,
}

/// Whether the loop keeps running after an event.
#[derive(PartialEq, Eq, Debug)]
enum Flow {
    /// Carry on to the render.
    Continue,
    /// End watch mode.
    Quit,
}

/// Fold one received [`Event`] into the loop's pending state.
///
/// Extracted because the loop receives events at three points — the timed wait,
/// the untimed wait, and the debounce drain — and a routing rule written three
/// times is a rule that will be updated twice. Every event goes through here,
/// so a variant added later cannot be handled in two of the three places.
///
/// `clock` is the loop's injected clock, and it is passed rather than an
/// instant so it is read only by the two events that need one: a self-expiring
/// status message counts its age from the moment the news arrived, and no other
/// event has a moment to record. Reading it up front instead would put a clock
/// call on every filesystem event in a burst, for the two that use it.
///
/// A key arrives unclassified and is resolved here against `ui`'s *current*
/// mode, which is what makes a burst read correctly: within one drain, the `p`
/// ahead of a `y` has already switched the mode by the time the `y` is looked
/// at. It then recurses exactly once — [`classify_input`] never returns
/// [`Event::Key`], so there is no second hop.
///
/// The exception worth naming is a `p` that switches nothing. Nothing renders
/// between two keys of one drain, so a rule the render path applies is applied
/// after the second key has already been classified — which is why
/// `cache.dims` is threaded down to [`PushUi::request`]: a pane with no row to
/// draw the question in raises no question, the mode does not move, and the `y`
/// or Enter behind that `p` is read as the ordinary key it is. `cache.dims` is
/// the pane the last render measured, which is the pane the user was looking at
/// when they pressed the key — the loop re-measures after this drain, not
/// during it.
///
/// `state` is everything the loop changes, in one value. Its cache comes whole,
/// and not as its snapshot and its pane, because the two always come from the
/// one cache. Its `issue` and `conflicts` are the state of the two keys that
/// start work off this thread, one run of each at a time. Its current worktree
/// is where the push, the issue key, and `m` do their work.
#[expect(
    clippy::type_complexity,
    reason = "the loop takes one generic for each of its fourteen hooks, so each hook stays \
              a plain closure that a test replaces with a fake. A type alias spells the same \
              fourteen generics, and the borrow of the whole value keeps every call of \
              absorb the same"
)]
fn absorb<
    Collect,
    RenderFn,
    RenderList,
    Dims,
    Paint,
    Clock,
    Tick,
    StartPush,
    StartBaseUpdate,
    StartIssue,
    StartConflicts,
    Worktrees,
    Paths,
    Switch,
>(
    event: Event,
    pending: &mut Pending,
    state: &mut LoopState,
    hooks: &mut LoopHooks<
        Collect,
        RenderFn,
        RenderList,
        Dims,
        Paint,
        Clock,
        Tick,
        StartPush,
        StartBaseUpdate,
        StartIssue,
        StartConflicts,
        Worktrees,
        Paths,
        Switch,
    >,
) -> Flow
where
    Clock: Fn() -> Instant,
    StartPush: FnMut(PushCommand, &WorktreePath),
    StartBaseUpdate: FnMut(crate::update::BaseUpdateCommand, &WorktreePath),
    StartIssue: FnMut(crate::shell::ShellCommand, &WorktreePath, Generation),
    StartConflicts: FnMut(&WorktreePath, Generation),
    Worktrees: FnMut() -> Vec<WorktreeEntry>,
    Paths: FnMut() -> Vec<WorktreePath>,
    Switch: FnMut(&WorktreePath) -> Result<Snapshot, String>,
{
    let clock = &hooks.clock;
    match event {
        Event::Quit => return Flow::Quit,
        Event::FsChanged => pending.fs = true,
        Event::Resize => pending.resize = true,
        Event::ForceRefresh => pending.force = true,
        Event::Key(key) => {
            if let Some(action) = classify_input(key, state.ui.mode(), state.issue.key()) {
                // Every key but `G` takes the arming away. The message that
                // asks for the second press is the armed state, and this key
                // is not that press.
                if !matches!(action, Event::IssueRequested) {
                    state.issue.disarm();
                }
                return absorb(action, pending, state, hooks);
            }
        }
        Event::PushRequested => {
            state
                .ui
                .request(&state.cache.snapshot, state.cache.dims, clock());
        }
        // `confirm` yields the command only once, so a second `y` that raced
        // the mode change starts nothing.
        Event::PushConfirmed => {
            if let Some(crate::push::Confirmed::Push(command)) = state.ui.confirm(clock()) {
                (hooks.start_push)(command, &state.current);
            }
        }
        Event::PushOutput(line) => state.ui.output_line(line),
        Event::PushCancelled => state.ui.cancel(),
        Event::Dismiss => state.ui.dismiss(),
        // Left and Right read the paths of the worktrees again at each press,
        // because `nwt` and `swt` add and remove worktrees while gsw runs.
        // They read no label, because they show none. Where no other worktree
        // is, neither finds a target, and nothing happens.
        Event::GoPrevious => {
            let paths = (hooks.worktree_paths)();
            if let Some(target) = crate::worktrees::previous(&paths, &state.current).cloned() {
                state.switch_to(target, clock, &mut hooks.switch);
            }
        }
        Event::GoNext => {
            let paths = (hooks.worktree_paths)();
            if let Some(target) = crate::worktrees::next(&paths, &state.current).cloned() {
                state.switch_to(target, clock, &mut hooks.switch);
            }
        }
        // Up on the home worktree does nothing: the frame shows it already.
        Event::GoHome => {
            if state.current != state.home {
                let home = state.home.clone();
                state.switch_to(home, clock, &mut hooks.switch);
            }
        }
        // Down reads the list again at each press, as Left and Right read the
        // paths, because `nwt` and `swt` add and remove worktrees while gsw
        // runs. The list carries the label of each row. The cursor starts on
        // the worktree that the frame shows.
        //
        // A pane with no row for the list opens none, so Enter never chooses
        // a row that the user did not see, as `p` never asks a question that
        // the pane cannot show. `cache.dims` is the pane that the user saw at
        // the press. The check comes before the read, so such a pane pays for
        // no read of the list.
        Event::OpenList => {
            if crate::list_rows(&state.cache.snapshot, state.cache.dims) == 0 {
                return Flow::Continue;
            }
            let entries = (hooks.worktrees)();
            if let Some(list) = WorktreeList::open(entries, &state.current, state.home.clone()) {
                state.ui.open_list(list);
            }
        }
        // The cursor moves, and nothing else does: no walk and no switch. gsw
        // walks the new worktree only after Enter.
        Event::ListUp => {
            if let Some(list) = state.ui.list_mut() {
                list.up();
            }
        }
        Event::ListDown => {
            if let Some(list) = state.ui.list_mut() {
                list.down();
            }
        }
        // Enter closes the list before the switch, so a switch that fails
        // puts its reason on a free row. Enter on the worktree that the frame
        // shows opens nothing, because the frame shows it already.
        Event::ListGo => {
            if let Some(list) = state.ui.close_list() {
                let target = list.selected().path.clone();
                if target != state.current {
                    state.switch_to(target, clock, &mut hooks.switch);
                }
            }
        }
        // The watch stays on the worktree that the frame showed before the
        // list opened. The row is free again, so the next frame posts the
        // oldest message that waited for the list.
        Event::ListClose => {
            let _ = state.ui.close_list();
        }
        Event::IssueRequested => {
            // One read of the clock, for both halves of one press. The arming
            // and the message it stands for must end at the same moment, and
            // two reads put the arming microseconds before the message. A test
            // clock that steps on every read makes the same gap a whole step.
            // The arm below takes this same instant, which is why it is right:
            // it runs only where the message went straight onto the row, so
            // `now` is the instant the message got there.
            let now = clock();
            match state.issue.press(now) {
                IssuePress::Run(command) => {
                    (hooks.start_issue)(command, &state.current, state.generation);
                }
                // The row arms the key, and not the press. A question or a
                // push in flight owns the row, and a notice that arrives then
                // waits in the queue — nobody has read it, so a second press
                // against it would run the command with no warning ever seen.
                IssuePress::Ask(message) => {
                    if state.ui.post_notice(message, now) == crate::push::Posted::OnRow {
                        state.issue.arm(now);
                    }
                }
                IssuePress::Nothing => {}
            }
        }
        Event::IssueCommandFound(command) => state.issue.found(command),
        // The check and the start are one step on the thread of the loop, so
        // a burst of presses starts one run. See [`ConflictsRun`].
        Event::ConflictsRequested => match state.conflicts.press() {
            ConflictsPress::Start => (hooks.start_conflicts)(&state.current, state.generation),
            ConflictsPress::Nothing => {}
        },
        // A run continues after a switch, in the worktree where it started. An
        // event of such a run describes a worktree that the frame no longer
        // shows, and a line under the frame must describe the worktree in the
        // frame. So it posts nothing. An outcome still frees its key, because
        // one run at a time is a rule for the whole process, and that run has
        // ended.
        Event::ConflictsStarted { generation, .. } if state.is_stale(generation) => {}
        Event::ConflictsFinished { generation, .. } if state.is_stale(generation) => {
            state.conflicts.finished();
        }
        Event::IssueFinished { generation, .. } if state.is_stale(generation) => {
            state.issue.finished();
        }
        // A busy row drops the notice and does not hold it. A held notice
        // reaches the row after the outcome, and says that a run is in flight
        // when none is. See [`PushUi::post_progress`].
        Event::ConflictsStarted { branch, .. } => {
            state
                .ui
                .post_progress(crate::conflicts::running_notice(&branch));
        }
        Event::ConflictsFinished { outcome, .. } => {
            state.conflicts.finished();
            // The outcome is gsw's report about a key the user pressed, so it
            // fades. A busy row holds it until the row is free, as it holds
            // every report. The answer goes unread, because no state here
            // stands on where the line landed.
            let _ = state.ui.post_notice(outcome.line(), clock());
        }
        Event::IssueFinished { outcome, .. } => {
            state.issue.finished();
            // A run that worked says nothing: the browser is the answer. A run
            // that failed says why, in the words of another program, so it
            // waits for a key the way git's error text does.
            if let Some(message) = outcome.message() {
                state.ui.post_error(message.to_string());
            }
        }
        Event::BaseUpdateFinished(_) => {}
        Event::PushFinished(outcome) => {
            let succeeded = outcome.success;
            state.ui.finished(outcome, clock());
            // A successful push moved the upstream, so the header's arrows and
            // tracking segment are stale the moment it lands — walk now rather
            // than leaving a wrong count on screen until the next refresh. A
            // failure changed nothing in the repository, so walking would only
            // pay for a status traversal to redraw the identical frame.
            if succeeded {
                pending.force = true;
            }
        }
    }
    Flow::Continue
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
/// window is recomputed after every render. Four sources feed it — the decay
/// cadence from `next_tick` (in `hooks`), the walk the schedule owes, while
/// a countdown is on screen [`CLOCK_CADENCE`], and, while a status message is
/// ageing under the frame, [`PushUi::next_tick`]. With all four absent the loop
/// blocks indefinitely on events.
///
/// `ui` is everything the push feature puts on screen, plus the input mode that
/// goes with it. It is taken by value rather than built here because it carries
/// the terminal's color depth, which is the caller's to resolve — and owned for
/// the rest of the loop because this is the only place that knows what is
/// displayed (see [`Event::Key`] for why the reader thread must not).
///
/// The loop watches one worktree at a time, the current worktree, and it starts
/// on `start.home`. The walk, `p`, `G`, and `m` act on the current worktree. The
/// arrow keys change it through [`LoopState::switch_to`], inside [`absorb`], so
/// the next key of the same burst already acts on the new worktree.
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
/// - a walk that fails because the worktree on the screen went away goes back
///   to the home worktree, with a line that says so
///   ([`LoopState::return_home_if_gone`]);
/// - while the list of the worktrees is open, the frame is the frame of the
///   list (`render_list` in `hooks`), and every other frame is the status
///   frame (`render`);
/// - [`Event::Quit`] ends the loop, as does every sender hanging up.
///
/// A failed collect is absorbed rather than propagated because the failures are
/// overwhelmingly transient and none of the user's doing — `git gc` swapping the
/// ref store out from under the walk, a worktree being pruned, `.git` renamed
/// mid-operation. This is the other half of [`RepoHandle::reopened`]'s "never a
/// blank screen" guarantee: that fallback keeps a *handle* when the re-open
/// fails, and this keeps a *frame* when the status walk on it fails. Either half
/// alone leaves the monitor dying on a repository that is momentarily
/// unreadable. The failed walk still arms the throttle from its measured cost,
/// which also holds the check after it and any open of the home worktree — so
/// a repo that fails every walk backs off on the same duty cycle instead of
/// hot-looping — and deliberately does *not* advance `collected_at`, so the
/// stale frame goes on aging honestly rather than resetting every displayed age
/// to zero. The accepted cost: a repository deleted for good leaves a frozen
/// (but visibly aging) frame until the user quits. That is the right failure for
/// a monitor — a wrong-but-labeled-old screen beats no screen. A worktree other
/// than the home worktree that goes away for good is the one exception: the
/// loop goes back to the home worktree ([`LoopState::return_home_if_gone`]).
#[expect(
    clippy::type_complexity,
    reason = "the loop takes one generic for each of its fourteen hooks, so each hook stays \
              a plain closure that a test replaces with a fake. A type alias spells the same \
              fourteen generics, as the expectation on absorb says"
)]
fn event_loop<
    Collect,
    RenderFn,
    RenderList,
    Dims,
    Paint,
    Clock,
    Tick,
    StartPush,
    StartBaseUpdate,
    StartIssue,
    StartConflicts,
    Worktrees,
    Paths,
    Switch,
>(
    rx: &Receiver<Event>,
    debounce: Duration,
    displayed: &mut String,
    start: LoopStart,
    mut hooks: LoopHooks<
        Collect,
        RenderFn,
        RenderList,
        Dims,
        Paint,
        Clock,
        Tick,
        StartPush,
        StartBaseUpdate,
        StartIssue,
        StartConflicts,
        Worktrees,
        Paths,
        Switch,
    >,
) -> Result<()>
where
    Collect: FnMut(&WorktreePath) -> Result<Snapshot>,
    RenderFn: FnMut(&Snapshot, Dimensions, FrameTiming) -> Render,
    RenderList: FnMut(&Snapshot, Dimensions, FrameTiming, &WorktreeList) -> Render,
    Dims: Fn() -> Dimensions,
    Paint: FnMut(&str) -> Result<()>,
    Clock: Fn() -> Instant,
    Tick: Fn(Option<Duration>) -> Option<Duration>,
    StartPush: FnMut(PushCommand, &WorktreePath),
    StartBaseUpdate: FnMut(crate::update::BaseUpdateCommand, &WorktreePath),
    StartIssue: FnMut(crate::shell::ShellCommand, &WorktreePath, Generation),
    StartConflicts: FnMut(&WorktreePath, Generation),
    Worktrees: FnMut() -> Vec<WorktreeEntry>,
    Paths: FnMut() -> Vec<WorktreePath>,
    Switch: FnMut(&WorktreePath) -> Result<Snapshot, String>,
{
    let LoopStart {
        cache,
        mut freshest,
        schedule,
        ui,
        session,
        home,
    } = start;
    let mut state = LoopState {
        cache,
        schedule,
        ui,
        // The probe answers on the loop's own channel, so the key is unbound
        // until it does and the loop never waits for it.
        issue: IssueRun::new(session),
        // The loop owns the state of `m`, and the thread that measures never
        // touches it. So the one-run rule needs no lock.
        conflicts: ConflictsRun::new(),
        // The loop starts on the worktree where the user started gsw.
        current: home.clone(),
        home,
        generation: Generation::default(),
    };
    loop {
        // Wait for the first event, or — when the decay timer is enabled — wake
        // after `interval` of quiet for a tick.
        // Track *which* triggers arrived so the render below can route them: a
        // filesystem change walks git, a resize re-renders the cache at the new
        // size, a bare timeout is a decay tick.
        let mut pending = Pending::default();
        // Wait window: the soonest of the decay-tick cadence, the next walk this
        // schedule owes (a timed refresh, or a walk deferred during a cooldown),
        // and — while a countdown is on screen — the cadence that countdown
        // needs to keep moving. The clock is read only when a walk is actually
        // owed.
        let walk_wait = state
            .schedule
            .next_walk_at()
            .map(|at| at.saturating_duration_since((hooks.clock)()));
        let wait = wait_window(&[
            (hooks.next_tick)(freshest),
            walk_wait,
            state.schedule.interval.map(|_| CLOCK_CADENCE),
            state.ui.next_tick(),
        ]);
        let woke_for_timeout = match wait {
            Some(interval) => match rx.recv_timeout(interval) {
                Ok(event) => {
                    if absorb(event, &mut pending, &mut state, &mut hooks) == Flow::Quit {
                        break;
                    }
                    false
                }
                Err(RecvTimeoutError::Timeout) => true,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(event) => {
                    if absorb(event, &mut pending, &mut state, &mut hooks) == Flow::Quit {
                        break;
                    }
                    false
                }
                Err(_) => break,
            },
        };

        // Coalesce a filesystem burst: keep draining until the channel stays
        // quiet for a full `debounce` — or, once a running push has streamed a
        // line into this drain, until `PUSH_DRAIN_BUDGET` has passed since it
        // did. A burst ends on its own, so a quiet channel is the signal that
        // it has; a push's output need not end for minutes, so it gets a
        // deadline instead of a signal. A tick has no burst behind it.
        let mut quitting = false;
        if !woke_for_timeout {
            // Armed by the first line of push output this drain sees, and left
            // `None` otherwise — so a drain with no push behind it reads the
            // clock exactly as many times as it did before this deadline
            // existed, and filesystem coalescing is unchanged.
            let mut drain_until: Option<Instant> = None;
            loop {
                match rx.recv_timeout(debounce) {
                    Ok(event) => {
                        // Asked before `absorb`, which takes the event by value.
                        let streamed = matches!(event, Event::PushOutput(_));
                        if absorb(event, &mut pending, &mut state, &mut hooks) == Flow::Quit {
                            // Unlike the first wake, a quit that arrives inside
                            // the drain still paints: the events ahead of it in
                            // this burst have already been applied, and the
                            // user should see the screen they asked for before
                            // it goes away.
                            quitting = true;
                            break;
                        }
                        if streamed {
                            let due = *drain_until
                                .get_or_insert_with(|| (hooks.clock)() + PUSH_DRAIN_BUDGET);
                            if (hooks.clock)() >= due {
                                break;
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => {
                        quitting = true;
                        break;
                    }
                }
            }
        }
        let (saw_fs, saw_resize, saw_force) = (pending.fs, pending.resize, pending.force);

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
            state.schedule.force();
            true
        } else if saw_fs {
            matches!(state.schedule.on_change(now), Walk::Now)
        } else if woke_for_timeout {
            // Only a walk the schedule OWES fires here — a deferred change's
            // coalesced walk, or a timed refresh — and only once it has actually
            // fallen due: a decay tick or a clock tick that fires ahead of it (a
            // shorter wait than the walk deadline) re-renders from cache without
            // walking, so Part A and Part B compose.
            matches!(state.schedule.next_walk_at(), Some(due) if now >= due)
        } else {
            false
        };

        // Re-measure the pane before rendering, on the same two triggers as
        // before: a walk and a resize. Hoisted out of the branches so the frame
        // height below is computed from dimensions that are already current.
        if walk_now || saw_resize {
            state.cache.dims = (hooks.dimensions)();
        }

        // The walk comes before the division of the pane below. It needs no
        // pane size, and what it finds can change what goes under the frame.
        if walk_now {
            let collected = (hooks.collect)(&state.current);
            match collected {
                Ok(snapshot) => {
                    // Re-seed the collection time to the walk's start so a later
                    // decay tick or resize advances ages from *this* walk, not
                    // the previous one.
                    state.cache.collected_at = now;
                    state.cache.snapshot = snapshot;
                }
                Err(_) => {
                    // A walk can fail for reasons that are none of the user's
                    // business and usually transient: `git gc` swapping the ref
                    // store, a worktree being pruned, `.git` renamed
                    // mid-operation. Ending watch mode over that would make the
                    // whole stale-configuration fallback in
                    // [`RepoHandle::reopened`] pointless, so absorb it: keep the
                    // last good snapshot and let the next event retry.
                    // `collected_at` is pointedly NOT advanced — a collection
                    // that never happened must not reset every displayed age to
                    // "just now", or the monitor would claim freshness exactly
                    // when it has none. The frame therefore keeps aging
                    // truthfully while the repository is unreadable.
                    //
                    // One failure does not pass: the worktree on the screen
                    // went away. Then gsw goes back to the home worktree.
                    state.return_home_if_gone(
                        now,
                        &hooks.clock,
                        &mut hooks.worktree_paths,
                        &mut hooks.switch,
                    );
                }
            }
            // Measure the wall-clock cost of the git work of this wake and feed
            // it to the throttle, which arms the next cooldown (= 100·cost)
            // from it. Deliberately outside the match: a *failed* walk still
            // paid for a status traversal, and a repo that is unreadable for a
            // while fails every walk, so gating the retries on the same
            // duty-cycle budget is what keeps a permanently-deleted repo from
            // pinning a core. Deliberately after the match too: after a failed
            // walk, the check for a worktree that is gone reads the paths of
            // the worktrees, and it can open home. That is git work of this
            // wake, so the cost holds it, whether the return works or not. A
            // return that works records its open first, and this record then
            // replaces that one, from the same start.
            let cost = (hooks.clock)().saturating_duration_since(now);
            state.schedule.record(now, cost);
        }

        // The open list takes the rows under the head of the frame. A pane
        // that leaves it no row closes it, so Enter never chooses a row that
        // the user did not see. The check comes after the walk, because the
        // snapshot sets how tall the head is, and before the overlay, so a
        // message that waited for the list reaches the row on the frame that
        // closes it.
        //
        // A list that stays open stores the scroll of the window that this
        // frame shows, so the next move of the cursor starts from the window
        // that the user saw.
        match crate::list_rows(&state.cache.snapshot, state.cache.dims) {
            0 => {
                let _ = state.ui.close_list();
            }
            rows => {
                if let Some(list) = state.ui.list_mut() {
                    list.settle(rows);
                }
            }
        }

        // What the push overlay will paint under the frame, and how tall the
        // frame is left — one call, because they are one division of the pane
        // both have to share. The frame is rendered shorter by exactly what the
        // overlay took, because it is laid out to fill the pane exactly:
        // appending to a full-height frame would push its bottom row, the file
        // list, off the screen. The arithmetic deliberately lives in
        // `PushUi::overlay` rather than here — a subtraction repeated in the
        // caller is a second opinion about the same rows, and this loop must
        // not be able to hold one.
        //
        // The call can also change what the keys mean, which is why it takes
        // `&mut ui`: a question whose row a resize took away is cancelled here
        // rather than left answerable by an Enter nobody was asked for. That is
        // the backstop only. A `p` pressed in a pane that was already too short
        // raises no question in the first place — `absorb` settles that with
        // `state.cache.dims`, because no render runs between two keys of one
        // burst — so what this catches is the pane that shrank under a question
        // that did fit when it was asked. It runs after this wake's events have
        // been absorbed and before the next wake reads one, so the key a user
        // presses in reaction to what this paints is classified against the
        // mode this pane actually showed them.
        let overlay = state.ui.overlay(state.cache.dims, now);
        let frame_dims = Dimensions {
            height: overlay.frame_rows(),
            ..state.cache.dims
        };

        // Every frame advances every displayed age by the time since the last
        // walk that succeeded. A walk that succeeded on this wake moved the
        // collection time to `now`, so its frame shows every age as it was
        // collected. A walk that failed, a resize, a decay tick, and a change
        // that the throttle deferred all leave the collection time where it
        // was, so the cached snapshot goes on ageing truthfully.
        let frame_timing = timing(
            now.saturating_duration_since(state.cache.collected_at),
            &state.schedule,
            now,
        );
        // The open list of the worktrees takes the pane, so its frame replaces
        // the status frame for as long as it is open.
        let render = match state.ui.list() {
            Some(list) => {
                (hooks.render_list)(&state.cache.snapshot, frame_dims, frame_timing, list)
            }
            None => (hooks.render)(&state.cache.snapshot, frame_dims, frame_timing),
        };

        // The painted screen is the frame with the push overlay under it. They
        // are compared as one string, so a frame that did not change but an
        // overlay that did still repaints — and neither can repaint alone and
        // leave the other stale.
        let output = compose(render.output, &overlay.text());
        if should_repaint(&output, displayed) {
            (hooks.paint)(&output)?;
            *displayed = output;
        }
        freshest = render.freshest_age;

        if quitting {
            break;
        }
    }
    Ok(())
}

/// Join a frame and the push overlay into the one string that gets painted.
///
/// An empty overlay returns the frame untouched, byte for byte. That is what
/// keeps every frame gsw painted before the push feature existed identical to
/// what it paints now — including the trailing-newline handling, which is the
/// frame's business and not this function's.
fn compose(frame: String, overlay: &str) -> String {
    if overlay.is_empty() {
        return frame;
    }
    let separator = if frame.ends_with('\n') { "" } else { "\n" };
    format!("{frame}{separator}{overlay}")
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
    let tty = termsize::stdout_size().map(|(w, h)| (usize::from(w), usize::from(h)));
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
/// when the event is irrelevant.
///
/// Deliberately does **not** decide what a key means. Key semantics depend on
/// the loop's [`InputMode`], which this thread cannot read without racing the
/// loop that writes it, so a key is forwarded whole and classified by
/// [`classify_input`] once it has arrived somewhere the mode is known.
///
/// - A key press becomes [`Event::Key`], carrying the key untouched.
/// - A terminal resize becomes [`Event::Resize`].
/// - Everything else is ignored.
fn forward_input(event: CtEvent) -> Option<Event> {
    match event {
        CtEvent::Key(key) => Some(Event::Key(key)),
        CtEvent::Resize(_, _) => Some(Event::Resize),
        _ => None,
    }
}

/// What one key press means in `mode`, or `None` when it means nothing at all.
///
/// Pure and terminal-free, so the whole key table is testable without a pty:
/// a test builds a [`KeyEvent`] and a mode and reads back the [`Event`].
///
/// - A key *release* is ignored in every mode (kitty/Windows report them; only
///   a press acts).
/// - **Ctrl-C quits from every mode**, including mid-push. A monitor that
///   cannot be quit while it waits on the network is a monitor that has to be
///   killed from another pane.
/// - [`InputMode::Normal`]: `q` quits, `r` forces a refresh, `p` asks to push,
///   `G` asks for the issue of the branch, and `m` asks to measure a rebase and
///   a merge against the default branch. Up goes to the home worktree, Left to
///   the previous worktree, Right to the next worktree, and Down opens the
///   list of the worktrees.
/// - [`InputMode::Confirm`]: `y` and Enter push, `n`, Esc, and `q` cancel.
///   Nothing else acts — with a question on screen, `q` is the answer "no",
///   not "quit", `r` is not a refresh, and an arrow key is no answer at all.
///   That is why the mode exists.
/// - [`InputMode::Pushing`]: `q` quits, `r` refreshes, `G` still asks for the
///   issue — a browser conflicts with nothing a push does — and `m` still asks
///   to measure, because a measurement is read-only for the repository. `p` is
///   inert, so an impatient second press cannot start an overlapping push. The
///   arrow keys are inert too: the window under the frame belongs to the
///   worktree that pushes, so the watch stays on that worktree.
/// - [`InputMode::List`]: Up and Down move the cursor, Enter goes to the
///   worktree under it, and Esc and `q` close the list. Every other key gives
///   `None` and does nothing at all: `r` does not walk, `p` does not ask, `G`
///   and `m` start nothing, and no line leaves the row. The list takes the
///   pane, so the frame that such a key acts on is not on the screen.
/// - `M` is not bound. It does what every unbound key does in the mode.
/// - `G` acts only where `issue` says a command exists. Where it does not, the
///   key does what any other unbound key does, which is the one silent case
///   this feature has.
/// - Every other press in the three other modes is [`Event::Dismiss`], which
///   clears a status message and otherwise does nothing.
fn classify_input(key: KeyEvent, mode: InputMode, issue: IssueKey) -> Option<Event> {
    let KeyEvent {
        code,
        modifiers,
        kind,
        ..
    } = key;

    if kind == KeyEventKind::Release {
        // Ignore key releases (kitty/Windows report them); only a press acts.
        return None;
    }

    // Checked before the mode table so no mode can trap the user mid-push.
    if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
        return Some(Event::Quit);
    }

    let event = match mode {
        InputMode::Normal | InputMode::Pushing => match code {
            KeyCode::Char('q') => Event::Quit,
            KeyCode::Char('r') => Event::ForceRefresh,
            // A push already running makes a second request meaningless
            // rather than harmless.
            KeyCode::Char('p') if mode == InputMode::Normal => Event::PushRequested,
            // A browser opens beside the monitor, so a push in flight is no
            // reason to refuse. With no command behind it the key falls
            // through to `Dismiss`, which is what every unbound key gives.
            KeyCode::Char('G') if issue == IssueKey::Bound => Event::IssueRequested,
            // A measurement is read-only for the repository of the user, so a
            // push in flight is no reason to refuse it either.
            KeyCode::Char('m') => Event::ConflictsRequested,
            // The arrow keys move the watch to another worktree. The window
            // of a running push belongs to the worktree that pushes, so they
            // act in the normal mode only, as `p` does.
            KeyCode::Up if mode == InputMode::Normal => Event::GoHome,
            KeyCode::Left if mode == InputMode::Normal => Event::GoPrevious,
            KeyCode::Right if mode == InputMode::Normal => Event::GoNext,
            // Down opens the list of the worktrees. The list takes the pane,
            // and the window of a running push belongs to the worktree that
            // pushes, so it opens in the normal mode only.
            KeyCode::Down if mode == InputMode::Normal => Event::OpenList,
            _ => Event::Dismiss,
        },
        InputMode::Confirm => match code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => Event::PushConfirmed,
            KeyCode::Char('n' | 'N' | 'q') | KeyCode::Esc => Event::PushCancelled,
            _ => Event::Dismiss,
        },
        InputMode::List => match code {
            KeyCode::Up => Event::ListUp,
            KeyCode::Down => Event::ListDown,
            KeyCode::Enter => Event::ListGo,
            // `q` closes the list, as it answers "no" to the push question.
            KeyCode::Esc | KeyCode::Char('q') => Event::ListClose,
            // The list takes the pane. A key that it does not name acts on
            // nothing, because the frame that the key acts on is not on the
            // screen.
            _ => return None,
        },
    };
    Some(event)
}

/// Spawn the crossterm event-reader thread. It blocks on `event::read`, routes
/// each event through [`forward_input`] (which passes key presses through whole
/// and maps terminal resizes to [`Event::Resize`]), forwards any resulting
/// [`Event`], and exits when the receiver is gone or reading fails.
fn spawn_event_reader(tx: Sender<Event>) {
    thread::spawn(move || {
        // Loop until reading fails (terminal closed) — the `while let` exits on
        // `Err` — or a forwarded send fails because the receiver is gone.
        while let Ok(ct_event) = event::read() {
            if let Some(event) = forward_input(ct_event) {
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
    use ignore::gitignore::GitignoreBuilder;
    use termwindow::WRAPPER_CHROME_ROWS;

    /// A [`RenderConfig`] for the fixture-backed walk tests: no explicit base,
    /// no caps, no log rows, no color. Only the git work matters here — the
    /// rendering knobs are exercised by the render tests.
    pub(super) fn walk_config() -> RenderConfig {
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

    /// `resolve_home` gives the root of the work tree in the spelling of
    /// [`WorktreePath::resolve`], so the loop compares the home worktree with
    /// the paths of the list correctly. A root that went away after `main`
    /// opened it is refused, before watch mode takes the screen.
    #[test]
    fn resolve_home_gives_the_resolved_root_and_refuses_a_root_that_went_away() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("repo");
        testrepo::init_repo_at(&root);
        let handle = RepoHandle::discover(&root).expect("the fixture is a work tree");

        assert_eq!(
            resolve_home(&handle).expect("the root is there"),
            WorktreePath::resolve(&root).expect("the fixture made the root"),
        );

        std::fs::remove_dir_all(&root).expect("delete the work tree");
        let refused = resolve_home(&handle).expect_err("no directory is at the root");
        assert!(
            refused.to_string().starts_with(HOME_UNRESOLVED),
            "the refusal must say why, got {refused}",
        );
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
        testcolor::strip_ansi(frame.output.lines().next().unwrap_or_default())
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
    fn the_two_clock_numbers_sum_to_the_interval_with_nothing_else_pending() {
        // "last refresh: 1s ago, next refresh: 58s" on a 60-second interval
        // makes a reader check their arithmetic. Both numbers are printed as
        // whole seconds, so the elapsed half rounds down and the remaining half
        // must round up — then the pair reads as one interval, and the countdown
        // never claims less time than is actually left.
        //
        // One interval is what the pair sums to only in this steady state. A
        // deferred change pulls the next walk in and a costly walk's cooldown
        // pushes it out; either way the sum is the wait actually being measured,
        // which those cases cover.
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

    /// One key press, with no modifiers.
    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Both values of the new key's availability. Every old key means the
    /// same thing under each of them: the new key must move no old one.
    const BOTH_AVAILABILITIES: [IssueKey; 2] = [IssueKey::Bound, IssueKey::Unbound];

    /// Every mode a key can arrive in.
    const EVERY_MODE: [InputMode; 4] = [
        InputMode::Normal,
        InputMode::Confirm,
        InputMode::Pushing,
        InputMode::List,
    ];

    /// What a key with no meaning gives in `mode`, as [`meaning`] names it.
    ///
    /// [`Event::Dismiss`] takes a status line off the row. While the list is
    /// open, a key with no meaning gives nothing at all, because the list
    /// takes the pane. The match is total, so a mode added later must say what
    /// an unbound key does in it.
    fn unbound(mode: InputMode) -> &'static str {
        match mode {
            InputMode::Normal | InputMode::Confirm | InputMode::Pushing => "Dismiss",
            InputMode::List => "nothing",
        }
    }

    #[test]
    fn classify_input_maps_the_r_key_to_force_refresh() {
        // Pressing `r` is the manual-refresh escape hatch: the input classifier
        // must turn an `r` key PRESS into Event::ForceRefresh.
        for issue in BOTH_AVAILABILITIES {
            assert!(
                matches!(
                    classify_input(press(KeyCode::Char('r')), InputMode::Normal, issue),
                    Some(Event::ForceRefresh),
                ),
                "`r` must refresh with {issue:?}",
            );
        }
    }

    #[test]
    fn classify_input_handles_keys_and_ignores_releases() {
        // Regression guard for the rest of the classifier's contract once `r`
        // joined it: a key RELEASE is dropped (kitty/Windows emit them and only
        // a press should act), and `q` and Ctrl-C still quit.

        // A key release — even of a key we act on — is ignored.
        let r_release = KeyEvent {
            kind: KeyEventKind::Release,
            ..press(KeyCode::Char('r'))
        };
        for issue in BOTH_AVAILABILITIES {
            assert!(
                classify_input(r_release, InputMode::Normal, issue).is_none(),
                "a key release must be ignored — only a press acts",
            );

            // `q` and Ctrl-C both request a quit.
            assert!(matches!(
                classify_input(press(KeyCode::Char('q')), InputMode::Normal, issue),
                Some(Event::Quit),
            ));
            let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
            assert!(matches!(
                classify_input(ctrl_c, InputMode::Normal, issue),
                Some(Event::Quit),
            ));

            // An unrelated key press acts on nothing, but is not silence: it
            // clears a status message that may be on screen.
            assert!(matches!(
                classify_input(press(KeyCode::Char('x')), InputMode::Normal, issue),
                Some(Event::Dismiss),
            ));
        }
    }

    #[test]
    fn the_reader_forwards_key_presses_whole_and_maps_resizes() {
        // The reader thread cannot know the mode, so it must not decide
        // anything about a key. A resize means the same thing in every mode, so
        // it is classified here.
        let key = press(KeyCode::Char('p'));
        assert!(
            matches!(forward_input(CtEvent::Key(key)), Some(Event::Key(sent)) if sent == key),
            "a key must reach the loop untouched",
        );
        assert!(matches!(
            forward_input(CtEvent::Resize(80, 24)),
            Some(Event::Resize),
        ));
    }

    #[test]
    fn p_asks_to_push_only_when_nothing_else_is_happening() {
        // The push key. It opens the confirmation from the normal mode, and is
        // inert while a push is already running — an impatient second press
        // must not start an overlapping push.
        for issue in BOTH_AVAILABILITIES {
            assert!(matches!(
                classify_input(press(KeyCode::Char('p')), InputMode::Normal, issue),
                Some(Event::PushRequested),
            ));
            assert!(matches!(
                classify_input(press(KeyCode::Char('p')), InputMode::Pushing, issue),
                Some(Event::Dismiss),
            ));
        }
    }

    #[test]
    fn g_asks_for_the_issue_while_the_monitor_and_a_push_run() {
        // The issue key. A browser opens beside the monitor, which conflicts
        // with nothing a push does — so it acts in both of those modes.
        for mode in [InputMode::Normal, InputMode::Pushing] {
            assert!(
                matches!(
                    classify_input(press(KeyCode::Char('G')), mode, IssueKey::Bound),
                    Some(Event::IssueRequested),
                ),
                "`G` must ask for the issue in {mode:?}",
            );
        }
    }

    #[test]
    fn g_does_nothing_while_the_confirmation_is_up() {
        // That mode owns the answer to a question. No new key may trap the
        // user in it.
        assert!(matches!(
            classify_input(
                press(KeyCode::Char('G')),
                InputMode::Confirm,
                IssueKey::Bound
            ),
            Some(Event::Dismiss),
        ));
    }

    #[test]
    fn g_does_nothing_where_the_command_does_not_exist() {
        // Silence belongs to this case only, and it is the silence of an
        // unbound key rather than a code path of its own.
        for mode in EVERY_MODE {
            assert_eq!(
                meaning(classify_input(
                    press(KeyCode::Char('G')),
                    mode,
                    IssueKey::Unbound
                )),
                unbound(mode),
                "`G` must do nothing in {mode:?} with no command behind it",
            );
        }
    }

    #[test]
    fn lowercase_g_stays_unbound() {
        // The user asked for `G`. A shifted key and an unshifted one are two
        // keys, and only one of them was asked for.
        for mode in EVERY_MODE {
            for issue in BOTH_AVAILABILITIES {
                assert_eq!(
                    meaning(classify_input(press(KeyCode::Char('g')), mode, issue)),
                    unbound(mode),
                    "`g` must stay unbound in {mode:?} with {issue:?}",
                );
            }
        }
    }

    #[test]
    fn a_release_of_the_issue_key_is_ignored() {
        // Only a press acts, and the new key is no exception.
        let g_release = KeyEvent {
            kind: KeyEventKind::Release,
            ..press(KeyCode::Char('G'))
        };
        for mode in EVERY_MODE {
            for issue in BOTH_AVAILABILITIES {
                assert!(
                    classify_input(g_release, mode, issue).is_none(),
                    "a release of `G` must be ignored in {mode:?} with {issue:?}",
                );
            }
        }
    }

    #[test]
    fn m_measures_while_the_monitor_and_a_push_run_and_never_answers_the_question() {
        // A measurement is read-only for the repository of the user, so a
        // push in flight is no reason to refuse it. A question on screen owns
        // its answer, so `m` there is a key with no meaning, and it must never
        // push or cancel. The match on the mode is total, so a mode added
        // later must say what `m` means in it.
        let m_release = KeyEvent {
            kind: KeyEventKind::Release,
            ..press(KeyCode::Char('m'))
        };
        for mode in EVERY_MODE {
            for issue in BOTH_AVAILABILITIES {
                let m = classify_input(press(KeyCode::Char('m')), mode, issue);
                match mode {
                    InputMode::Normal | InputMode::Pushing => assert!(
                        matches!(m, Some(Event::ConflictsRequested)),
                        "`m` must ask to measure in {mode:?} with {issue:?}",
                    ),
                    InputMode::Confirm => assert!(
                        matches!(m, Some(Event::Dismiss)),
                        "`m` must not answer the push question with {issue:?}",
                    ),
                    // The list takes the pane, so a measurement of the frame
                    // under it is a key the user pressed at nothing.
                    InputMode::List => assert!(
                        m.is_none(),
                        "`m` must do nothing while the list is open with {issue:?}",
                    ),
                }

                // A shifted key and an unshifted one are two keys, and only
                // one of them was asked for.
                assert_eq!(
                    meaning(classify_input(press(KeyCode::Char('M')), mode, issue)),
                    unbound(mode),
                    "`M` must stay unbound in {mode:?} with {issue:?}",
                );

                // Only a press acts, and the new key is no exception.
                assert!(
                    classify_input(m_release, mode, issue).is_none(),
                    "a release of `m` must be ignored in {mode:?} with {issue:?}",
                );
            }
        }
    }

    #[test]
    fn the_confirmation_accepts_y_and_enter() {
        for code in [KeyCode::Char('y'), KeyCode::Char('Y'), KeyCode::Enter] {
            for issue in BOTH_AVAILABILITIES {
                assert!(
                    matches!(
                        classify_input(press(code), InputMode::Confirm, issue),
                        Some(Event::PushConfirmed),
                    ),
                    "{code:?} must confirm the push with {issue:?}",
                );
            }
        }
    }

    #[test]
    fn the_confirmation_is_cancelled_by_n_esc_and_q() {
        // `q` cancels rather than quits while a question is on screen: the safe
        // reading of "get me out of here" is backing out of the push, not
        // ending the session with a prompt still up.
        for code in [
            KeyCode::Char('n'),
            KeyCode::Char('N'),
            KeyCode::Char('q'),
            KeyCode::Esc,
        ] {
            for issue in BOTH_AVAILABILITIES {
                assert!(
                    matches!(
                        classify_input(press(code), InputMode::Confirm, issue),
                        Some(Event::PushCancelled),
                    ),
                    "{code:?} must cancel the push with {issue:?}",
                );
            }
        }
    }

    #[test]
    fn the_confirmation_ignores_the_ordinary_keys() {
        // With a question on screen, `r` must not refresh and `p` must not
        // re-ask. Anything that is not an answer does nothing.
        for code in [KeyCode::Char('r'), KeyCode::Char('p'), KeyCode::Char('x')] {
            for issue in BOTH_AVAILABILITIES {
                assert!(
                    matches!(
                        classify_input(press(code), InputMode::Confirm, issue),
                        Some(Event::Dismiss),
                    ),
                    "{code:?} must not act while the confirmation is up",
                );
            }
        }
    }

    #[test]
    fn ctrl_c_quits_from_every_mode() {
        // A monitor that cannot be quit while it waits on the network is one
        // that has to be killed from another pane.
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        for mode in EVERY_MODE {
            for issue in BOTH_AVAILABILITIES {
                assert!(
                    matches!(classify_input(ctrl_c, mode, issue), Some(Event::Quit)),
                    "Ctrl-C must quit from {mode:?}",
                );
            }
        }
    }

    #[test]
    fn q_and_r_still_work_while_a_push_runs() {
        // The push runs off this thread, so the monitor stays live underneath
        // it: quitting and refreshing keep working.
        for issue in BOTH_AVAILABILITIES {
            assert!(matches!(
                classify_input(press(KeyCode::Char('q')), InputMode::Pushing, issue),
                Some(Event::Quit),
            ));
            assert!(matches!(
                classify_input(press(KeyCode::Char('r')), InputMode::Pushing, issue),
                Some(Event::ForceRefresh),
            ));
        }
    }

    /// The four arrow keys.
    const ARROWS: [KeyCode; 4] = [KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right];

    /// What [`classify_input`] gave, as a name that a failed assertion can
    /// print. The arrow tests compare these names.
    fn meaning(event: Option<Event>) -> &'static str {
        match event {
            Some(Event::GoHome) => "GoHome",
            Some(Event::GoPrevious) => "GoPrevious",
            Some(Event::GoNext) => "GoNext",
            Some(Event::OpenList) => "OpenList",
            Some(Event::ListUp) => "ListUp",
            Some(Event::ListDown) => "ListDown",
            Some(Event::ListGo) => "ListGo",
            Some(Event::ListClose) => "ListClose",
            Some(Event::Quit) => "Quit",
            Some(Event::Dismiss) => "Dismiss",
            Some(Event::PushConfirmed) => "PushConfirmed",
            Some(Event::PushCancelled) => "PushCancelled",
            Some(_) => "another event",
            None => "nothing",
        }
    }

    /// The keys of the table of the input modes: the four arrow keys, Enter,
    /// and Esc, in that order.
    const TABLE_KEYS: [KeyCode; 6] = [
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Enter,
        KeyCode::Esc,
    ];

    #[test]
    fn each_arrow_key_enter_and_esc_mean_one_thing_in_each_input_mode() {
        // The whole table, as the issue states it. In Normal, Up goes home,
        // Left and Right go to the neighbours in path order, and Down opens
        // the list. A question owns its answer, so an arrow key there answers
        // nothing. A push in flight owns the window under the frame, so an
        // arrow key there does nothing. In the list, Up and Down move the
        // cursor, Enter goes, Esc closes, and Left and Right do nothing at
        // all.
        let table: [(InputMode, [&str; 6]); 4] = [
            (
                InputMode::Normal,
                [
                    "GoHome",
                    "OpenList",
                    "GoPrevious",
                    "GoNext",
                    "Dismiss",
                    "Dismiss",
                ],
            ),
            (
                InputMode::Confirm,
                [
                    "Dismiss",
                    "Dismiss",
                    "Dismiss",
                    "Dismiss",
                    "PushConfirmed",
                    "PushCancelled",
                ],
            ),
            (InputMode::Pushing, ["Dismiss"; 6]),
            (
                InputMode::List,
                [
                    "ListUp",
                    "ListDown",
                    "nothing",
                    "nothing",
                    "ListGo",
                    "ListClose",
                ],
            ),
        ];
        assert_eq!(
            table.map(|(mode, _)| mode),
            EVERY_MODE,
            "the table must cover every input mode",
        );
        for issue in BOTH_AVAILABILITIES {
            for (mode, meanings) in table {
                for (code, expected) in TABLE_KEYS.into_iter().zip(meanings) {
                    assert_eq!(
                        meaning(classify_input(press(code), mode, issue)),
                        expected,
                        "{code:?} in {mode:?} with {issue:?}",
                    );
                }
            }
        }
    }

    #[test]
    fn in_the_list_q_closes_it_and_every_other_key_does_nothing() {
        // `q` closes the list, as `q` answers "no" to the push question. The
        // list takes the pane, so every other key does nothing at all: `r`
        // does not walk, `p` does not ask, `G` and `m` start nothing, and no
        // key takes a line off the row. Ctrl-C still quits.
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        for issue in BOTH_AVAILABILITIES {
            assert_eq!(
                meaning(classify_input(
                    press(KeyCode::Char('q')),
                    InputMode::List,
                    issue
                )),
                "ListClose",
                "`q` must close the list with {issue:?}",
            );
            for code in [
                KeyCode::Char('r'),
                KeyCode::Char('p'),
                KeyCode::Char('G'),
                KeyCode::Char('m'),
                KeyCode::Char('y'),
                KeyCode::Char('n'),
                KeyCode::Char('Q'),
                KeyCode::Char('x'),
                KeyCode::Tab,
                KeyCode::Backspace,
                KeyCode::PageDown,
            ] {
                assert_eq!(
                    meaning(classify_input(press(code), InputMode::List, issue)),
                    "nothing",
                    "{code:?} must do nothing in the list with {issue:?}",
                );
            }
            assert_eq!(
                meaning(classify_input(ctrl_c, InputMode::List, issue)),
                "Quit",
                "Ctrl-C must quit from the list with {issue:?}",
            );
        }
    }

    #[test]
    fn the_arrow_keys_never_answer_the_push_question_and_do_nothing_while_a_push_runs() {
        // A question on the screen owns its answer, so an arrow key there does
        // what an unbound key does, and never pushes or cancels. A push in
        // flight owns the window under the frame, and that window belongs to
        // the worktree that pushes, so an arrow key does nothing then either.
        // Enter and Esc keep their meaning at the question.
        for issue in BOTH_AVAILABILITIES {
            for mode in [InputMode::Confirm, InputMode::Pushing] {
                for code in ARROWS {
                    assert_eq!(
                        meaning(classify_input(press(code), mode, issue)),
                        "Dismiss",
                        "{code:?} in {mode:?} with {issue:?}",
                    );
                }
            }
            assert_eq!(
                meaning(classify_input(
                    press(KeyCode::Enter),
                    InputMode::Confirm,
                    issue
                )),
                "PushConfirmed",
                "Enter must still push with {issue:?}",
            );
            assert_eq!(
                meaning(classify_input(
                    press(KeyCode::Esc),
                    InputMode::Confirm,
                    issue
                )),
                "PushCancelled",
                "Esc must still cancel with {issue:?}",
            );
        }
    }

    #[test]
    fn a_release_of_an_arrow_key_is_ignored_in_every_mode() {
        // Only a press acts, and the arrow keys are no exception.
        for mode in EVERY_MODE {
            for issue in BOTH_AVAILABILITIES {
                for code in ARROWS {
                    let release = KeyEvent {
                        kind: KeyEventKind::Release,
                        ..press(code)
                    };
                    assert!(
                        classify_input(release, mode, issue).is_none(),
                        "a release of {code:?} must be ignored in {mode:?} with {issue:?}",
                    );
                }
            }
        }
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
    pub(super) const TEST_DEBOUNCE: Duration = Duration::from_millis(20);

    /// No timed refresh: the loop under test is purely event-driven, so a walk
    /// can only come from a filesystem change. Every test that predates the
    /// timed refresh passes this, keeping its subject isolated from it.
    fn no_timed_refresh() -> WalkSchedule {
        WalkSchedule::unscheduled()
    }

    /// A `next_tick` that always disables the timer, so the loop blocks purely
    /// on channel events. The event-driven tests use this to stay independent
    /// of the decay-timer behavior, which has its own dedicated tests.
    pub(super) fn timer_off(_freshest: Option<Duration>) -> Option<Duration> {
        None
    }

    /// The home worktree of the loop tests in this module. It is a fake path:
    /// no hook of these tests touches the filesystem through it.
    pub(super) fn loop_home() -> WorktreePath {
        WorktreePath::fake("/code/home")
    }

    /// A `switch` hook that refuses every switch, and so changes nothing. No
    /// loop test that uses it presses an arrow key, so the loop never calls it.
    pub(super) fn no_switch(_target: &WorktreePath) -> Result<Snapshot, String> {
        Err("this loop test watches one worktree".to_string())
    }

    /// A `render_list` hook for the loop tests that never open the list of the
    /// worktrees. Their `worktrees` hook gives no worktree, so Down opens no
    /// list, and the loop never calls it.
    pub(super) fn no_list(
        _snapshot: &Snapshot,
        _dims: Dimensions,
        _timing: FrameTiming,
        _list: &WorktreeList,
    ) -> Render {
        frame("LIST")
    }

    /// Build a [`Render`] with the given frame and no freshest age — enough for
    /// the event-driven loop tests, which don't exercise the cadence.
    pub(super) fn frame(output: &str) -> Render {
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
            push_remote: None,
            worktree: None,
        }
    }

    /// Dimensions used by loop tests that don't exercise resize.
    pub(super) const TEST_DIMS: Dimensions = Dimensions {
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
    ///
    /// It also counts the reads, which a frozen clock cannot. Two reads of a
    /// frozen clock give the same instant as one read, so a frozen clock hides
    /// a read the code makes and does not need. A clock that moves on every
    /// read turns that extra read into a whole step, which a test can hold an
    /// interval against.
    pub(super) fn stepping_clock(base: Instant, step: Duration) -> impl Fn() -> Instant {
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
            LoopStart {
                cache: seeded_cache(base),
                freshest: Some(Duration::ZERO),
                schedule: WalkSchedule::new(Some(interval), base, Duration::ZERO),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(collected_at),
                freshest: Some(Duration::ZERO),
                schedule: WalkSchedule::new(
                    Some(Duration::from_secs(60)),
                    collected_at,
                    Duration::ZERO,
                ),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| Ok(empty_snapshot()),
                render: |_snap: &Snapshot, _dims: Dimensions, timing: FrameTiming| {
                    seen = Some(timing);
                    let _ = tx.send(Event::Quit);
                    frame("tick")
                },
                dimensions: || TEST_DIMS,
                paint: |_output: &str| Ok(()),
                clock: || clock_at,
                next_tick: |_freshest| Some(Duration::from_millis(5)),
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(base),
                freshest: Some(Duration::ZERO),
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(now),
                freshest: None,
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(now),
                freshest: None,
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(now),
                freshest: None,
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(now),
                freshest: Some(Duration::ZERO),
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| Ok(empty_snapshot()),
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(now),
                freshest: Some(Duration::from_secs(30)),
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| Ok(empty_snapshot()),
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(collected_at),
                freshest: Some(Duration::ZERO),
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(now),
                freshest: None,
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(base),
                freshest: Some(Duration::ZERO),
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| Ok(empty_snapshot()),
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(base),
                freshest: None, // decay timer off: isolate the throttle from tick behavior
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(base),
                freshest: Some(Duration::ZERO),
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(base),
                freshest: Some(Duration::ZERO),
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(base),
                freshest: None, // decay timer off: isolate the throttle from tick behavior
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(base),
                freshest: None, // decay timer off: isolate the throttle from tick behavior
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(base),
                freshest: None, // decay timer off: isolate the forced walk from tick behavior
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache,
                freshest: None, // decay timer off: isolate the failed walk from tick behavior
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache: seeded_cache(base),
                freshest: None, // decay timer off: isolate the throttle from tick behavior
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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
            LoopStart {
                cache,
                freshest: None, // decay timer off: isolate recovery from tick behavior
                schedule: no_timed_refresh(),
                ui: PushUi::new(false),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| {
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
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
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

#[cfg(test)]
mod push_loop_tests {
    use super::tests::{
        frame, loop_home, no_list, no_switch, stepping_clock, timer_off, TEST_DEBOUNCE, TEST_DIMS,
    };
    use super::*;
    use crate::conflicts::ConflictsOutcome;
    use crate::push::PushOutcome;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use testcolor::strip_ansi;
    use unicode_width::UnicodeWidthStr;

    /// A snapshot on an untracked `gsw-push` with `origin` available, so the
    /// push feature has something real to plan.
    fn pushable_snapshot() -> Snapshot {
        Snapshot {
            branch: "gsw-push".into(),
            base: "main".into(),
            commits_ahead: 2,
            commits_behind: 0,
            files: Vec::new(),
            log: Vec::new(),
            upstream: None,
            operation: None,
            push_remote: Some("origin".into()),
            worktree: None,
        }
    }

    fn cache_at(collected_at: Instant) -> SnapshotCache {
        cache_in(collected_at, TEST_DIMS)
    }

    fn cache_in(collected_at: Instant, dims: Dimensions) -> SnapshotCache {
        SnapshotCache {
            snapshot: pushable_snapshot(),
            collected_at,
            dims,
        }
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// A [`PushUi`] holding the message a push that succeeded at `at` leaves
    /// behind — the state the loop has to take back off the screen by itself.
    fn pushed_ui(at: Instant) -> PushUi {
        let mut ui = PushUi::new(false);
        ui.request(&pushable_snapshot(), TEST_DIMS, at);
        ui.confirm(at);
        ui.finished(
            crate::push::PushOutcome {
                success: true,
                output: String::new(),
            },
            at,
        );
        ui
    }

    /// What one loop run observed. The loop takes its hooks by value, so the
    /// counters live behind `RefCell` and are read after it returns.
    #[derive(Default)]
    struct Seen {
        collects: usize,
        pushes: Vec<PushCommand>,
        frame_heights: Vec<usize>,
        /// Every screen the loop actually painted, in order. The last of them
        /// is what the returned `displayed` holds; the list is what a test
        /// about *when* the screen moved needs, because a loop that paints the
        /// right thing once at the end and a loop that paints it as it happens
        /// leave the same final screen behind.
        paints: Vec<String>,
        /// Every issue command the loop started a run of, in order.
        issue_runs: Vec<crate::shell::ShellCommand>,
        /// How many measurements the loop started with `m`.
        conflict_runs: usize,
        /// Every worktree the `switch` hook was asked to open, in order,
        /// with the refused ones.
        switches: Vec<WorktreePath>,
        /// How many times the loop read the list of the worktrees with the
        /// labels. Only Down reads it.
        listings: usize,
        /// How many times the loop read the paths of the worktrees: Left,
        /// Right, and the check after a failed walk.
        path_listings: usize,
        /// The worktree each walk read, in order.
        collected_from: Vec<WorktreePath>,
        /// The worktree each push started in, in the order of
        /// [`Seen::pushes`].
        push_paths: Vec<WorktreePath>,
        /// Every rebase or merge of the base the loop started, in order.
        base_updates: Vec<crate::update::BaseUpdateCommand>,
        /// The worktree each of those started in, in the order of
        /// [`Seen::base_updates`].
        base_update_paths: Vec<WorktreePath>,
        /// The worktree and the generation of each run of the issue command,
        /// in the order of [`Seen::issue_runs`].
        issue_paths: Vec<(WorktreePath, Generation)>,
        /// The worktree and the generation of each run of `m`, in order.
        conflict_paths: Vec<(WorktreePath, Generation)>,
        /// The timing of each frame the loop rendered, in order.
        timings: Vec<FrameTiming>,
    }

    /// Run the loop over a pre-loaded event queue and report what it did.
    ///
    /// Every event is queued before the loop starts, so the first wake takes one
    /// and the debounce drain absorbs the rest — the whole sequence is applied
    /// against one render, which is exactly the state the assertions are about.
    /// The queue must end with [`Event::Quit`] or the loop blocks.
    fn run_loop(events: Vec<Event>) -> (String, Seen) {
        run_loop_with(events, TEST_DIMS, |_frame_dims| "FRAME".to_string())
    }

    /// [`run_loop`] on a remote shell.
    ///
    /// A helper of its own rather than a parameter of [`run_loop`], because
    /// every other key means the same thing in both sessions and only the `G`
    /// key reads this.
    fn run_loop_remote(events: Vec<Event>) -> (String, Seen) {
        let base = Instant::now();
        run_loop_in_session(
            events,
            TEST_DIMS,
            |_frame_dims| "FRAME".to_string(),
            move || base,
            crate::remote::Session::Remote,
        )
    }

    /// Run the loop in a pane of the given size, with a frame that really fills
    /// the rows it was given.
    ///
    /// [`run_loop`]'s canned one-line frame cannot show an overflow: what lands
    /// in the pane is the frame's rows plus the overlay's, so a test about how
    /// many rows are painted needs a render hook that emits as many rows as it
    /// was asked for — which is what the production renderer does.
    fn run_loop_in_pane(events: Vec<Event>, dims: Dimensions) -> (String, Seen) {
        run_loop_with(events, dims, |frame_dims| {
            (1..=frame_dims.height)
                .map(|row| format!("ROW{row}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    /// The shared body: pre-load the queue, run the loop against `dims`, and
    /// report the last painted screen plus what the hooks saw.
    ///
    /// The clock is frozen, so nothing on screen ages between paints and every
    /// assertion is about the queued events alone.
    fn run_loop_with(
        events: Vec<Event>,
        dims: Dimensions,
        render_frame: fn(Dimensions) -> String,
    ) -> (String, Seen) {
        let base = Instant::now();
        run_loop_clocked(events, dims, render_frame, move || base)
    }

    /// [`run_loop_with`] with the loop's clock supplied by the caller.
    ///
    /// Split out because one test is about a *deadline* rather than about
    /// events, and a clock that steps on every read is what crosses a deadline
    /// without sleeping. `clock` is read once up front for the cache's
    /// collection time, so a frozen clock lands on exactly the instant this
    /// helper used before the split.
    fn run_loop_clocked<Clock: Fn() -> Instant>(
        events: Vec<Event>,
        dims: Dimensions,
        render_frame: fn(Dimensions) -> String,
        clock: Clock,
    ) -> (String, Seen) {
        run_loop_in_session(
            events,
            dims,
            render_frame,
            clock,
            crate::remote::Session::Local,
        )
    }

    /// [`run_loop_clocked`] with the session supplied by the caller.
    ///
    /// One more knob, added the way every knob above it was added: the tests
    /// that do not name a session read exactly as they did, and they all get
    /// the local one, which is the session every one of them assumed.
    fn run_loop_in_session<Clock: Fn() -> Instant>(
        events: Vec<Event>,
        dims: Dimensions,
        render_frame: fn(Dimensions) -> String,
        clock: Clock,
        session: crate::remote::Session,
    ) -> (String, Seen) {
        drive(
            events,
            Setup {
                dims,
                ui: PushUi::new(false),
                measured: dims,
                render: Box::new(move |_snapshot: &Snapshot, frame_dims: Dimensions| {
                    render_frame(frame_dims)
                }),
                session,
                schedule: no_timed_refresh_for_push(),
                world: World::alone(),
            },
            clock,
        )
    }

    /// The text that the render hook of a loop test paints for a snapshot, in
    /// a frame of the given size.
    type FrameText = Box<dyn Fn(&Snapshot, Dimensions) -> String>;

    /// The text that the `render_list` hook of a loop test paints: `LIST`,
    /// the branch of the snapshot, and the label of each row that a pane of
    /// `dims` shows, with `>` before the cursor row and `⌂` after the home
    /// row. For example `LIST bravo: alpha >bravo⌂ charlie`.
    ///
    /// The branch says which snapshot is under the list. The rows are the
    /// window of [`crate::list_rows`] rows, as in the frame that production
    /// draws.
    fn list_frame_of(snapshot: &Snapshot, dims: Dimensions, list: &WorktreeList) -> String {
        let rows: Vec<String> = list
            .window(crate::list_rows(snapshot, dims))
            .iter()
            .map(|row| {
                let cursor = if row.cursor { ">" } else { "" };
                let home = if row.home { "⌂" } else { "" };
                format!("{cursor}{}{home}", row.entry.label)
            })
            .collect();
        format!("LIST {}: {}", snapshot.branch, rows.join(" "))
    }

    /// How one loop run is set up.
    ///
    /// [`run_loop_in_session`] fills it for the tests that predate the arrow
    /// keys: one worktree, a frame that ignores its snapshot, and no timed
    /// refresh. The worktree tests fill it with more worktrees and a frame
    /// that names its snapshot.
    struct Setup {
        /// The pane the loop renders into.
        dims: Dimensions,
        /// What the loop shows under the frame when it starts, and the input
        /// mode that goes with it.
        ///
        /// Almost every test starts with nothing on the row and puts a question
        /// there with a key. The keys of the rebase and the merge arrive in a
        /// later slice, so a test about those reaches the row through
        /// [`PushUi::request_base_update`] and hands the loop what it built.
        ui: PushUi,
        /// The pane that the `dimensions` hook measures, which the loop reads
        /// at each walk and each resize. It differs from `dims` only in a test
        /// of a pane that changes size under the loop.
        measured: Dimensions,
        /// What the render hook paints for a snapshot, in a frame of the given
        /// size.
        render: FrameText,
        /// Where the person who reads the screen sits.
        session: crate::remote::Session,
        /// The walk schedule the loop starts from.
        schedule: WalkSchedule,
        /// The worktrees, and how each switch comes out.
        world: World,
    }

    /// The name of the one worktree of [`World::alone`]. Its snapshot is
    /// [`pushable_snapshot`], because [`snapshot_of`] names the branch after
    /// the worktree.
    const ALONE: &str = "gsw-push";

    /// The first of the three worktrees of [`World::three`], in path order.
    const ALPHA: &str = "alpha";

    /// The second of the three worktrees of [`World::three`], and its home.
    const BRAVO: &str = "bravo";

    /// The last of the three worktrees of [`World::three`], in path order.
    const CHARLIE: &str = "charlie";

    /// The worktree `/code/<name>`. No filesystem call touches it.
    fn worktree(name: &str) -> WorktreePath {
        WorktreePath::fake(format!("/code/{name}"))
    }

    /// The snapshot of the worktree at `path`: [`pushable_snapshot`], on a
    /// branch named after the last component of the path. A frame then says
    /// which worktree it shows, and a push says which branch it pushes.
    fn snapshot_of(path: &WorktreePath) -> Snapshot {
        let name = path
            .as_path()
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .expect("a fake worktree path has a name");
        Snapshot {
            branch: name.to_string(),
            ..pushable_snapshot()
        }
    }

    /// The reason the fake `switch` hook gives for a worktree it refuses.
    const REFUSED: &str = "no such worktree";

    /// The reason the fake `collect` hook gives for a walk that fails.
    const UNWALKABLE: &str = "the walk failed";

    /// The worktrees a loop run can move between, and how each switch comes
    /// out.
    ///
    /// The fake `worktrees` hook gives [`World::listed`], and the fake
    /// `worktree_paths` hook gives the paths of those worktrees. Each read of
    /// either hook moves the clock of the loop on by [`World::list_cost`].
    /// The fake `switch` hook gives [`snapshot_of`] its target, or refuses a
    /// target in [`World::refused`] with [`REFUSED`].
    struct World {
        /// The worktree where gsw started.
        home: WorktreePath,
        /// Every worktree, sorted by path, as the listing gives them. The
        /// `worktrees` hook gives these entries at each press of Down, and the
        /// `worktree_paths` hook gives their paths, in the same order.
        listed: Vec<WorktreeEntry>,
        /// The worktrees whose switch fails, as the switch to a worktree that
        /// stopped existing fails.
        refused: Vec<WorktreePath>,
        /// The worktrees that go away as the loop switches to them, as `git
        /// worktree remove` in another pane makes a worktree go. The switch
        /// works. From then on, the list does not hold the worktree, and every
        /// walk of it and every switch to it fails.
        vanishing: Vec<WorktreePath>,
        /// The worktrees that are gone from the start: the list does not hold
        /// them, and every walk of them and every switch to them fails.
        removed: Vec<WorktreePath>,
        /// The worktrees whose walks fail while the list still holds them, as
        /// a walk fails when `git gc` swaps the ref store under it.
        unreadable: Vec<WorktreePath>,
        /// How long each read of a list of the worktrees takes. Each read
        /// moves the clock of the loop on by this cost, as a real read of the
        /// list takes time. It is zero unless a test sets it.
        list_cost: Duration,
    }

    impl World {
        /// The worktrees `/code/<name>` for each of `names`, with the home
        /// worktree at `home`. Nothing is refused.
        ///
        /// # Panics
        ///
        /// Panics when `names` is not sorted. The moves search by sort order,
        /// so an unsorted list gives wrong answers with no panic of its own.
        fn of(names: &[&str], home: &str) -> Self {
            let listed: Vec<WorktreeEntry> = names
                .iter()
                .map(|name| WorktreeEntry {
                    path: worktree(name),
                    label: (*name).to_string(),
                })
                .collect();
            assert!(
                listed.iter().map(|entry| &entry.path).is_sorted(),
                "the fake list must be sorted by path, as the listing gives it: {names:?}",
            );
            Self {
                home: worktree(home),
                listed,
                refused: Vec::new(),
                vanishing: Vec::new(),
                removed: Vec::new(),
                unreadable: Vec::new(),
                list_cost: Duration::ZERO,
            }
        }

        /// The one worktree that every test before the arrow keys watched.
        fn alone() -> Self {
            Self::of(&[ALONE], ALONE)
        }

        /// Three worktrees in path order, with the home worktree between the
        /// two others.
        fn three() -> Self {
            Self::three_at(BRAVO)
        }

        /// The three worktrees of [`World::three`], with the home worktree at
        /// `home`.
        fn three_at(home: &str) -> Self {
            Self::of(&[ALPHA, BRAVO, CHARLIE], home)
        }

        /// This world, where the switch to the worktree `name` fails with
        /// [`REFUSED`].
        fn refusing(mut self, name: &str) -> Self {
            self.refused.push(worktree(name));
            self
        }

        /// This world, where the worktree `name` goes away as the loop
        /// switches to it. See [`World::vanishing`].
        fn vanishing(mut self, name: &str) -> Self {
            self.vanishing.push(worktree(name));
            self
        }

        /// This world, where the worktree `name` is gone from the start. See
        /// [`World::removed`].
        fn removed(mut self, name: &str) -> Self {
            self.removed.push(worktree(name));
            self
        }

        /// This world, where every walk of the worktree `name` fails while
        /// the list still holds it. See [`World::unreadable`].
        fn unreadable(mut self, name: &str) -> Self {
            self.unreadable.push(worktree(name));
            self
        }

        /// This world, where each read of a list of the worktrees moves the
        /// clock of the loop on by `cost`. See [`World::list_cost`].
        fn slow_list(mut self, cost: Duration) -> Self {
            self.list_cost = cost;
            self
        }
    }

    /// The shared body of every helper above: pre-load the queue, run the loop
    /// as `setup` says, and report the last painted screen plus what the hooks
    /// saw.
    ///
    /// `clock` is read once up front for the cache's collection time, so a
    /// frozen clock lands on exactly the instant the loop reads later.
    fn drive<Clock: Fn() -> Instant>(
        events: Vec<Event>,
        setup: Setup,
        clock: Clock,
    ) -> (String, Seen) {
        drive_bursts(vec![events], setup, clock)
    }

    /// [`drive`] over several bursts of events, with one frame for each.
    ///
    /// The first burst is queued before the loop starts, as [`drive`] queues
    /// its events. The frame of each burst queues the next burst, so the loop
    /// takes each burst in one wake and draws one frame for it. A test then
    /// reads in [`Seen::paints`] what each burst put on the screen. A burst
    /// that changes nothing on the screen paints nothing. The last burst must
    /// end with [`Event::Quit`], or the loop blocks.
    fn drive_bursts<Clock: Fn() -> Instant>(
        bursts: Vec<Vec<Event>>,
        setup: Setup,
        clock: Clock,
    ) -> (String, Seen) {
        let Setup {
            dims,
            ui,
            measured,
            render,
            session,
            schedule,
            world,
        } = setup;
        let (tx, rx) = mpsc::channel();
        let mut bursts = VecDeque::from(bursts);
        for event in bursts.pop_front().unwrap_or_default() {
            tx.send(event).expect("queue event");
        }
        let later = RefCell::new(bursts);
        // Each wake of the loop draws exactly one frame, through `render` or
        // through `render_list`, so each of the two queues the next burst.
        let queue_next_burst = || {
            if let Some(burst) = later.borrow_mut().pop_front() {
                for event in burst {
                    tx.send(event).expect("queue event");
                }
            }
        };
        let seen = RefCell::new(Seen::default());
        // The worktrees that are gone: the removed ones from the start, and
        // each vanishing one from the switch that reached it.
        let removed = RefCell::new(world.removed.clone());
        let mut displayed = String::new();
        // Each read of a list of the worktrees moves the clock of the loop on
        // by `world.list_cost`, and `skew` holds the sum. Each read of the
        // loop clock reads `clock` exactly once, so a clock that counts its
        // reads counts the same reads as before. The start of the harness
        // goes through the loop clock too.
        let skew = std::cell::Cell::new(Duration::ZERO);
        let loop_clock = || clock() + skew.get();
        let base = loop_clock();

        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            LoopStart {
                cache: SnapshotCache {
                    snapshot: snapshot_of(&world.home),
                    collected_at: base,
                    dims,
                },
                freshest: None,
                schedule,
                ui,
                session,
                home: world.home.clone(),
            },
            LoopHooks {
                collect: |current: &WorktreePath| {
                    let mut seen = seen.borrow_mut();
                    seen.collects += 1;
                    seen.collected_from.push(current.clone());
                    if removed.borrow().contains(current) || world.unreadable.contains(current) {
                        anyhow::bail!("{UNWALKABLE}: {}", current.as_path().display());
                    }
                    Ok(snapshot_of(current))
                },
                render: |snap: &Snapshot, frame_dims: Dimensions, timing: FrameTiming| {
                    queue_next_burst();
                    let mut seen = seen.borrow_mut();
                    seen.frame_heights.push(frame_dims.height);
                    seen.timings.push(timing);
                    frame(&render(snap, frame_dims))
                },
                render_list: |snap: &Snapshot,
                              frame_dims: Dimensions,
                              _timing: FrameTiming,
                              list: &WorktreeList| {
                    queue_next_burst();
                    frame(&list_frame_of(snap, frame_dims, list))
                },
                dimensions: move || measured,
                paint: |output: &str| {
                    seen.borrow_mut().paints.push(output.to_string());
                    Ok(())
                },
                clock: loop_clock,
                next_tick: timer_off,
                start_push: |command: PushCommand, current: &WorktreePath| {
                    let mut seen = seen.borrow_mut();
                    seen.pushes.push(command);
                    seen.push_paths.push(current.clone());
                },
                start_base_update: |command: crate::update::BaseUpdateCommand,
                                    current: &WorktreePath| {
                    let mut seen = seen.borrow_mut();
                    seen.base_updates.push(command);
                    seen.base_update_paths.push(current.clone());
                },
                start_issue: |command: crate::shell::ShellCommand,
                              current: &WorktreePath,
                              generation: Generation| {
                    let mut seen = seen.borrow_mut();
                    seen.issue_runs.push(command);
                    seen.issue_paths.push((current.clone(), generation));
                },
                start_conflicts: |current: &WorktreePath, generation: Generation| {
                    let mut seen = seen.borrow_mut();
                    seen.conflict_runs += 1;
                    seen.conflict_paths.push((current.clone(), generation));
                },
                worktrees: || {
                    seen.borrow_mut().listings += 1;
                    skew.set(skew.get() + world.list_cost);
                    let removed = removed.borrow();
                    world
                        .listed
                        .iter()
                        .filter(|entry| !removed.contains(&entry.path))
                        .cloned()
                        .collect()
                },
                worktree_paths: || {
                    seen.borrow_mut().path_listings += 1;
                    skew.set(skew.get() + world.list_cost);
                    let removed = removed.borrow();
                    world
                        .listed
                        .iter()
                        .filter(|entry| !removed.contains(&entry.path))
                        .map(|entry| entry.path.clone())
                        .collect()
                },
                switch: |target: &WorktreePath| {
                    seen.borrow_mut().switches.push(target.clone());
                    if world.refused.contains(target) || removed.borrow().contains(target) {
                        return Err(format!("{REFUSED}: {}", target.as_path().display()));
                    }
                    // A vanishing worktree exists at the switch, and goes away
                    // at once.
                    if world.vanishing.contains(target) {
                        removed.borrow_mut().push(target.clone());
                    }
                    Ok(snapshot_of(target))
                },
            },
        )
        .expect("loop");

        (displayed, seen.into_inner())
    }

    /// Purely event-driven: no timed refresh, so any walk came from an event.
    fn no_timed_refresh_for_push() -> WalkSchedule {
        WalkSchedule::unscheduled()
    }

    /// Longer than any status message lives, so a clock jump of this size is
    /// past the point one has to be off the screen. Deliberately not the push
    /// module's own constant: this test is about the loop waking itself, and
    /// borrowing the exact lifetime would tie it to a number it does not care
    /// about.
    const PAST_ANY_LIFETIME: Duration = Duration::from_secs(600);

    /// How long the rescue quit waits before it ends a loop that should have
    /// ended on its own. Generous enough that a slow machine cannot trip it,
    /// and short enough to keep the suite quick — it only elapses when the
    /// test is already failing.
    const RESCUE_AFTER: Duration = Duration::from_secs(3);

    /// The command the probe found, which is what binds the `G` key.
    fn found_command() -> crate::shell::ShellCommand {
        crate::shell::ShellCommand::new(None, crate::issue::DEFAULT_ISSUE_COMMAND)
            .expect("the default names a command")
    }

    /// The probe's answer, as the loop receives it.
    fn probe_answered() -> Event {
        Event::IssueCommandFound(found_command())
    }

    /// One press of `G`.
    fn press_g() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE))
    }

    /// An [`IssueRun`] in `session`, whose probe has answered.
    fn issue_run_in(session: crate::remote::Session) -> IssueRun {
        let mut issue = IssueRun::new(session);
        issue.found(found_command());
        issue
    }

    /// Both places the person who reads the screen can sit.
    const BOTH_SESSIONS: [crate::remote::Session; 2] = [
        crate::remote::Session::Local,
        crate::remote::Session::Remote,
    ];

    /// The message the first press on a remote shell puts under the frame.
    ///
    /// Written out here rather than taken from the code it pins, so a change
    /// to the words of the message is a change these tests report. The command
    /// is the whole command line, because that is what the second press runs.
    const SECOND_PRESS_NOTICE: &str = "remote shell — press G again to run ggs";

    #[test]
    fn a_press_on_a_local_shell_runs_the_command_at_once() {
        // The browser opens where the person sits, so there is nothing to ask.
        let mut issue = issue_run_in(crate::remote::Session::Local);
        assert_eq!(
            issue.press(Instant::now()),
            IssuePress::Run(found_command()),
            "a local shell must run the command on the first press",
        );
    }

    #[test]
    fn a_first_press_on_a_remote_shell_asks_for_a_second_one() {
        // The browser opens on the machine that runs gsw, where nobody sits.
        let mut issue = issue_run_in(crate::remote::Session::Remote);
        assert_eq!(
            issue.press(Instant::now()),
            IssuePress::Ask(SECOND_PRESS_NOTICE.to_string()),
            "the message must name the command the second press runs",
        );
    }

    #[test]
    fn a_second_press_on_a_remote_shell_runs_the_command() {
        // The caller arms, because only the row knows whether the words it
        // returned reached the screen. `absorb` makes this pair of calls.
        let now = Instant::now();
        let mut issue = issue_run_in(crate::remote::Session::Remote);
        issue.press(now);
        issue.arm(now);
        assert_eq!(
            issue.press(now + Duration::from_secs(1)),
            IssuePress::Run(found_command()),
            "the second press must run the command the message named",
        );
    }

    #[test]
    fn a_second_press_one_lifetime_later_asks_again() {
        // The message on screen is the armed state. It leaves the screen after
        // one `STATUS_LIFETIME`, and the arming leaves with it, so the two end
        // at the same moment. The caller arms, as it does in `absorb`, and it
        // arms at the instant the message took the row.
        let now = Instant::now();
        let mut issue = issue_run_in(crate::remote::Session::Remote);
        issue.press(now);
        issue.arm(now);
        assert_eq!(
            issue.press(now + crate::push::STATUS_LIFETIME),
            IssuePress::Ask(SECOND_PRESS_NOTICE.to_string()),
            "a press one lifetime after the first must ask again",
        );
    }

    #[test]
    fn a_key_between_two_presses_takes_the_arming_away() {
        // That key also takes the message off the screen, and the message is
        // the armed state. The caller arms first, as it does in `absorb`,
        // because there is no arming to take away otherwise.
        let now = Instant::now();
        let mut issue = issue_run_in(crate::remote::Session::Remote);
        issue.press(now);
        issue.arm(now);
        issue.disarm();
        assert_eq!(
            issue.press(now),
            IssuePress::Ask(SECOND_PRESS_NOTICE.to_string()),
            "a key other than `G` must leave the next press asking",
        );
    }

    #[test]
    fn a_press_while_a_run_is_in_flight_does_nothing() {
        // One run at a time, in both sessions: a browser opening twice is two
        // tabs nobody asked for. A press here must not arm anything either,
        // because the message would ask for a press that runs nothing.
        for session in BOTH_SESSIONS {
            let mut issue = issue_run_in(session);
            issue.running = true;
            assert_eq!(
                issue.press(Instant::now()),
                IssuePress::Nothing,
                "a run in flight must answer nothing in {session:?}",
            );
        }
    }

    #[test]
    fn a_press_with_no_command_does_nothing() {
        // The probe has not answered yet, or it found no command. That is the
        // one silent case the key has, and it is silent in both sessions.
        for session in BOTH_SESSIONS {
            let mut issue = IssueRun::new(session);
            assert_eq!(
                issue.press(Instant::now()),
                IssuePress::Nothing,
                "no command must answer nothing in {session:?}",
            );
        }
    }

    #[test]
    fn m_starts_one_run_and_nothing_more_until_that_run_finishes() {
        // The user can press `m` ten times, and gsw starts one run. Two runs
        // at once double the scratch worktrees and tell the user nothing new.
        let mut conflicts = ConflictsRun::new();
        assert_eq!(
            conflicts.press(),
            ConflictsPress::Start,
            "the first press must start a run",
        );
        for press in 2..=10 {
            assert_eq!(
                conflicts.press(),
                ConflictsPress::Nothing,
                "press {press} must start nothing while the run is in flight",
            );
        }
    }

    #[test]
    fn m_starts_a_new_run_once_the_run_has_finished() {
        // The flag is a flag, not a latch. A press after the outcome arrived
        // measures the repository again, because the user can change it
        // between two presses.
        let mut conflicts = ConflictsRun::new();
        assert_eq!(conflicts.press(), ConflictsPress::Start);
        assert_eq!(
            conflicts.press(),
            ConflictsPress::Nothing,
            "the first run is in flight",
        );

        conflicts.finished();
        assert_eq!(
            conflicts.press(),
            ConflictsPress::Start,
            "a press after the run ended must start a new run",
        );
        assert_eq!(
            conflicts.press(),
            ConflictsPress::Nothing,
            "the new run is the only run in flight",
        );
    }

    #[test]
    fn a_first_g_on_a_remote_shell_starts_no_run_and_asks_again() {
        // The browser would open on the machine that runs gsw, where nobody
        // sits. So the first press spends a row rather than a tab.
        let (screen, seen) = run_loop_remote(vec![probe_answered(), press_g(), Event::Quit]);
        assert!(
            seen.issue_runs.is_empty(),
            "a first press on a remote shell must start no run, got {:?}",
            seen.issue_runs,
        );
        let painted = strip_ansi(&screen);
        assert!(
            painted.contains(SECOND_PRESS_NOTICE),
            "the message must reach the screen, got {painted:?}",
        );
    }

    #[test]
    fn a_second_g_on_a_remote_shell_starts_the_run() {
        let (_screen, seen) =
            run_loop_remote(vec![probe_answered(), press_g(), press_g(), Event::Quit]);
        assert_eq!(
            seen.issue_runs,
            vec![found_command()],
            "the second press must run the command",
        );
    }

    /// Two thirds of the window the message that asks for a second press
    /// stands in.
    ///
    /// The size is what makes the read count visible. One step is inside
    /// [`crate::push::STATUS_LIFETIME`] and two steps are past it, so two
    /// presses one clock read apart find the arming, and two presses two clock
    /// reads apart find nothing. A function rather than a constant, because
    /// the arithmetic on a [`Duration`] does not run in a constant.
    fn press_step() -> Duration {
        crate::push::STATUS_LIFETIME * 2 / 3
    }

    #[test]
    fn each_g_press_reads_the_clock_once_so_the_second_press_still_stands() {
        // The rule: one press of `G` reads the clock once. The loop's own
        // reads then never eat the window the second press stands in.
        //
        // An extra read inside one press costs a whole step of that window. A
        // frozen clock cannot show that cost, because two reads of a frozen
        // clock give the same instant as one read. This clock steps on every
        // read instead. `absorb` reads the clock for a key only in the
        // `Event::IssueRequested` arm, so the two presses land on consecutive
        // reads and sit one step apart, which is inside the window. A second
        // read for the notice puts the presses two steps apart, which is past
        // the window, and the second press then only asks again.
        //
        // This pins commit 54a9cb2b, which took that second read out.
        let (_screen, seen) = run_loop_in_session(
            vec![probe_answered(), press_g(), press_g(), Event::Quit],
            TEST_DIMS,
            |_frame_dims| "FRAME".to_string(),
            stepping_clock(Instant::now(), press_step()),
            crate::remote::Session::Remote,
        );
        assert_eq!(
            seen.issue_runs,
            vec![found_command()],
            "each press must read the clock once, so the second press runs the command",
        );
    }

    #[test]
    fn a_g_pressed_while_a_push_owns_the_row_arms_nothing() {
        // The message that asks for the second press *is* the offer, so the
        // offer cannot stand while that message is still in the queue. A push
        // owns the row for minutes, and a notice posted then waits for it. A
        // press that armed the key there would let the next `G` open a browser
        // on the machine nobody sits at, with nobody ever asked a second time
        // for it — which is the exact harm this feature exists to stop.
        let (screen, seen) = run_loop_remote(vec![
            probe_answered(),
            Event::PushRequested,
            Event::PushConfirmed,
            press_g(),
            press_g(),
            Event::Quit,
        ]);
        assert!(
            seen.issue_runs.is_empty(),
            "a press made while a push owns the row must arm nothing, got {:?}",
            seen.issue_runs,
        );
        let painted = strip_ansi(&screen);
        assert!(
            !painted.contains(SECOND_PRESS_NOTICE),
            "the notice must wait for the row the push owns, got {painted:?}",
        );
    }

    #[test]
    fn a_key_between_two_g_presses_on_a_remote_shell_starts_nothing() {
        // The message is the armed state, and every other key takes it off the
        // screen. A `G` after that key is a first press again.
        let (screen, seen) = run_loop_remote(vec![
            probe_answered(),
            press_g(),
            key(KeyCode::Char('x')),
            press_g(),
            Event::Quit,
        ]);
        assert!(
            seen.issue_runs.is_empty(),
            "a key between the presses must leave the second one asking, got {:?}",
            seen.issue_runs,
        );
        let painted = strip_ansi(&screen);
        assert!(
            painted.contains(SECOND_PRESS_NOTICE),
            "the second press must ask again, got {painted:?}",
        );
    }

    #[test]
    fn g_starts_nothing_until_the_probe_has_answered() {
        // The loop never waits for the probe, so a key pressed in the first
        // second of a session finds the command unresolved. An unbound key
        // does nothing, and this is the one silent case the feature has.
        let (_screen, seen) = run_loop(vec![press_g(), Event::Quit]);
        assert!(
            seen.issue_runs.is_empty(),
            "an unresolved command must start no run, got {:?}",
            seen.issue_runs,
        );
    }

    #[test]
    fn g_starts_the_command_once_the_probe_has_answered() {
        let (_screen, seen) = run_loop(vec![probe_answered(), press_g(), Event::Quit]);
        assert_eq!(
            seen.issue_runs,
            vec![found_command()],
            "the key must run the command the probe found",
        );
    }

    #[test]
    fn a_second_g_while_a_run_is_in_flight_starts_nothing() {
        // A browser opening twice is two tabs nobody asked for.
        let (_screen, seen) = run_loop(vec![probe_answered(), press_g(), press_g(), Event::Quit]);
        assert_eq!(
            seen.issue_runs.len(),
            1,
            "one run at a time, got {:?}",
            seen.issue_runs,
        );
    }

    #[test]
    fn g_starts_another_run_once_the_first_has_ended() {
        // The flag is a flag, not a latch.
        let (_screen, seen) = run_loop(vec![
            probe_answered(),
            press_g(),
            Event::IssueFinished {
                generation: Generation::default(),
                outcome: crate::issue::IssueOutcome::new("ggs", true, &[], "exit status: 0"),
            },
            press_g(),
            Event::Quit,
        ]);
        assert_eq!(
            seen.issue_runs.len(),
            2,
            "a key after a run ended must start another, got {:?}",
            seen.issue_runs,
        );
    }

    #[test]
    fn a_run_that_failed_puts_the_last_line_the_child_wrote_under_the_frame() {
        // `ggs` refuses with exit status 2 on a branch that names no issue,
        // and that refusal is the whole reason the browser did not open.
        let (screen, _seen) = run_loop(vec![
            probe_answered(),
            press_g(),
            Event::IssueFinished {
                generation: Generation::default(),
                outcome: crate::issue::IssueOutcome::new(
                    "ggs",
                    false,
                    &["branch main names no issue".to_string()],
                    "exit status: 2",
                ),
            },
            Event::Quit,
        ]);
        assert!(
            screen.contains("branch main names no issue"),
            "the refusal must reach the screen, got {screen:?}",
        );
    }

    #[test]
    fn a_run_that_worked_puts_nothing_under_the_frame() {
        // The browser is the answer.
        let (screen, _seen) = run_loop(vec![
            probe_answered(),
            press_g(),
            Event::IssueFinished {
                generation: Generation::default(),
                outcome: crate::issue::IssueOutcome::new("ggs", true, &[], "exit status: 0"),
            },
            Event::Quit,
        ]);
        assert_eq!(
            screen, "FRAME",
            "a run that worked must cost the frame no row, got {screen:?}",
        );
    }

    /// One press of `m`.
    fn press_m() -> Event {
        key(KeyCode::Char('m'))
    }

    /// The notice a run against `main` puts under the frame.
    ///
    /// Written out here rather than taken from the code it pins, so a change to
    /// the words is a change these tests report.
    const RUNNING_AGAINST_MAIN: &str = "Running grind and grime against main…";

    /// The line that reports [`measured_clean`].
    const MEASURED_CLEAN: &str = "main: rebase clean · merge clean";

    /// The branch of a run against `main`, as the loop receives it.
    fn started_against_main() -> Event {
        Event::ConflictsStarted {
            generation: Generation::default(),
            branch: "main".to_string(),
        }
    }

    /// The outcome of a run, as the loop receives it.
    fn finished(outcome: ConflictsOutcome) -> Event {
        Event::ConflictsFinished {
            generation: Generation::default(),
            outcome,
        }
    }

    /// A run against `main` whose two replays both came back clean.
    fn measured_clean() -> ConflictsOutcome {
        ConflictsOutcome::Measured {
            branch: "main".to_string(),
            rebase: Ok(gitscratch::Conflicts::nothing_replayed()),
            merge: Ok(gitscratch::Conflicts::nothing_replayed()),
            dirty: false,
        }
    }

    /// A run against `main` whose rebase conflicted, in a work tree with
    /// uncommitted work, so its line is far wider than a narrow pane.
    fn measured_in_conflict() -> ConflictsOutcome {
        let three_hunks = std::num::NonZeroUsize::new(3).expect("three is not zero");
        ConflictsOutcome::Measured {
            branch: "main".to_string(),
            rebase: Ok(gitscratch::Conflicts::from_files(
                [(std::path::PathBuf::from("shared.txt"), three_hunks)],
                gitscratch::Stops::new(2),
            )),
            merge: Ok(gitscratch::Conflicts::nothing_replayed()),
            dirty: true,
        }
    }

    #[test]
    fn three_m_presses_before_the_outcome_start_exactly_one_run() {
        // The user can press `m` ten times, and gsw starts one run.
        let (_screen, seen) = run_loop(vec![press_m(), press_m(), press_m(), Event::Quit]);
        assert_eq!(seen.conflict_runs, 1, "one run at a time");
    }

    #[test]
    fn an_m_after_the_outcome_starts_a_second_run() {
        // The outcome frees the key, so the next press measures again.
        let (_screen, seen) = run_loop(vec![
            press_m(),
            started_against_main(),
            finished(measured_clean()),
            press_m(),
            Event::Quit,
        ]);
        assert_eq!(
            seen.conflict_runs, 2,
            "a press after the outcome must start a second run",
        );
    }

    #[test]
    fn the_running_notice_stays_under_the_frame_through_a_key() {
        // The notice says that a press of `m` does nothing now. That is true
        // until the outcome arrives, and a key does not end the run.
        let (screen, _seen) = run_loop(vec![
            press_m(),
            started_against_main(),
            key(KeyCode::Char('x')),
            Event::Quit,
        ]);
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME\n{RUNNING_AGAINST_MAIN}"),
            "the notice must stay under the frame, with no age",
        );
    }

    #[test]
    fn the_outcome_replaces_the_running_notice_with_a_line_that_fades() {
        // The age after the line is what a report that fades shows. A notice
        // that stayed beside it, or a line that waited for a key, would show
        // no age.
        let (screen, _seen) = run_loop(vec![
            press_m(),
            started_against_main(),
            finished(measured_clean()),
            Event::Quit,
        ]);
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME\n{MEASURED_CLEAN} (0s ago)"),
            "the outcome must take the row of the notice, and fade",
        );
    }

    /// A push in flight, and a whole run of `m` inside it.
    fn a_run_during_a_push() -> Vec<Event> {
        vec![
            Event::PushRequested,
            Event::PushConfirmed,
            press_m(),
            started_against_main(),
            finished(measured_clean()),
        ]
    }

    #[test]
    fn a_push_in_flight_keeps_the_notice_off_the_row_and_holds_the_outcome() {
        // A push owns the row. A held notice would reach the row after the
        // outcome and say that a run is in flight when none is, so the notice
        // goes nowhere. The outcome is a report, so it waits for the row.
        let mut events = a_run_during_a_push();
        events.push(Event::Quit);
        let (screen, seen) = run_loop(events);
        assert_eq!(
            seen.conflict_runs, 1,
            "a push in flight is no reason to refuse a measurement",
        );
        let painted = strip_ansi(&screen);
        assert!(
            painted.contains("Pushing"),
            "the push must keep the row, got {painted:?}",
        );
        assert!(
            !painted.contains(RUNNING_AGAINST_MAIN),
            "the notice must not reach a row the push owns, got {painted:?}",
        );
        assert!(
            !painted.contains(MEASURED_CLEAN),
            "the outcome must wait for the row, got {painted:?}",
        );

        // The push fails, and a key takes its error away. The row is free, so
        // the outcome takes it, and the notice never does.
        let mut events = a_run_during_a_push();
        events.extend([
            Event::PushFinished(PushOutcome {
                success: false,
                output: "error: failed to push some refs\n".to_string(),
            }),
            key(KeyCode::Char('x')),
            Event::Quit,
        ]);
        let (screen, _seen) = run_loop(events);
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME\n{MEASURED_CLEAN} (0s ago)"),
            "the held outcome must reach the row once the push gives it up",
        );
    }

    /// A pane far narrower than either line of a run.
    const NARROW: Dimensions = Dimensions {
        width: 20,
        height: 24,
    };

    /// The events of one case, built fresh for each run of the loop.
    type Events = fn() -> Vec<Event>;

    #[test]
    fn a_pane_too_narrow_for_the_line_gets_one_row_that_does_not_wrap() {
        // gsw paints nothing that wraps or scrolls the pane it measured. A row
        // that wraps pushes the top row of the frame off the screen.
        let cases: [(&str, Events, &str); 2] = [
            (
                "the running notice",
                || vec![press_m(), started_against_main(), Event::Quit],
                "Running grind",
            ),
            (
                "the outcome",
                || {
                    vec![
                        press_m(),
                        started_against_main(),
                        finished(measured_in_conflict()),
                        Event::Quit,
                    ]
                },
                "main: rebase",
            ),
        ];

        for (what, events, starts_with) in cases {
            let (screen, seen) = run_loop_in_pane(events(), NARROW);
            let painted = strip_ansi(&screen);
            for row in painted.lines() {
                assert!(
                    UnicodeWidthStr::width(row) <= NARROW.width,
                    "{what}: the row {row:?} is wider than the pane",
                );
            }
            assert_eq!(
                seen.frame_heights.last().copied(),
                Some(NARROW.height - 1),
                "{what}: the line must take exactly one row from the frame",
            );
            assert_eq!(
                painted.lines().count(),
                NARROW.height,
                "{what}: the screen must fill the pane exactly, got {painted:?}",
            );
            let last = painted.lines().last().unwrap_or_default();
            assert!(
                last.starts_with(starts_with),
                "{what}: the bottom row must carry the line, got {last:?}",
            );
        }
    }

    #[test]
    fn a_refused_run_shows_its_reason_as_a_fading_line_and_m_starts_again() {
        // No default branch, a detached HEAD, an empty repository: gitscratch
        // refuses with a reason. gsw must not crash, and `m` must work again
        // after the line appears.
        let (screen, seen) = run_loop(vec![
            press_m(),
            finished(ConflictsOutcome::Refused {
                reason: "no default branch resolves here".to_string(),
            }),
            press_m(),
            Event::Quit,
        ]);
        assert_eq!(
            strip_ansi(&screen),
            "FRAME\ngrind and grime failed: no default branch resolves here (0s ago)",
            "the reason must reach the row as a line that fades",
        );
        assert_eq!(
            seen.conflict_runs, 2,
            "a press after a refusal must start a new run",
        );
    }

    #[test]
    fn an_m_between_two_g_presses_on_a_remote_shell_takes_the_arming_away() {
        // `m` is a key other than `G`, so it takes the arming away, as every
        // such key does. The run it asks for still starts.
        let (_screen, seen) = run_loop_remote(vec![
            probe_answered(),
            press_g(),
            press_m(),
            press_g(),
            Event::Quit,
        ]);
        assert!(
            seen.issue_runs.is_empty(),
            "a press of `m` between the presses must leave the second one asking, got {:?}",
            seen.issue_runs,
        );
        assert_eq!(
            seen.conflict_runs, 1,
            "the press of `m` must still start its run",
        );
    }

    #[test]
    fn the_loop_wakes_itself_to_take_an_expired_message_off_the_screen() {
        // A status message expires against the clock, and on a quiet
        // repository nothing else is due to wake the loop: no filesystem
        // event, no timed refresh under `--refresh-interval 0`, and no decay
        // tick once the newest commit is past the fade. Without a deadline of
        // its own the message would come off the screen only when something
        // unrelated happened — which is the "stays forever" this feature
        // exists to answer.
        let (tx, rx) = mpsc::channel();
        // One event, so the first frame is painted straight away rather than a
        // cadence later. After it the queue is empty and only a timeout can
        // wake the loop again.
        tx.send(Event::Resize).expect("queue the first wake");

        // A quit from outside, long after the loop should have acted on its
        // own, so a missing deadline fails this test instead of hanging it.
        let rescue = tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(RESCUE_AFTER);
            let _ = rescue.send(Event::Quit);
        });

        let base = Instant::now();
        let jumped = std::cell::Cell::new(false);
        let painted = RefCell::new(Vec::<String>::new());
        let mut displayed = String::new();

        event_loop(
            &rx,
            TEST_DEBOUNCE,
            &mut displayed,
            LoopStart {
                cache: cache_at(base),
                freshest: None, // decay timer off: the message is the only thing that ages
                schedule: no_timed_refresh_for_push(),
                ui: pushed_ui(base),
                session: crate::remote::Session::Local,
                home: loop_home(),
            },
            LoopHooks {
                collect: |_current: &WorktreePath| Ok(pushable_snapshot()),
                render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| frame("FRAME"),
                dimensions: || TEST_DIMS,
                paint: |output: &str| {
                    painted.borrow_mut().push(output.to_string());
                    // Every paint moves the clock past the message's lifetime,
                    // so the wake after the first one finds it expired.
                    jumped.set(true);
                    if painted.borrow().len() >= 2 {
                        let _ = tx.send(Event::Quit);
                    }
                    Ok(())
                },
                clock: || {
                    if jumped.get() {
                        base + PAST_ANY_LIFETIME
                    } else {
                        base
                    }
                },
                next_tick: timer_off,
                start_push: |_command: PushCommand, _current: &WorktreePath| {},
                start_base_update: |_command: crate::update::BaseUpdateCommand,
                                    _current: &WorktreePath| {},
                start_issue: |_command: crate::shell::ShellCommand,
                              _current: &WorktreePath,
                              _generation: Generation| {},
                start_conflicts: |_current: &WorktreePath, _generation: Generation| {},
                worktrees: Vec::new,
                worktree_paths: Vec::new,
                switch: no_switch,
                render_list: no_list,
            },
        )
        .expect("loop");

        let painted = painted.into_inner();
        assert!(
            painted[0].contains("Created origin/gsw-push"),
            "the first frame must carry the message, got {:?}",
            painted[0],
        );
        assert_eq!(
            painted.len(),
            2,
            "the loop must wake itself once more to clear the message, painted {painted:?}",
        );
        assert_eq!(
            painted[1], "FRAME",
            "the expired message must leave the frame exactly as it was before it",
        );
    }

    #[test]
    fn composing_does_not_double_the_frames_trailing_newline() {
        // A real gsw frame ends with a newline. Joining with another one would
        // leave a blank row between the frame and the overlay — and the frame
        // was already rendered one row shorter to make room, so the overlay
        // would be pushed off the pane it was measured to fit.
        assert_eq!(compose("a\nb\n".to_string(), "note"), "a\nb\nnote");
    }

    #[test]
    fn composing_separates_a_frame_that_has_no_trailing_newline() {
        assert_eq!(compose("a\nb".to_string(), "note"), "a\nb\nnote");
    }

    #[test]
    fn composing_an_empty_overlay_returns_the_frame_untouched() {
        // Every frame gsw painted before the push feature existed must still be
        // painted byte for byte, trailing newline and all.
        assert_eq!(compose("a\nb\n".to_string(), ""), "a\nb\n");
        assert_eq!(compose("a\nb".to_string(), ""), "a\nb");
    }

    #[test]
    fn pressing_p_puts_the_question_under_the_frame() {
        // The key has to reach the overlay through the loop, not just through
        // the classifier: the frame and the question are painted together.
        let (displayed, _) = run_loop(vec![key(KeyCode::Char('p')), Event::Quit]);
        assert!(displayed.starts_with("FRAME"), "got {displayed:?}");
        assert!(
            displayed.contains("Create new remote branch origin/gsw-push?"),
            "the question must be painted under the frame, got {displayed:?}",
        );
    }

    #[test]
    fn confirming_starts_the_push_the_question_described() {
        // `p` then `y` must run the command the confirmation named — the whole
        // safety property of asking first.
        let (_, seen) = run_loop(vec![
            key(KeyCode::Char('p')),
            key(KeyCode::Char('y')),
            Event::Quit,
        ]);
        let [command] = seen.pushes.as_slice() else {
            panic!(
                "one confirmed push must reach the runner, got {:?}",
                seen.pushes
            );
        };
        assert_eq!(command.args(), ["push", "-u", "origin", "gsw-push"]);
        // The branch travels with the arguments all the way through the loop:
        // the runner checks it against the checkout before git sees it.
        assert_eq!(command.branch(), "gsw-push");
    }

    #[test]
    fn a_lone_y_pushes_nothing() {
        // Without a question on screen, `y` is an ordinary key. It must never
        // reach the push.
        let (_, seen) = run_loop(vec![key(KeyCode::Char('y')), Event::Quit]);
        assert!(seen.pushes.is_empty(), "y alone must not push");
    }

    /// How far behind the base the snapshot of a base-update test stands. Any
    /// count above zero does: the count is the reason the key acts at all.
    const BEHIND: u32 = 5;

    /// A [`PushUi`] with the question of a rebase already on the row, asked at
    /// `at` against a branch [`BEHIND`] commits behind its base.
    ///
    /// The key that asks it arrives in a later slice, so these tests reach the
    /// row through the door the key will use. The question is the state the
    /// loop has to act on, and a press is only one way to reach it.
    fn asking_rebase_ui(at: Instant) -> PushUi {
        let mut ui = PushUi::new(false);
        ui.request_base_update(
            &Snapshot {
                commits_behind: BEHIND,
                ..pushable_snapshot()
            },
            crate::update::BaseUpdate::Rebase,
            &crate::shell::ShellCommand::new(
                None,
                crate::update::BaseUpdate::Rebase.default_command(),
            )
            .expect("a name"),
            TEST_DIMS,
            at,
        );
        ui
    }

    /// A [`PushUi`] with that rebase already running, confirmed at `at`.
    fn running_rebase_ui(at: Instant) -> PushUi {
        let mut ui = asking_rebase_ui(at);
        ui.confirm(at).expect("the question must confirm");
        ui
    }

    #[test]
    fn confirming_a_base_update_starts_the_command_the_question_described() {
        // `y` on the question of `R` must run the command the sentence named,
        // in the worktree the frame shows — the whole safety property of asking
        // first. It must reach the runner of that act and no other: a push
        // started here would push the branch without the rebase the user asked
        // for.
        let base = Instant::now();
        let (_displayed, seen) = drive(
            vec![key(KeyCode::Char('y')), Event::Quit],
            Setup {
                ui: asking_rebase_ui(base),
                ..in_world(World::alone())
            },
            move || base,
        );

        let [command] = seen.base_updates.as_slice() else {
            panic!(
                "one confirmed base update must reach the runner, got {:?}",
                seen.base_updates,
            );
        };
        assert_eq!(command.update(), crate::update::BaseUpdate::Rebase);
        assert_eq!(command.branch(), ALONE);
        assert_eq!(command.base(), "main");
        assert_eq!(command.command().name(), "grp");
        assert_eq!(
            seen.base_update_paths,
            vec![worktree(ALONE)],
            "the run belongs to the worktree the frame shows",
        );
        assert!(
            seen.pushes.is_empty(),
            "a confirmed rebase must start no push, got {:?}",
            seen.pushes,
        );
    }

    #[test]
    fn every_outcome_of_a_base_update_walks_git_again() {
        // **Unlike a push.** A push that failed changed nothing to re-read, so
        // it walks nothing. A rebase that failed rewrote part of the branch and
        // stopped in the middle of it, and the `⚠ rebase` row of the header is
        // what says so — only a walk puts it there. A rebase that worked moved
        // every commit and pushed them, so the counts in the header are stale
        // the moment it lands.
        let cases = [
            (
                "a rebase that worked",
                PushOutcome {
                    success: true,
                    output: "grp: rebased onto 'main'\n".to_string(),
                },
                "Rebased gsw-push onto main with grp",
            ),
            (
                "a rebase that stopped on a conflict",
                PushOutcome {
                    success: false,
                    output: "error: could not apply d3eee9d… feature edit\n".to_string(),
                },
                "error: could not apply d3eee9d… feature edit",
            ),
        ];

        for (what, outcome, shows) in cases {
            let base = Instant::now();
            let (displayed, seen) = drive(
                vec![Event::BaseUpdateFinished(outcome), Event::Quit],
                Setup {
                    ui: running_rebase_ui(base),
                    ..in_world(World::alone())
                },
                move || base,
            );
            assert_eq!(
                seen.collects, 1,
                "{what} must re-walk, or the header describes a repository that moved",
            );
            assert!(
                displayed.contains(shows),
                "{what} must reach the row, got {displayed:?}",
            );
        }
    }

    #[test]
    fn cancelling_takes_the_question_off_the_screen_and_pushes_nothing() {
        let (displayed, seen) = run_loop(vec![
            key(KeyCode::Char('p')),
            key(KeyCode::Char('n')),
            Event::Quit,
        ]);
        assert!(seen.pushes.is_empty(), "n must not push");
        assert_eq!(displayed, "FRAME", "the question must be gone");
    }

    #[test]
    fn a_successful_push_walks_git_again() {
        // The push moved the upstream, so the header's arrows and tracking
        // segment are stale the moment it lands. Only a walk fixes them.
        let (displayed, seen) = run_loop(vec![
            key(KeyCode::Char('p')),
            key(KeyCode::Char('y')),
            Event::PushFinished(PushOutcome {
                success: true,
                output: String::new(),
            }),
            Event::Quit,
        ]);
        assert_eq!(
            seen.collects, 1,
            "a successful push must re-walk so the header matches what happened",
        );
        assert!(
            displayed.contains("Created origin/gsw-push"),
            "got {displayed:?}",
        );
    }

    #[test]
    fn a_failed_push_shows_the_error_and_does_not_walk() {
        // Nothing changed in the repository, so a walk would cost a status
        // traversal to redraw the identical frame.
        let (displayed, seen) = run_loop(vec![
            key(KeyCode::Char('p')),
            key(KeyCode::Char('y')),
            Event::PushFinished(PushOutcome {
                success: false,
                output: "error: failed to push some refs\n".to_string(),
            }),
            Event::Quit,
        ]);
        assert_eq!(seen.collects, 0, "a failed push changed nothing to re-read");
        assert!(
            displayed.contains("error: failed to push some refs"),
            "got {displayed:?}",
        );
    }

    #[test]
    fn the_frame_is_rendered_shorter_while_the_overlay_is_up() {
        // The overlay is painted under a frame that was measured to fill the
        // pane. Without giving the rows back, the frame's bottom row — the file
        // list — falls off the screen.
        let (_, seen) = run_loop(vec![key(KeyCode::Char('p')), Event::Quit]);
        assert_eq!(
            seen.frame_heights,
            vec![TEST_DIMS.height - 1],
            "one overlay row must cost the frame one row",
        );
    }

    #[test]
    fn a_three_row_error_costs_the_frame_three_rows() {
        let (_, seen) = run_loop(vec![
            key(KeyCode::Char('p')),
            key(KeyCode::Char('y')),
            Event::PushFinished(PushOutcome {
                success: false,
                output: "error: one\nerror: two\nerror: three\nerror: four\n".to_string(),
            }),
            Event::Quit,
        ]);
        assert_eq!(
            seen.frame_heights.last().copied(),
            Some(TEST_DIMS.height - 3),
        );
    }

    /// A pane exactly as tall as the tallest status message gsw can show. That
    /// is where the frame and the overlay start competing for the same rows,
    /// and the only size at which an unclamped overlay is visible as an
    /// overflow rather than as a very short frame.
    const SHORT_PANE: Dimensions = Dimensions {
        width: 80,
        height: 3,
    };

    /// The events that put a three-row push error on screen.
    fn a_three_row_failure() -> Vec<Event> {
        vec![
            key(KeyCode::Char('p')),
            key(KeyCode::Char('y')),
            Event::PushFinished(PushOutcome {
                success: false,
                output: "To /tmp/origin\n\
                         ! [rejected] gsw-push -> gsw-push (fetch first)\n\
                         error: failed to push some refs\n"
                    .to_string(),
            }),
            Event::Quit,
        ]
    }

    #[test]
    fn the_overlay_never_overflows_a_pane_shorter_than_the_message() {
        // gsw paints into an alternate screen it cleared and laid out to fill
        // exactly. One row too many scrolls the pane, which is the same
        // never-wrap contract the overlay's width truncation exists to hold.
        let (displayed, seen) = run_loop_in_pane(a_three_row_failure(), SHORT_PANE);
        // The failure path colors its rows red, and `colored`'s override is
        // process-global across this test binary, so count visible rows rather
        // than assume some other test left color off.
        let painted = strip_ansi(&displayed);
        assert!(
            painted.lines().count() <= SHORT_PANE.height,
            "painted {} rows into a {}-row pane: {painted:?}",
            painted.lines().count(),
            SHORT_PANE.height,
        );
        assert!(
            matches!(seen.frame_heights.last().copied(), Some(rows) if rows >= 1),
            "the frame must keep a row of its own, got {:?}",
            seen.frame_heights,
        );
    }

    /// A pane with nothing to spare: the frame keeps the only row there is, so
    /// the push feature has nowhere to put a question.
    const NO_ROOM_PANE: Dimensions = Dimensions {
        width: 80,
        height: 1,
    };

    #[test]
    fn a_push_never_starts_from_a_question_the_pane_never_painted() {
        // Key autorepeat, a paste, or a fast double-tap delivers `p` and the
        // answer to it inside one debounce window, and every key in a window is
        // classified before the loop renders again. Nothing in between can
        // notice that the question was never drawn, so the pane has to be
        // consulted when the question is raised rather than when it is painted.
        // Enter is the key this is really about: it is what a user presses out
        // of reflex at a frame that did not change.
        for confirm in [KeyCode::Char('y'), KeyCode::Enter] {
            let (_, seen) = run_loop_in_pane(
                vec![key(KeyCode::Char('p')), key(confirm), Event::Quit],
                NO_ROOM_PANE,
            );
            assert!(
                seen.pushes.is_empty(),
                "{confirm:?} started a push from a question the pane never painted: {:?}",
                seen.pushes,
            );
        }
    }

    #[test]
    fn a_line_from_a_running_push_reaches_the_screen() {
        // The runner reports on its own reader threads, and this is the hop
        // that turns a reported line into a painted row. Without it the window
        // is a state nothing ever fills.
        let (displayed, _) = run_loop(vec![
            key(KeyCode::Char('p')),
            key(KeyCode::Char('y')),
            Event::PushOutput("Compiling gsw v0.1.0".to_string()),
            Event::Quit,
        ]);

        let painted = strip_ansi(&displayed);
        assert!(
            painted.contains("Compiling gsw v0.1.0"),
            "the reported line must reach the pane, got {painted:?}",
        );
    }

    /// How far the flood test's clock jumps on every read. Short enough that
    /// several lines land inside one drain budget — a deadline that fired on
    /// the first line would prove far less than this test claims — and long
    /// enough that the whole flood cannot fit inside one.
    const FLOOD_STEP: Duration = Duration::from_millis(50);

    /// How many lines the flood queues behind the confirmation. More than any
    /// one drain budget can swallow at [`FLOOD_STEP`], so a loop that only
    /// leaves the drain on a quiet channel paints exactly once.
    const FLOOD_LINES: usize = 20;

    #[test]
    fn a_flooding_push_paints_while_it_floods() {
        // Why the window exists at all: a pre-push hook that builds and tests
        // a workspace prints faster than one line per debounce window, and the
        // drain ends only on a channel that goes quiet. Every line absorbed
        // before the loop leaves the drain is a line nobody sees until the
        // flood is over — the notice's age frozen with it — which is exactly
        // the frozen screen this feature was supposed to answer.
        //
        // The clock steps on every read, so the drain's deadline is crossed
        // without sleeping: deterministic, and parallel-safe.
        let mut events = vec![key(KeyCode::Char('p')), key(KeyCode::Char('y'))];
        events.extend(
            (1..=FLOOD_LINES).map(|line| Event::PushOutput(format!("Compiling crate {line}"))),
        );
        events.push(Event::Quit);

        let (_, seen) = run_loop_clocked(
            events,
            TEST_DIMS,
            |_frame_dims| "FRAME".to_string(),
            stepping_clock(Instant::now(), FLOOD_STEP),
        );

        assert!(
            seen.paints.len() > 1,
            "a flooding push must paint while it floods, not once when it stops; \
             {FLOOD_LINES} lines produced {} painted screen(s)",
            seen.paints.len(),
        );
        let first = strip_ansi(seen.paints.first().expect("at least one paint"));
        assert!(
            first.contains("Compiling crate 1"),
            "the first paint must carry the head of the flood, got {first:?}",
        );
        assert!(
            !first.contains(&format!("Compiling crate {FLOOD_LINES}")),
            "the first paint must land while the flood is still running, got {first:?}",
        );
    }

    #[test]
    fn an_overlay_that_does_not_fit_drops_its_first_lines() {
        // A failure's reason is the last thing said about it, so a message
        // that has to lose rows loses them off the top. `To <remote>` names a
        // remote the frame above already shows, which makes it the row the
        // clip can most afford.
        //
        // The same rule is stated against `PushUi` directly in
        // `a_message_taller_than_the_pane_keeps_the_rows_the_frame_can_spare`.
        // This one is here because the loop divides the pane, and a division
        // that disagreed with the overlay would scroll the screen.
        let (displayed, _) = run_loop_in_pane(a_three_row_failure(), SHORT_PANE);
        let painted = strip_ansi(&displayed);
        assert!(
            painted.contains("error: failed to push some refs"),
            "the verdict must survive the clip, got {painted:?}",
        );
        assert!(painted.contains("! [rejected]"), "got {painted:?}");
        assert!(
            !painted.contains("To /tmp/origin"),
            "the head must be the part that is dropped, got {painted:?}",
        );
    }

    #[test]
    fn the_frame_goes_back_to_full_height_once_the_status_is_dismissed() {
        let (displayed, seen) = run_loop(vec![
            key(KeyCode::Char('p')),
            key(KeyCode::Char('y')),
            Event::PushFinished(PushOutcome {
                success: false,
                output: "error: failed to push some refs\n".to_string(),
            }),
            key(KeyCode::Char('x')),
            Event::Quit,
        ]);
        assert_eq!(displayed, "FRAME", "the dismissed error must leave nothing");
        assert_eq!(
            seen.frame_heights.last().copied(),
            Some(TEST_DIMS.height),
            "the rows must come back",
        );
    }

    #[test]
    fn quitting_still_works_with_a_question_on_screen() {
        // Ctrl-C during a confirmation must end the loop rather than being read
        // as an answer.
        //
        // Run on a thread with a deadline, because the failure mode here is a
        // loop that never ends: with no timed refresh and no decay tick, a
        // Ctrl-C the loop does not act on leaves it blocked in `recv` forever.
        // Asserting inline would hang the whole suite instead of failing.
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (tx, rx) = mpsc::channel();
            tx.send(key(KeyCode::Char('p'))).expect("queue p");
            tx.send(Event::Key(KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
            )))
            .expect("queue ctrl-c");
            let seen = RefCell::new(Seen::default());
            let mut displayed = String::new();
            let base = Instant::now();

            event_loop(
                &rx,
                TEST_DEBOUNCE,
                &mut displayed,
                LoopStart {
                    cache: cache_at(base),
                    freshest: None,
                    schedule: no_timed_refresh_for_push(),
                    ui: PushUi::new(false),
                    session: crate::remote::Session::Local,
                    home: loop_home(),
                },
                LoopHooks {
                    collect: |_current: &WorktreePath| Ok(pushable_snapshot()),
                    render: |_snap: &Snapshot, _dims: Dimensions, _timing: FrameTiming| {
                        frame("FRAME")
                    },
                    dimensions: || TEST_DIMS,
                    paint: |_output: &str| Ok(()),
                    clock: move || base,
                    next_tick: timer_off,
                    start_push: |command: PushCommand, _current: &WorktreePath| {
                        seen.borrow_mut().pushes.push(command)
                    },
                    start_base_update:
                        |command: crate::update::BaseUpdateCommand, _current: &WorktreePath| {
                            seen.borrow_mut().base_updates.push(command)
                        },
                    start_issue: |command: crate::shell::ShellCommand,
                                  _current: &WorktreePath,
                                  _generation: Generation| {
                        seen.borrow_mut().issue_runs.push(command);
                    },
                    start_conflicts: |_current: &WorktreePath, _generation: Generation| {
                        seen.borrow_mut().conflict_runs += 1
                    },
                    worktrees: Vec::new,
                    worktree_paths: Vec::new,
                    switch: no_switch,
                    render_list: no_list,
                },
            )
            .expect("loop");

            let _ = done_tx.send(seen.into_inner().pushes);
        });

        let pushes = done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("Ctrl-C must end the loop rather than leave it blocked");
        assert!(pushes.is_empty(), "ctrl-c must not push");
    }

    /// A frame that names the branch of its snapshot, so a test reads which
    /// worktree the frame shows.
    fn frame_of(snapshot: &Snapshot, _frame_dims: Dimensions) -> String {
        format!("FRAME {}", snapshot.branch)
    }

    /// The setup of the worktree tests: `world`, a frame that names the branch
    /// of its snapshot, a local shell, and no timed refresh.
    fn in_world(world: World) -> Setup {
        Setup {
            dims: TEST_DIMS,
            ui: PushUi::new(false),
            measured: TEST_DIMS,
            render: Box::new(frame_of),
            session: crate::remote::Session::Local,
            schedule: no_timed_refresh_for_push(),
            world,
        }
    }

    /// Run the loop over `events` in `world`, set up as [`in_world`] says, on
    /// a frozen clock.
    fn run_in(world: World, events: Vec<Event>) -> (String, Seen) {
        let base = Instant::now();
        drive(events, in_world(world), move || base)
    }

    /// The outcome of a run of `m` that gitscratch refused, as the loop
    /// receives it. Its line is a status line, which an unbound key takes
    /// away.
    fn refused_run() -> Event {
        finished(ConflictsOutcome::Refused {
            reason: "no default branch resolves here".to_string(),
        })
    }

    /// The line that [`refused_run`] puts under the frame.
    const REFUSED_RUN_LINE: &str = "grind and grime failed: no default branch resolves here";

    #[test]
    fn right_goes_to_the_next_worktree_in_path_order_and_wraps_from_the_last() {
        // Right visits the worktrees in the order of `cwt -f`, and Right on
        // the last worktree goes to the first, as `cwt -f` does.
        let (_screen, seen) = run_in(World::three(), vec![key(KeyCode::Right), Event::Quit]);
        assert_eq!(
            seen.switches,
            vec![worktree(CHARLIE)],
            "Right from the middle"
        );

        let (_screen, seen) = run_in(
            World::three_at(CHARLIE),
            vec![key(KeyCode::Right), Event::Quit],
        );
        assert_eq!(
            seen.switches,
            vec![worktree(ALPHA)],
            "Right on the last worktree wraps to the first",
        );
    }

    #[test]
    fn left_goes_to_the_previous_worktree_in_path_order_and_wraps_from_the_first() {
        // Left visits the worktrees in the order of `cwt -p`, and Left on the
        // first worktree goes to the last, as `cwt -p` does.
        let (_screen, seen) = run_in(World::three(), vec![key(KeyCode::Left), Event::Quit]);
        assert_eq!(seen.switches, vec![worktree(ALPHA)], "Left from the middle");

        let (_screen, seen) = run_in(
            World::three_at(ALPHA),
            vec![key(KeyCode::Left), Event::Quit],
        );
        assert_eq!(
            seen.switches,
            vec![worktree(CHARLIE)],
            "Left on the first worktree wraps to the last",
        );
    }

    #[test]
    fn left_and_right_read_the_list_of_worktrees_again_at_each_press() {
        // `nwt` and `swt` add and remove worktrees while gsw runs, so a list
        // read once at start is soon wrong.
        let (_screen, seen) = run_in(
            World::three(),
            vec![
                key(KeyCode::Right),
                key(KeyCode::Left),
                key(KeyCode::Right),
                Event::Quit,
            ],
        );
        assert_eq!(
            seen.path_listings, 3,
            "each press of Left and Right must read the paths"
        );
    }

    #[test]
    fn up_on_the_home_worktree_does_nothing() {
        // The frame shows the home worktree already. Nothing switches, and the
        // line under the frame stays, because Up is not an unbound key.
        let (screen, seen) = run_in(
            World::three(),
            vec![press_m(), refused_run(), key(KeyCode::Up), Event::Quit],
        );
        assert!(
            seen.switches.is_empty(),
            "Up on the home worktree must not switch, got {:?}",
            seen.switches,
        );
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME {BRAVO}\n{REFUSED_RUN_LINE} (0s ago)"),
            "Up on the home worktree must take nothing away",
        );
    }

    #[test]
    fn with_one_worktree_up_left_and_right_do_nothing() {
        // No other worktree is there to go to: no switch, no message, and
        // nothing taken away.
        let (screen, seen) = run_in(
            World::alone(),
            vec![
                press_m(),
                refused_run(),
                key(KeyCode::Up),
                key(KeyCode::Left),
                key(KeyCode::Right),
                Event::Quit,
            ],
        );
        assert!(
            seen.switches.is_empty(),
            "one worktree has no other to switch to, got {:?}",
            seen.switches,
        );
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME {ALONE}\n{REFUSED_RUN_LINE} (0s ago)"),
            "with one worktree an arrow key must take nothing away",
        );
    }

    /// The worktrees of `runs`, in order, without their generations.
    fn paths_only(runs: &[(WorktreePath, Generation)]) -> Vec<WorktreePath> {
        runs.iter().map(|(path, _)| path.clone()).collect()
    }

    /// A clock that gives `early` for its first `reads` reads, and `late` for
    /// every read after them.
    fn clock_that_jumps(early: Instant, reads: usize, late: Instant) -> impl Fn() -> Instant {
        let count = std::cell::Cell::new(0_usize);
        move || {
            let read = count.get();
            count.set(read + 1);
            if read < reads {
                early
            } else {
                late
            }
        }
    }

    /// How often the timed walk of the refresh-clock test runs.
    const REFRESH: Duration = Duration::from_secs(60);

    #[test]
    fn right_and_left_go_on_from_the_worktree_the_last_switch_reached() {
        // Three presses go once round the list of three and end at home, so
        // each press starts from the worktree that the press before reached.
        let (_screen, seen) = run_in(
            World::three(),
            vec![
                key(KeyCode::Right),
                key(KeyCode::Right),
                key(KeyCode::Right),
                Event::Quit,
            ],
        );
        assert_eq!(
            seen.switches,
            vec![worktree(CHARLIE), worktree(ALPHA), worktree(BRAVO)],
            "Right three times goes once round the list",
        );

        let (_screen, seen) = run_in(
            World::three(),
            vec![
                key(KeyCode::Left),
                key(KeyCode::Left),
                key(KeyCode::Left),
                Event::Quit,
            ],
        );
        assert_eq!(
            seen.switches,
            vec![worktree(ALPHA), worktree(CHARLIE), worktree(BRAVO)],
            "Left three times goes once round the list the other way",
        );
    }

    #[test]
    fn up_goes_to_the_home_worktree_and_up_on_it_again_does_nothing() {
        let (screen, seen) = run_in(
            World::three(),
            vec![
                key(KeyCode::Right),
                key(KeyCode::Up),
                key(KeyCode::Up),
                Event::Quit,
            ],
        );
        assert_eq!(
            seen.switches,
            vec![worktree(CHARLIE), worktree(BRAVO)],
            "Up must go home once, and the second Up finds the frame at home",
        );
        assert_eq!(strip_ansi(&screen), format!("FRAME {BRAVO}"));
    }

    #[test]
    fn the_painted_frame_after_a_switch_is_the_snapshot_the_switch_gave() {
        // The frame must never show the snapshot of one worktree under the
        // name of another.
        let (screen, _seen) = run_in(World::three(), vec![key(KeyCode::Right), Event::Quit]);
        assert_eq!(strip_ansi(&screen), format!("FRAME {CHARLIE}"));
    }

    #[test]
    fn after_a_switch_a_filesystem_event_walks_the_new_worktree() {
        // The harness reads the clock once for the cache, and the switch reads
        // it twice: before and after it opens the worktree. The filesystem
        // event is read a minute later, past the cooldown that the walk of the
        // switch armed, so it walks.
        let base = Instant::now();
        let (_screen, seen) = drive(
            vec![key(KeyCode::Right), Event::FsChanged, Event::Quit],
            in_world(World::three()),
            clock_that_jumps(base, 3, base + Duration::from_secs(60)),
        );
        assert_eq!(
            seen.collected_from,
            vec![worktree(CHARLIE)],
            "the walk must read the worktree on the screen",
        );
    }

    #[test]
    fn after_a_switch_p_g_and_m_act_on_the_new_worktree() {
        // Each key acts on the worktree on the screen at the press.
        let (_screen, seen) = run_in(
            World::three(),
            vec![
                probe_answered(),
                key(KeyCode::Right),
                key(KeyCode::Char('p')),
                key(KeyCode::Char('y')),
                press_g(),
                press_m(),
                Event::Quit,
            ],
        );
        assert_eq!(
            seen.push_paths,
            vec![worktree(CHARLIE)],
            "`p` pushes from it"
        );
        assert_eq!(
            seen.pushes.first().map(PushCommand::branch),
            Some(CHARLIE),
            "the push must name the branch of the worktree on the screen",
        );
        assert_eq!(
            paths_only(&seen.issue_paths),
            vec![worktree(CHARLIE)],
            "`G` runs in it",
        );
        assert_eq!(
            paths_only(&seen.conflict_paths),
            vec![worktree(CHARLIE)],
            "`m` measures it",
        );
    }

    #[test]
    fn a_burst_of_right_p_y_pushes_from_the_new_worktree() {
        // One burst, read with no frame between its keys. The switch happens
        // when Right is read, so `p` plans the push of the new worktree, as a
        // `y` after a `p` in one burst reads the new mode.
        let (_screen, seen) = run_in(
            World::three(),
            vec![
                key(KeyCode::Right),
                key(KeyCode::Char('p')),
                key(KeyCode::Char('y')),
                Event::Quit,
            ],
        );
        let [command] = seen.pushes.as_slice() else {
            panic!("one push must start, got {:?}", seen.pushes);
        };
        assert_eq!(command.args(), ["push", "-u", "origin", CHARLIE]);
        assert_eq!(seen.push_paths, vec![worktree(CHARLIE)]);
    }

    #[test]
    fn a_switch_starts_the_refresh_clock_again() {
        // The switch walks the new worktree, so the next timed walk is a whole
        // interval from the switch. The clock stands 50 seconds after the last
        // walk of the old worktree, so with no switch 10 seconds are left.
        let base = Instant::now();
        let later = base + Duration::from_secs(50);
        let refresh_in = |events: Vec<Event>| {
            let (_screen, seen) = drive(
                events,
                Setup {
                    schedule: WalkSchedule::new(Some(REFRESH), base, Duration::ZERO),
                    ..in_world(World::three())
                },
                move || later,
            );
            seen.timings
                .last()
                .and_then(|timing| timing.next_refresh_in)
        };

        assert_eq!(
            refresh_in(vec![key(KeyCode::Char('x')), Event::Quit]),
            Some(Duration::from_secs(10)),
            "with no switch, the clock of the old worktree runs on",
        );
        assert_eq!(
            refresh_in(vec![key(KeyCode::Right), Event::Quit]),
            Some(REFRESH),
            "a switch starts the clock again",
        );
    }

    #[test]
    fn the_arrow_keys_do_nothing_while_a_push_runs() {
        // The window under the frame belongs to the worktree that pushes. The
        // switch comes before the push, so Up has a home to go to, and it
        // still must not go there.
        let (screen, seen) = run_in(
            World::three(),
            vec![
                key(KeyCode::Right),
                key(KeyCode::Char('p')),
                key(KeyCode::Char('y')),
                key(KeyCode::Up),
                key(KeyCode::Left),
                key(KeyCode::Right),
                key(KeyCode::Down),
                Event::Quit,
            ],
        );
        assert_eq!(
            seen.switches,
            vec![worktree(CHARLIE)],
            "no arrow key may switch while a push runs",
        );
        let painted = strip_ansi(&screen);
        assert!(
            painted.contains("Pushing"),
            "the push window must stay, got {painted:?}",
        );
    }

    /// The last line that [`issue_failed_in`] reports.
    const ISSUE_REFUSAL: &str = "branch main names no issue";

    /// The outcome of a run of the issue command that failed, with the
    /// generation `generation`, as the loop receives it. Its line waits for a
    /// key.
    fn issue_failed_in(generation: Generation) -> Event {
        Event::IssueFinished {
            generation,
            outcome: crate::issue::IssueOutcome::new(
                "ggs",
                false,
                &[ISSUE_REFUSAL.to_string()],
                "exit status: 2",
            ),
        }
    }

    /// Run the loop over `events` in the three worktrees, set up as
    /// [`in_world`] says, in `session`, on a frozen clock.
    fn run_in_three(session: crate::remote::Session, events: Vec<Event>) -> (String, Seen) {
        let base = Instant::now();
        drive(
            events,
            Setup {
                session,
                ..in_world(World::three())
            },
            move || base,
        )
    }

    #[test]
    fn a_switch_removes_every_message_under_the_frame() {
        // Each message describes the worktree the frame showed before the
        // switch. After the switch the row is empty, and a message held for
        // the row does not take it later.
        let cases: [(&str, crate::remote::Session, Events); 6] = [
            (
                "a push result that fades",
                crate::remote::Session::Local,
                || {
                    vec![
                        key(KeyCode::Char('p')),
                        key(KeyCode::Char('y')),
                        Event::PushFinished(PushOutcome {
                            success: true,
                            output: String::new(),
                        }),
                        key(KeyCode::Right),
                        Event::Quit,
                    ]
                },
            ),
            (
                "the remote-shell offer of `G`",
                crate::remote::Session::Remote,
                || {
                    vec![
                        probe_answered(),
                        press_g(),
                        key(KeyCode::Right),
                        Event::Quit,
                    ]
                },
            ),
            ("a `G` error", crate::remote::Session::Local, || {
                vec![
                    probe_answered(),
                    press_g(),
                    issue_failed_in(Generation::default()),
                    key(KeyCode::Right),
                    Event::Quit,
                ]
            }),
            (
                "the notice of an `m` run",
                crate::remote::Session::Local,
                || {
                    vec![
                        press_m(),
                        started_against_main(),
                        key(KeyCode::Right),
                        Event::Quit,
                    ]
                },
            ),
            (
                "the result of an `m` run",
                crate::remote::Session::Local,
                || {
                    vec![
                        press_m(),
                        started_against_main(),
                        finished(measured_clean()),
                        key(KeyCode::Right),
                        Event::Quit,
                    ]
                },
            ),
            (
                "a `G` error held behind a push result",
                crate::remote::Session::Local,
                || {
                    vec![
                        probe_answered(),
                        key(KeyCode::Char('p')),
                        key(KeyCode::Char('y')),
                        press_g(),
                        issue_failed_in(Generation::default()),
                        Event::PushFinished(PushOutcome {
                            success: true,
                            output: String::new(),
                        }),
                        key(KeyCode::Right),
                        Event::Quit,
                    ]
                },
            ),
        ];

        for (what, session, events) in cases {
            let (screen, seen) = run_in_three(session, events());
            assert_eq!(
                seen.switches,
                vec![worktree(CHARLIE)],
                "{what}: Right must switch"
            );
            assert_eq!(
                strip_ansi(&screen),
                format!("FRAME {CHARLIE}"),
                "{what}: the switch must leave nothing under the frame",
            );
        }
    }

    #[test]
    fn a_g_after_a_switch_on_a_remote_shell_asks_again() {
        // The message is the armed state, and the switch takes the message
        // away. So a `G` after the switch is a first press again.
        let (screen, seen) = run_in_three(
            crate::remote::Session::Remote,
            vec![
                probe_answered(),
                press_g(),
                key(KeyCode::Right),
                press_g(),
                Event::Quit,
            ],
        );
        assert!(
            seen.issue_runs.is_empty(),
            "a `G` after a switch must not run the command, got {:?}",
            seen.issue_runs,
        );
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME {CHARLIE}\n{SECOND_PRESS_NOTICE} (0s ago)"),
            "the `G` after the switch must ask again",
        );
    }

    #[test]
    fn a_switch_takes_the_arming_of_g_away_whatever_made_it() {
        // Every key but `G` takes the arming away already, so no arrow key
        // can show this. A switch that no key made must take the arming away
        // too: the message is the armed state, and the switch takes the
        // message off the row.
        let now = Instant::now();
        let mut issue = issue_run_in(crate::remote::Session::Remote);
        issue.arm(now);
        let mut state = LoopState {
            cache: SnapshotCache {
                snapshot: snapshot_of(&worktree(BRAVO)),
                collected_at: now,
                dims: TEST_DIMS,
            },
            schedule: no_timed_refresh_for_push(),
            ui: PushUi::new(false),
            issue,
            conflicts: ConflictsRun::new(),
            home: worktree(BRAVO),
            current: worktree(BRAVO),
            generation: Generation::default(),
        };
        assert!(state.issue.is_armed(now), "the fixture must start armed");

        state.switch_to(worktree(CHARLIE), &|| now, &mut |target: &WorktreePath| {
            Ok(snapshot_of(target))
        });

        assert_eq!(
            state.current,
            worktree(CHARLIE),
            "the switch must reach charlie"
        );
        assert!(
            !state.issue.is_armed(now),
            "a switch must take the arming of `G` away",
        );
    }

    /// The reason the fake `switch` hook gives when it refuses the worktree
    /// `name`, as it reaches the row.
    fn refusal_of(name: &str) -> String {
        format!("{REFUSED}: {}", worktree(name).as_path().display())
    }

    #[test]
    fn a_switch_that_fails_stays_and_shows_the_reason_on_a_fading_line() {
        // The open fails: the chosen worktree stopped existing, or its
        // directory is not a work tree. gsw stays on the worktree it shows,
        // and the reason is gsw's report about a key, so it fades.
        let (screen, seen) = run_in(
            World::three().refusing(CHARLIE),
            vec![key(KeyCode::Right), Event::Quit],
        );
        assert_eq!(
            seen.switches,
            vec![worktree(CHARLIE)],
            "Right must try the next worktree",
        );
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME {BRAVO}\n{} (0s ago)", refusal_of(CHARLIE)),
            "the frame must stay on the worktree it showed, with the reason under it",
        );
    }

    #[test]
    fn after_a_switch_that_failed_p_g_and_m_act_on_the_old_worktree() {
        // Nothing moves when a switch fails. Every key acts on the worktree
        // the frame still shows, and the next Right tries the same worktree
        // again.
        let (_screen, seen) = run_in(
            World::three().refusing(CHARLIE),
            vec![
                probe_answered(),
                key(KeyCode::Right),
                key(KeyCode::Right),
                key(KeyCode::Char('p')),
                key(KeyCode::Char('y')),
                press_g(),
                press_m(),
                Event::Quit,
            ],
        );
        assert_eq!(
            seen.switches,
            vec![worktree(CHARLIE), worktree(CHARLIE)],
            "a failed switch leaves Right where it was",
        );
        assert_eq!(
            seen.push_paths,
            vec![worktree(BRAVO)],
            "`p` pushes from bravo"
        );
        assert_eq!(
            seen.pushes.first().map(PushCommand::branch),
            Some(BRAVO),
            "the push must name the branch of the worktree on the screen",
        );
        assert_eq!(
            paths_only(&seen.issue_paths),
            vec![worktree(BRAVO)],
            "`G` runs in bravo",
        );
        assert_eq!(
            paths_only(&seen.conflict_paths),
            vec![worktree(BRAVO)],
            "`m` measures bravo",
        );
    }

    #[test]
    fn a_switch_that_fails_leaves_the_refresh_clock_alone() {
        // The frame still shows the old worktree, and a failed open walked
        // nothing of it, so its clock runs on: 10 seconds are left, as with
        // no switch at all.
        let base = Instant::now();
        let later = base + Duration::from_secs(50);
        let (_screen, seen) = drive(
            vec![key(KeyCode::Right), Event::Quit],
            Setup {
                schedule: WalkSchedule::new(Some(REFRESH), base, Duration::ZERO),
                ..in_world(World::three().refusing(CHARLIE))
            },
            move || later,
        );
        assert_eq!(
            seen.timings
                .last()
                .and_then(|timing| timing.next_refresh_in),
            Some(Duration::from_secs(10)),
        );
    }

    /// The generation of a run that started before the first switch.
    fn before_the_switch() -> Generation {
        Generation::default()
    }

    /// The generation of a run that started after one switch.
    fn after_one_switch() -> Generation {
        Generation::default().next()
    }

    /// A run of `m` with the generation `generation` knows it measures against
    /// `main`, as the loop receives it.
    fn started_in(generation: Generation) -> Event {
        Event::ConflictsStarted {
            generation,
            branch: "main".to_string(),
        }
    }

    /// A run of `m` with the generation `generation` ended with `outcome`, as
    /// the loop receives it.
    fn finished_in(generation: Generation, outcome: ConflictsOutcome) -> Event {
        Event::ConflictsFinished {
            generation,
            outcome,
        }
    }

    #[test]
    fn an_m_outcome_from_before_a_switch_is_dropped_and_m_works_again() {
        // The run continues in bravo, where it started, and its outcome
        // arrives with the frame on charlie. A line under the frame must
        // describe the worktree in the frame, so the outcome goes. The key is
        // free again, and the next run carries the new generation.
        let (screen, seen) = run_in(
            World::three(),
            vec![
                press_m(),
                started_in(before_the_switch()),
                key(KeyCode::Right),
                finished_in(before_the_switch(), measured_clean()),
                press_m(),
                Event::Quit,
            ],
        );
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME {CHARLIE}"),
            "the outcome of the run in bravo must not reach the row of charlie",
        );
        assert_eq!(
            seen.conflict_paths,
            vec![
                (worktree(BRAVO), before_the_switch()),
                (worktree(CHARLIE), after_one_switch()),
            ],
            "the outcome must free the key, and the next run must carry the new generation",
        );
    }

    #[test]
    fn a_g_outcome_from_before_a_switch_is_dropped_and_g_works_again() {
        // The same rule for the issue key: its error describes the worktree
        // where the run started.
        let (screen, seen) = run_in(
            World::three(),
            vec![
                probe_answered(),
                press_g(),
                key(KeyCode::Right),
                issue_failed_in(before_the_switch()),
                press_g(),
                Event::Quit,
            ],
        );
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME {CHARLIE}"),
            "the error of the run in bravo must not reach the row of charlie",
        );
        assert_eq!(
            seen.issue_paths,
            vec![
                (worktree(BRAVO), before_the_switch()),
                (worktree(CHARLIE), after_one_switch()),
            ],
            "the outcome must free the key, and the next run must carry the new generation",
        );
    }

    #[test]
    fn a_stale_m_notice_puts_nothing_under_the_frame() {
        // A run from before the switch says which branch it measures against
        // only after the switch. The notice describes bravo, so it goes
        // nowhere.
        let (screen, _seen) = run_in(
            World::three(),
            vec![
                press_m(),
                key(KeyCode::Right),
                started_in(before_the_switch()),
                Event::Quit,
            ],
        );
        assert_eq!(strip_ansi(&screen), format!("FRAME {CHARLIE}"));
    }

    #[test]
    fn the_one_run_rule_of_m_and_g_spans_a_switch() {
        // A run continues after a switch, in the worktree where it started,
        // and the rule of one run at a time holds for the whole process.
        let (_screen, seen) = run_in(
            World::three(),
            vec![press_m(), key(KeyCode::Right), press_m(), Event::Quit],
        );
        assert_eq!(
            seen.conflict_runs, 1,
            "a run of `m` in flight must refuse a second run after a switch",
        );

        let (_screen, seen) = run_in(
            World::three(),
            vec![
                probe_answered(),
                press_g(),
                key(KeyCode::Right),
                press_g(),
                Event::Quit,
            ],
        );
        assert_eq!(
            seen.issue_runs.len(),
            1,
            "a run of `G` in flight must refuse a second run after a switch, got {:?}",
            seen.issue_runs,
        );
    }

    #[test]
    fn the_outcome_of_a_run_that_started_after_the_switch_reaches_the_row() {
        // Only an old generation is dropped. A run that started on charlie
        // describes charlie, so its notice and its result reach the row.
        let (screen, _seen) = run_in(
            World::three(),
            vec![
                key(KeyCode::Right),
                press_m(),
                started_in(after_one_switch()),
                finished_in(after_one_switch(), measured_clean()),
                Event::Quit,
            ],
        );
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME {CHARLIE}\n{MEASURED_CLEAN} (0s ago)"),
        );
    }

    #[test]
    fn down_opens_the_list_with_the_cursor_on_the_current_worktree() {
        // The list opens on the worktree that the frame shows, and the screen
        // is what the list hook drew, with nothing under it. Down reads the
        // list once, and walks and switches nothing.
        let (screen, seen) = run_in(World::three(), vec![key(KeyCode::Down), Event::Quit]);
        assert_eq!(
            strip_ansi(&screen),
            format!("LIST {BRAVO}: {ALPHA} >{BRAVO}⌂ {CHARLIE}"),
        );
        assert_eq!(seen.listings, 1, "Down must read the list once");
        assert_eq!(seen.collects, 0, "Down must not walk");
        assert!(seen.switches.is_empty(), "Down must not switch");

        // After a switch, the cursor is on the worktree that the frame shows,
        // and the home mark stays on the home worktree.
        let (screen, _seen) = run_in(
            World::three(),
            vec![key(KeyCode::Right), key(KeyCode::Down), Event::Quit],
        );
        assert_eq!(
            strip_ansi(&screen),
            format!("LIST {CHARLIE}: {ALPHA} {BRAVO}⌂ >{CHARLIE}"),
        );
    }

    #[test]
    fn with_one_worktree_down_opens_a_list_of_one_row() {
        let (screen, _seen) = run_in(World::alone(), vec![key(KeyCode::Down), Event::Quit]);
        assert_eq!(strip_ansi(&screen), format!("LIST {ALONE}: >{ALONE}⌂"));
    }

    #[test]
    fn while_the_list_is_open_every_other_key_does_nothing() {
        // The list takes the pane. `r` walks nothing, `p` and `y` push
        // nothing, `G` and `m` start nothing, Left and Right switch nothing,
        // and the list stays open with its cursor where it was.
        let (screen, seen) = run_in(
            World::three(),
            vec![
                probe_answered(),
                key(KeyCode::Down),
                key(KeyCode::Char('r')),
                key(KeyCode::Char('p')),
                key(KeyCode::Char('y')),
                press_g(),
                press_m(),
                key(KeyCode::Left),
                key(KeyCode::Right),
                key(KeyCode::Char('x')),
                Event::Quit,
            ],
        );
        assert_eq!(
            strip_ansi(&screen),
            format!("LIST {BRAVO}: {ALPHA} >{BRAVO}⌂ {CHARLIE}"),
        );
        assert_eq!(seen.collects, 0, "`r` must not walk");
        assert!(seen.pushes.is_empty(), "`p` and `y` must not push");
        assert!(seen.issue_runs.is_empty(), "`G` must not run");
        assert_eq!(seen.conflict_runs, 0, "`m` must not measure");
        assert!(
            seen.switches.is_empty(),
            "Left and Right must not switch, got {:?}",
            seen.switches,
        );
    }

    /// A pane with room for the header and the separator of a frame, and no
    /// row under them.
    const NO_ROW_FOR_THE_LIST: Dimensions = Dimensions {
        width: 80,
        height: 2,
    };

    /// A pane with room for the header, the separator, and one row under
    /// them.
    const ONE_ROW_FOR_THE_LIST: Dimensions = Dimensions {
        width: 80,
        height: 3,
    };

    /// Run the loop over `events` in the three worktrees, in a pane of `dims`,
    /// on a frozen clock.
    fn run_in_pane_of(dims: Dimensions, events: Vec<Event>) -> (String, Seen) {
        let base = Instant::now();
        drive(
            events,
            Setup {
                dims,
                measured: dims,
                ..in_world(World::three())
            },
            move || base,
        )
    }

    #[test]
    fn a_pane_too_short_for_the_list_opens_nothing() {
        // Enter must never choose a row that the user did not see, as `p`
        // never asks a question that the pane cannot show. A pane with no row
        // under the separator opens no list, and does not read the list
        // either. A pane with one row there opens a list of that one row.
        let (screen, seen) =
            run_in_pane_of(NO_ROW_FOR_THE_LIST, vec![key(KeyCode::Down), Event::Quit]);
        assert_eq!(strip_ansi(&screen), format!("FRAME {BRAVO}"));
        assert_eq!(
            seen.listings, 0,
            "a pane that cannot show the list must not read it",
        );

        let (screen, _seen) =
            run_in_pane_of(ONE_ROW_FOR_THE_LIST, vec![key(KeyCode::Down), Event::Quit]);
        assert_eq!(strip_ansi(&screen), format!("LIST {BRAVO}: >{BRAVO}⌂"));
    }

    /// Run the loop over `bursts` in `world`, set up as [`in_world`] says, on
    /// a frozen clock. Gives every screen the loop painted, as visible glyphs,
    /// and what the hooks saw.
    fn paints_in(world: World, bursts: Vec<Vec<Event>>) -> (Vec<String>, Seen) {
        paints_of(in_world(world), bursts)
    }

    /// Run the loop over `bursts` as `setup` says, on a frozen clock. Gives
    /// every screen the loop painted, as visible glyphs, and what the hooks
    /// saw.
    fn paints_of(setup: Setup, bursts: Vec<Vec<Event>>) -> (Vec<String>, Seen) {
        let base = Instant::now();
        let (_screen, seen) = drive_bursts(bursts, setup, move || base);
        let paints = seen.paints.iter().map(|paint| strip_ansi(paint)).collect();
        (paints, seen)
    }

    #[test]
    fn esc_and_q_close_the_list_and_gsw_stays() {
        // The cursor is on another worktree when the list closes, and gsw
        // stays on the worktree that it showed before the list opened. `q`
        // closes the list, as it answers "no" to the push question. A quit
        // there would still paint the list, because a quit inside a burst
        // paints the burst first.
        for code in [KeyCode::Esc, KeyCode::Char('q')] {
            let (screen, seen) = run_in(
                World::three(),
                vec![
                    key(KeyCode::Down),
                    key(KeyCode::Down),
                    key(code),
                    Event::Quit,
                ],
            );
            assert_eq!(
                strip_ansi(&screen),
                format!("FRAME {BRAVO}"),
                "{code:?} must close the list",
            );
            assert!(
                seen.switches.is_empty(),
                "{code:?} must not switch, got {:?}",
                seen.switches,
            );
        }
    }

    #[test]
    fn a_message_that_arrives_while_the_list_is_open_waits_for_the_frame_that_closes_it() {
        // A message waits in the queue while the list is open, as it waits
        // while a push owns the row. The frame that closes the list carries
        // it, with its whole life ahead of it.
        let (paints, _seen) = paints_in(
            World::three(),
            vec![
                vec![press_m(), key(KeyCode::Down)],
                vec![finished(measured_clean())],
                vec![key(KeyCode::Esc), Event::Quit],
            ],
        );
        assert_eq!(
            paints,
            [
                format!("LIST {BRAVO}: {ALPHA} >{BRAVO}⌂ {CHARLIE}"),
                format!("FRAME {BRAVO}\n{MEASURED_CLEAN} (0s ago)"),
            ],
            "the outcome must wait under the list, and reach the row on the frame that closes it",
        );
    }

    #[test]
    fn down_replaces_a_status_line_that_does_not_come_back() {
        // The list opens over the line, as `p` asks its question over it. The
        // line described the frame that the user stopped reading, so it does
        // not come back when the list closes.
        let (paints, _seen) = paints_in(
            World::three(),
            vec![
                vec![press_m(), refused_run()],
                vec![key(KeyCode::Down)],
                vec![key(KeyCode::Esc), Event::Quit],
            ],
        );
        assert_eq!(
            paints,
            [
                format!("FRAME {BRAVO}\n{REFUSED_RUN_LINE} (0s ago)"),
                format!("LIST {BRAVO}: {ALPHA} >{BRAVO}⌂ {CHARLIE}"),
                format!("FRAME {BRAVO}"),
            ],
        );
    }

    #[test]
    fn enter_goes_to_the_worktree_under_the_cursor_and_closes_the_list() {
        // Enter takes the one switch that every key takes, so the frame shows
        // the snapshot that the switch gave, and a run that starts after it
        // carries the new generation.
        let (screen, seen) = run_in(
            World::three(),
            vec![
                key(KeyCode::Down),
                key(KeyCode::Down),
                key(KeyCode::Enter),
                press_m(),
                Event::Quit,
            ],
        );
        assert_eq!(seen.switches, vec![worktree(CHARLIE)], "Enter on charlie");
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME {CHARLIE}"),
            "the list must close on the frame of charlie",
        );
        assert_eq!(
            seen.conflict_paths,
            vec![(worktree(CHARLIE), after_one_switch())],
            "`m` after Enter must measure charlie, in the new generation",
        );

        let (screen, seen) = run_in(
            World::three(),
            vec![
                key(KeyCode::Down),
                key(KeyCode::Up),
                key(KeyCode::Enter),
                Event::Quit,
            ],
        );
        assert_eq!(seen.switches, vec![worktree(ALPHA)], "Enter on alpha");
        assert_eq!(strip_ansi(&screen), format!("FRAME {ALPHA}"));
    }

    #[test]
    fn enter_on_the_current_worktree_closes_the_list_and_does_not_switch() {
        // The frame shows that worktree already, so there is nothing to open
        // and nothing to walk.
        let (screen, seen) = run_in(
            World::three(),
            vec![key(KeyCode::Down), key(KeyCode::Enter), Event::Quit],
        );
        assert!(
            seen.switches.is_empty(),
            "Enter on the current worktree must not switch, got {:?}",
            seen.switches,
        );
        assert_eq!(
            seen.collects, 0,
            "Enter on the current worktree must not walk"
        );
        assert_eq!(strip_ansi(&screen), format!("FRAME {BRAVO}"));
    }

    #[test]
    fn enter_on_a_worktree_whose_open_fails_closes_the_list_and_shows_the_reason() {
        // The chosen worktree stopped existing before Enter. The list closes,
        // gsw stays on the worktree it showed, and the reason is gsw's report
        // about a key, so it fades.
        let (screen, seen) = run_in(
            World::three().refusing(CHARLIE),
            vec![
                key(KeyCode::Down),
                key(KeyCode::Down),
                key(KeyCode::Enter),
                Event::Quit,
            ],
        );
        assert_eq!(
            seen.switches,
            vec![worktree(CHARLIE)],
            "Enter tries charlie"
        );
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME {BRAVO}\n{} (0s ago)", refusal_of(CHARLIE)),
            "the frame must stay on bravo, with the reason under it",
        );
    }

    #[test]
    fn a_pane_that_shrinks_under_the_list_closes_it_and_shows_the_held_message() {
        // The list closes when the pane leaves no row for it, so Enter never
        // chooses a row that the user did not see. A message that waited for
        // the list reaches the row on the frame that closes it, and not one
        // frame later.
        let (paints, seen) = paints_of(
            Setup {
                measured: NO_ROW_FOR_THE_LIST,
                ..in_world(World::three())
            },
            vec![
                vec![press_m(), key(KeyCode::Down)],
                vec![finished(measured_clean())],
                vec![Event::Resize, Event::Quit],
            ],
        );
        assert_eq!(
            paints,
            [
                format!("LIST {BRAVO}: {ALPHA} >{BRAVO}⌂ {CHARLIE}"),
                format!("FRAME {BRAVO}\n{MEASURED_CLEAN} (0s ago)"),
            ],
            "the frame that closes the list must carry the message that waited for it",
        );
        assert_eq!(
            seen.frame_heights.last().copied(),
            Some(NO_ROW_FOR_THE_LIST.height - 1),
            "the message takes one row of the shrunken pane from the frame",
        );
    }

    /// The line that the return to the home worktree puts under the frame,
    /// after the worktree `name` went away.
    ///
    /// Written out here rather than taken from the code it pins, so a change
    /// to the words is a change these tests report.
    fn gone_line(name: &str) -> String {
        format!(
            "{} no longer exists — back to the home worktree",
            worktree(name).as_path().display()
        )
    }

    /// A clock that stands still at the start for the first `reads` reads,
    /// and one minute later for every read after them.
    fn a_minute_after(reads: usize) -> impl Fn() -> Instant {
        let base = Instant::now();
        clock_that_jumps(base, reads, base + Duration::from_secs(60))
    }

    #[test]
    fn a_failed_walk_of_a_worktree_no_longer_listed_goes_back_home_and_says_so() {
        // The worktree on the screen stopped existing (`git worktree remove`,
        // `swt merge`). Its walk fails, and the list no longer holds it, so
        // gsw goes back to the home worktree through the one switch, and says
        // why on a line that fades. The frame is the fresh snapshot of home,
        // and `m` after the return measures home.
        let (paints, seen) = paints_in(
            World::three().vanishing(CHARLIE),
            vec![
                vec![key(KeyCode::Right), Event::ForceRefresh],
                vec![press_m(), Event::Quit],
            ],
        );
        assert_eq!(
            paints,
            [format!("FRAME {BRAVO}\n{} (0s ago)", gone_line(CHARLIE))],
        );
        assert_eq!(
            seen.switches,
            vec![worktree(CHARLIE), worktree(BRAVO)],
            "Right goes to charlie, and the failed walk goes back home",
        );
        assert_eq!(
            seen.collected_from,
            vec![worktree(CHARLIE)],
            "the walk that failed must be the walk of charlie",
        );
        assert_eq!(
            seen.timings.last().map(|timing| timing.age_offset),
            Some(Duration::ZERO),
            "the frame of home must be fresh",
        );
        assert_eq!(
            seen.conflict_paths,
            vec![(worktree(BRAVO), after_one_switch().next())],
            "`m` after the return must measure home, in the generation of the return",
        );
    }

    #[test]
    fn a_failed_walk_while_a_push_runs_stays_and_a_later_failed_walk_goes_home() {
        // The window under the frame belongs to the worktree that pushes, so
        // a walk that fails during the push does not go home. The first walk
        // that fails after the push does.
        let (paints, seen) = paints_in(
            World::three().vanishing(CHARLIE),
            vec![
                vec![
                    key(KeyCode::Right),
                    key(KeyCode::Char('p')),
                    key(KeyCode::Char('y')),
                    Event::ForceRefresh,
                ],
                vec![
                    Event::PushFinished(PushOutcome {
                        success: false,
                        output: "error: failed to push some refs\n".to_string(),
                    }),
                    Event::ForceRefresh,
                    Event::Quit,
                ],
            ],
        );
        let during = paints.first().expect("the push paints a frame");
        assert!(
            during.starts_with(&format!("FRAME {CHARLIE}\nPushing")),
            "a failed walk during the push must stay on charlie, got {during:?}",
        );
        assert_eq!(
            paints.last(),
            Some(&format!("FRAME {BRAVO}\n{} (0s ago)", gone_line(CHARLIE))),
            "the failed walk after the push must go home",
        );
        assert_eq!(seen.switches, vec![worktree(CHARLIE), worktree(BRAVO)]);
    }

    #[test]
    fn a_failed_return_home_keeps_the_last_good_frame_at_its_age_and_says_nothing() {
        // The home worktree went away too, so the switch to it fails. gsw
        // then does what it does today for a failed walk: it keeps the last
        // good snapshot at its true age. It posts no line, because each later
        // failed walk tries again, and a line would come back on each one.
        //
        // The harness reads the clock once, and the switch of Right reads it
        // twice. The filesystem event comes a minute later.
        let (screen, seen) = drive(
            vec![key(KeyCode::Right), Event::FsChanged, Event::Quit],
            in_world(World::three().vanishing(CHARLIE).removed(BRAVO)),
            a_minute_after(3),
        );
        assert_eq!(
            seen.switches,
            vec![worktree(CHARLIE), worktree(BRAVO)],
            "gsw must try to go home",
        );
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME {CHARLIE}"),
            "the last good frame must stay, with no line under it",
        );
        assert_eq!(
            seen.timings.last().map(|timing| timing.age_offset),
            Some(Duration::from_secs(60)),
            "the last good snapshot must show its true age",
        );
    }

    #[test]
    fn a_failed_walk_of_the_home_worktree_keeps_the_last_good_frame() {
        // The home worktree stopped existing while gsw shows it. There is no
        // home to go back to, so gsw keeps the last good snapshot at its true
        // age, and reads neither the list nor the paths of the worktrees for a
        // return that cannot happen.
        //
        // The harness reads the clock once, and the filesystem event comes a
        // minute later.
        let (screen, seen) = drive(
            vec![Event::FsChanged, Event::Quit],
            in_world(World::three().removed(BRAVO)),
            a_minute_after(1),
        );
        assert!(
            seen.switches.is_empty(),
            "gsw must not switch, got {:?}",
            seen.switches,
        );
        assert_eq!(seen.listings, 0, "gsw must not read the list");
        assert_eq!(seen.path_listings, 0, "gsw must not read the paths");
        assert_eq!(strip_ansi(&screen), format!("FRAME {BRAVO}"));
        assert_eq!(
            seen.timings.last().map(|timing| timing.age_offset),
            Some(Duration::from_secs(60)),
            "the last good snapshot must show its true age",
        );
    }

    #[test]
    fn a_failed_walk_of_a_worktree_still_listed_keeps_the_last_good_frame() {
        // A walk can fail for a moment while the worktree still exists. The
        // list still holds it, so gsw stays on it with the last good snapshot
        // at its true age.
        //
        // The harness reads the clock once, and the switch of Right reads it
        // twice. The filesystem event comes a minute later.
        let (screen, seen) = drive(
            vec![key(KeyCode::Right), Event::FsChanged, Event::Quit],
            in_world(World::three().unreadable(CHARLIE)),
            a_minute_after(3),
        );
        assert_eq!(
            seen.switches,
            vec![worktree(CHARLIE)],
            "gsw must stay on charlie"
        );
        assert_eq!(strip_ansi(&screen), format!("FRAME {CHARLIE}"));
        assert_eq!(
            seen.timings.last().map(|timing| timing.age_offset),
            Some(Duration::from_secs(60)),
            "the last good snapshot must show its true age",
        );
    }

    #[test]
    fn the_return_home_closes_the_list_and_drops_the_question() {
        // Both describe the worktree that went away. The list shows the
        // worktrees as they stood when it opened, and the question asks to
        // push the branch of a worktree that is gone.
        let (screen, _seen) = run_in(
            World::three().vanishing(CHARLIE),
            vec![
                key(KeyCode::Right),
                key(KeyCode::Down),
                Event::ForceRefresh,
                Event::Quit,
            ],
        );
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME {BRAVO}\n{} (0s ago)", gone_line(CHARLIE)),
            "the return must close the list",
        );

        let (paints, seen) = paints_in(
            World::three().vanishing(CHARLIE),
            vec![
                vec![
                    key(KeyCode::Right),
                    key(KeyCode::Char('p')),
                    Event::ForceRefresh,
                ],
                vec![key(KeyCode::Char('y')), Event::Quit],
            ],
        );
        assert_eq!(
            paints.first(),
            Some(&format!("FRAME {BRAVO}\n{} (0s ago)", gone_line(CHARLIE))),
            "the return must drop the question",
        );
        assert!(
            seen.pushes.is_empty(),
            "a `y` after the return must not push, got {:?}",
            seen.pushes,
        );
    }

    #[test]
    fn a_removed_worktree_sends_gsw_back_to_the_home_worktree_with_a_fading_line() {
        // The whole path, end to end. The user opens the list and goes to
        // charlie with Enter, and another pane removes charlie. The
        // filesystem event of the removal walks charlie, the walk fails, and
        // gsw goes back to the home worktree and says why.
        //
        // The harness reads the clock once, and the switch of Enter reads it
        // twice. The filesystem event comes a minute later, past the cooldown
        // that the switch armed, so it walks.
        let (screen, seen) = drive(
            vec![
                key(KeyCode::Down),
                key(KeyCode::Down),
                key(KeyCode::Enter),
                Event::FsChanged,
                Event::Quit,
            ],
            in_world(World::three().vanishing(CHARLIE)),
            a_minute_after(3),
        );
        assert_eq!(seen.switches, vec![worktree(CHARLIE), worktree(BRAVO)]);
        assert_eq!(seen.collected_from, vec![worktree(CHARLIE)]);
        assert_eq!(
            strip_ansi(&screen),
            format!("FRAME {BRAVO}\n{} (0s ago)", gone_line(CHARLIE)),
        );
    }

    #[test]
    fn left_right_and_the_check_after_a_failed_walk_read_no_label() {
        // Left and Right need the paths of the worktrees alone, and so does
        // the check after a failed walk. A label costs an open of each linked
        // worktree, so none of the three reads the list with the labels. Only
        // Down shows the labels.
        let (_screen, seen) = run_in(
            World::three(),
            vec![
                key(KeyCode::Right),
                key(KeyCode::Left),
                key(KeyCode::Right),
                Event::Quit,
            ],
        );
        assert_eq!(
            seen.switches,
            vec![worktree(CHARLIE), worktree(BRAVO), worktree(CHARLIE)],
        );
        assert_eq!(
            seen.listings, 0,
            "Left and Right must not read the list with the labels",
        );

        // The walk of charlie fails while the list still holds charlie, so
        // the check reads the paths and gsw stays. The harness reads the clock
        // once, and the switch of Right reads it twice. The filesystem event
        // comes a minute later.
        let (_screen, seen) = drive(
            vec![key(KeyCode::Right), Event::FsChanged, Event::Quit],
            in_world(World::three().unreadable(CHARLIE)),
            a_minute_after(3),
        );
        assert_eq!(
            seen.collected_from,
            vec![worktree(CHARLIE)],
            "the walk of charlie must run and fail",
        );
        assert_eq!(
            seen.listings, 0,
            "Right and the check must not read the list with the labels",
        );
    }

    /// Run the loop in `world`, where each read of a list of the worktrees
    /// takes one second: Right, then a filesystem event in the same burst,
    /// then one more filesystem event in the next burst. Gives what the hooks
    /// saw.
    ///
    /// The harness reads the clock once, and the switch of Right reads it
    /// twice. The first filesystem event comes a minute later, past the
    /// cooldown that the switch armed, so it walks the worktree that Right
    /// reached.
    fn right_then_two_events_with_slow_lists(world: World) -> Seen {
        let (_screen, seen) = drive_bursts(
            vec![
                vec![key(KeyCode::Right), Event::FsChanged],
                vec![Event::FsChanged, Event::Quit],
            ],
            in_world(world.slow_list(Duration::from_secs(1))),
            a_minute_after(3),
        );
        seen
    }

    #[test]
    fn the_duty_cycle_pays_for_the_check_after_a_failed_walk() {
        // After a failed walk, the check reads the paths of the worktrees,
        // and gsw tries to open home when the worktree on the screen is gone.
        // Both are git work of the wake, so the cooldown that the wake arms
        // holds their cost, whether the return happens or not. Without that,
        // a worktree that cannot come back costs a read of the list at each
        // wake, and the duty cycle does not see it.
        //
        // The check takes one second here, so the wake of the failed walk
        // costs one second and arms a cooldown of 100 seconds. The second
        // event comes one second later, inside that cooldown, so it walks
        // nothing.
        //
        // The walk of charlie fails for a moment, and the list still holds
        // charlie.
        let seen = right_then_two_events_with_slow_lists(World::three().unreadable(CHARLIE));
        assert_eq!(
            seen.collected_from,
            vec![worktree(CHARLIE)],
            "the check must pay into the cooldown, so the second event must not walk",
        );

        // Charlie is gone, and home is gone too, so the open of home fails.
        // The cooldown holds the check and the open that failed.
        let seen =
            right_then_two_events_with_slow_lists(World::three().vanishing(CHARLIE).removed(BRAVO));
        assert_eq!(
            seen.collected_from,
            vec![worktree(CHARLIE)],
            "a return that fails must pay into the cooldown too",
        );
        assert_eq!(
            seen.switches,
            vec![worktree(CHARLIE), worktree(BRAVO)],
            "gsw must try to go home once",
        );
    }

    /// A pane with room for the head of a frame, two rows of the list, and
    /// the hint.
    const TWO_ROWS_FOR_THE_LIST: Dimensions = Dimensions {
        width: 80,
        height: 5,
    };

    #[test]
    fn a_list_longer_than_the_pane_scrolls_only_as_far_as_the_cursor_needs() {
        // Six worktrees in a pane of two list rows. The window follows the
        // cursor down. When the cursor comes back up one row, the window that
        // the user saw still holds it, so the window stays where it was. A
        // window that moved then would put the cursor row where the user did
        // not look for it.
        let names = [ALPHA, BRAVO, CHARLIE, "delta", "echo", "foxtrot"];
        let (paints, _seen) = paints_of(
            Setup {
                dims: TWO_ROWS_FOR_THE_LIST,
                measured: TWO_ROWS_FOR_THE_LIST,
                ..in_world(World::of(&names, ALPHA))
            },
            vec![
                vec![key(KeyCode::Down)],
                vec![key(KeyCode::Down)],
                vec![key(KeyCode::Down)],
                vec![key(KeyCode::Down)],
                vec![key(KeyCode::Up), Event::Quit],
            ],
        );
        assert_eq!(
            paints,
            [
                format!("LIST {ALPHA}: >{ALPHA}⌂ {BRAVO}"),
                format!("LIST {ALPHA}: {ALPHA}⌂ >{BRAVO}"),
                format!("LIST {ALPHA}: {BRAVO} >{CHARLIE}"),
                format!("LIST {ALPHA}: {CHARLIE} >delta"),
                format!("LIST {ALPHA}: >{CHARLIE} delta"),
            ],
        );
    }

    #[test]
    fn up_and_down_move_the_cursor_stop_at_the_ends_and_neither_walk_nor_switch() {
        // The frame does not change while the cursor moves: gsw walks the new
        // worktree only after Enter. Each burst below gets a frame. Down on
        // the bottom row and Up on the top row change nothing, so the loop
        // paints nothing for them.
        let (paints, seen) = paints_in(
            World::three(),
            vec![
                vec![key(KeyCode::Down)],
                vec![key(KeyCode::Down)],
                vec![key(KeyCode::Down)],
                vec![key(KeyCode::Up)],
                vec![key(KeyCode::Up)],
                vec![key(KeyCode::Up), Event::Quit],
            ],
        );
        assert_eq!(
            paints,
            [
                format!("LIST {BRAVO}: {ALPHA} >{BRAVO}⌂ {CHARLIE}"),
                format!("LIST {BRAVO}: {ALPHA} {BRAVO}⌂ >{CHARLIE}"),
                format!("LIST {BRAVO}: {ALPHA} >{BRAVO}⌂ {CHARLIE}"),
                format!("LIST {BRAVO}: >{ALPHA} {BRAVO}⌂ {CHARLIE}"),
            ],
        );
        assert_eq!(seen.collects, 0, "a move of the cursor must not walk");
        assert!(
            seen.switches.is_empty(),
            "a move of the cursor must not switch, got {:?}",
            seen.switches,
        );
        assert_eq!(
            seen.listings, 1,
            "only the Down that opens the list reads it",
        );
    }
}

/// The tests of [`Watched`] and of the production switch, against real
/// repositories and real filesystem watchers.
///
/// Every fixture is a repository of `crate::testrepo` in a [`tempfile::TempDir`]
/// of its own, so the tests stay parallel-safe and never touch the repository
/// that the suite runs in.
#[cfg(test)]
mod watched_tests {
    use std::cell::RefCell;
    use std::path::Path;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use super::tests::walk_config;
    use super::{
        listed, listed_paths, switch_watched, Event, Watched, DIRECTORY_GONE, NOT_A_WORK_TREE,
    };
    use crate::render::Snapshot;
    use crate::repo::RepoHandle;
    use crate::testrepo::{git, git_stdout, init_repo, init_repo_at, init_repo_with_worktree};
    use crate::worktrees::{WorktreeBadge, WorktreeEntry, WorktreePath};

    /// The main worktree of [`siblings`], on branch `main`. Its path sorts
    /// last.
    const MAIN: &str = "main";

    /// The linked worktree of [`siblings`], on the branch of the same name. Its
    /// path sorts between the two others.
    const LINKED: &str = "linked";

    /// The detached linked worktree of [`siblings`]. Its path sorts first.
    const DETACHED: &str = "detached";

    /// How many hex digits of the commit a detached HEAD shows: the length
    /// that `cwt` shows. Stated here as the oracle, apart from the constant of
    /// the code under test.
    const CWT_SHORT_HASH: usize = 7;

    /// A repository with three worktrees side by side in one [`TempDir`]:
    ///
    /// | Path       | Worktree                   |
    /// | ---------- | -------------------------- |
    /// | `detached` | linked, detached           |
    /// | `linked`   | linked, on branch `linked` |
    /// | `main`     | main, on branch `main`     |
    ///
    /// No worktree is inside another, so the recursive watch of one worktree
    /// never covers another. The drop of the [`TempDir`] deletes all three.
    fn siblings() -> TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join(MAIN);
        init_repo_at(&main);
        add_worktree(&main, &dir.path().join(LINKED), &["-b", LINKED]);
        add_worktree(&main, &dir.path().join(DETACHED), &["--detach"]);
        dir
    }

    /// Add a linked worktree of the repository at `main`, at `path`. `how`
    /// holds the options of `git worktree add` that pick the HEAD: a new
    /// branch (`-b <name>`) or `--detach`.
    fn add_worktree(main: &Path, path: &Path, how: &[&str]) {
        let mut args = vec!["worktree", "add", "-q"];
        args.extend_from_slice(how);
        args.push(path.to_str().expect("utf-8 tempdir path"));
        git(main, &args);
    }

    /// The [`WorktreePath`] of a directory that the fixture made.
    fn resolved(path: &Path) -> WorktreePath {
        WorktreePath::resolve(path).expect("the fixture made this directory")
    }

    /// The label of the detached HEAD at `dir`, as `cwt` shows it: `HEAD@` and
    /// the first [`CWT_SHORT_HASH`] hex digits of the id that git reports.
    fn detached_label(dir: &Path) -> String {
        let full = git_stdout(dir, &["rev-parse", "HEAD"]);
        let short: String = full.chars().take(CWT_SHORT_HASH).collect();
        format!("HEAD@{short}")
    }

    /// A [`Watched`] on the worktree at `path`, built as [`super::run`] builds
    /// the first one: from a handle that is open on the worktree already. Its
    /// watcher sends on `tx`.
    fn seeded(path: &Path, tx: mpsc::Sender<Event>) -> Watched {
        let handle = RepoHandle::discover(path).expect("the fixture is a work tree");
        Watched::from_handle(handle, resolved(path), tx).expect("watch the fixture")
    }

    /// A walk puts the badge of its worktree on the snapshot: the position of
    /// the worktree among the worktrees in path order, their count, whether it
    /// is the home worktree, and its label. The label of a detached worktree is
    /// `HEAD@` and the short hash, as `cwt` shows it, because a detached
    /// worktree has no branch.
    ///
    /// The home worktree is the middle one, so a badge that marks the first or
    /// the last worktree as home fails.
    #[test]
    fn a_walk_puts_the_badge_of_its_worktree_on_the_snapshot() {
        let dir = siblings();
        let home = resolved(&dir.path().join(LINKED));
        let expected = [
            (
                DETACHED,
                WorktreeBadge {
                    position: 1,
                    count: 3,
                    home: false,
                    label: detached_label(&dir.path().join(DETACHED)),
                },
            ),
            (
                LINKED,
                WorktreeBadge {
                    position: 2,
                    count: 3,
                    home: true,
                    label: LINKED.to_string(),
                },
            ),
            (
                MAIN,
                WorktreeBadge {
                    position: 3,
                    count: 3,
                    home: false,
                    label: MAIN.to_string(),
                },
            ),
        ];
        let (tx, _rx) = mpsc::channel();

        for (name, badge) in expected {
            let mut watched = seeded(&dir.path().join(name), tx.clone());
            let snapshot = watched
                .walk(&walk_config(), &home)
                .expect("walk the fixture");
            assert_eq!(
                snapshot.worktree,
                Some(badge),
                "the walk of the worktree {name}"
            );
        }
    }

    /// A repository with one worktree gets no badge, so the header of its frame
    /// stays as it was before gsw moved between worktrees.
    #[test]
    fn a_walk_of_a_repository_with_one_worktree_puts_no_badge_on_the_snapshot() {
        let dir = init_repo();
        let home = resolved(dir.path());
        let (tx, _rx) = mpsc::channel();

        let mut watched = seeded(dir.path(), tx);
        let snapshot = watched
            .walk(&walk_config(), &home)
            .expect("walk the fixture");

        assert_eq!(snapshot.worktree, None);
    }

    /// The name of the file that a test changes in a worktree, to learn which
    /// worktree a walk read.
    const CHANGED: &str = "changed.txt";

    /// How long a test waits for a filesystem watcher: for the old watcher to
    /// hang up, and for the new watcher to report a change. Generous, because a
    /// loaded machine delays both. Each wait ends as soon as its answer
    /// arrives.
    const WATCHER_DEADLINE: Duration = Duration::from_secs(20);

    /// How long the channel of the new watcher must stay quiet before the test
    /// writes the file that must wake it. The events of the switch itself
    /// arrive in this window, so the event that the test then waits for comes
    /// from its own write.
    const QUIET: Duration = Duration::from_millis(500);

    /// The paths of the files that `snapshot` lists.
    fn files_of(snapshot: &Snapshot) -> Vec<&str> {
        snapshot
            .files
            .iter()
            .map(|entry| entry.path.as_str())
            .collect()
    }

    /// Assert that the watch is still on the worktree at `dir`, on `branch`:
    /// the path of `watched` is `dir`, and a walk reads a file that changes in
    /// `dir`.
    fn assert_still_on(watched: &RefCell<Watched>, dir: &Path, branch: &str, home: &WorktreePath) {
        assert_eq!(
            watched.borrow().path,
            resolved(dir),
            "the watch must stay on the old worktree",
        );
        std::fs::write(dir.join(CHANGED), "changed\n").expect("write in the old worktree");
        let later = watched
            .borrow_mut()
            .walk(&walk_config(), home)
            .expect("walk the old worktree");
        assert_eq!(
            later.branch, branch,
            "a later walk must read the old worktree"
        );
        assert!(
            files_of(&later).contains(&CHANGED),
            "a later walk must read the old worktree, got {:?}",
            files_of(&later),
        );
    }

    /// Whether every sender of `rx` hangs up before `deadline` passes. An event
    /// that arrives first is read and dropped, because it was sent before the
    /// hang-up.
    fn hangs_up(rx: &Receiver<Event>, deadline: Duration) -> bool {
        let give_up_at = Instant::now() + deadline;
        while let Some(left) = give_up_at.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(_) => {}
                Err(RecvTimeoutError::Disconnected) => return true,
                Err(RecvTimeoutError::Timeout) => return false,
            }
        }
        false
    }

    /// Whether a filesystem event reaches `rx` before `deadline` passes.
    fn wakes(rx: &Receiver<Event>, deadline: Duration) -> bool {
        let give_up_at = Instant::now() + deadline;
        while let Some(left) = give_up_at.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(Event::FsChanged) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
        false
    }

    /// Read and drop every event on `rx` until the channel stays quiet for
    /// [`QUIET`], or until `deadline` passes.
    fn drain(rx: &Receiver<Event>, deadline: Duration) {
        let give_up_at = Instant::now() + deadline;
        while Instant::now() < give_up_at {
            if rx.recv_timeout(QUIET).is_err() {
                return;
            }
        }
    }

    /// A switch that works moves every later walk to the new worktree. The
    /// switch gives the first frame of the new worktree. A later walk reads a
    /// file that changed in the new worktree, and its badge names the new
    /// worktree.
    #[test]
    fn a_switch_that_works_makes_every_later_walk_read_the_new_worktree() {
        let dir = siblings();
        let main = dir.path().join(MAIN);
        let linked = dir.path().join(LINKED);
        let home = resolved(&main);
        let cfg = walk_config();
        let (tx, _rx) = mpsc::channel();
        let watched = RefCell::new(seeded(&main, tx.clone()));

        let first = switch_watched(&watched, &resolved(&linked), tx, &cfg, &home)
            .unwrap_or_else(|reason| panic!("the switch to {LINKED} must work: {reason}"));
        assert_eq!(
            first.branch, LINKED,
            "the switch gives the first frame of the new worktree",
        );
        assert_eq!(watched.borrow().path, resolved(&linked));

        std::fs::write(linked.join(CHANGED), "changed\n").expect("write in the new worktree");
        let later = watched
            .borrow_mut()
            .walk(&cfg, &home)
            .expect("walk the new worktree");

        assert_eq!(later.branch, LINKED);
        assert!(
            files_of(&later).contains(&CHANGED),
            "a later walk must read the new worktree, got {:?}",
            files_of(&later),
        );
        assert_eq!(
            later.worktree,
            Some(WorktreeBadge {
                position: 2,
                count: 3,
                home: false,
                label: LINKED.to_string(),
            }),
            "the badge must name the new worktree",
        );
    }

    /// A switch to a worktree whose directory is gone fails. The reason names
    /// the directory, and the watch stays on the old worktree.
    #[test]
    fn a_switch_to_a_directory_that_is_gone_leaves_the_old_worktree_and_gives_the_reason() {
        let dir = siblings();
        let main = dir.path().join(MAIN);
        let linked = dir.path().join(LINKED);
        let home = resolved(&main);
        let target = resolved(&linked);
        std::fs::remove_dir_all(&linked).expect("delete the directory of the linked worktree");
        let (tx, _rx) = mpsc::channel();
        let watched = RefCell::new(seeded(&main, tx.clone()));

        let reason = switch_watched(&watched, &target, tx, &walk_config(), &home)
            .expect_err("a directory that is gone must refuse the switch");

        assert_still_on(&watched, &main, MAIN, &home);
        assert!(
            reason.contains(&target.as_path().display().to_string()),
            "the reason must name the directory: {reason}",
        );
    }

    /// An index that gix reads and refuses: bytes that are not an index, and
    /// more of them than the checksum at the end of an index takes. gix checks
    /// that checksum first, finds it wrong, and gives an error.
    ///
    /// A shorter file makes gix-index 0.51 panic on an overflow of a
    /// subtraction (`src/file/init.rs:73`), where it must give an error. So a
    /// walk of a worktree whose index is shorter than 20 bytes ends watch mode.
    /// That is a defect of gix. This fixture stays clear of it, because the
    /// test is about a walk that fails, and not about a walk that panics.
    const SPOILED_INDEX: &[u8] = &[0xAB; 64];

    /// A switch whose walk fails leaves the old worktree in place, as a switch
    /// whose open fails does: the switch replaces the old worktree only when
    /// both work. The new worktree opens, but its index is [`SPOILED_INDEX`],
    /// so its status walk fails. The reason names the directory.
    #[test]
    fn a_switch_whose_walk_fails_leaves_the_old_worktree_and_gives_the_reason() {
        let dir = siblings();
        let main = dir.path().join(MAIN);
        let linked = dir.path().join(LINKED);
        let home = resolved(&main);
        let target = resolved(&linked);
        let index = git_stdout(
            &linked,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        );
        std::fs::write(&index, SPOILED_INDEX).expect("spoil the index of the linked worktree");
        let (tx, _rx) = mpsc::channel();
        let watched = RefCell::new(seeded(&main, tx.clone()));

        let reason = switch_watched(&watched, &target, tx, &walk_config(), &home)
            .expect_err("a worktree whose walk fails must refuse the switch");

        assert_still_on(&watched, &main, MAIN, &home);
        assert!(
            reason.contains(&target.as_path().display().to_string()),
            "the reason must name the directory: {reason}",
        );
    }

    /// After a switch, a change to a file in the new worktree wakes the loop,
    /// and a change in the old worktree does not.
    ///
    /// The watchers are real `notify` watchers, on sibling worktrees. A
    /// worktree inside another is under the recursive watch of the other by
    /// design, so a nested layout proves nothing about the switch.
    ///
    /// The first watcher sends on one channel, and the switch gives the new
    /// watcher another channel. That makes the negative half exact, where a
    /// quiet window only makes it likely: the old channel reports that every
    /// sender hung up. The old watcher and its sender are then gone, so no
    /// change in the old worktree can ever reach the loop through them, and a
    /// write in the old worktree finds the channel hung up still. In production
    /// both watchers send on the one channel of the loop, and the new watcher
    /// does not watch the old worktree, because the two are siblings.
    ///
    /// The positive half first waits for the channel of the new watcher to go
    /// quiet, so the event that it waits for comes from the write of the test.
    /// Every wait has a deadline. The drop of `watched` at the end stops the
    /// new watcher and joins its thread, so nothing of the test outlives it.
    #[test]
    fn after_a_switch_a_change_in_the_new_worktree_wakes_the_loop_and_one_in_the_old_does_not() {
        let dir = siblings();
        let main = dir.path().join(MAIN);
        let linked = dir.path().join(LINKED);
        let home = resolved(&main);
        let (old_tx, old_rx) = mpsc::channel();
        let watched = RefCell::new(seeded(&main, old_tx));
        let (new_tx, new_rx) = mpsc::channel();

        switch_watched(&watched, &resolved(&linked), new_tx, &walk_config(), &home)
            .unwrap_or_else(|reason| panic!("the switch to {LINKED} must work: {reason}"));

        assert!(
            hangs_up(&old_rx, WATCHER_DEADLINE),
            "the switch must stop the old watcher, and drop its sender, within {}s",
            WATCHER_DEADLINE.as_secs(),
        );
        std::fs::write(main.join(CHANGED), "changed\n").expect("write in the old worktree");
        assert!(
            matches!(old_rx.try_recv(), Err(TryRecvError::Disconnected)),
            "a change in the old worktree must not reach the loop",
        );

        drain(&new_rx, WATCHER_DEADLINE);
        std::fs::write(linked.join(CHANGED), "changed\n").expect("write in the new worktree");
        assert!(
            wakes(&new_rx, WATCHER_DEADLINE),
            "a change in the new worktree must wake the loop within {}s",
            WATCHER_DEADLINE.as_secs(),
        );
    }

    /// Down reads every worktree of the repository, sorted by path, whatever
    /// worktree the watch is on: the main worktree, a linked worktree, or a
    /// detached one. Each entry carries its label, and the label of the
    /// detached worktree is `HEAD@` and the short hash. Left and Right read
    /// the paths of the same worktrees, in the same order, with no label.
    #[test]
    fn the_list_holds_every_worktree_of_the_repository_from_each_worktree() {
        let dir = siblings();
        let expected = vec![
            WorktreeEntry {
                path: resolved(&dir.path().join(DETACHED)),
                label: detached_label(&dir.path().join(DETACHED)),
            },
            WorktreeEntry {
                path: resolved(&dir.path().join(LINKED)),
                label: LINKED.to_string(),
            },
            WorktreeEntry {
                path: resolved(&dir.path().join(MAIN)),
                label: MAIN.to_string(),
            },
        ];
        let paths: Vec<WorktreePath> = expected.iter().map(|entry| entry.path.clone()).collect();
        let (tx, _rx) = mpsc::channel();

        for name in [DETACHED, LINKED, MAIN] {
            let watched = RefCell::new(seeded(&dir.path().join(name), tx.clone()));
            assert_eq!(
                listed(&watched),
                expected,
                "the list read from the worktree {name}",
            );
            assert_eq!(
                listed_paths(&watched),
                paths,
                "the paths read from the worktree {name}",
            );
        }
    }

    /// Open the worktree at `path` through [`Watched::open`], as a switch opens
    /// its target. The watcher of an open that works sends on a channel that
    /// nobody reads, because these tests wait for no event.
    fn opened(path: &WorktreePath) -> Result<Watched, String> {
        let (tx, _rx) = mpsc::channel();
        Watched::open(path, tx)
    }

    /// A linked worktree opens, on its own path, and its walk reads that
    /// worktree.
    #[test]
    fn open_opens_a_linked_worktree() {
        let dir = siblings();
        let linked = resolved(&dir.path().join(LINKED));

        let mut watched = opened(&linked)
            .unwrap_or_else(|reason| panic!("the linked worktree must open: {reason}"));

        assert_eq!(watched.path, linked);
        let snapshot = watched
            .walk(&walk_config(), &linked)
            .expect("walk the linked worktree");
        assert_eq!(snapshot.branch, LINKED);
    }

    /// A worktree whose directory no longer exists is refused with
    /// [`DIRECTORY_GONE`], and the reason names the directory.
    #[test]
    fn open_refuses_a_directory_that_no_longer_exists() {
        let dir = siblings();
        let linked = resolved(&dir.path().join(LINKED));
        std::fs::remove_dir_all(linked.as_path()).expect("delete the linked worktree");

        let reason = opened(&linked)
            .err()
            .expect("a directory that is gone must not open");

        assert_eq!(
            reason,
            format!("{DIRECTORY_GONE}: {}", linked.as_path().display()),
        );
    }

    /// A plain directory is not a git work tree, so it is refused with
    /// [`NOT_A_WORK_TREE`], and the reason names the directory.
    #[test]
    fn open_refuses_a_plain_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = resolved(dir.path());

        let reason = opened(&plain)
            .err()
            .expect("a plain directory must not open");

        assert_eq!(
            reason,
            format!("{NOT_A_WORK_TREE}: {}", plain.as_path().display()),
        );
    }

    /// A linked worktree inside the main worktree that lost its `.git` file is
    /// refused with [`NOT_A_WORK_TREE`].
    ///
    /// Discovery walks up from the directory, so it finds the main worktree
    /// around it. An open that took that answer would watch the parent
    /// repository under the name of the child, and the frame would show the
    /// status of one worktree under the name of another. The test first
    /// asserts that discovery really finds the parent, or the refusal proves
    /// nothing.
    #[test]
    fn open_refuses_a_nested_worktree_that_lost_its_git_file_and_never_opens_the_parent() {
        let (repo, nested) = init_repo_with_worktree();
        std::fs::remove_file(nested.join(".git")).expect("remove the .git file of the worktree");
        let target = resolved(&nested);
        assert_eq!(
            RepoHandle::discover(&nested)
                .and_then(|handle| handle.repo().workdir().and_then(WorktreePath::resolve)),
            Some(resolved(repo.path())),
            "discovery must find the main worktree around the directory",
        );

        let reason = opened(&target)
            .err()
            .expect("a directory that lost its .git file must not open");

        assert_eq!(
            reason,
            format!("{NOT_A_WORK_TREE}: {}", target.as_path().display()),
        );
    }
}

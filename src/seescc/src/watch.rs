//! Watch mode: the pure decision helpers that drive the live refresh loop.
//!
//! Phase 4 adds a self-refreshing watch view to seescc. Every *decision* the
//! watch loop makes that does not require a live terminal — which rendering
//! mode to run in, whether a key press means "quit", and whether colors should
//! survive being piped through a watch-like wrapper — lives here as a pure,
//! terminal-free function so it can be unit-tested without a pty. The terminal
//! lifecycle and event loop that consume these helpers are wired in later in
//! this phase.

use std::io::{self, Write};
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

/// An external event the watch loop reacts to between polls.
///
/// Unlike `gsw` (which is filesystem-event driven), seescc's loop is
/// **timer-driven**: sccache exposes nothing to subscribe to, so a fresh poll is
/// triggered by a `recv_timeout` *timeout*, not by an event. The only events
/// carried on the channel are the two things a poll can't produce on its own — a
/// terminal resize and a user quit — both originating from the keyboard /
/// resize reader thread wired in by a later slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    /// The terminal was resized. The previously-painted bytes are now laid out
    /// for the old dimensions and are visually stale, so the loop must
    /// re-render at the new size and repaint **unconditionally**, bypassing the
    /// byte-compare suppression that would otherwise swallow an identical frame.
    Resize,
    /// The user asked to quit (`q`, `Esc`, or Ctrl-C). The loop returns cleanly.
    Quit,
}

/// The poll-outcome state machine that backs the watch loop's rendering.
///
/// Each poll of sccache feeds its outcome into [`WatchState::apply_poll`], which
/// is the *only* way the three displayed facts evolve:
///
/// - `last_good` — the most recent successfully-parsed [`crate::stats::Stats`].
///   The render closure draws the numeric rows from this, so it must survive a
///   failed poll: when the server is briefly unreachable we keep showing the
///   last numbers we trust rather than blanking the table.
/// - `error` — the banner message for the *current* poll, or `None`. A poll
///   failure is **non-fatal** (design §6: "error banner + keep last good
///   frame"), so it sets the banner without disturbing `last_good`; the next
///   successful poll clears the banner (recovery), so a transient blip leaves no
///   lingering error once the server is back.
/// - `history` — the window-bounded [`crate::history::History`] ring of
///   timestamped snapshots that backs the per-metric sparklines. Every
///   successful poll is *also* pushed here (a failed poll leaves it untouched,
///   so the trend keeps drawing from the last good samples during an outage),
///   and [`compose_watch_frame`] buckets it to the terminal's spark budget at
///   render time. It is private so the only way to feed or read it is through
///   `apply_poll` / `compose_watch_frame`, keeping the ring's retention rule
///   from ever desyncing.
pub(crate) struct WatchState {
    /// The most recent stats from a *successful* poll, retained across failures
    /// so the table keeps showing trustworthy numbers during an outage.
    last_good: Option<crate::stats::Stats>,
    /// The error banner for the current poll, or `None` when the last poll
    /// succeeded. Cleared on recovery.
    error: Option<String>,
    /// The window-bounded history of successful polls, bucketed into the
    /// sparkline columns by [`compose_watch_frame`]. Empty at launch and filled
    /// in over the first `window` of runtime.
    history: crate::history::History,
}

impl WatchState {
    /// Create a fresh watch state whose history retains samples observed within
    /// `window`, with its sparkline slice grid anchored at `epoch`.
    ///
    /// `window` comes from the resolved [`crate::config::Config`]; it sizes the
    /// [`crate::history::History`] ring that backs the sparklines. `epoch` is the
    /// fixed slice-grid anchor, captured once at watch startup (production passes
    /// `Instant::now()`; tests fabricate it) and forwarded straight into
    /// [`crate::history::History::new`] so bucket assignments stay stable between
    /// frames. Nothing is sampled yet, so the watch view starts with a blank
    /// sparkline that fills in over the first `window` of runtime (design §4:
    /// "in-memory only, empty at launch").
    pub(crate) fn new(window: Duration, epoch: Instant) -> Self {
        WatchState {
            last_good: None,
            error: None,
            history: crate::history::History::new(window, epoch),
        }
    }

    /// The last successfully-polled stats, if any poll has ever succeeded.
    ///
    /// The render closure draws the metric rows from this; `None` only before
    /// the first successful poll (e.g. a first-poll failure), where the caller
    /// renders a banner with no rows.
    pub(crate) fn last_good(&self) -> Option<&crate::stats::Stats> {
        self.last_good.as_ref()
    }

    /// The current error-banner message, or `None` when the last poll
    /// succeeded.
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Bucket the retained history into `columns` time slices spanning
    /// `[now - window, now]`, newest slice last.
    ///
    /// A thin pass-through to [`crate::history::History::bucket_last`] that keeps
    /// the `history` field private: [`compose_watch_frame`] reaches the bucketed
    /// snapshots only through here, never the raw ring, so the retention rule
    /// stays encapsulated. The returned slots are `Some(stats)` for the most
    /// recent sample in that slice and `None` for a gap (drawn at baseline).
    fn bucket_history(&self, now: Instant, columns: usize) -> Vec<Option<&crate::stats::Stats>> {
        self.history.bucket_last(now, columns)
    }

    /// Fold a poll outcome into the state, timestamped at `at`.
    ///
    /// - `Ok(stats)` is a successful poll: it becomes the new `last_good`, is
    ///   *also* pushed into `history` at `at` (so the sparklines accumulate a
    ///   sample per successful poll), and clears any error banner (recovery — a
    ///   prior failure must not linger once the server is back). The stats are
    ///   cloned into the history ring so `last_good` and the ring each own a
    ///   copy.
    /// - `Err(e)` is a *non-fatal* failed poll: it sets the banner to `e`'s
    ///   display and leaves both `last_good` and `history` untouched, so the
    ///   table and the trend keep showing the last good data (design §6).
    ///
    /// `at` is the monotonic [`Instant`] the poll completed; the loop passes
    /// `Instant::now()`, and tests fabricate it so history can be exercised
    /// without sleeping.
    pub(crate) fn apply_poll(&mut self, outcome: anyhow::Result<crate::stats::Stats>, at: Instant) {
        match outcome {
            Ok(stats) => {
                // Success: record the sample in the history ring (for the
                // sparklines) and adopt the fresh numbers as last_good, then
                // clear any banner — a recovered poll must not keep showing a
                // stale error. The ring gets its own clone so both retain the
                // stats independently.
                self.history.push(at, stats.clone());
                self.last_good = Some(stats);
                self.error = None;
            }
            Err(e) => {
                // Failure is non-fatal: raise the banner but leave `last_good`
                // and `history` alone so the table and trend keep the last
                // trustworthy data.
                self.error = Some(e.to_string());
            }
        }
    }
}

/// The terminal-free core of the watch loop, with every side effect injected.
///
/// seescc's loop is **timer-driven** (sccache has no events to subscribe to), so
/// the structure differs from `gsw`'s fs-event loop in two ways: a poll is
/// triggered by a `recv_timeout` *timeout* rather than an event, and the channel
/// only ever carries [`Event::Resize`] / [`Event::Quit`]. Three closures keep
/// the loop unit-testable with no TTY, no real sccache, and no real time:
///
/// - `poll` fetches fresh stats (production: shell out to sccache); its outcome
///   is folded into `state` via [`WatchState::apply_poll`], so a failed poll is
///   absorbed there as a non-fatal banner and **never** propagates out of the
///   loop — a transient outage must not tear the watch view down.
/// - `render` turns the current `state` into a frame string (production: query
///   terminal size + wall clock, draw the table with banner/footer).
/// - `paint` writes a frame to the screen.
///
/// Behavior:
///
/// - **First frame is immediate.** On entry the loop polls, applies, renders,
///   and paints once *before* waiting, so the user sees data right away instead
///   of staring at a blank pane for a full `poll_interval`.
/// - **Timeout ⇒ poll.** Each `recv_timeout(poll_interval)` timeout is a tick:
///   poll, apply, render, and repaint **only if the frame differs** from what's
///   displayed. Byte-identical suppression skips repaints for ticks landing
///   within the same clock second (sub-second intervals only, since the render
///   closure embeds a live `%H:%M:%S` header that advances each second).
/// - **Resize ⇒ forced repaint.** A resize re-renders from the *current* state
///   (no poll — the numbers haven't changed) and paints **unconditionally**,
///   even when the bytes match `displayed`, because the on-screen layout is
///   stale after the resize and suppression would wrongly skip the redraw.
/// - **Quit / disconnect ⇒ clean return.** [`Event::Quit`] returns `Ok(())`
///   with no further work; a disconnected channel (every sender dropped) is
///   likewise a clean shutdown.
fn event_loop<P, R, Pa>(
    rx: &Receiver<Event>,
    poll_interval: Duration,
    state: &mut WatchState,
    displayed: &mut String,
    mut poll: P,
    mut render: R,
    mut paint: Pa,
) -> Result<()>
where
    P: FnMut() -> Result<crate::stats::Stats>,
    R: FnMut(&WatchState) -> String,
    Pa: FnMut(&str) -> Result<()>,
{
    // The immediate first frame: poll, fold the outcome into state (a failure
    // becomes a non-fatal banner here, never an early return), render, and paint
    // unconditionally — there is nothing on screen yet to compare against. This
    // is why the user sees data without waiting a full poll_interval.
    poll_once(state, &mut poll, &mut render, &mut paint, displayed, true)?;

    loop {
        match rx.recv_timeout(poll_interval) {
            // A timeout is a tick: re-poll and repaint only if the frame
            // changed (suppression skips repaints within a clock second).
            Err(RecvTimeoutError::Timeout) => {
                poll_once(state, &mut poll, &mut render, &mut paint, displayed, false)?;
            }
            // A resize re-renders from the *current* state at the new
            // dimensions (no poll — only the geometry changed) and paints
            // unconditionally: the previously-painted bytes are laid out for the
            // old size and are visually stale, so suppression must be bypassed
            // even when the new frame is byte-identical.
            Ok(Event::Resize) => {
                let frame = render(state);
                paint(&frame)?;
                *displayed = frame;
            }
            // Quit, or every sender gone: clean shutdown.
            Ok(Event::Quit) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Run one poll → apply → render → paint cycle, repainting either always
/// (`force`) or only when the new frame differs from `displayed`.
///
/// Factoring this out keeps the immediate first frame (`force = true`, nothing
/// on screen to compare) and each timeout-driven tick (`force = false`,
/// byte-compare suppression) on one code path, so they can never drift apart. A
/// failed poll is absorbed by [`WatchState::apply_poll`] into a non-fatal
/// banner, so this returns `Err` only when `paint` itself fails — never for a
/// poll error.
fn poll_once<P, R, Pa>(
    state: &mut WatchState,
    poll: &mut P,
    render: &mut R,
    paint: &mut Pa,
    displayed: &mut String,
    force: bool,
) -> Result<()>
where
    P: FnMut() -> Result<crate::stats::Stats>,
    R: FnMut(&WatchState) -> String,
    Pa: FnMut(&str) -> Result<()>,
{
    state.apply_poll(poll(), Instant::now());
    let frame = render(state);
    if force || frame != *displayed {
        paint(&frame)?;
        *displayed = frame;
    }
    Ok(())
}

/// Which rendering mode `seescc` runs in.
///
/// Watch mode is the default, but it only makes sense when there is a live
/// terminal to take over; otherwise seescc renders a single frame and exits.
/// The choice is made by [`decide_mode`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    /// Render a single frame and exit. Used for `--one-shot` and any non-TTY
    /// (piped or captured) stdout.
    OneShot,
    /// Long-lived watch loop that owns the whole terminal pane.
    Watch,
}

/// Which rendering mode to run in once sccache stats are available.
///
/// Watch mode is the default, but it only makes sense when there is a live
/// terminal to take over: `--one-shot` and any non-TTY stdout (a pipe, a file,
/// a watch-like wrapper) fall back to a single render.
pub(crate) fn decide_mode(force_one_shot: bool, stdout_is_tty: bool) -> Mode {
    if force_one_shot || !stdout_is_tty {
        Mode::OneShot
    } else {
        Mode::Watch
    }
}

/// Whether a key event should end the watch loop.
///
/// The watch loop quits on a **press** (never a key *release* — kitty and
/// Windows report releases too) of any of the conventional quit keys: `q`,
/// `Esc`, or Ctrl-C. Every other key — a plain `c` without Control, an
/// unrelated character, `Enter` — leaves the loop running.
pub(crate) fn is_quit_event(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> bool {
    // Ignore key-release events (kitty/Windows report them); only a press
    // should ever quit.
    if kind == KeyEventKind::Release {
        return false;
    }
    matches!(code, KeyCode::Char('q') | KeyCode::Esc)
        || (modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c')))
}

/// What the event-reader thread should do after handling one key event.
///
/// Extracted so the reader thread's per-key shutdown decision is testable
/// without a live terminal — the raw `event::read` loop can't be exercised in a
/// unit test, but this pure routing function can. The loop
/// ([`spawn_event_reader`]) routes every key event through
/// [`reader_action_after_key`] and acts on the result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReaderAction {
    /// Keep reading terminal events.
    Continue,
    /// The reader's job is done; let the thread exit.
    Stop,
}

/// Decide what the reader thread should do after handling one key event.
///
/// `is_quit` is whether the key was a quit key (per [`is_quit_event`]) and
/// `send_ok` is whether forwarding the resulting [`Event::Quit`] to the watch
/// loop succeeded. For a non-quit key there is nothing to forward, so the value
/// of `send_ok` is irrelevant and the reader keeps running.
pub(crate) fn reader_action_after_key(is_quit: bool, send_ok: bool) -> ReaderAction {
    if is_quit && !send_ok {
        ReaderAction::Stop
    } else {
        ReaderAction::Continue
    }
}

/// Whether colors should be force-enabled despite stdout not being a TTY.
///
/// True only when output is captured by a watch-like wrapper (stdout is not a
/// TTY *and* `COLUMNS` is set in env), and the user has not asked to suppress
/// colors via `NO_COLOR`. The wrapper renders the captured bytes inside its own
/// TTY-backed UI, so colors should pass through rather than be stripped as they
/// normally would be for a pipe.
pub(crate) fn should_force_colors(
    stdout_is_tty: bool,
    columns_env_present: bool,
    no_color_env: bool,
) -> bool {
    !stdout_is_tty && columns_env_present && !no_color_env
}

/// Render width assumed when no terminal-size signal is available at all — stdout
/// is piped and the wrapper didn't export `COLUMNS`. The classic 80-column VT100
/// default; the one-cell safety margin in [`effective_width`] still applies on
/// top, so a plain pipe lays out at 79. One source of truth for both the one-shot
/// and watch fallbacks so they can never disagree about the no-signal width.
pub(crate) const FALLBACK_TERMINAL_WIDTH: usize = 80;

/// Decide the effective terminal width seescc should render a frame for.
///
/// Mirrors `gsw`'s `effective_terminal_width` (sans gsw's extra `width_offset`,
/// which seescc has no flag for). Always leaves one cell of margin against the
/// detected column count, applied uniformly to every branch — including the
/// fallback, for consistency:
/// - **Watch-like wrapper** (`stdout_is_tty == false` *and* `COLUMNS` parsed,
///   e.g. viddy/watch capturing stdout): trust `columns_env`. The wrapper reports
///   the full terminal width via `COLUMNS` but renders into a content area one
///   column narrower (its scroll/refresh chrome), so the margin keeps the
///   rightmost cell clear. This is the same signal [`should_force_colors`] keys
///   on, so colors and width now follow `COLUMNS` together instead of width
///   stranding at 80 while colors pass through.
/// - **Direct TTY, or no `COLUMNS`** (`stdout_is_tty == true`, or a plain pipe):
///   use the queried `tty_width`, or [`FALLBACK_TERMINAL_WIDTH`] when none was
///   reported. A leaked `COLUMNS` is ignored on a real TTY — `terminal_size` is
///   authoritative there. The margin still applies: rendering a row exactly
///   `cols` cells wide collides with DECAWM auto-wrap and right-edge chrome on
///   many terminals, pushing the last glyph onto the next line.
///
/// The result is always at least 1, so a 1-column terminal can never collapse the
/// frame to zero width.
pub(crate) fn effective_width(
    tty_width: Option<usize>,
    columns_env: Option<usize>,
    stdout_is_tty: bool,
) -> usize {
    let detected = match (stdout_is_tty, columns_env) {
        (false, Some(cols)) => cols,
        _ => tty_width.unwrap_or(FALLBACK_TERMINAL_WIDTH),
    };
    detected.saturating_sub(1).max(1)
}

/// The header label used when the config selects no specific languages, meaning
/// per-language metrics are summed across every language.
///
/// Shared by the one-shot and watch frames so a config with no `languages`
/// filter renders the same `"all"` header in both views.
pub(crate) const ALL_LANGUAGES_LABEL: &str = "all";

/// The header language label for `config`: the configured languages joined with
/// `", "`, or [`ALL_LANGUAGES_LABEL`] when the list is empty (per-language
/// metrics summed across every language).
///
/// One source of truth for the header label so the one-shot and watch frames
/// can never disagree about how a given config is summarized.
pub(crate) fn languages_label(config: &crate::config::Config) -> String {
    if config.languages.is_empty() {
        ALL_LANGUAGES_LABEL.to_string()
    } else {
        config.languages.join(", ")
    }
}

/// Build the display rows for `config` from `stats`: one [`crate::render::Row`]
/// per configured metric, its value extracted via
/// [`crate::aggregate::metric_value`] (honoring the language filter) and
/// formatted for display.
///
/// Shared by the one-shot human frame and the watch frame so the two build their
/// tables identically — the rows can never drift apart between the two views.
/// Every row starts with `spark: None`: the one-shot frame keeps it that way (a
/// single sample has no history to draw), while the watch frame overwrites the
/// `spark == true` rows afterward in [`attach_sparklines`]. Keeping the spark
/// out of this shared helper is what lets one-shot stay sparkline-free without a
/// second row builder.
pub(crate) fn build_rows(
    config: &crate::config::Config,
    stats: &crate::stats::Stats,
) -> Vec<crate::render::Row> {
    config
        .metrics
        .iter()
        .map(|spec| crate::render::Row {
            label: spec.label.clone(),
            value: crate::aggregate::metric_value(spec.key, stats, &config.languages).format(),
            // No sparkline here: the one-shot frame wants none, and the watch
            // frame fills in the spark=true rows later via `attach_sparklines`.
            spark: None,
        })
        .collect()
}

/// Compose the full watch frame from the loop's current [`WatchState`] and the
/// resolved [`crate::config::Config`].
///
/// This is the bridge between the poll-outcome state machine and the pure
/// [`crate::render`] layer: it decides *what* to draw from `state` and hands the
/// pieces to [`crate::render::build_watch`]. The behavior tracks design §4/§6
/// directly:
///
/// - **With last-good stats** (a healthy poll, or a failure that still has the
///   prior good numbers to fall back on): draw the configured metric rows from
///   those stats and a footer summarizing cache occupancy and the history
///   window. The error banner is shown iff the *current* poll failed
///   ([`WatchState::error`]) — so a recovered server has rows + footer + no
///   banner, while a mid-outage server keeps the same rows + footer *plus* the
///   banner.
///
///   **Sparkline attachment** happens on top of those rows: the spark column's
///   width budget is [`crate::render::sparkline_budget`] for this `width` and the
///   rows' own label/value footprint. When the budget is positive the history is
///   bucketed to exactly that many columns ([`WatchState::bucket_history`] at
///   `now`), and every `spark == true` metric's row gets a rendered sparkline of
///   its [`crate::history::metric_series`] (Count → per-bucket deltas, Rate →
///   windowed hit rate, Size → carried-forward absolutes). Rate sparklines are
///   direction-colored per bucket — green where the windowed rate rose, red
///   where it fell ([`crate::sparkline::metric_sparkline`]). `spark == false`
///   metrics stay `None`. When the budget is **0** (a terminal too narrow to
///   carry even [`crate::render::MIN_SPARK_WIDTH`] glyphs) every row stays `None`:
///   the numbers take priority and the trend column is dropped entirely (design
///   §6, "Narrow terminal"). A failed poll never disturbs the history ring, so
///   the sparklines keep rendering from the last good samples through an outage.
/// - **With no last-good stats** (the very first poll failed and we have nothing
///   trustworthy to show): draw only the header and the banner — no rows, no
///   footer, no sparklines — because every number would be a fabrication.
///
/// `width` is the terminal column count to lay the frame out for, `clock` is the
/// preformatted wall-clock string, and `now` is the monotonic [`Instant`] the
/// history is bucketed up to; all three are injected by the loop's render closure
/// so this stays pure and testable without a TTY or a real clock.
pub(crate) fn compose_watch_frame(
    state: &WatchState,
    config: &crate::config::Config,
    width: usize,
    clock: &str,
    now: Instant,
) -> String {
    let label = languages_label(config);
    let banner = state.error();
    match state.last_good() {
        // We have trustworthy numbers: draw the table and the cache/window
        // footer, with the banner shown only while the current poll is failing.
        Some(stats) => {
            let mut rows = build_rows(config, stats);
            attach_sparklines(&mut rows, state, config, width, now);
            let footer =
                crate::render::build_footer(stats.cache_size, stats.max_cache_size, config.window);
            crate::render::build_watch(&label, clock, width, &rows, banner, Some(&footer))
        }
        // No good poll yet (first poll failed): header + banner only. Every
        // number would be a fabrication, so draw no rows and no footer.
        None => crate::render::build_watch(&label, clock, width, &[], banner, None),
    }
}

/// Attach a rendered sparkline to each `spark == true` row in `rows`, in place,
/// sizing the column to whatever the frame `width` can spare after the numbers.
///
/// The budget is [`crate::render::sparkline_budget`] for `width` and the rows'
/// label/value footprint. When it is 0 — a terminal too narrow to carry even
/// [`crate::render::MIN_SPARK_WIDTH`] glyphs — this returns without touching any
/// row, so the numbers are never sacrificed to a squashed stub (design §6). When
/// it is positive the history is bucketed once to that many columns (at `now`)
/// and reused for every metric; each `config` metric is matched to its row by
/// position (`build_rows` emits one row per metric in order), and a `spark == true`
/// metric's row gets `metric_sparkline(kind, metric_series(..))` — the
/// kind-aware renderer that draws rate cells green/red by the direction the
/// windowed rate moved (see [`crate::sparkline::metric_sparkline`]).
/// `spark == false` rows keep the `None` that [`build_rows`] gave them.
fn attach_sparklines(
    rows: &mut [crate::render::Row],
    state: &WatchState,
    config: &crate::config::Config,
    width: usize,
    now: Instant,
) {
    let budget = crate::render::sparkline_budget(width, rows);
    // Too narrow for a useful trend: drop the column entirely and let the numbers
    // stand alone. Every row keeps build_rows's `None`.
    if budget == 0 {
        return;
    }

    // One bucketing pass shared by every metric: the columns are identical, only
    // the metric extracted from each bucket differs.
    let buckets = state.bucket_history(now, budget);
    for (spec, row) in config.metrics.iter().zip(rows.iter_mut()) {
        if spec.spark {
            let series = crate::history::metric_series(spec.key, &buckets, &config.languages);
            row.spark = Some(crate::sparkline::metric_sparkline(spec.key.kind(), &series));
        }
    }
}

/// Run the live watch loop: take over the alternate screen and repaint the
/// sccache stats on a timer until the user quits with `q`, `Esc`, or Ctrl-C.
///
/// This is the thin, untestable shell — every decision it makes is delegated to
/// a tested pure function ([`compose_watch_frame`], [`is_quit_event`],
/// [`decide_mode`]) or to the already-tested [`event_loop`]. Its job is purely
/// to acquire the terminal, wire up the event reader, and supply the loop's
/// three side-effecting closures:
///
/// - **poll** is the caller-supplied stats fetch (production: shell out to
///   sccache); a failure is folded into the loop's [`WatchState`] as a non-fatal
///   banner and never tears the view down.
/// - **render** queries the live terminal width and wall-clock each frame and
///   composes the frame via [`compose_watch_frame`], so a resize redraws at the
///   new width and the clock stays current.
/// - **paint** writes the frame into the alternate screen via [`paint_output`].
///
/// The [`TerminalGuard`] restores the main screen, cursor, and cooked mode on
/// every exit path — normal return, propagated error, or panic — so the terminal
/// can never be left wedged. The `poll_interval` comes from the resolved config.
pub(crate) fn run(
    config: &crate::config::Config,
    poll: impl FnMut() -> Result<crate::stats::Stats>,
) -> Result<()> {
    let _guard = TerminalGuard::enter()?;

    let (tx, rx) = mpsc::channel();
    spawn_event_reader(tx);

    // Anchor the sparkline slice grid once, here at watch startup, so every frame
    // buckets against the same grid and completed columns never shift under the
    // user. `Instant::now()` is captured a single time and threaded into the
    // history ring via `WatchState::new`.
    let epoch = Instant::now();
    let mut state = WatchState::new(config.window, epoch);
    let mut displayed = String::new();
    event_loop(
        &rx,
        config.poll_interval,
        &mut state,
        &mut displayed,
        poll,
        // Re-query the terminal size, clock, and monotonic instant every frame so
        // a resize lays out at the new width, the header clock stays live, and the
        // sparklines bucket history up to "now".
        |state| {
            let width = current_watch_width();
            let clock = chrono::Local::now().format("%H:%M:%S").to_string();
            compose_watch_frame(state, config, width, &clock, Instant::now())
        },
        paint_output,
    )
}

/// The width the watch frame should lay out for, from the live terminal size.
///
/// A thin production wrapper over [`effective_width`]: the watch loop only runs
/// behind a live TTY (see [`decide_mode`]), so it always takes the direct-TTY
/// branch — `stdout_is_tty = true` makes the leaked `COLUMNS` irrelevant and
/// trusts the queried column count, falling back to [`FALLBACK_TERMINAL_WIDTH`]
/// when `terminal_size` reports nothing. The one-cell DECAWM safety margin and
/// the fallback both come from `effective_width`, so the watch and one-shot
/// paths can never disagree about either.
fn current_watch_width() -> usize {
    let tty_width = terminal_size::terminal_size().map(|(w, _h)| usize::from(w.0));
    effective_width(tty_width, None, true)
}

/// Paint `output` into the alternate screen, replacing whatever frame is there.
///
/// Copied faithfully from `gsw`: move home, clear the whole screen (so a shorter
/// frame can't leave stale glyphs from a taller previous one), then write the
/// frame. In raw mode a bare `'\n'` moves down *without* returning to column 0,
/// which would stair-step the table, so every newline is translated to CRLF.
fn paint_output(output: &str) -> Result<()> {
    let mut out = io::stdout();
    let painted = output.replace('\n', "\r\n");
    execute!(out, MoveTo(0, 0), Clear(ClearType::All))?;
    write!(out, "{painted}")?;
    out.flush()?;
    Ok(())
}

/// Spawn the crossterm event-reader thread feeding the watch loop.
///
/// It blocks on `event::read`, translating any quit key recognized by the tested
/// [`is_quit_event`] into [`Event::Quit`] and terminal resizes into
/// [`Event::Resize`], and ignoring everything else. The thread exits when a send
/// fails (the loop ended, so the receiver is gone) or when reading fails (the
/// terminal closed) — there is nothing left to deliver in either case.
fn spawn_event_reader(tx: Sender<Event>) {
    thread::spawn(move || loop {
        match event::read() {
            Ok(CtEvent::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            })) => {
                let is_quit = is_quit_event(code, modifiers, kind);
                let send_ok = !is_quit || tx.send(Event::Quit).is_ok();
                if reader_action_after_key(is_quit, send_ok) == ReaderAction::Stop {
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
/// the main screen and cursor on drop *and* via a panic hook, so no exit path —
/// normal return, propagated error, or panic — can leave the terminal in a
/// wedged state. The panic hook restores *before* the default handler prints, so
/// the panic message lands on the main screen rather than the torn-down
/// alternate one. On drop the pre-watch panic hook is reinstated, so our
/// terminal-restoring wrapper never lingers as global process state once the
/// guard is gone.
struct TerminalGuard {
    /// The panic hook in effect before [`TerminalGuard::enter`] wrapped it,
    /// reinstated on drop. `Option` only so `Drop` can move it back out.
    previous_hook: Option<PanicHook>,
}

/// Run the raw-mode + screen-entry sequence as one all-or-nothing step.
///
/// Splitting this out of [`TerminalGuard::enter`] makes the failure-recovery
/// path testable without a live TTY (the real `enable_raw_mode`/`execute!` both
/// need one): the caller passes the three terminal operations as closures, and
/// the tests drive it with recording stand-ins.
///
/// The contract this guards: `enable_raw` runs first; if the subsequent
/// `enter_screen` step fails, `disable_raw` is invoked (best-effort) before the
/// error propagates, so a partially-entered terminal can never be left in raw
/// mode with no [`TerminalGuard`] alive to restore it. When `enter_screen`
/// succeeds, `disable_raw` is **not** called — the guard's `Drop` owns teardown
/// from that point on.
fn enter_terminal_sequence(
    enable_raw: impl FnOnce() -> Result<()>,
    enter_screen: impl FnOnce() -> Result<()>,
    disable_raw: impl FnOnce(),
) -> Result<()> {
    enable_raw()?;
    // Raw mode is now on. If entering the alternate screen fails, the caller
    // ([`TerminalGuard::enter`]) returns Err without constructing the guard, so
    // no Drop will ever run restore_terminal — undo raw mode here ourselves
    // (best-effort) so the failure can't strand the shell in raw mode.
    if let Err(err) = enter_screen() {
        disable_raw();
        return Err(err);
    }
    Ok(())
}

impl TerminalGuard {
    /// Enter raw mode and the alternate screen, hide the cursor, and install a
    /// terminal-restoring panic hook chained in front of the existing one.
    ///
    /// Entry is all-or-nothing for raw mode: the raw-mode + screen-entry step is
    /// routed through [`enter_terminal_sequence`], which disables raw mode again
    /// if the alternate-screen step fails. So an error here always returns with
    /// raw mode off — it can never leave the shell raw with no guard alive to
    /// restore it. Only a fully successful entry constructs the [`TerminalGuard`],
    /// after which `Drop` (and the panic hook) own teardown.
    fn enter() -> Result<Self> {
        enter_terminal_sequence(
            || enable_raw_mode().map_err(Into::into),
            || execute!(io::stdout(), EnterAlternateScreen, Hide).map_err(Into::into),
            || {
                let _ = disable_raw_mode();
            },
        )?;

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
/// the hook, then unwinding runs `Drop`), and a partially-entered terminal must
/// still be cleaned up, so every step is independently ignored on error.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::mpsc;

    use super::*;

    /// The captured real-sccache fixture, parsed into [`crate::stats::Stats`] —
    /// the same data the rest of the crate's tests use, so [`WatchState`] is
    /// exercised against a realistic payload rather than an ad-hoc literal.
    const FIXTURE: &str = include_str!("../tests/fixtures/sccache-0.15.0.json");

    /// Parse the captured fixture into [`crate::stats::Stats`] for the state
    /// tests. The exact field values don't matter here — these tests assert
    /// *which* stats are retained, not their contents — so any successfully
    /// parsed payload works.
    fn fixture_stats() -> crate::stats::Stats {
        crate::stats::parse(FIXTURE).expect("fixture should parse")
    }

    /// A generous default history window for the state/frame tests. Large enough
    /// that none of the fabricated multi-poll sequences age out of the ring, so a
    /// test asserting "the trend lit up" never has a sample pruned out from under
    /// it.
    const TEST_WINDOW: Duration = Duration::from_secs(15 * 60);

    /// The spacing between fabricated polls in the multi-poll spark tests. Chosen
    /// well above any plausible per-bucket slice width for [`TEST_WINDOW`] over a
    /// width-80 frame's spark budget, so successive samples land in *distinct*
    /// time buckets rather than collapsing into one slice (where "newest wins"
    /// would flatten the series). This is what lets a rising counter actually
    /// draw a shape, and all three timestamps still sit comfortably inside the
    /// window so none is pruned. (A real 1 s poll fills buckets gradually; the
    /// tests jump the clock to exercise the same bucketing deterministically.)
    const POLL_STEP: Duration = Duration::from_secs(180);

    /// The seven block-drawing sparkline glyphs, mirrored from
    /// [`crate::sparkline::SPARK_GLYPHS`]. The frame tests inspect composed lines
    /// for these characters directly (never byte-slicing — repo UTF-8 rule).
    const SPARK_GLYPHS: [char; 7] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇'];
    /// The baseline glyph an inactive/flat bucket renders at.
    const BASELINE_GLYPH: char = '▁';

    /// Whether `line` contains any sparkline block glyph at all.
    fn has_any_spark_glyph(line: &str) -> bool {
        line.chars().any(|c| SPARK_GLYPHS.contains(&c))
    }

    /// Whether `line` carries a non-baseline glyph — i.e. the trend "lit up"
    /// above the flat baseline somewhere.
    fn has_above_baseline_glyph(line: &str) -> bool {
        line.chars()
            .any(|c| SPARK_GLYPHS.contains(&c) && c != BASELINE_GLYPH)
    }

    /// The first composed frame line whose label cell is `label`, or `None`.
    ///
    /// Rows are emitted as ` <label><pad>  …`, so the label appears right after
    /// the single leading space; `trim_start().starts_with(label)` finds the
    /// row without depending on the exact padding width.
    fn row_line<'a>(frame: &'a str, label: &str) -> Option<&'a str> {
        frame
            .lines()
            .find(|line| line.trim_start().starts_with(label))
    }

    /// The default-config metric rows for `stats`, built exactly the way the
    /// renderer builds them (label + formatted value per configured metric).
    /// The frame tests assert these strings appear in the composed frame, so
    /// they're derived from the live config rather than hardcoded.
    fn expected_rows(
        config: &crate::config::Config,
        stats: &crate::stats::Stats,
    ) -> Vec<(String, String)> {
        config
            .metrics
            .iter()
            .map(|spec| {
                let value =
                    crate::aggregate::metric_value(spec.key, stats, &config.languages).format();
                (spec.label.clone(), value)
            })
            .collect()
    }

    #[test]
    fn compose_watch_frame_healthy_has_rows_and_footer_and_no_banner() {
        // A healthy poll: the frame must carry every configured metric row
        // (label + formatted value), the cache/window footer, and — crucially —
        // no error banner, since the current poll succeeded.
        let config = crate::config::Config::default();
        let stats = fixture_stats();
        let now = Instant::now();
        let mut state = WatchState::new(TEST_WINDOW, now);
        state.apply_poll(Ok(stats.clone()), now);

        let frame = compose_watch_frame(&state, &config, 80, "12:34:56", now);

        for (label, value) in expected_rows(&config, &stats) {
            assert!(
                frame.contains(&label),
                "healthy frame must contain row label {label:?}; frame:\n{frame}",
            );
            assert!(
                frame.contains(&value),
                "healthy frame must contain row value {value:?}; frame:\n{frame}",
            );
        }

        let footer =
            crate::render::build_footer(stats.cache_size, stats.max_cache_size, config.window);
        assert!(
            frame.contains(&footer),
            "healthy frame must contain the footer {footer:?}; frame:\n{frame}",
        );

        assert!(
            !frame.contains("connection refused"),
            "a healthy frame must carry no error banner; frame:\n{frame}",
        );
    }

    #[test]
    fn compose_watch_frame_error_after_good_keeps_last_numbers_and_shows_banner() {
        // A poll failure after a good poll is non-fatal: the banner appears AND
        // the last good numbers stay on screen (design §6 — never blank the
        // table during a transient outage).
        let config = crate::config::Config::default();
        let stats = fixture_stats();
        let base = Instant::now();
        let mut state = WatchState::new(TEST_WINDOW, base);
        state.apply_poll(Ok(stats.clone()), base);
        state.apply_poll(
            Err(anyhow::anyhow!("connection refused")),
            base + Duration::from_secs(1),
        );

        let frame = compose_watch_frame(
            &state,
            &config,
            80,
            "12:34:56",
            base + Duration::from_secs(1),
        );

        assert!(
            frame.contains("connection refused"),
            "a failed poll after a good one must show the error banner; frame:\n{frame}",
        );
        for (label, value) in expected_rows(&config, &stats) {
            assert!(
                frame.contains(&label),
                "the last good row label {label:?} must survive the outage; frame:\n{frame}",
            );
            assert!(
                frame.contains(&value),
                "the last good row value {value:?} must survive the outage; frame:\n{frame}",
            );
        }
    }

    #[test]
    fn compose_watch_frame_error_with_no_good_has_banner_but_no_rows_or_footer() {
        // A first-poll failure has no trustworthy numbers to show: the frame is
        // header + banner only — no rows, no footer, since every number would be
        // a fabrication.
        let config = crate::config::Config::default();
        let stats = fixture_stats();
        let now = Instant::now();
        let mut state = WatchState::new(TEST_WINDOW, now);
        state.apply_poll(Err(anyhow::anyhow!("connection refused")), now);

        let frame = compose_watch_frame(&state, &config, 80, "12:34:56", now);

        assert!(
            frame.contains("connection refused"),
            "a first-poll failure must show the error banner; frame:\n{frame}",
        );
        // No fabricated rows: the default labels must be absent entirely.
        for (label, _value) in expected_rows(&config, &stats) {
            assert!(
                !frame.contains(&label),
                "with no good poll the frame must not draw row {label:?}; frame:\n{frame}",
            );
        }
        // No footer either — there is no cache size to report.
        assert!(
            !frame.contains("window"),
            "with no good poll the frame must carry no footer; frame:\n{frame}",
        );
        // ...and no sparkline glyphs anywhere: there is no history to draw.
        assert!(
            !has_any_spark_glyph(&frame),
            "a first-poll failure must draw no sparkline glyphs; frame:\n{frame}",
        );
    }

    /// The default-config label for a `spark = true` metric (`cache_hits`),
    /// resolved from the live config so the test never hardcodes a label that
    /// could drift from the catalog.
    fn spark_label(config: &crate::config::Config) -> String {
        config
            .metrics
            .iter()
            .find(|m| m.spark)
            .expect("default config has at least one spark metric")
            .label
            .clone()
    }

    /// A fixture-based [`crate::stats::Stats`] with the Rust `cache_hits` /
    /// `cache_misses` counters overridden to `hits` / `misses`. Mutating these
    /// between pushes is how the multi-poll tests drive per-bucket activity into
    /// the sparkline series for the default Rust-filtered config.
    fn fixture_with_rust(hits: u64, misses: u64) -> crate::stats::Stats {
        let mut stats = fixture_stats();
        stats
            .stats
            .cache_hits
            .counts
            .insert("Rust".to_string(), hits);
        stats
            .stats
            .cache_misses
            .counts
            .insert("Rust".to_string(), misses);
        stats
    }

    #[test]
    fn compose_attaches_sparklines_that_fill_in_over_successive_polls() {
        // Several successful polls with the Rust cache_hits counter rising between
        // them must light up the `cache_hits` sparkline — its row carries block
        // glyphs and at least one rises above the flat baseline (activity is
        // visible). The spark=false rows (compile_requests / requests_executed)
        // must carry NO glyphs at all: only the configured spark metrics draw a
        // trend.
        let config = crate::config::Config::default();
        let base = Instant::now();
        let mut state = WatchState::new(TEST_WINDOW, base);

        // Rust cache_hits climbs 1700 -> 1800 -> 2000 across three polls spaced so
        // they land in distinct time buckets, so the per-bucket deltas are
        // non-zero and uneven — a shape, not a flat line.
        state.apply_poll(Ok(fixture_with_rust(1700, 900)), base);
        state.apply_poll(Ok(fixture_with_rust(1800, 950)), base + POLL_STEP);
        let now = base + POLL_STEP * 2;
        state.apply_poll(Ok(fixture_with_rust(2000, 1100)), now);

        let frame = compose_watch_frame(&state, &config, 80, "12:34:56", now);

        let hits_label = "Cache hits";
        let hits_row =
            row_line(&frame, hits_label).expect("the cache_hits row must be present in the frame");
        assert!(
            has_any_spark_glyph(hits_row),
            "the spark=true cache_hits row must carry sparkline glyphs; row: {hits_row:?}",
        );
        assert!(
            has_above_baseline_glyph(hits_row),
            "rising activity must light the cache_hits sparkline above baseline; row: {hits_row:?}",
        );

        // The two spark=false rows must stay glyph-free.
        for non_spark in ["Compile requests", "Requests executed"] {
            let row = row_line(&frame, non_spark)
                .unwrap_or_else(|| panic!("the {non_spark} row must be present"));
            assert!(
                !has_any_spark_glyph(row),
                "a spark=false row ({non_spark}) must carry no glyphs; row: {row:?}",
            );
        }
        // Sanity: spark_label resolves to a real spark metric label so the helper
        // stays exercised alongside the literal labels above.
        assert!(
            config
                .metrics
                .iter()
                .any(|m| m.spark && m.label == spark_label(&config)),
            "spark_label must name a configured spark=true metric",
        );
    }

    #[test]
    fn compose_at_launch_draws_flat_baseline_sparklines_of_budget_length() {
        // A single successful poll: every spark=true row shows a present-but-flat
        // sparkline — all baseline glyphs (▁) — of exactly the width budget the
        // renderer allots. One sample yields one observed bucket whose delta is 0,
        // and the rest are gaps, so the whole series is flat.
        let config = crate::config::Config::default();
        let now = Instant::now();
        let mut state = WatchState::new(TEST_WINDOW, now);
        state.apply_poll(Ok(fixture_stats()), now);

        let width = 80;
        let frame = compose_watch_frame(&state, &config, width, "12:34:56", now);

        // The budget the renderer computes for this frame, from the same rows it
        // lays out — the spark length must match it exactly.
        let rows = build_rows(&config, &fixture_stats());
        let budget = crate::render::sparkline_budget(width, &rows);
        assert!(
            budget > 0,
            "precondition: width 80 must leave a spark budget"
        );

        for spark_metric in config.metrics.iter().filter(|m| m.spark) {
            let row = row_line(&frame, &spark_metric.label)
                .unwrap_or_else(|| panic!("the {} row must be present", spark_metric.label));
            let glyphs: Vec<char> = row.chars().filter(|c| SPARK_GLYPHS.contains(c)).collect();
            assert_eq!(
                glyphs.len(),
                budget,
                "spark for {} must be exactly the budget length; row: {row:?}",
                spark_metric.label,
            );
            assert!(
                glyphs.iter().all(|c| *c == BASELINE_GLYPH),
                "a single poll must render a flat (all-baseline) spark for {}; row: {row:?}",
                spark_metric.label,
            );
        }
    }

    #[test]
    fn compose_in_a_narrow_terminal_drops_sparks_but_keeps_numbers() {
        // A width that leaves no spark budget: the frame must carry NO glyphs, yet
        // every label and value string stays intact and untruncated — numbers take
        // priority over the trend (design §6).
        let config = crate::config::Config::default();
        let base = Instant::now();
        let mut state = WatchState::new(TEST_WINDOW, base);
        state.apply_poll(Ok(fixture_with_rust(1700, 900)), base);
        state.apply_poll(Ok(fixture_with_rust(1900, 1000)), base + POLL_STEP);
        let now = base + POLL_STEP * 2;
        state.apply_poll(Ok(fixture_with_rust(2100, 1200)), now);

        // Choose a width at which sparkline_budget collapses to 0.
        let stats = fixture_with_rust(2100, 1200);
        let rows = build_rows(&config, &stats);
        let narrow = 26; // below overhead + MIN_SPARK_WIDTH for the default rows
        assert_eq!(
            crate::render::sparkline_budget(narrow, &rows),
            0,
            "precondition: the chosen narrow width must zero the spark budget",
        );

        let frame = compose_watch_frame(&state, &config, narrow, "12:34:56", now);

        assert!(
            !has_any_spark_glyph(&frame),
            "a zero-budget width must draw no sparkline glyphs; frame:\n{frame}",
        );
        // Every label and value survives intact.
        for (label, value) in expected_rows(&config, &stats) {
            assert!(
                frame.contains(&label),
                "narrow frame must keep the full label {label:?}; frame:\n{frame}",
            );
            assert!(
                frame.contains(&value),
                "narrow frame must keep the full value {value:?}; frame:\n{frame}",
            );
        }
    }

    #[test]
    fn compose_hit_rate_row_sparks_the_windowed_rate_without_nan() {
        // The hit_rate row sparks the WINDOWED rate per bucket. With crafted
        // hit/miss deltas the row carries glyphs (a 0%/50%/100% spread is a real
        // shape), zero-activity buckets render baseline, and nothing panics on a
        // 0+0 bucket (no NaN). cache_hits / cache_misses cumulatives across four
        // polls produce per-bucket (hits_delta, misses_delta):
        //   p0 first observed -> 0/0   -> 0%
        //   p1 +10h/+10m       -> 50%
        //   p2 +0h/+0m         -> 0% (zero-activity baseline)
        //   p3 +10h/+0m        -> 100%
        let config = crate::config::Config::default();
        let base = Instant::now();
        let mut state = WatchState::new(TEST_WINDOW, base);
        state.apply_poll(Ok(fixture_with_rust(10, 10)), base);
        state.apply_poll(Ok(fixture_with_rust(20, 20)), base + POLL_STEP);
        state.apply_poll(Ok(fixture_with_rust(20, 20)), base + POLL_STEP * 2);
        let now = base + POLL_STEP * 3;
        state.apply_poll(Ok(fixture_with_rust(30, 20)), now);

        let frame = compose_watch_frame(&state, &config, 80, "12:34:56", now);

        let rate_row =
            row_line(&frame, "Hit rate").expect("the hit_rate row must be present in the frame");
        assert!(
            has_any_spark_glyph(rate_row),
            "the hit_rate row must carry sparkline glyphs; row: {rate_row:?}",
        );
        assert!(
            has_above_baseline_glyph(rate_row),
            "a varied windowed rate must light the hit_rate spark above baseline; row: {rate_row:?}",
        );
        // No NaN can ever surface as a glyph — the whole frame stays clean.
        assert!(
            !frame.contains("NaN"),
            "the windowed rate must never render NaN; frame:\n{frame}",
        );
    }

    #[test]
    fn compose_after_a_failed_poll_still_renders_sparklines_from_history() {
        // poll ok, poll ok, then poll err: the banner shows AND the spark cells
        // still render from the untouched history. A failed poll never disturbs
        // the ring, so the trend persists through the outage.
        let config = crate::config::Config::default();
        let base = Instant::now();
        let mut state = WatchState::new(TEST_WINDOW, base);
        state.apply_poll(Ok(fixture_with_rust(1700, 900)), base);
        state.apply_poll(Ok(fixture_with_rust(1900, 1000)), base + POLL_STEP);
        let now = base + POLL_STEP * 2;
        state.apply_poll(Err(anyhow::anyhow!("connection refused")), now);

        let frame = compose_watch_frame(&state, &config, 80, "12:34:56", now);

        assert!(
            frame.contains("connection refused"),
            "the failed poll must still show its banner; frame:\n{frame}",
        );
        let hits_row =
            row_line(&frame, "Cache hits").expect("the cache_hits row must survive the outage");
        assert!(
            has_any_spark_glyph(hits_row),
            "history is untouched by a failed poll, so the spark must still render; row: {hits_row:?}",
        );
        assert!(
            has_above_baseline_glyph(hits_row),
            "the pre-outage activity must still light the spark above baseline; row: {hits_row:?}",
        );
    }

    #[test]
    fn apply_poll_ok_stores_stats_and_clears_a_prior_error() {
        // A successful poll must (1) record its stats as the new last-good frame
        // and (2) clear any banner left over from an earlier failure — recovery
        // wipes the error the moment the server is reachable again.
        let base = Instant::now();
        let mut state = WatchState::new(TEST_WINDOW, base);
        // Seed a prior failure so we can prove recovery clears it.
        state.apply_poll(Err(anyhow::anyhow!("connection refused")), base);
        assert!(
            state.error().is_some(),
            "a failed poll must leave an error banner to be cleared",
        );

        state.apply_poll(Ok(fixture_stats()), base + Duration::from_secs(1));
        assert!(
            state.last_good().is_some(),
            "a successful poll must store its stats as last_good",
        );
        assert_eq!(
            state.error(),
            None,
            "a successful poll must clear a prior error (recovery)",
        );
        // The retained stats are the ones we just supplied.
        assert_eq!(
            state.last_good().expect("last_good present").version,
            "0.15.0",
            "last_good must be the stats from the successful poll",
        );
    }

    #[test]
    fn apply_poll_err_sets_error_and_retains_previous_good_stats() {
        // After at least one good poll, a failure is non-fatal: it raises the
        // banner but must NOT discard the last good numbers — the table keeps
        // showing them through the outage (design §6).
        let base = Instant::now();
        let mut state = WatchState::new(TEST_WINDOW, base);
        state.apply_poll(Ok(fixture_stats()), base);

        state.apply_poll(
            Err(anyhow::anyhow!("sccache --show-stats failed")),
            base + Duration::from_secs(1),
        );
        assert_eq!(
            state.error(),
            Some("sccache --show-stats failed"),
            "a failed poll must surface the error's display as the banner",
        );
        assert!(
            state.last_good().is_some(),
            "a failed poll must retain the previous good stats, not blank them",
        );
        assert_eq!(
            state.last_good().expect("last_good retained").version,
            "0.15.0",
            "the retained stats must be the last successful poll's, unchanged",
        );
    }

    #[test]
    fn apply_poll_err_with_no_prior_good_leaves_last_good_none() {
        // A first-poll failure (server down at startup) raises the banner but
        // has no good frame to keep — last_good stays None, and the caller
        // renders a banner with no rows.
        let mut state = WatchState::new(TEST_WINDOW, Instant::now());
        state.apply_poll(Err(anyhow::anyhow!("connection refused")), Instant::now());
        assert_eq!(
            state.error(),
            Some("connection refused"),
            "a first-poll failure must still raise the banner",
        );
        assert!(
            state.last_good().is_none(),
            "a first-poll failure has no good frame to keep: last_good stays None",
        );
    }

    /// A tiny poll interval so the timeout-driven tests fire a tick almost
    /// immediately instead of waiting a realistic second. The tick tests
    /// terminate by having a closure send [`Event::Quit`], so this value only
    /// affects how fast the first tick arrives, not correctness.
    const TEST_POLL_INTERVAL: Duration = Duration::from_millis(5);

    #[test]
    fn event_loop_paints_first_frame_immediately_before_waiting() {
        // The user must see data right away, not after a full poll_interval. On
        // entry the loop polls once, renders, and paints — all before its first
        // recv_timeout. A pre-queued Quit ends the loop right after that first
        // frame so the test stays fast and deterministic.
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Quit).expect("queue quit");
        drop(tx);

        let mut state = WatchState::new(TEST_WINDOW, Instant::now());
        let mut displayed = String::new();
        let mut polls = 0_usize;
        let mut renders = 0_usize;
        let mut paints = 0_usize;
        event_loop(
            &rx,
            TEST_POLL_INTERVAL,
            &mut state,
            &mut displayed,
            || {
                polls += 1;
                Ok(fixture_stats())
            },
            |_state| {
                renders += 1;
                "first".to_string()
            },
            |_frame| {
                paints += 1;
                Ok(())
            },
        )
        .expect("loop");

        assert_eq!(polls, 1, "the immediate first frame must poll exactly once");
        assert_eq!(renders, 1, "the immediate first frame must render once");
        assert_eq!(paints, 1, "the immediate first frame must paint once");
        assert_eq!(displayed, "first", "displayed must hold the first frame");
    }

    #[test]
    fn event_loop_timeout_tick_repaints_a_changed_frame() {
        // A poll_interval timeout is a tick: poll, render, and — because the new
        // frame differs from what's displayed — repaint. The render closure
        // sends Quit so the loop ends right after this one tick-driven frame.
        // `polls` is a shared Cell so both the poll and render closures can read
        // it without one's mutable borrow conflicting with the other's read.
        let (tx, rx) = mpsc::channel();
        let mut state = WatchState::new(TEST_WINDOW, Instant::now());
        let mut displayed = String::new();
        let polls = Cell::new(0_usize);
        let mut paints = 0_usize;
        event_loop(
            &rx,
            TEST_POLL_INTERVAL,
            &mut state,
            &mut displayed,
            || {
                polls.set(polls.get() + 1);
                Ok(fixture_stats())
            },
            |_state| {
                // Distinct frame per call so the immediate frame and the tick
                // frame differ, and end the loop after the tick frame. `tx`
                // outlives the call, so the channel stays open for the tick.
                let n = polls.get();
                let frame = format!("frame {n}");
                if n >= 2 {
                    let _ = tx.send(Event::Quit);
                }
                frame
            },
            |_frame| {
                paints += 1;
                Ok(())
            },
        )
        .expect("loop");

        // One immediate frame + one tick frame = two polls, two paints (both
        // frames differ from the prior displayed bytes).
        assert_eq!(polls.get(), 2, "immediate frame + one tick = two polls");
        assert_eq!(paints, 2, "both the immediate and changed tick frame paint");
        assert_eq!(displayed, "frame 2", "displayed holds the latest frame");
    }

    #[test]
    fn event_loop_timeout_tick_with_identical_frame_polls_but_does_not_repaint() {
        // An idle server: the tick still polls, but the rendered frame is
        // byte-identical to what's displayed, so suppression skips the paint.
        // The first (immediate) frame paints; the identical tick frame does not.
        let (tx, rx) = mpsc::channel();
        let mut state = WatchState::new(TEST_WINDOW, Instant::now());
        let mut displayed = String::new();
        let polls = Cell::new(0_usize);
        let mut paints = 0_usize;
        event_loop(
            &rx,
            TEST_POLL_INTERVAL,
            &mut state,
            &mut displayed,
            || {
                polls.set(polls.get() + 1);
                Ok(fixture_stats())
            },
            |_state| {
                // Always the same bytes, so the tick frame matches the immediate
                // frame exactly. End after the tick.
                if polls.get() >= 2 {
                    let _ = tx.send(Event::Quit);
                }
                "steady".to_string()
            },
            |_frame| {
                paints += 1;
                Ok(())
            },
        )
        .expect("loop");

        assert_eq!(polls.get(), 2, "the idle tick still polls");
        assert_eq!(
            paints, 1,
            "only the immediate frame paints; the identical tick frame is suppressed",
        );
    }

    #[test]
    fn event_loop_quit_after_first_frame_returns_cleanly() {
        // After the immediate first frame, a Quit must end the loop with no
        // further poll/render/paint.
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Quit).expect("queue quit");
        drop(tx);

        let mut state = WatchState::new(TEST_WINDOW, Instant::now());
        let mut displayed = String::new();
        let mut polls = 0_usize;
        let mut paints = 0_usize;
        let result = event_loop(
            &rx,
            TEST_POLL_INTERVAL,
            &mut state,
            &mut displayed,
            || {
                polls += 1;
                Ok(fixture_stats())
            },
            |_state| "frame".to_string(),
            |_frame| {
                paints += 1;
                Ok(())
            },
        );

        assert!(result.is_ok(), "Quit must return Ok cleanly");
        assert_eq!(polls, 1, "only the immediate frame polls; Quit adds none");
        assert_eq!(paints, 1, "only the immediate frame paints; Quit adds none");
    }

    #[test]
    fn event_loop_disconnected_channel_returns_cleanly() {
        // Every sender dropped (e.g. the reader thread exited) is a clean
        // shutdown: after the immediate frame, recv_timeout returns Disconnected
        // and the loop returns Ok rather than erroring or spinning.
        let (tx, rx) = mpsc::channel::<Event>();
        drop(tx); // no events will ever arrive; channel is disconnected

        let mut state = WatchState::new(TEST_WINDOW, Instant::now());
        let mut displayed = String::new();
        let mut polls = 0_usize;
        let result = event_loop(
            &rx,
            TEST_POLL_INTERVAL,
            &mut state,
            &mut displayed,
            || {
                polls += 1;
                Ok(fixture_stats())
            },
            |_state| "frame".to_string(),
            |_frame| Ok(()),
        );

        assert!(result.is_ok(), "a disconnected channel is a clean shutdown");
        assert_eq!(polls, 1, "only the immediate frame polls before disconnect");
    }

    #[test]
    fn event_loop_absorbs_a_failed_poll_and_keeps_running() {
        // A failed poll is non-fatal: the loop must NOT return Err, and render
        // must still be called with the error visible in state (so the banner
        // shows). We fail the very first poll, then end on the next tick.
        let (tx, rx) = mpsc::channel();
        let mut state = WatchState::new(TEST_WINDOW, Instant::now());
        let mut displayed = String::new();
        let polls = Cell::new(0_usize);
        let saw_error_in_render = Cell::new(false);
        let result = event_loop(
            &rx,
            TEST_POLL_INTERVAL,
            &mut state,
            &mut displayed,
            || {
                polls.set(polls.get() + 1);
                // First poll fails; the loop must keep going regardless.
                if polls.get() == 1 {
                    Err(anyhow::anyhow!("connection refused"))
                } else {
                    Ok(fixture_stats())
                }
            },
            |state| {
                // The first (immediate) render runs after the failed poll, so
                // the error must be visible in state here.
                if state.error().is_some() {
                    saw_error_in_render.set(true);
                }
                let n = polls.get();
                if n >= 2 {
                    let _ = tx.send(Event::Quit);
                }
                format!("frame {n}")
            },
            |_frame| Ok(()),
        );

        assert!(
            result.is_ok(),
            "a failed poll must be absorbed, not propagated as Err",
        );
        assert!(
            saw_error_in_render.get(),
            "render must be called with the failed-poll error visible in state",
        );
    }

    #[test]
    fn event_loop_resize_forces_a_repaint_without_polling_even_when_bytes_match() {
        // A resize must re-render from the *current* state at the new dimensions
        // and repaint UNCONDITIONALLY — the on-screen layout is stale after a
        // resize, so suppression must NOT skip the redraw even when the bytes
        // are byte-identical to what's displayed. And a resize must NOT poll:
        // the numbers haven't changed, only the geometry has.
        let (tx, rx) = mpsc::channel();
        // One resize, then quit so the loop ends right after handling it.
        tx.send(Event::Resize).expect("queue resize");
        tx.send(Event::Quit).expect("queue quit");
        drop(tx);

        let mut state = WatchState::new(TEST_WINDOW, Instant::now());
        let mut displayed = String::new();
        let mut polls = 0_usize;
        let mut renders = 0_usize;
        let mut paints = 0_usize;
        event_loop(
            &rx,
            TEST_POLL_INTERVAL,
            &mut state,
            &mut displayed,
            || {
                polls += 1;
                Ok(fixture_stats())
            },
            |_state| {
                // Always identical bytes, so the resize's re-render matches what
                // the immediate frame already painted — the perfect adversary
                // for suppression. Only a forced repaint will paint again.
                renders += 1;
                "steady".to_string()
            },
            |_frame| {
                paints += 1;
                Ok(())
            },
        )
        .expect("loop");

        assert_eq!(
            polls, 1,
            "a resize must not poll — only the immediate frame polls",
        );
        assert_eq!(
            renders, 2,
            "a resize must re-render from current state (immediate + resize)",
        );
        assert_eq!(
            paints, 2,
            "a resize must paint unconditionally even when bytes are identical",
        );
        assert_eq!(displayed, "steady", "displayed holds the re-rendered frame");
    }

    #[test]
    fn decide_mode_watches_only_on_a_live_tty_without_force() {
        // A live terminal and no `--one-shot`: the default watch experience.
        assert_eq!(
            decide_mode(false, true),
            Mode::Watch,
            "a live TTY with no --one-shot must enter the watch loop",
        );

        // `--one-shot` always wins, even on a live terminal.
        assert_eq!(
            decide_mode(true, true),
            Mode::OneShot,
            "--one-shot must force a single render even on a TTY",
        );

        // A piped / captured stdout can't host the watch UI, so it falls back
        // to a single render regardless of the flag.
        assert_eq!(
            decide_mode(false, false),
            Mode::OneShot,
            "a non-TTY stdout must fall back to one-shot",
        );
        assert_eq!(
            decide_mode(true, false),
            Mode::OneShot,
            "--one-shot on a non-TTY is still one-shot",
        );
    }

    #[test]
    fn is_quit_event_only_quits_on_a_press_of_q_esc_or_ctrl_c() {
        // The three conventional quit keys, all as presses.
        assert!(
            is_quit_event(KeyCode::Char('q'), KeyModifiers::NONE, KeyEventKind::Press),
            "pressing q must quit",
        );
        assert!(
            is_quit_event(KeyCode::Esc, KeyModifiers::NONE, KeyEventKind::Press),
            "pressing Esc must quit",
        );
        assert!(
            is_quit_event(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            ),
            "Ctrl-C must quit",
        );

        // A key *release* must never quit, even for a quit key — kitty and
        // Windows report releases and the loop should ignore them.
        assert!(
            !is_quit_event(
                KeyCode::Char('q'),
                KeyModifiers::NONE,
                KeyEventKind::Release
            ),
            "releasing q must not quit",
        );

        // A plain 'c' without Control is just a character, not Ctrl-C.
        assert!(
            !is_quit_event(KeyCode::Char('c'), KeyModifiers::NONE, KeyEventKind::Press),
            "plain c (no Control) must not quit",
        );

        // Unrelated keys leave the loop running.
        assert!(
            !is_quit_event(KeyCode::Char('x'), KeyModifiers::NONE, KeyEventKind::Press),
            "an unrelated character must not quit",
        );
        assert!(
            !is_quit_event(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Press),
            "Enter must not quit",
        );
    }

    #[test]
    fn reader_thread_stops_after_a_quit_key_regardless_of_send_outcome() {
        // A quit key whose Quit forward *succeeded* still ends the reader: its
        // job is done the moment it has signalled quit. Continuing to read would
        // leave the thread alive nondeterministically, consuming input events
        // until some later send happens to fail.
        assert_eq!(
            reader_action_after_key(true, true),
            ReaderAction::Stop,
            "a successful quit send must still stop the reader",
        );

        // A quit key whose forward *failed* (the loop already dropped the
        // receiver) also ends the reader — there is nothing left to deliver.
        assert_eq!(
            reader_action_after_key(true, false),
            ReaderAction::Stop,
            "a failed quit send must stop the reader",
        );

        // A non-quit key leaves the reader running; nothing was forwarded, so
        // the send outcome is irrelevant.
        assert_eq!(
            reader_action_after_key(false, true),
            ReaderAction::Continue,
            "a non-quit key must keep the reader running",
        );
        assert_eq!(
            reader_action_after_key(false, false),
            ReaderAction::Continue,
            "a non-quit key keeps the reader running regardless of send outcome",
        );
    }

    #[test]
    fn should_force_colors_only_when_captured_by_a_wrapper_without_no_color() {
        // The one case that forces colors: a wrapper has captured stdout (not a
        // TTY but COLUMNS is exported) and the user hasn't set NO_COLOR.
        assert!(
            should_force_colors(false, true, false),
            "a watch wrapper (no TTY, COLUMNS set, no NO_COLOR) must force colors",
        );

        // A live TTY needs no override — the colored crate already emits color.
        assert!(
            !should_force_colors(true, true, false),
            "a TTY must not force colors",
        );

        // No COLUMNS means a plain pipe, not a wrapper — leave colors stripped.
        assert!(
            !should_force_colors(false, false, false),
            "a plain pipe (no COLUMNS) must not force colors",
        );

        // NO_COLOR is an explicit opt-out that wins even inside a wrapper.
        assert!(
            !should_force_colors(false, true, true),
            "NO_COLOR must suppress colors even when captured by a wrapper",
        );
    }

    #[test]
    fn effective_width_under_a_wrapper_uses_columns_minus_the_margin() {
        // A watch-like wrapper (stdout not a TTY) that exported COLUMNS=120: trust
        // it, minus the one-cell safety margin → 119. `terminal_size` can't see
        // through the pipe, so the leaked tty_width here would be wrong; COLUMNS
        // wins.
        assert_eq!(
            effective_width(Some(200), Some(120), false),
            119,
            "a wrapper's COLUMNS (minus the 1-cell margin) must drive the width, \
             not the unreachable tty_width",
        );
    }

    #[test]
    fn effective_width_on_a_tty_ignores_leaked_columns() {
        // A direct TTY: `terminal_size` is authoritative, so a COLUMNS that leaked
        // from the parent shell must be ignored and the queried width used (minus
        // the margin) → 100-1 = 99.
        assert_eq!(
            effective_width(Some(100), Some(120), true),
            99,
            "on a real TTY the queried width wins and a leaked COLUMNS is ignored",
        );
    }

    #[test]
    fn effective_width_with_no_signal_falls_back_to_eighty_minus_margin() {
        // No TTY width reported and no COLUMNS (a plain pipe, or a TTY whose size
        // query failed): fall back to FALLBACK_TERMINAL_WIDTH, still minus the
        // margin → 79. Both the TTY-with-no-size branch and the plain-pipe branch
        // land here.
        assert_eq!(
            effective_width(None, None, false),
            FALLBACK_TERMINAL_WIDTH - 1,
            "no width signal must fall back to the shared default minus the margin",
        );
        assert_eq!(
            effective_width(None, None, true),
            FALLBACK_TERMINAL_WIDTH - 1,
            "a TTY whose size query failed must also fall back to the default minus margin",
        );
    }

    #[test]
    fn effective_width_saturates_to_at_least_one_on_a_tiny_terminal() {
        // A 1-column terminal (or wrapper) must never collapse the frame to zero
        // width: the margin subtraction saturates and the result floors at 1.
        assert_eq!(
            effective_width(Some(1), None, true),
            1,
            "a 1-column TTY must floor at width 1, not 0",
        );
        assert_eq!(
            effective_width(None, Some(1), false),
            1,
            "a 1-column wrapper must floor at width 1, not 0",
        );
        assert_eq!(
            effective_width(Some(0), None, true),
            1,
            "a 0-column query must saturate to 1, never underflow",
        );
    }

    #[test]
    fn enter_terminal_sequence_undoes_raw_mode_when_screen_entry_fails() {
        // The leak guard: if raw mode is enabled but the alternate-screen step
        // then fails, the sequence must disable raw mode (best-effort) before
        // propagating the error. Otherwise `enter()` returns Err with no
        // TerminalGuard constructed, no Drop ever runs, and the user's shell is
        // stranded in raw mode (no echo, no line buffering) until `reset`.
        let enabled = Cell::new(false);
        let disabled = Cell::new(false);

        let result = enter_terminal_sequence(
            || {
                enabled.set(true);
                Ok(())
            },
            || Err(anyhow::anyhow!("EnterAlternateScreen failed")),
            || disabled.set(true),
        );

        assert!(enabled.get(), "raw mode must be enabled first");
        assert!(
            result.is_err(),
            "a failed screen-entry step must propagate its error",
        );
        assert_eq!(
            result.unwrap_err().to_string(),
            "EnterAlternateScreen failed",
            "the original screen-entry error must propagate unchanged",
        );
        assert!(
            disabled.get(),
            "raw mode must be disabled when screen entry fails, or the shell is \
             left raw with no guard alive to restore it",
        );
    }

    #[test]
    fn enter_terminal_sequence_leaves_raw_mode_on_when_screen_entry_succeeds() {
        // The success path must NOT disable raw mode: once the alternate screen
        // is entered, the live TerminalGuard's Drop owns teardown. Disabling here
        // would tear down raw mode while the watch loop is still running.
        let disabled = Cell::new(false);

        let result = enter_terminal_sequence(|| Ok(()), || Ok(()), || disabled.set(true));

        assert!(result.is_ok(), "a fully successful entry must return Ok");
        assert!(
            !disabled.get(),
            "raw mode must stay enabled on success — Drop, not enter, tears it down",
        );
    }
}

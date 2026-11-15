//! The handle of the progress display that [`Console`](crate::Console) draws.
//!
//! A replay can take many seconds, and a person who sees no output cannot tell
//! a slow replay from a stuck one. The display draws on standard error, so the
//! verdict on standard output stays clean. It has two shapes. A spinner shows
//! the phase of the replay. A bar shows the commit that a rebase replays now.
//!
//! The template of each shape holds no text from a caller. The tool name and
//! the action go through the prefix and the message of the bar, so a brace in
//! a branch name stays text and never reads as a template key.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressState, ProgressStyle};

use crate::progress::{Phase, ReplayObserver, ReplayProgress};

/// How long the display thread waits between two ticks, so the spinner and the
/// clock move while no report arrives.
const TICK: Duration = Duration::from_millis(100);

/// The key of the template that writes the elapsed time.
const CLOCK_KEY: &str = "clock";

/// The template of the spinner, which shows while no commit position is known.
const PHASE_TEMPLATE: &str = "{prefix}: {msg}  {spinner}  {clock}";

/// The words after the position on the line of the bar.
const COMMITS_SUFFIX: &str = " commits";

/// The columns that the clock needs: `H:MM:SS` is the longest text.
const CLOCK_COLUMNS: u16 = 7;

/// The columns of the three gaps of the line of the bar, each two wide.
const GAP_COLUMNS: u16 = 6;

/// The columns of the `": "` after the prefix.
const PREFIX_SEPARATOR_COLUMNS: u16 = 2;

/// The message of the spinner while git creates the scratch worktree.
const CREATE_SCRATCH_MESSAGE: &str = "creating the scratch worktree";

/// The message of the spinner while git counts the conflict hunks.
const COUNT_HUNKS_MESSAGE: &str = "counting the conflict hunks";

/// The template of the bar, with a bar of `bar_width` columns.
fn commit_template(bar_width: u16) -> String {
    format!("{{prefix}}: {{msg}}  {{bar:{bar_width}}}  {{pos}}/{{len}}{COMMITS_SUFFIX}  {{clock}}")
}

/// Build a style from `template` that knows the clock key.
///
/// Returns `None` when the template does not parse. The caller then keeps the
/// style that it has, because a display that fails costs the display and never
/// the verdict.
fn style(template: &str) -> Option<ProgressStyle> {
    ProgressStyle::with_template(template).ok().map(|style| {
        style.with_key(
            CLOCK_KEY,
            |state: &ProgressState, out: &mut dyn std::fmt::Write| {
                let _ = write!(out, "{}", clock(state.elapsed()));
            },
        )
    })
}

/// The handle that a tool gives to a replay, so the replay can draw its
/// progress on standard error.
///
/// [`Console::progress`](crate::Console::progress) gives the handle. The handle
/// is a [`ReplayObserver`], and a clone of it reports to the same display. A
/// handle of a hidden display does nothing on a report. A display is hidden
/// when `-q` is on, when standard error is not a terminal, or when the display
/// failed to start.
#[derive(Clone)]
pub struct Progress {
    /// The live display, or `None` for a hidden one.
    live: Option<Arc<Live>>,
}

/// The state of one live display.
struct Live {
    /// The bar that `indicatif` draws. The thread of the display and every
    /// report share it.
    bar: ProgressBar,
    /// The sentence of the replay. It is the message of the bar.
    action: String,
    /// The width of the terminal, read once when the display starts.
    width: u16,
    /// The columns of the line of the bar that are not the bar and not the
    /// position. They hold the prefix, the action, the gaps, and the clock.
    fixed_columns: u16,
    /// The `total` that the style of the bar was built for, or `None` while the
    /// display still holds the style of the spinner.
    built_for: Mutex<Option<usize>>,
}

/// The parts of a display that [`Console`](crate::Console) keeps, so it can
/// end the display before it writes a line of its own.
pub(super) struct Active {
    /// The bar to clear.
    bar: ProgressBar,
    /// The sender that keeps the thread of the display alive. The thread ends
    /// when the sender drops.
    sender: Sender<()>,
    /// The thread that waits for the delay, and then ticks the display.
    thread: JoinHandle<()>,
}

/// The loop of the display thread.
///
/// The thread waits for `delay`. A disconnect during the wait means the run
/// ended before the display had to show, so the thread returns and the bar
/// never draws. When the delay passes, the thread points the bar at standard
/// error. The thread then ticks the bar each [`TICK`], so the spinner and the
/// clock move while no report arrives. A disconnect ends the loop.
///
/// A delay of zero skips the wait, because the caller points the bar at
/// standard error itself, and a report right after the start then draws at
/// once, with no race against this thread.
fn tick_loop(bar: &ProgressBar, receiver: &Receiver<()>, delay: Duration) {
    if !delay.is_zero() {
        match receiver.recv_timeout(delay) {
            Err(RecvTimeoutError::Timeout) => {
                bar.set_draw_target(ProgressDrawTarget::stderr());
                bar.tick();
            }
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
        }
    }
    while let Err(RecvTimeoutError::Timeout) = receiver.recv_timeout(TICK) {
        bar.tick();
    }
}

impl Progress {
    /// A handle that draws nothing.
    pub(super) fn hidden() -> Self {
        Progress { live: None }
    }

    /// Start a display for `tool` and `action`, which shows after `delay`.
    ///
    /// Returns the handle for the replay and the part that the console keeps.
    /// Returns `None` when the thread of the display does not start. The bar
    /// then stays on the hidden target, and its drop draws nothing, so a
    /// failure of the display costs the display and never the verdict.
    pub(super) fn start(tool: &str, action: &str, delay: Duration) -> Option<(Self, Active)> {
        let width = termbar::TerminalWidth::get_or_default();
        let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::hidden());
        if let Some(phase_style) = style(PHASE_TEMPLATE) {
            bar.set_style(phase_style);
        }
        bar.set_prefix(tool.to_string());
        bar.set_message(action.to_string());

        let (sender, receiver) = mpsc::channel();
        let thread_bar = bar.clone();
        let thread = std::thread::Builder::new()
            .name("gitscratch-progress".to_string())
            .spawn(move || tick_loop(&thread_bar, &receiver, delay))
            .ok()?;
        if delay.is_zero() {
            bar.set_draw_target(ProgressDrawTarget::stderr());
            bar.tick();
        }

        let fixed_columns = termbar::str_display_width_as_u16(tool)
            .saturating_add(PREFIX_SEPARATOR_COLUMNS)
            .saturating_add(termbar::str_display_width_as_u16(action))
            .saturating_add(GAP_COLUMNS)
            .saturating_add(CLOCK_COLUMNS);
        let live = Live {
            bar: bar.clone(),
            action: action.to_string(),
            width,
            fixed_columns,
            built_for: Mutex::new(None),
        };
        Some((
            Progress {
                live: Some(Arc::new(live)),
            },
            Active {
                bar,
                sender,
                thread,
            },
        ))
    }
}

impl Active {
    /// End the display and take its lines off the screen.
    ///
    /// The thread ends first, so no tick draws after the clear. The draw
    /// target turns hidden last, so a report that arrives later draws nothing.
    pub(super) fn stop(self) {
        let Active {
            bar,
            sender,
            thread,
        } = self;
        drop(sender);
        // A panic of the thread costs the display and nothing else.
        let _ = thread.join();
        bar.finish_and_clear();
        bar.set_draw_target(ProgressDrawTarget::hidden());
    }
}

impl Live {
    /// Make the bar style for a replay of `total` commits and keep `total`.
    ///
    /// The bar is as wide as the terminal leaves room for, after the text of
    /// the line. The text holds `total/total commits`, the widest that the
    /// position can be. A template that does not parse leaves the old style.
    ///
    /// A new style draws at once. `set_style` of `indicatif` draws nothing, so
    /// without the tick the line keeps the spinner until the next tick of the
    /// thread, and a verdict that comes first clears a bar that never showed.
    fn build_commit_style(&self, total: usize) {
        let mut built = self
            .built_for
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *built == Some(total) {
            return;
        }
        let position = format!("{total}/{total}{COMMITS_SUFFIX}");
        let overhead = self
            .fixed_columns
            .saturating_add(termbar::str_display_width_as_u16(&position));
        let bar_width = termbar::calculate_bar_width(self.width, overhead);
        if let Some(commit_style) = style(&commit_template(bar_width)) {
            self.bar
                .set_style(commit_style.progress_chars(termbar::PROGRESS_CHARS));
            *built = Some(total);
            self.bar.tick();
        }
    }
}

impl ReplayObserver for Progress {
    /// Draw one report. A hidden display ignores it.
    fn report(&self, progress: ReplayProgress) {
        let Some(live) = &self.live else {
            return;
        };
        match progress {
            ReplayProgress::Phase(phase) => live.bar.set_message(match phase {
                Phase::CreateScratch => CREATE_SCRATCH_MESSAGE.to_string(),
                Phase::Replay => live.action.clone(),
                Phase::CountHunks => COUNT_HUNKS_MESSAGE.to_string(),
            }),
            ReplayProgress::Commit { position, total } => {
                // Each call below draws a frame. The values come first and
                // the bar style last, so no frame shows a count that no
                // report gave: under the spinner style, the length and the
                // position are not on the line. The style first drew
                // `0/0 commits` and then `0/10 commits` before `3/10`.
                live.bar.set_message(live.action.clone());
                live.bar.set_length(total as u64);
                live.bar.set_position(position as u64);
                live.build_commit_style(total);
            }
        }
    }
}

/// The number of seconds in one minute.
const SECONDS_PER_MINUTE: u64 = 60;

/// The number of seconds in one hour.
const SECONDS_PER_HOUR: u64 = 3600;

/// Format the elapsed time of a display as `M:SS`, or as `H:MM:SS` from one
/// hour on.
///
/// The part of a second is dropped, so the clock counts whole seconds. The
/// format of `indicatif` for elapsed time reads `5s`, and a clock that a
/// person reads at a glance needs the colon form.
fn clock(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let hours = seconds / SECONDS_PER_HOUR;
    let minutes = seconds % SECONDS_PER_HOUR / SECONDS_PER_MINUTE;
    let rest = seconds % SECONDS_PER_MINUTE;
    if hours == 0 {
        format!("{minutes}:{rest:02}")
    } else {
        format!("{hours}:{minutes:02}:{rest:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table of elapsed times and the text of the clock for each one.
    #[test]
    fn the_clock_reads_minutes_and_seconds_and_adds_hours_from_one_hour() {
        for (seconds, text) in [
            (0, "0:00"),
            (5, "0:05"),
            (65, "1:05"),
            (3600, "1:00:00"),
            (3725, "1:02:05"),
        ] {
            assert_eq!(clock(Duration::from_secs(seconds)), text, "{seconds} s");
        }
    }

    /// The clock drops the part of a second.
    #[test]
    fn the_clock_drops_the_part_of_a_second() {
        assert_eq!(clock(Duration::from_millis(5999)), "0:05");
    }
}

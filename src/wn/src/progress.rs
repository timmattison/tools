//! The line that stands while a run of `claude` works.
//!
//! A plan of a whole backlog takes minutes, and one measured run of it took 9
//! minutes and 36 seconds against a deadline of 10 minutes. For all of that
//! time the line carried one constant, so a run that worked and a run that died
//! eight minutes earlier painted the same words. The reader could not tell them
//! apart, and the reader who cannot tell them apart kills a run that works or
//! waits on a run that is gone.
//!
//! So the line carries two facts that move.
//!
//! **How long the run waited, and how long it may.** The clock comes off the
//! bar itself, which knows when it started, so nothing has to feed it and it
//! moves whatever the run is doing. A reader who sees `9m30s of 10m0s` knows
//! the run is about to be killed, and a reader who sees the same reading twice
//! knows the tool itself is gone.
//!
//! **What the run does right now.** [`crate::stream`] hands each reach on as
//! the run makes it, and [`Doing`] is what puts it on the line. A steady tick
//! moves the braille frame while one API call is held open, so the animation is
//! no evidence that the run works. The name of the tool is such evidence.
//!
//! # The line draws on standard error, or on nothing at all
//!
//! The plan goes to standard output, and a reader who pipes that output must
//! get the document alone. indicatif draws nothing when standard error is not a
//! terminal either, so a redirected run collects no frames.

use std::time::Duration;

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressState, ProgressStyle};

/// The words the line opens with.
const WORKING: &str = "plan-parallel-work: reading the backlog…";

/// How often the line is painted again.
///
/// The clock reads whole seconds, so this is ten times as often as it needs to
/// be for the clock alone. The braille frame is what wants it: a frame that
/// moved one time a second would read as a run that is stuck.
const TICK: Duration = Duration::from_millis(100);

/// The template of the line.
///
/// [`CLOCK`] is a key of this crate, and it is what makes the line move on its
/// own. The words about the tool go last and they go through `wide_msg`, which
/// cuts them to the columns that are left. A line that ran past the window
/// would wrap, and a wrapped line is painted again under itself rather than
/// over itself.
const TEMPLATE: &str = "{spinner:.cyan} {prefix} {clock}{wide_msg}";

/// The key of [`TEMPLATE`] that the clock fills.
const CLOCK: &str = "clock";

/// The frames of the spinner.
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The mark between the clock and what the run does now.
const BETWEEN: &str = " · ";

/// The word between how long the run waited and how long it may.
const OF: &str = " of ";

/// The seconds of a minute.
const A_MINUTE: u64 = 60;

/// The seconds of an hour.
const AN_HOUR: u64 = 3_600;

/// The line one run of `claude` stands behind.
pub struct Progress {
    /// The bar that paints it.
    bar: ProgressBar,
}

impl Progress {
    /// Start the line, for a run that may take `waited`.
    ///
    /// It draws on standard error. The document goes to standard output, and a
    /// reader who pipes that output must get the document alone.
    #[must_use]
    pub fn start(waited: Duration) -> Self {
        let progress = Self::drawing_on(ProgressDrawTarget::stderr(), waited);
        progress.bar.enable_steady_tick(TICK);
        progress
    }

    /// The same line, painted on `target`.
    ///
    /// One constructor under both entrances, so a test of the painted line
    /// reads the line the reader reads and never a second one built beside it.
    fn drawing_on(target: ProgressDrawTarget, waited: Duration) -> Self {
        let bar = ProgressBar::with_draw_target(None, target);
        if let Ok(style) = ProgressStyle::with_template(TEMPLATE) {
            bar.set_style(style.tick_strings(&FRAMES).with_key(
                CLOCK,
                move |state: &ProgressState, line: &mut dyn std::fmt::Write| {
                    // The bar knows when it started, so the clock needs no
                    // second reading of it and no thread to keep it moving.
                    let _ = write!(line, "{}", clock(state.elapsed(), waited));
                },
            ));
        }
        bar.set_prefix(WORKING);
        Self { bar }
    }

    /// The handle that says what the run is doing.
    #[must_use]
    pub fn doing(&self) -> Doing {
        Doing {
            bar: self.bar.clone(),
        }
    }

    /// Take the line off the terminal.
    ///
    /// What stands there after it is the report of what the run cost, and a
    /// line left behind would have the report painted over it.
    pub fn stop(&self) {
        self.bar.finish_and_clear();
    }
}

/// The half of [`Progress`] that says what the run is doing.
///
/// A handle of its own, because the thread that reads the stream is the one
/// that knows, and that thread cannot borrow the line it writes on. The bar is
/// itself a handle, so a clone of it writes the same line.
pub struct Doing {
    /// The bar the words go on.
    bar: ProgressBar,
}

impl Doing {
    /// Say that the run is doing `what`.
    pub fn set(&self, what: &str) {
        self.bar.set_message(format!("{BETWEEN}{what}"));
    }
}

/// How long the run waited, and how long it may.
fn clock(elapsed: Duration, waited: Duration) -> String {
    format!("{}{OF}{}", spelled(elapsed), spelled(waited))
}

/// `span`, as the line writes it.
///
/// Whole seconds throughout. The report of a finished run writes tenths,
/// because a fast run and a slow one differ by fractions there. A line that a
/// reader watches for ten minutes wants the shortest reading that still
/// answers, and a tenth of a second on it would be a digit that never rests.
fn spelled(span: Duration) -> String {
    let whole = span.as_secs();
    let hours = whole / AN_HOUR;
    let minutes = (whole % AN_HOUR) / A_MINUTE;
    let seconds = whole % A_MINUTE;
    if hours > 0 {
        format!("{hours}h{minutes}m{seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The deadline of a run that names none.
    const TEN_MINUTES: Duration = Duration::from_secs(600);

    #[test]
    fn a_run_under_a_minute_reads_in_seconds() {
        assert_eq!(spelled(Duration::from_secs(0)), "0s");
        assert_eq!(spelled(Duration::from_secs(59)), "59s");
    }

    #[test]
    fn a_run_of_a_minute_and_up_reads_in_minutes_and_seconds() {
        assert_eq!(spelled(Duration::from_secs(60)), "1m0s");
        assert_eq!(spelled(Duration::from_secs(252)), "4m12s");
        assert_eq!(spelled(TEN_MINUTES), "10m0s");
    }

    #[test]
    fn a_run_of_an_hour_and_up_reads_in_hours_as_well() {
        assert_eq!(spelled(Duration::from_secs(3_600)), "1h0m0s");
        assert_eq!(spelled(Duration::from_secs(7_384)), "2h3m4s");
    }

    #[test]
    fn the_seconds_are_cut_and_never_rounded() {
        // A line must not name a second the run has not reached. A reading of
        // `10m0s of 10m0s` on a run with 400 milliseconds left would say the
        // deadline is here when it is not.
        assert_eq!(spelled(Duration::from_millis(1_900)), "1s");
        assert_eq!(spelled(Duration::from_millis(599_900)), "9m59s");
    }

    #[test]
    fn the_clock_names_the_wait_and_the_deadline_it_is_measured_against() {
        assert_eq!(
            clock(Duration::from_secs(252), TEN_MINUTES),
            "4m12s of 10m0s"
        );
    }

    #[test]
    fn what_the_run_does_stands_behind_a_mark_of_its_own() {
        // The mark is what keeps the words about the tool from reading as part
        // of the clock they follow.
        let progress = Progress::drawing_on(ProgressDrawTarget::hidden(), TEN_MINUTES);
        progress.doing().set("Bash: Check wn CLI flags");
        assert_eq!(progress.bar.message(), " · Bash: Check wn CLI flags");
    }

    #[test]
    fn a_run_that_reached_for_nothing_yet_says_nothing_yet() {
        // The clock and the words the line opens with stand on their own, so a
        // run that has not called a tool paints no empty mark.
        let progress = Progress::drawing_on(ProgressDrawTarget::hidden(), TEN_MINUTES);
        assert_eq!(progress.bar.message(), "");
    }

    #[test]
    fn the_newest_reach_is_the_one_the_line_carries() {
        let progress = Progress::drawing_on(ProgressDrawTarget::hidden(), TEN_MINUTES);
        let doing = progress.doing();
        doing.set("Read: Read the open issues");
        doing.set("Bash: Check wn CLI flags");
        assert_eq!(progress.bar.message(), " · Bash: Check wn CLI flags");
    }

    #[test]
    fn the_template_names_every_key_the_line_is_built_from() {
        // A template that named a key nothing fills paints that part of the
        // line as nothing at all, with no error anywhere. So the three parts
        // that carry the words are held here.
        for key in ["{prefix}", "{clock}", "{wide_msg}"] {
            assert!(TEMPLATE.contains(key), "{TEMPLATE}");
        }
        assert!(ProgressStyle::with_template(TEMPLATE).is_ok(), "{TEMPLATE}");
    }
}

//! The indicator that a hunt shows while it runs.
//!
//! A hunt of 64 destinations takes minutes, and it draws no live table. Without
//! an indicator it prints one line at the start and nothing more until the
//! summary, which reads as a tool that died. The indicator says which round the
//! hunt stands in, which address it traces, how many destinations answered, and
//! how long the hunt took so far.
//!
//! Two looks serve two kinds of standard output, as the two screens of `live.rs`
//! do for a run:
//!
//! - [`Style::Line`] is for a terminal. One line redraws in place, and the stop
//!   of the hunt takes that line back, so the summary prints on a clean line.
//! - [`Style::Log`] is for a pipe or a file, which keeps every line it takes. It
//!   writes one whole line for each destination that the hunt finished, and it
//!   writes no control text at all.
//!
//! The [`Status`] trait is the seam. A test of the hunt hands the loop a status
//! that records the events, so no test of `hunt.rs` needs a terminal, and the
//! hunt itself knows nothing about which look it drives.

use crate::hunt::PARTIAL;
use crate::live::Clock;
use crate::ui;
use crate::REACHED;
use std::io::Write;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

/// What a hunt tells the world about itself while it runs.
///
/// The hunt reports the events and never draws. One implementation draws them,
/// and a test of the hunt records them.
pub(crate) trait Status {
    /// Takes one event of the hunt.
    fn show(&mut self, event: &Event);
}

/// One thing that happened inside a hunt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    /// The hunt started a round on this destination.
    Target(Ipv4Addr),
    /// The trace of the destination that stands took one turn. A turn is one
    /// probe round that arrived, or one poll of the run loop that read none.
    Tick,
    /// The hunt finished the destination that stood. `reached` is true when
    /// that destination answered.
    Scored {
        /// True when the destination answered.
        reached: bool,
    },
    /// The hunt stopped, and the indicator gives the line back.
    Stop,
}

/// How an indicator shows the events of a hunt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Style {
    /// One line that redraws in place, for a standard output that is a
    /// terminal.
    Line,
    /// One whole line for each destination that the hunt finished, for a
    /// standard output that is a pipe or a file.
    Log,
}

/// The style that a standard output takes.
///
/// A terminal takes the line that redraws, because a terminal puts the cursor
/// back where the carriage return sends it. A pipe and a file keep every byte
/// they take, so a line that redraws would leave them holding one long line of
/// every frame the hunt ever painted.
///
/// The read of the terminal stands apart from this decision, so a test names
/// the answer without a terminal to name it with.
pub(crate) const fn style_of(terminal: bool) -> Style {
    if terminal {
        Style::Line
    } else {
        Style::Log
    }
}

/// The glyphs of the spinner, in the order they turn.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The glyph of a cell of the bar that the hunt filled whole.
const BAR_FULL: char = '█';

/// The glyph of a cell of the bar that the hunt has not reached.
const BAR_EMPTY: char = '░';

/// The number of parts that one cell of the bar divides into.
const EIGHTHS: usize = 8;

/// The glyphs of a cell that the hunt filled in part, from one eighth to seven.
const BAR_PARTS: [char; EIGHTHS - 1] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];

/// The number of columns that the bar takes when the terminal leaves it room.
const BAR_WIDTH: usize = 24;

/// The fewest columns that the bar prints in.
///
/// A bar of three cells moves one cell for every third of the hunt, so it says
/// less than the two numbers beside it. A terminal that leaves less room than
/// this gets those numbers and no bar.
const BAR_FLOOR: usize = 4;

/// The control text that puts the cursor back at the left edge of the line.
const CARRIAGE_RETURN: &str = "\r";

/// The indicator of one hunt.
///
/// The indicator counts the rounds itself. The hunt reports what happened, and
/// every number of the line comes off these fields, so no caller of [`Status`]
/// carries a count that this one could disagree with.
pub(crate) struct Indicator<W: Write, C: Clock> {
    /// How this indicator shows the events.
    style: Style,
    /// Where the text goes.
    sink: W,
    /// The clock that times the hunt.
    clock: C,
    /// The moment the hunt started.
    started: Instant,
    /// The number of columns that the line prints in.
    columns: u16,
    /// The number of destinations that the hunt will trace.
    rounds: u64,
    /// The number of destinations that the hunt started, the one that stands
    /// included.
    round: u64,
    /// The destination that the hunt traces now.
    target: Option<Ipv4Addr>,
    /// The number of destinations that answered.
    reached: usize,
    /// The number of destinations that answered nothing.
    partial: usize,
    /// The number of turns that the hunt took, which is the frame of the
    /// spinner.
    frame: usize,
    /// The number of columns that the last line took.
    painted: usize,
}

impl<W: Write, C: Clock> Indicator<W, C> {
    /// Builds the indicator of a hunt of `rounds` destinations.
    ///
    /// The width comes from the caller and the indicator keeps it, as the live
    /// table of a run keeps the size it started with. A window that changes
    /// size while the hunt runs leaves the line at the width it started with.
    pub(crate) fn new(style: Style, rounds: u64, columns: u16, sink: W, clock: C) -> Self {
        let started = clock.now();
        Self {
            style,
            sink,
            clock,
            started,
            columns,
            rounds,
            round: 0,
            target: None,
            reached: 0,
            partial: 0,
            frame: 0,
            painted: 0,
        }
    }
}

impl<W: Write, C: Clock> Status for Indicator<W, C> {
    fn show(&mut self, _event: &Event) {}
}

#[cfg(test)]
mod tests {
    use super::{Event, Indicator, Status, Style};
    use crate::testing::FakeClock;
    use std::net::Ipv4Addr;
    use std::rc::Rc;
    use std::time::Duration;

    /// The number of destinations that the hunt of a test traces.
    const ROUNDS: u64 = 64;

    /// The width of the terminal of a test. The width leaves room for the bar
    /// and every field beside it.
    const COLUMNS: u16 = 100;

    /// The destination that the hunt of a test traces.
    const TARGET: &str = "203.0.113.7";

    /// Reads an address that a test names.
    fn address(text: &str) -> Ipv4Addr {
        text.parse().expect("the test address must parse")
    }

    /// The indicator of a test, over bytes and a clock the test moves.
    fn indicator(style: Style, clock: &Rc<FakeClock>) -> Indicator<Vec<u8>, Rc<FakeClock>> {
        Indicator::new(style, ROUNDS, COLUMNS, Vec::new(), Rc::clone(clock))
    }

    /// The text of the line that the indicator painted last.
    fn painted(indicator: Indicator<Vec<u8>, Rc<FakeClock>>) -> String {
        let text = String::from_utf8(indicator.sink).expect("the indicator writes text");
        text.rsplit('\r')
            .next()
            .unwrap_or_default()
            .trim_end()
            .to_owned()
    }

    /// An indicator that traced `round` destinations of which `reached`
    /// answered, and that stands on one more.
    fn hunting(clock: &Rc<FakeClock>, reached: usize, partial: usize) -> Indicator<Vec<u8>, Rc<FakeClock>> {
        let mut indicator = indicator(Style::Line, clock);
        for _ in 0..reached {
            indicator.show(&Event::Target(address(TARGET)));
            indicator.show(&Event::Scored { reached: true });
        }
        for _ in 0..partial {
            indicator.show(&Event::Target(address(TARGET)));
            indicator.show(&Event::Scored { reached: false });
        }
        indicator.show(&Event::Target(address(TARGET)));
        indicator
    }

    #[test]
    fn the_line_names_the_round_that_the_hunt_stands_in_and_the_rounds_it_will_take() {
        let clock = FakeClock::new();
        assert!(
            painted(hunting(&clock, 1, 1)).contains("3/64"),
            "the line must name the round of the hunt: {:?}",
            painted(hunting(&clock, 1, 1))
        );
    }

    #[test]
    fn the_line_names_the_destination_that_the_hunt_traces() {
        let clock = FakeClock::new();
        assert!(
            painted(hunting(&clock, 0, 0)).contains(TARGET),
            "the line must name the destination: {:?}",
            painted(hunting(&clock, 0, 0))
        );
    }

    #[test]
    fn the_line_counts_the_destinations_that_answered_and_the_ones_that_did_not() {
        let clock = FakeClock::new();
        let line = painted(hunting(&clock, 2, 3));
        assert!(line.contains("2 reached"), "the line must count the reached: {line:?}");
        assert!(line.contains("3 partial"), "the line must count the partial: {line:?}");
    }

    #[test]
    fn the_line_names_the_time_that_the_hunt_took() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Line, &clock);
        indicator.show(&Event::Target(address(TARGET)));
        clock.advance(Duration::from_millis(42_500));
        indicator.show(&Event::Tick);
        let line = painted(indicator);
        assert!(line.contains("42s"), "the line must name the time: {line:?}");
    }
}

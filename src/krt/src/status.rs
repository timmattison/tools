//! The indicator that a hunt shows while it runs.
//!
//! A hunt takes minutes, and it draws no live table. Without an indicator it
//! prints one line at the start and nothing more until the summary, which reads
//! as a tool that died. The indicator says how many rounds the hunt holds of
//! the rounds it wants, how many destinations it started of the ones it may
//! start, which address it traces, and how long the hunt took so far.
//!
//! Two styles serve two kinds of standard output, as the two screens of
//! `live.rs` do for a run:
//!
//! - [`Style::Line`] is for a terminal. One line redraws in place, and the stop
//!   of the hunt takes that line back, so the summary prints on a clean line.
//! - [`Style::Log`] is for a pipe or a file, which keeps every line it takes. It
//!   writes one whole line for each destination that the hunt finished, and it
//!   writes no control text at all.
//!
//! The [`Status`] trait is the seam. A test of the hunt hands the loop a status
//! that records the events, so no test of `hunt.rs` needs a terminal, and the
//! hunt itself knows nothing about which style it drives.

use crate::hunt::{Bounds, PARTIAL};
use crate::live::Clock;
use crate::ui;
use crate::{REACHED, TARGETS};
use std::io::Write;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

/// What a hunt tells the world about itself while it runs.
///
/// The hunt reports the events and never draws. One implementation draws them,
/// and a test of the hunt records them.
pub(crate) trait Status {
    /// Takes one event of the hunt.
    fn show(&mut self, event: Event);
}

/// One thing that happened inside a hunt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    /// The tracer of this destination started. A destination whose tracer
    /// refused shows no event, because the hunt never probed it.
    Target(Ipv4Addr),
    /// The trace of the destination that stands took one turn. A turn is one
    /// probe round that arrived, or one poll of the run loop that read none.
    Tick,
    /// The hunt finished one destination. A hunt holds many of them at once,
    /// so the event names which one.
    Scored {
        /// The destination that finished.
        target: Ipv4Addr,
        /// True when that destination answered.
        reached: bool,
        /// The address of the first hit whose mine drew that destination. A
        /// destination of an independent draw holds none.
        mine: Option<Ipv4Addr>,
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

/// The word that a log line of a mined destination carries, in front of the
/// address of the first hit that started that mine.
pub(crate) const MINE: &str = "mine";

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

/// The place of the destination that the hunt traces among the fields of the
/// line, as [`Indicator::fields`] builds them.
const ADDRESS: usize = 1;

/// The place of the targets of the hunt among those same fields.
const BUDGET: usize = 2;

/// The place of the time that the hunt took among those same fields.
const TIME: usize = 3;

/// The order that the fields of the line go away in, first dropped first.
///
/// A terminal too narrow for every field keeps the fields that say the most to
/// a reader who is watching a hunt: the rounds it holds, and the address it
/// traces. The targets go first, because the summary at the end counts the same
/// thing. The time goes next, for the same reason. The rounds of the hunt are
/// in no order at all, because they never go away.
const DROP_ORDER: [usize; 3] = [BUDGET, TIME, ADDRESS];

/// The fields that stand, as one line.
fn join(fields: &[Option<String>]) -> String {
    fields
        .iter()
        .flatten()
        .map(String::as_str)
        .collect::<Vec<&str>>()
        .join(ui::FIELD_SEPARATOR)
}

/// The eighths of `cells` that `done` of `whole` fills.
///
/// A bound of zero is a bound that the hunt met before it started, and the
/// share of it is the whole bar: a division by it has no answer. A count above
/// its own bound fills the bar and no more, so no share can push the line past
/// the columns it stands in.
fn share(done: u64, whole: u64, cells: usize) -> usize {
    if whole == 0 {
        return cells;
    }
    let width = u128::try_from(cells).unwrap_or(u128::MAX);
    let filled = u128::from(done) * width / u128::from(whole);
    usize::try_from(filled).unwrap_or(usize::MAX).min(cells)
}

/// The text between the destination that a line names and the number of the
/// destinations that stand beside it.
const MORE_STANDING: &str = " +";

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
    /// The two numbers that stop the hunt.
    bounds: Bounds,
    /// The number of destinations whose tracer started, the one that stands
    /// included.
    targets: u64,
    /// The destination that started last.
    target: Option<Ipv4Addr>,
    /// The number of destinations that the hunt holds at this moment.
    ///
    /// A destination that the user cut short gives no answer, so it stays in
    /// this count until the stop of the hunt takes the line back.
    flying: usize,
    /// The number of destinations that answered, which is the number of rounds
    /// that the hunt holds.
    reached: u64,
    /// The number of turns that the hunt took, which is the frame of the
    /// spinner.
    frame: usize,
    /// The number of columns that the last line took.
    painted: usize,
}

impl<W: Write, C: Clock> Indicator<W, C> {
    /// Builds the indicator of a hunt of these bounds.
    ///
    /// The width comes from the caller and the indicator keeps it, as the live
    /// table of a run keeps the size it started with. A window that changes
    /// size while the hunt runs leaves the line at the width it started with.
    pub(crate) fn new(style: Style, bounds: Bounds, columns: u16, sink: W, clock: C) -> Self {
        let started = clock.now();
        Self {
            style,
            sink,
            clock,
            started,
            columns,
            bounds,
            targets: 0,
            target: None,
            flying: 0,
            reached: 0,
            frame: 0,
            painted: 0,
        }
    }
}

impl<W: Write, C: Clock> Indicator<W, C> {
    /// The time that the hunt took so far, to the whole second.
    ///
    /// The line redraws ten times a second, and a duration that carries a
    /// remainder of milliseconds prints as milliseconds. A field that reads
    /// `42137ms` at one frame and `42238ms` at the next says the same thing
    /// twice and asks the reader to read four digits to see it.
    fn elapsed(&self) -> Duration {
        Duration::from_secs(self.clock.now().duration_since(self.started).as_secs())
    }

    /// The glyph of the spinner at the turn that the hunt stands on.
    fn spinner(&self) -> char {
        SPINNER[self.frame % SPINNER.len()]
    }

    /// The bar of the hunt, in `width` columns.
    ///
    /// The bar fills in eighths of a cell, so a hunt moves it on most of its
    /// draws. A bar of whole cells alone would stand still for three draws of a
    /// hunt of 64 targets in a bar of 24 cells.
    fn bar(&self, width: usize) -> String {
        let cells = width * EIGHTHS;
        let filled = self.filled(cells);
        let mut bar: String = std::iter::repeat_n(BAR_FULL, filled / EIGHTHS).collect();
        if !filled.is_multiple_of(EIGHTHS) {
            bar.push(BAR_PARTS[filled % EIGHTHS - 1]);
        }
        while ui::display_width(&bar) < width {
            bar.push(BAR_EMPTY);
        }
        bar
    }

    /// The number of eighths of the bar that the hunt filled, of `cells`.
    ///
    /// The hunt stops on whichever bound it meets first, so the bar reads the
    /// bound it stands closer to. A bar of the rounds alone would stand still
    /// while a hunt that answers nothing spends every target it has, and a bar
    /// of the targets alone would sit at one eighth as a hunt that answers well
    /// finished.
    fn filled(&self, cells: usize) -> usize {
        let by_rounds = share(self.reached, self.bounds.rounds, cells);
        let by_targets = share(self.targets, self.bounds.max_targets, cells);
        by_rounds.max(by_targets)
    }

    /// The line that a terminal shows.
    ///
    /// The fields take the width first, and the bar takes the columns they
    /// leave, up to [`BAR_WIDTH`]. A terminal that leaves the bar less room
    /// than [`BAR_FLOOR`] gets the fields alone: the numbers say what the bar
    /// says, and they say it in every width.
    ///
    /// The cut at the end is the last resort. It catches a terminal too narrow
    /// even for the one field that never goes away.
    fn line(&self) -> String {
        let head = format!("{} ", self.spinner());
        let columns = usize::from(self.columns);
        let tail = self.fields(columns.saturating_sub(ui::display_width(&head)));
        let room = columns.saturating_sub(
            ui::display_width(&head)
                + ui::display_width(&tail)
                + ui::display_width(ui::FIELD_SEPARATOR),
        );
        let width = room.min(BAR_WIDTH);
        let line = if width < BAR_FLOOR {
            format!("{head}{tail}")
        } else {
            format!("{head}{}{}{tail}", self.bar(width), ui::FIELD_SEPARATOR)
        };
        ui::truncate_to_width(&line, columns)
    }

    /// The fields of the line that stand in `width` columns.
    ///
    /// A field goes away whole. A line that cut one in the middle would print
    /// `0 reached   11` and leave a reader reading a count that says nothing.
    /// The fields therefore go away in the order of [`DROP_ORDER`], one at a
    /// time, while the line is too wide, and the round of the hunt never goes
    /// away.
    ///
    /// A hunt that started no destination yet holds none, and that field is
    /// absent rather than empty between two separators.
    fn fields(&self, width: usize) -> String {
        // The order of these four is the order that [`ADDRESS`], [`BUDGET`],
        // and [`TIME`] name, and [`DROP_ORDER`] reads them by that name.
        let mut fields = [
            Some(self.rounds_field()),
            self.address_field(),
            Some(self.targets_field()),
            Some(ui::render_duration(self.elapsed())),
        ];
        for dropped in DROP_ORDER {
            if ui::display_width(&join(&fields)) <= width {
                break;
            }
            fields[dropped] = None;
        }
        join(&fields)
    }

    /// The destination that started last, and the number of the others that
    /// stand beside it.
    ///
    /// A hunt that started no destination yet holds none, and the field is
    /// then absent rather than empty between two separators.
    ///
    /// The hunt holds many destinations at once, and the line has room for one
    /// address. The count is what tells a reader that the others are there: a
    /// line of one address alone reads as a hunt of one destination at a time.
    /// A hunt that holds one destination has none beside it, so that line
    /// carries the address alone.
    fn address_field(&self) -> Option<String> {
        let target = self.target?;
        let beside = self.flying.saturating_sub(1);
        if beside == 0 {
            return Some(target.to_string());
        }
        Some(format!("{target}{MORE_STANDING}{beside}"))
    }

    /// The rounds that the hunt holds, of the rounds it wants.
    ///
    /// The field never goes away. It is the one number that says whether the
    /// hunt is measuring anything at all.
    fn rounds_field(&self) -> String {
        format!("{}/{} {REACHED}", self.reached, self.bounds.rounds)
    }

    /// The destinations that started, of the ones the hunt may start.
    ///
    /// The count holds the destination that stands, so a reader who sees the
    /// last target of the budget knows that this one ends the hunt.
    fn targets_field(&self) -> String {
        format!("{}/{} {TARGETS}", self.targets, self.bounds.max_targets)
    }

    /// Paints the line, and wipes the tail of a longer line in front of it.
    ///
    /// A frame that does not reach the terminal loses that frame and nothing
    /// else. The recording is the purpose of the tool, and the line is one view
    /// of it, so a reader who closes the pipe of the display keeps the hunt.
    fn paint(&mut self) {
        let line = self.line();
        let width = ui::display_width(&line);
        let wipe = " ".repeat(self.painted.saturating_sub(width));
        self.painted = width;
        drop(write!(self.sink, "{CARRIAGE_RETURN}{line}{wipe}"));
        drop(self.sink.flush());
    }

    /// Writes the whole line of a destination that the hunt finished.
    ///
    /// The line carries the same two ratios as the line of a terminal, and it
    /// carries the destination and the answer of that one destination beside
    /// them. A reader of the file counts the answers from the lines, and the
    /// summary at the end counts them again.
    ///
    /// The destination comes off the event and never off the field of the
    /// indicator. A hunt holds many destinations at once, so the one that
    /// finishes is rarely the one that started last.
    fn log(&mut self, target: Ipv4Addr, reached: bool, mine: Option<Ipv4Addr>) {
        let answer = if reached { REACHED } else { PARTIAL };
        let fields: Vec<String> = [
            Some(self.rounds_field()),
            Some(self.targets_field()),
            Some(target.to_string()),
            Some(answer.to_owned()),
            // A mined destination raises no ratio, so a reader who counted the
            // answers of the lines would read more of them than the ratio
            // beside them holds. This field is what says which lines those are.
            mine.map(|first| format!("{MINE} {first}")),
            Some(ui::render_duration(self.elapsed())),
        ]
        .into_iter()
        .flatten()
        .collect();
        drop(writeln!(self.sink, "{}", fields.join(ui::FIELD_SEPARATOR)));
        drop(self.sink.flush());
    }

    /// Takes the line back, so the text that follows starts on a clean line.
    fn wipe(&mut self) {
        if self.painted == 0 {
            return;
        }
        let blanks = " ".repeat(self.painted);
        self.painted = 0;
        drop(write!(
            self.sink,
            "{CARRIAGE_RETURN}{blanks}{CARRIAGE_RETURN}"
        ));
        drop(self.sink.flush());
    }
}

impl<W: Write, C: Clock> Status for Indicator<W, C> {
    fn show(&mut self, event: Event) {
        match event {
            Event::Target(target) => {
                self.targets += 1;
                self.target = Some(target);
                self.flying += 1;
            }
            Event::Tick => self.frame += 1,
            Event::Scored { reached, mine, .. } => {
                self.flying = self.flying.saturating_sub(1);
                // A mined destination costs the hunt no round, so it raises no
                // ratio here. The indicator and the summary count the same
                // destinations, and a line that counted one would read a number
                // that the summary under it denies.
                if reached && mine.is_none() {
                    self.reached += 1;
                }
            }
            Event::Stop => self.flying = 0,
        }
        match (self.style, event) {
            // A terminal puts the cursor back where the carriage return sends
            // it, so every event redraws the one line and the stop takes it
            // back.
            (Style::Line, Event::Stop) => self.wipe(),
            (Style::Line, _) => self.paint(),
            // A pipe and a file keep every byte they take. One line for each
            // destination that finished is the pace of the hunt itself, and a
            // frame of every turn would fill the file with the same line ten
            // times a second.
            (Style::Log, Event::Scored {
                target,
                reached,
                mine,
            }) => self.log(target, reached, mine),
            (Style::Log, _) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Event, Indicator, Status, Style, BAR_EMPTY, BAR_FULL, CARRIAGE_RETURN, MINE};
    use crate::hunt::{Bounds, PARTIAL};
    use crate::testing::FakeClock;
    use crate::ui::FIELD_SEPARATOR;
    use crate::REACHED;
    use std::net::Ipv4Addr;
    use std::rc::Rc;
    use std::time::Duration;

    /// The bounds of the hunt of a test: eight rounds, and 64 destinations to
    /// find them in.
    const BOUNDS: Bounds = Bounds {
        rounds: 8,
        max_targets: 64,
    };

    /// The width of the terminal of a test. The width leaves room for the bar
    /// and every field beside it.
    const COLUMNS: u16 = 100;

    /// The destination that the hunt of a test traces.
    const TARGET: &str = "203.0.113.7";

    /// The width of a terminal that leaves the bar no room.
    ///
    /// Every field but the rounds of the hunt goes away at this width, and the
    /// columns that they take still leave the bar under its floor.
    const A_NARROW_TERMINAL: u16 = 19;

    /// The second destination that the hunt of a test traces.
    const ANOTHER_TARGET: &str = "198.51.100.9";

    /// The number of destinations that the hunt of a log test finishes.
    const LOGGED_ROUNDS: usize = 3;

    /// The number of rounds that the hunt of a test wants, as a count of the
    /// destinations that a test scripts.
    const WANTED_ROUNDS: usize = 8;

    /// The width of a terminal that leaves no room for the targets of the hunt.
    const WITHOUT_THE_TARGETS: u16 = 45;

    /// The width of a terminal that leaves no room for the time either.
    const WITHOUT_THE_TIME: u16 = 32;

    /// Reads an address that a test names.
    fn address(text: &str) -> Ipv4Addr {
        text.parse().expect("the test address must parse")
    }

    /// The indicator of a test, over bytes and a clock the test moves.
    fn indicator(style: Style, clock: &Rc<FakeClock>) -> Indicator<Vec<u8>, Rc<FakeClock>> {
        Indicator::new(style, BOUNDS, COLUMNS, Vec::new(), Rc::clone(clock))
    }

    /// The frames that the indicator wrote, in the order it wrote them.
    ///
    /// A frame starts at the carriage return that puts the cursor back at the
    /// left edge, so the text in front of the first one is empty and the split
    /// drops it.
    fn frames(sink: &[u8]) -> Vec<String> {
        let text = String::from_utf8(sink.to_vec()).expect("the indicator writes text");
        text.split(CARRIAGE_RETURN)
            .skip(1)
            .map(ToOwned::to_owned)
            .collect()
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

    /// An indicator that traced `reached` destinations that answered and
    /// `partial` that did not, and that stands on one more.
    fn hunting(
        clock: &Rc<FakeClock>,
        reached: usize,
        partial: usize,
    ) -> Indicator<Vec<u8>, Rc<FakeClock>> {
        hunting_at(clock, COLUMNS, reached, partial)
    }

    /// The same hunt, on a terminal of the width that a test names.
    fn hunting_at(
        clock: &Rc<FakeClock>,
        columns: u16,
        reached: usize,
        partial: usize,
    ) -> Indicator<Vec<u8>, Rc<FakeClock>> {
        let mut indicator =
            Indicator::new(Style::Line, BOUNDS, columns, Vec::new(), Rc::clone(clock));
        for _ in 0..reached {
            indicator.show(Event::Target(address(TARGET)));
            indicator.show(Event::Scored {
                target: address(TARGET),
                reached: true,
                mine: None,
            });
        }
        for _ in 0..partial {
            indicator.show(Event::Target(address(TARGET)));
            indicator.show(Event::Scored {
                target: address(TARGET),
                reached: false,
                mine: None,
            });
        }
        indicator.show(Event::Target(address(TARGET)));
        indicator
    }

    #[test]
    fn the_line_names_the_rounds_that_the_hunt_holds_and_the_rounds_it_wants() {
        let clock = FakeClock::new();
        let line = painted(hunting(&clock, 1, 1));
        assert!(
            line.contains("1/8 reached"),
            "the line must name the rounds of the hunt: {line:?}"
        );
    }

    /// The line names what the hunt spent looking for those rounds.
    ///
    /// A reader who sees one round of eight after 40 of 64 targets knows that
    /// the hunt will give up long before it holds what it wants.
    #[test]
    fn the_line_names_the_targets_that_the_hunt_traced_and_the_ones_it_may_trace() {
        let clock = FakeClock::new();
        let line = painted(hunting(&clock, 1, 1));
        assert!(
            line.contains("3/64 targets"),
            "the line must name the targets of the hunt: {line:?}"
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

    /// The two ratios count the destinations that answered and every start.
    ///
    /// The count of the destinations that answered nothing is the difference of
    /// the two, and the summary at the end prints it.
    #[test]
    fn the_line_counts_the_destinations_that_answered_and_every_one_it_started() {
        let clock = FakeClock::new();
        let line = painted(hunting(&clock, 2, 3));
        assert!(
            line.contains("2/8 reached"),
            "the line must count the reached: {line:?}"
        );
        assert!(
            line.contains("6/64 targets"),
            "the line must count every destination it started: {line:?}"
        );
    }

    /// The number of cells that the bar of a wide terminal holds.
    const BAR_CELLS: usize = 24;

    #[test]
    fn the_bar_fills_in_proportion_to_the_rounds() {
        let clock = FakeClock::new();
        // The hunt holds two rounds of eight, which is one quarter of the bar.
        // It started three targets of 64, which is a smaller share.
        let line = painted(hunting(&clock, 2, 0));
        assert_eq!(
            line.matches(BAR_FULL).count(),
            BAR_CELLS / 4,
            "the bar of a quarter of the rounds must fill a quarter of its cells: {line:?}"
        );
    }

    #[test]
    fn the_bar_of_the_last_round_holds_no_empty_cell() {
        let clock = FakeClock::new();
        let line = painted(hunting(&clock, WANTED_ROUNDS, 0));
        assert_eq!(
            line.matches(BAR_FULL).count(),
            BAR_CELLS,
            "the bar of the last round must fill every cell: {line:?}"
        );
        assert!(
            !line.contains(BAR_EMPTY),
            "the bar of the last round must hold no empty cell: {line:?}"
        );
    }

    #[test]
    fn the_bar_fills_a_cell_in_eighths() {
        let clock = FakeClock::new();
        // The first target of 64 fills three eighths of the first cell of a bar
        // of 24 cells, and a bar of whole cells alone would show nothing at all.
        let line = painted(hunting(&clock, 0, 0));
        assert!(
            line.contains('▍'),
            "the bar must fill the first cell in part: {line:?}"
        );
        assert_eq!(
            line.matches(BAR_FULL).count(),
            0,
            "the bar of the first round must fill no whole cell: {line:?}"
        );
    }

    /// The bar reads whichever bound the hunt stands closer to.
    ///
    /// A hunt that answers nothing holds no round, and a bar of the rounds
    /// alone would stand still while the hunt spent every target it has.
    #[test]
    fn the_bar_reads_the_targets_when_they_stand_closer_than_the_rounds() {
        let clock = FakeClock::new();
        // The hunt holds no round of eight, and it started 48 targets of 64,
        // which is three quarters of the bar.
        let line = painted(hunting(&clock, 0, 47));
        assert_eq!(
            line.matches(BAR_FULL).count(),
            BAR_CELLS * 3 / 4,
            "the bar must read the targets of a hunt that answered nothing: {line:?}"
        );
    }

    #[test]
    fn the_spinner_turns_on_a_tick() {
        let clock = FakeClock::new();
        let mut first = indicator(Style::Line, &clock);
        first.show(Event::Target(address(TARGET)));
        let mut second = indicator(Style::Line, &clock);
        second.show(Event::Target(address(TARGET)));
        second.show(Event::Tick);
        assert_ne!(
            painted(first).chars().next(),
            painted(second).chars().next(),
            "a tick must turn the spinner"
        );
    }

    #[test]
    fn the_line_never_runs_past_the_columns_of_the_terminal() {
        let clock = FakeClock::new();
        for columns in 0..=COLUMNS {
            let mut indicator =
                Indicator::new(Style::Line, BOUNDS, columns, Vec::new(), Rc::clone(&clock));
            indicator.show(Event::Target(address(TARGET)));
            let line = painted(indicator);
            assert!(
                crate::ui::display_width(&line) <= usize::from(columns),
                "a line of {columns} columns must not run past them: {line:?}"
            );
        }
    }

    #[test]
    fn a_terminal_too_narrow_for_the_bar_shows_the_fields_alone() {
        let clock = FakeClock::new();
        let mut indicator = Indicator::new(
            Style::Line,
            BOUNDS,
            A_NARROW_TERMINAL,
            Vec::new(),
            Rc::clone(&clock),
        );
        indicator.show(Event::Target(address(TARGET)));
        let line = painted(indicator);
        assert!(
            !line.contains(BAR_EMPTY) && !line.contains(BAR_FULL),
            "a narrow terminal must get no bar: {line:?}"
        );
        assert!(
            line.contains("0/8"),
            "a narrow terminal must still get the rounds of the hunt: {line:?}"
        );
    }

    #[test]
    fn the_line_redraws_in_place() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Line, &clock);
        indicator.show(Event::Target(address(TARGET)));
        indicator.show(Event::Tick);
        let text = String::from_utf8(indicator.sink).expect("the indicator writes text");
        assert!(
            !text.contains('\n'),
            "a line that redraws in place holds no newline: {text:?}"
        );
        assert_eq!(
            text.matches(CARRIAGE_RETURN).count(),
            2,
            "each frame must start at the left edge: {text:?}"
        );
    }

    #[test]
    fn a_frame_wipes_the_tail_of_a_wider_frame_in_front_of_it() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Line, &clock);
        indicator.show(Event::Target(address(TARGET)));
        // The time field reads `59s` at one frame and `1m` at the next, so the
        // second frame is one column narrower than the first.
        clock.advance(Duration::from_secs(59));
        indicator.show(Event::Tick);
        clock.advance(Duration::from_secs(1));
        indicator.show(Event::Tick);
        let frames = frames(&indicator.sink);
        assert!(
            crate::ui::display_width(&frames[1]) > crate::ui::display_width(frames[2].trim_end()),
            "the test must make a narrower frame follow a wider one: {frames:?}"
        );
        assert_eq!(
            crate::ui::display_width(&frames[2]),
            crate::ui::display_width(&frames[1]),
            "the narrower frame must wipe the tail of the wider one: {frames:?}"
        );
    }

    #[test]
    fn the_stop_takes_the_line_back() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Line, &clock);
        indicator.show(Event::Target(address(TARGET)));
        indicator.show(Event::Stop);
        let frames = frames(&indicator.sink);
        assert!(
            frames[1].chars().all(|glyph| glyph == ' '),
            "the stop must write blanks over the line: {frames:?}"
        );
        assert_eq!(
            crate::ui::display_width(&frames[1]),
            crate::ui::display_width(&frames[0]),
            "the blanks must cover the whole line: {frames:?}"
        );
        assert!(
            frames[2].is_empty(),
            "the cursor must end at the left edge: {frames:?}"
        );
    }

    #[test]
    fn a_log_writes_one_whole_line_for_each_destination_that_finished() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Log, &clock);
        for _ in 0..LOGGED_ROUNDS {
            indicator.show(Event::Target(address(TARGET)));
            indicator.show(Event::Tick);
            indicator.show(Event::Scored {
                target: address(TARGET),
                reached: true,
                mine: None,
            });
        }
        indicator.show(Event::Stop);
        let text = String::from_utf8(indicator.sink).expect("the indicator writes text");
        assert_eq!(
            text.lines().count(),
            LOGGED_ROUNDS,
            "a log must write one line for each destination: {text:?}"
        );
    }

    /// The place of the answer of one destination among the fields of a log
    /// line.
    const ANSWER: usize = 3;

    #[test]
    fn a_log_line_names_the_bounds_the_destination_and_whether_it_answered() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Log, &clock);
        indicator.show(Event::Target(address(TARGET)));
        indicator.show(Event::Scored {
            target: address(TARGET),
            reached: true,
            mine: None,
        });
        indicator.show(Event::Target(address(ANOTHER_TARGET)));
        indicator.show(Event::Scored {
            target: address(ANOTHER_TARGET),
            reached: false,
            mine: None,
        });
        let text = String::from_utf8(indicator.sink).expect("the indicator writes text");
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[0].contains("1/8 reached")
                && lines[0].contains("1/64 targets")
                && lines[0].contains(TARGET),
            "the line must name both bounds and the destination: {lines:?}"
        );
        assert_eq!(
            fields(lines[0])[ANSWER],
            REACHED,
            "the line of a destination that answered must say so: {lines:?}"
        );
        assert!(
            lines[1].contains("1/8 reached")
                && lines[1].contains("2/64 targets")
                && lines[1].contains(ANOTHER_TARGET),
            "the line must name both bounds and the destination: {lines:?}"
        );
        assert_eq!(
            fields(lines[1])[ANSWER],
            PARTIAL,
            "the line of a destination that answered nothing must say so: {lines:?}"
        );
    }

    /// The fields of one line, as the separator divides them.
    fn fields(line: &str) -> Vec<&str> {
        line.split(FIELD_SEPARATOR).collect()
    }

    #[test]
    fn a_log_writes_nothing_while_a_destination_traces() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Log, &clock);
        indicator.show(Event::Target(address(TARGET)));
        indicator.show(Event::Tick);
        indicator.show(Event::Tick);
        assert!(
            indicator.sink.is_empty(),
            "a log must write nothing until a destination finishes: {:?}",
            indicator.sink
        );
    }

    /// The first hit whose mine drew the destination of a mined test event.
    const A_FIRST_HIT: &str = "93.184.216.34";

    /// A mined destination costs the hunt no round, so it counts none here.
    ///
    /// The indicator and the summary count the same destinations. A mined
    /// destination that answered and that raised this ratio would leave the
    /// line of the hunt reading a number that the summary under it denies.
    #[test]
    fn the_indicator_counts_no_round_for_a_mined_destination() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Line, &clock);
        indicator.show(Event::Target(address(TARGET)));
        indicator.show(Event::Scored {
            target: address(TARGET),
            reached: true,
            mine: Some(address(A_FIRST_HIT)),
        });
        let line = painted(indicator);
        assert!(
            line.contains("0/8 reached"),
            "a mined destination costs no round: {line:?}"
        );
    }

    /// The line of a mined destination names the first hit that started the
    /// mine.
    ///
    /// A reader of a file counts the answers from the lines, and a mined
    /// destination that answered raises no ratio. The mark is what tells such a
    /// reader why the count of the lines runs past the ratio beside them.
    #[test]
    fn a_log_line_of_a_mined_destination_names_the_first_hit_that_started_it() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Log, &clock);
        indicator.show(Event::Target(address(TARGET)));
        indicator.show(Event::Scored {
            target: address(TARGET),
            reached: true,
            mine: Some(address(A_FIRST_HIT)),
        });
        let text = String::from_utf8(indicator.sink).expect("the indicator writes text");
        assert!(
            text.contains(MINE) && text.contains(A_FIRST_HIT),
            "the line must name the mine that drew the destination: {text:?}"
        );
    }

    /// The line of an independent destination names no mine.
    #[test]
    fn a_log_line_of_an_independent_destination_names_no_mine() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Log, &clock);
        indicator.show(Event::Target(address(TARGET)));
        indicator.show(Event::Scored {
            target: address(TARGET),
            reached: true,
            mine: None,
        });
        let text = String::from_utf8(indicator.sink).expect("the indicator writes text");
        assert!(
            !text.contains(MINE),
            "an independent destination names no mine: {text:?}"
        );
    }

    #[test]
    fn a_log_holds_no_control_text() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Log, &clock);
        indicator.show(Event::Target(address(TARGET)));
        indicator.show(Event::Tick);
        indicator.show(Event::Scored {
            target: address(TARGET),
            reached: true,
            mine: None,
        });
        indicator.show(Event::Stop);
        let text = String::from_utf8(indicator.sink).expect("the indicator writes text");
        assert!(
            !text.contains(CARRIAGE_RETURN),
            "a pipe and a file keep every byte, so a log must hold no carriage return: {text:?}"
        );
    }

    /// A log line names the destination that finished, and not the one that
    /// the hunt started last.
    ///
    /// A hunt holds many destinations at once, so the one that finishes is
    /// rarely the one that started last. A line that named the newest address
    /// would put the answer of one destination beside the address of another.
    #[test]
    fn a_log_line_names_the_destination_that_finished() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Log, &clock);
        indicator.show(Event::Target(address(TARGET)));
        indicator.show(Event::Target(address(ANOTHER_TARGET)));
        indicator.show(Event::Scored {
            target: address(TARGET),
            reached: true,
            mine: None,
        });
        let text = String::from_utf8(indicator.sink).expect("the indicator writes text");
        assert!(
            text.contains(TARGET) && !text.contains(ANOTHER_TARGET),
            "the line must name the destination that finished: {text:?}"
        );
    }

    /// The line says how many destinations stand beside the one it names.
    ///
    /// A hunt of eight destinations at once shows one address, and a reader who
    /// saw that address alone would read the hunt as a hunt of one destination
    /// at a time.
    #[test]
    fn the_line_says_how_many_destinations_stand_beside_the_one_it_names() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Line, &clock);
        indicator.show(Event::Target(address(TARGET)));
        indicator.show(Event::Target(address(ANOTHER_TARGET)));
        let line = painted(indicator);
        assert!(
            line.contains(&format!("{ANOTHER_TARGET} +1")),
            "the line must say that one more destination stands: {line:?}"
        );
    }

    /// A hunt of one destination at a time names that destination and no count.
    ///
    /// The count says how many destinations stand beside the one the line
    /// names, and a hunt that holds one has none beside it.
    #[test]
    fn the_line_of_one_destination_holds_no_count_beside_it() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Line, &clock);
        indicator.show(Event::Target(address(TARGET)));
        let line = painted(indicator);
        assert!(
            line.contains(TARGET) && !line.contains('+'),
            "the line must name the destination alone: {line:?}"
        );
    }

    #[test]
    fn a_terminal_takes_the_line_and_a_pipe_takes_the_log() {
        assert_eq!(super::style_of(true), Style::Line);
        assert_eq!(super::style_of(false), Style::Log);
    }

    #[test]
    fn a_terminal_too_narrow_for_the_targets_drops_them_whole() {
        let clock = FakeClock::new();
        let line = painted(hunting_at(&clock, WITHOUT_THE_TARGETS, 0, 11));
        assert!(
            line.contains("0/8 reached") && line.contains(TARGET),
            "the rounds and the destination must stand: {line:?}"
        );
        assert!(
            !line.contains(crate::TARGETS),
            "the targets must go away whole and never in part: {line:?}"
        );
    }

    #[test]
    fn a_terminal_narrower_still_drops_the_time_and_keeps_the_destination() {
        let clock = FakeClock::new();
        let mut indicator = hunting_at(&clock, WITHOUT_THE_TIME, 0, 11);
        indicator.show(Event::Tick);
        let line = painted(indicator);
        assert!(
            line.contains("0/8 reached") && line.contains(TARGET),
            "the rounds and the destination must stand: {line:?}"
        );
        assert!(
            !line.contains("0ms"),
            "the time must go away before the destination: {line:?}"
        );
    }

    #[test]
    fn the_time_stands_while_the_targets_are_gone() {
        let clock = FakeClock::new();
        let mut indicator = hunting_at(&clock, WITHOUT_THE_TARGETS, 0, 11);
        clock.advance(Duration::from_secs(41));
        indicator.show(Event::Tick);
        let line = painted(indicator);
        assert!(
            line.contains("41s"),
            "the targets go away in front of the time: {line:?}"
        );
    }

    #[test]
    fn the_line_names_the_time_that_the_hunt_took() {
        let clock = FakeClock::new();
        let mut indicator = indicator(Style::Line, &clock);
        indicator.show(Event::Target(address(TARGET)));
        clock.advance(Duration::from_millis(42_500));
        indicator.show(Event::Tick);
        let line = painted(indicator);
        assert!(
            line.contains("42s"),
            "the line must name the time: {line:?}"
        );
    }
}

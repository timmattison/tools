//! The live display of a run, and the keys that drive it.
//!
//! A live run holds the terminal in raw mode, so the terminal sends the bytes
//! of every key straight to this process. Raw mode clears `ISIG`, which is the
//! setting that turns Ctrl-C into a `SIGINT`. Ctrl-C therefore arrives here as
//! a key press, and the signal handler of `main.rs` never sees it. That is the
//! reason this module classifies the keys itself: it is the one part of the
//! live run that can stop a run that the user asked to stop.

use crate::record::{NameRecord, RoundRecord};
use crate::stats::HopTable;
use crate::ui;
use crossterm::cursor::MoveTo;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::queue;
use crossterm::terminal::{Clear, ClearType};
use std::collections::BTreeMap;
use std::io::Write;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

/// What one key press asks for.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the tests of this module read the whole table, and the display that acts on a command joins it in the next step of this work. The expectation fails once that display lands, which takes the attribute back off"
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    /// Stop the run.
    Quit,
    /// Hold the display where it stands, or let it move again.
    Pause,
    /// Show the names of the addresses, or show the addresses.
    Names,
    /// Empty the table of the display, and count the rounds again from zero.
    Reset,
    /// Show the list of the keys, or hide it.
    Help,
}

/// What one key press means, or nothing.
///
/// - A key release means nothing. A kitty terminal and a Windows terminal both
///   report a release, and only a press acts.
/// - Ctrl-C stops the run, and `q` stops it too. Raw mode clears `ISIG`, so
///   Ctrl-C arrives as this key press and the process takes no `SIGINT`. A run
///   that ignored it would need a second terminal to stop.
/// - `p` holds the display, `n` turns the names on or off, `r` empties the
///   table of the display, and `?` shows the list of the keys.
/// - Every other key means nothing.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the tests of this module read the whole table, and the display that polls the keyboard joins it in the next step of this work. The expectation fails once that display lands, which takes the attribute back off"
    )
)]
pub(crate) fn classify(key: KeyEvent) -> Option<Command> {
    let KeyEvent {
        code,
        modifiers,
        kind,
        ..
    } = key;

    if kind == KeyEventKind::Release {
        return None;
    }

    // Checked in front of the table of the letters, so no mode that a later
    // display adds can trap the user.
    if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
        return Some(Command::Quit);
    }

    match code {
        KeyCode::Char('q') => Some(Command::Quit),
        KeyCode::Char('p') => Some(Command::Pause),
        KeyCode::Char('n') => Some(Command::Names),
        KeyCode::Char('r') => Some(Command::Reset),
        KeyCode::Char('?') => Some(Command::Help),
        _ => None,
    }
}

/// The end of every line that a draw writes.
///
/// Raw mode returns no carriage on a bare line feed, so a frame of bare line
/// feeds walks down the screen one column further to the right for each line
/// of it.
const LINE_END: &str = "\r\n";

/// A source of key presses.
pub(crate) trait Keys {
    /// The commands of the keys that arrived since the last ask.
    ///
    /// The ask never waits. A run that waited for a key would take no round
    /// while it waited, and the table would then stand still while the path
    /// moved.
    fn presses(&mut self) -> Vec<Command>;
}

/// The key source of a run that reads no key.
pub(crate) struct NoKeys;

impl Keys for NoKeys {
    fn presses(&mut self) -> Vec<Command> {
        Vec::new()
    }
}

/// What a live run shows.
pub(crate) trait Screen {
    /// Takes the key presses that arrived. Answers whether the user asked the
    /// run to stop.
    fn poll(&mut self) -> bool;
    /// One round arrived.
    fn round(&mut self, round: &RoundRecord);
    /// The names that arrived.
    fn names(&mut self, names: &[NameRecord]);
}

/// The facts of a run that every frame of that run repeats.
///
/// The header line names them, and no round changes any of them, so the table
/// takes them one time at the start of the run. They travel as one value
/// because a constructor of eight parameters says nothing about which
/// parameter is which.
pub(crate) struct RunFacts {
    /// The destination as the command line named it.
    pub(crate) destination: String,
    /// The address that the destination resolved to.
    pub(crate) address: IpAddr,
    /// The source address of the run.
    pub(crate) source: IpAddr,
    /// The period of one round.
    pub(crate) interval: Duration,
    /// The path of the recorded file.
    ///
    /// The header line names the size of that file, and the file grows with
    /// every round, so each draw reads the size again.
    pub(crate) path: PathBuf,
}

/// The live table of a run.
///
/// The table folds every round that arrives, and it draws the frame of that
/// fold. It holds the terminal in raw mode on the alternate screen, so each
/// draw clears the screen and moves the cursor to the origin first, and every
/// line ends with a carriage return and a line feed. Raw mode returns no
/// carriage on a bare line feed, and a frame of bare line feeds walks down the
/// screen one column further to the right for each line of it.
pub(crate) struct Table<W: Write, K: Keys> {
    /// The facts of the run that the header line names.
    facts: RunFacts,
    /// The name of the recorded file, without its directory.
    file: String,
    /// The fold of every round that arrived.
    table: HopTable,
    /// The host name of each address that a name record named.
    names: BTreeMap<IpAddr, String>,
    /// The map that a table of the raw addresses hands the frame.
    ///
    /// One empty map that stands beside the names, and not a map that a draw
    /// builds: a run draws one frame for each round, and each of those draws
    /// would build the same empty map and drop it again.
    nameless: BTreeMap<IpAddr, String>,
    /// The number of rounds that the table folded.
    ///
    /// This is the counter of the display, and not the counter of the run. The
    /// `end` record counts what the file holds, and a reader of the table asks
    /// how many rounds the picture in front of them stands on. The reset
    /// command therefore zeroes this counter and touches no file.
    rounds: usize,
    /// Whether the table holds where it stands.
    paused: bool,
    /// Whether the hosts read as names.
    named: bool,
    /// Whether the list of the keys stands under the table.
    help: bool,
    /// Where the frames go.
    sink: W,
    /// Where the commands come from.
    keys: K,
    /// The number of terminal columns that a frame draws in.
    width: u16,
}

impl<W: Write, K: Keys> Table<W, K> {
    /// A table of one run, which draws into `sink` and reads `keys`.
    ///
    /// The names start on. A reader who wants the raw addresses asks for them
    /// with a key, and a run that resolves no name shows the addresses anyway,
    /// because the map of the names then stays empty.
    pub(crate) fn new(facts: RunFacts, sink: W, keys: K, width: u16) -> Self {
        Self {
            file: crate::file_name(&facts.path),
            facts,
            table: HopTable::new(),
            names: BTreeMap::new(),
            nameless: BTreeMap::new(),
            rounds: 0,
            paused: false,
            named: true,
            help: false,
            sink,
            keys,
            width,
        }
    }

    /// The lines of one frame of this table.
    ///
    /// The frame stands apart from the draw that puts it on the screen, so a
    /// reader of the lines reads the glyphs of the table and no control
    /// sequence of a terminal.
    ///
    /// A table of the raw addresses hands the frame the empty map that stands
    /// beside the names. The frame names an address that its map holds none of
    /// by the address itself, so one empty map turns every host of the table
    /// back into the number that answered.
    fn frame_lines(&self) -> Vec<String> {
        let frame = ui::Frame {
            header: ui::Header {
                destination: Some(&self.facts.destination),
                address: Some(self.facts.address),
                source: Some(self.facts.source),
                rounds: self.rounds,
                interval: Some(self.facts.interval),
                file: &self.file,
                // The file grows with every round, so the size comes off the
                // file at the moment of the draw. A file that the run cannot
                // measure still folds, and the header then names the file and
                // no size.
                bytes: std::fs::metadata(&self.facts.path)
                    .ok()
                    .map(|data| data.len()),
            },
            table: &self.table,
            names: if self.named {
                &self.names
            } else {
                &self.nameless
            },
            destination: Some(self.facts.address),
        };
        frame.lines(self.width)
    }

    /// Puts one frame on the screen, over the frame that stands there.
    ///
    /// A frame that does not print stops nothing. The recording is the purpose
    /// of the tool, and the frame is one view of it, so a run whose terminal
    /// goes away loses the display and keeps the recording.
    fn draw(&mut self) {
        let lines = self.frame_lines();
        drop(self.paint(&lines));
    }

    /// Writes the lines of one frame.
    ///
    /// The clear comes first, so a frame of fewer lines than the frame before
    /// it leaves none of the older lines on the screen.
    ///
    /// # Errors
    ///
    /// Answers the fault that the sink raised. The caller of this function
    /// drops that fault, for the reason that [`Table::draw`] states.
    fn paint(&mut self, lines: &[String]) -> std::io::Result<()> {
        queue!(self.sink, Clear(ClearType::All), MoveTo(0, 0))?;
        for line in lines {
            write!(self.sink, "{line}{LINE_END}")?;
        }
        self.sink.flush()
    }
}

impl<W: Write, K: Keys> Screen for Table<W, K> {
    fn poll(&mut self) -> bool {
        false
    }

    fn round(&mut self, round: &RoundRecord) {
        self.table.observe(round);
        self.rounds += 1;
        self.draw();
    }

    fn names(&mut self, _names: &[NameRecord]) {}
}

#[cfg(test)]
mod tests {
    use super::{classify, Command, Keys, RunFacts, Screen, Table};
    use crate::testing::{address, round};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    /// Builds the press of a key that no modifier holds.
    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// The number of terminal columns that every table below draws in.
    ///
    /// The nominal frame is 97 columns wide: every column of the table, with a
    /// Host column of 30. The rows of `tests/replay.rs` stand at that same
    /// width, so a row of this file reads as a row of that one.
    const WIDTH: u16 = 97;

    /// The destination of every table below.
    const DESTINATION: &str = "example.com";

    /// The address that the destination resolved to.
    const DESTINATION_ADDRESS: &str = "93.184.216.34";

    /// The source address of every run below.
    const SOURCE: &str = "1.2.3.4";

    /// The period of one round of every run below.
    const INTERVAL: Duration = Duration::from_secs(1);

    /// The address of the one router that answers the rounds below.
    const ROUTER: &str = "10.0.0.1";

    /// The round-trip time of the answer of that router, in milliseconds.
    const RTT: f64 = 0.87;

    /// The first TTL of every round below, which is also the last one.
    const TTL: u8 = 1;

    /// The start of the header line of a table that folded one round.
    ///
    /// The name of the recorded file ends that line, and each test builds a
    /// path of its own, so the assertion reads the start of the line and not
    /// the whole of it.
    const ONE_ROUND_HEADER: &str = " krt  example.com → 93.184.216.34   src 1.2.3.4   round 1   1s   ";

    /// The row of a table that folded one round of one TTL.
    ///
    /// The round answers at TTL 1 from 10.0.0.1 at 0.87, which one decimal
    /// place writes as 0.9. One answer is its own last, smallest, mean, and
    /// largest time, and the population standard deviation of one sample is
    /// 0.0. The TTL answered the one probe it took, so it loses nothing. One
    /// sample draws one bar, and a window of one sample varies by nothing, so
    /// that bar is the lowest one.
    const ONE_ROUND_ROW: &str =
        "   1  10.0.0.1                          0.0%      1    0.9    0.9    0.9    0.9    0.0  ▁";

    /// The escape character that starts every control sequence of a draw.
    const ESCAPE: char = '\u{1b}';

    /// A key source that hands back one list of commands for each turn.
    ///
    /// A turn past the end of the script took no key.
    struct FakeKeys {
        /// The commands of each turn, the next turn first.
        turns: VecDeque<Vec<Command>>,
    }

    impl FakeKeys {
        /// A key source of one script.
        fn of(script: &[&[Command]]) -> Self {
            Self {
                turns: script.iter().map(|turn| turn.to_vec()).collect(),
            }
        }
    }

    impl Keys for FakeKeys {
        fn presses(&mut self) -> Vec<Command> {
            self.turns.pop_front().unwrap_or_default()
        }
    }

    /// Builds a path under the temporary directory that no other run reaches.
    ///
    /// Two runs of one test can overlap, because `cargo test` runs on many
    /// threads and more than one `cargo test` can run at once. The process
    /// identifier and the nanosecond keep the two runs apart.
    ///
    /// # Panics
    ///
    /// Panics on a clock that stands before the epoch. Such a clock is a fault
    /// of the machine, not an answer of the code under test.
    fn temp_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock must stand after the epoch")
            .as_nanos();
        let process = std::process::id();
        std::env::temp_dir().join(format!("krt-live-{label}-{process}-{nanos}.jsonl"))
    }

    /// A table that draws into bytes, reads `keys`, and heads its frames with
    /// the recorded file at `path`.
    fn table_at<K: Keys>(path: PathBuf, keys: K) -> Table<Vec<u8>, K> {
        Table::new(
            RunFacts {
                destination: DESTINATION.to_owned(),
                address: address(DESTINATION_ADDRESS),
                source: address(SOURCE),
                interval: INTERVAL,
                path,
            },
            Vec::new(),
            keys,
            WIDTH,
        )
    }

    /// A table that draws into bytes and takes the keys of a script.
    ///
    /// The path of its recorded file names no file, so the header line of it
    /// names no size. The one test that reads a size writes a file of its own.
    fn table(script: &[&[Command]]) -> Table<Vec<u8>, FakeKeys> {
        table_at(temp_path("frame"), FakeKeys::of(script))
    }

    /// The text that a terminal prints for what the draws wrote, with the
    /// control sequences taken out.
    ///
    /// Every sequence of a draw starts with the escape character and ends with
    /// a letter: the clear of the screen ends `J`, and the move of the cursor
    /// ends `H`. The reader therefore drops from one escape to the letter that
    /// closes it, and what stays is what a reader of the terminal sees.
    fn glyphs(painted: &[u8]) -> String {
        let text = String::from_utf8_lossy(painted);
        let mut kept = String::new();
        let mut characters = text.chars();
        while let Some(character) = characters.next() {
            if character != ESCAPE {
                kept.push(character);
                continue;
            }
            for inside in characters.by_ref() {
                if inside.is_ascii_alphabetic() {
                    break;
                }
            }
        }
        kept
    }

    /// The lines that the draws wrote into a sink, in the order they arrived.
    fn painted(sink: &[u8]) -> Vec<String> {
        glyphs(sink)
            .split_terminator(super::LINE_END)
            .map(str::to_owned)
            .collect()
    }

    /// One round of one TTL, which the router answered.
    fn one_round() -> crate::record::RoundRecord {
        round(TTL, TTL, &[(TTL, ROUTER, RTT)])
    }

    /// The word that a table which holds where it stands writes under its
    /// table.
    ///
    /// The test spells the word, and the module spells it again. That is on
    /// purpose: a test that read the constant of the module would agree with
    /// every word the module ever holds, and this word is what a reader of the
    /// screen sees.
    const PAUSED: &str = "paused";

    /// The last `count` lines of a frame, or every line of a frame that holds
    /// fewer than that.
    ///
    /// A frame of too few lines must fail the assertion of the test that reads
    /// the end of it, and must not stop that test with an index.
    fn last_lines(lines: &[String], count: usize) -> Vec<&str> {
        lines
            .iter()
            .skip(lines.len().saturating_sub(count))
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn one_round_reaches_the_sink_as_a_row_of_the_table() {
        let mut screen = table(&[]);
        screen.round(&one_round());
        let lines = painted(&screen.sink);
        assert!(
            lines
                .first()
                .is_some_and(|line| line.starts_with(ONE_ROUND_HEADER)),
            "the header line names the target, the source, the one round, and the interval: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line == ONE_ROUND_ROW),
            "the row of the TTL stands under that header: {lines:?}"
        );
    }

    #[test]
    fn the_pause_holds_the_table_and_the_frame_of_it_names_the_pause() {
        let mut screen = table(&[&[Command::Pause]]);
        screen.round(&one_round());
        screen.sink.clear();

        // A poll draws while the table holds as well, so the mark of the pause
        // reaches the screen of a run that no round moves.
        screen.poll();
        let held = painted(&screen.sink);
        assert_eq!(
            last_lines(&held, 2),
            ["", PAUSED],
            "one blank line and the word of the pause close the frame: {held:?}"
        );

        screen.sink.clear();
        screen.round(&one_round());
        assert!(
            screen.sink.is_empty(),
            "a round of a table that holds draws nothing: {:?}",
            painted(&screen.sink)
        );
    }

    #[test]
    fn the_q_key_asks_for_a_quit() {
        assert_eq!(classify(press(KeyCode::Char('q'))), Some(Command::Quit));
    }

    #[test]
    fn ctrl_c_asks_for_a_quit() {
        // Raw mode clears `ISIG`, so Ctrl-C arrives as a key press and not as a
        // signal. A run that ignored it would need a second terminal to stop.
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(classify(ctrl_c), Some(Command::Quit));
    }

    #[test]
    fn the_p_key_asks_for_a_pause() {
        assert_eq!(classify(press(KeyCode::Char('p'))), Some(Command::Pause));
    }

    #[test]
    fn the_n_key_asks_for_the_names() {
        assert_eq!(classify(press(KeyCode::Char('n'))), Some(Command::Names));
    }

    #[test]
    fn the_r_key_asks_for_a_reset() {
        assert_eq!(classify(press(KeyCode::Char('r'))), Some(Command::Reset));
    }

    #[test]
    fn the_question_mark_asks_for_the_help() {
        assert_eq!(classify(press(KeyCode::Char('?'))), Some(Command::Help));
    }

    #[test]
    fn a_release_of_a_mapped_key_asks_for_nothing() {
        // A kitty terminal and a Windows terminal both report a release. Only a
        // press acts, or one press of `q` stops the run two times.
        let release = KeyEvent {
            kind: KeyEventKind::Release,
            ..press(KeyCode::Char('q'))
        };
        assert_eq!(classify(release), None);
    }

    #[test]
    fn a_release_of_ctrl_c_asks_for_nothing() {
        // The check of the release stands in front of the check of Ctrl-C, so
        // the escape that no table can trap also obeys the rule of the press.
        let release = KeyEvent {
            kind: KeyEventKind::Release,
            ..KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        };
        assert_eq!(classify(release), None);
    }

    #[test]
    fn a_letter_that_the_table_does_not_hold_asks_for_nothing() {
        assert_eq!(classify(press(KeyCode::Char('x'))), None);
    }

    #[test]
    fn a_key_that_carries_no_letter_asks_for_nothing() {
        assert_eq!(classify(press(KeyCode::Enter)), None);
    }
}

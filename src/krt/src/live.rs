//! The live display of a run, and the keys that drive it.
//!
//! A live run holds the terminal in raw mode, so the terminal sends the bytes
//! of every key straight to this process. Raw mode clears `ISIG`, which is the
//! setting that turns Ctrl-C into a `SIGINT`. Ctrl-C therefore arrives here as
//! a key press, and the signal handler of `main.rs` never sees it. That is the
//! reason this module classifies the keys itself: it is the one part of the
//! live run that can stop a run that the user asked to stop.

// Nothing outside this module names any item of it. The run loop takes a
// screen in the next step of this work, and every item below is then live. One
// attribute stands here, and not one attribute for each item, because the
// items are dead for one reason: no caller. The expectation fails once the
// whole module is live, which takes this attribute back off.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the run loop joins this module in the next step of this work"
    )
)]

use crate::record::{NameRecord, RoundRecord};
use crate::stats::HopTable;
use crate::ui;
use crate::ui::render_duration;
use crate::{counted, HOP, NEVER_REACHED, REACHED, ROUND, SUMMARY_SEPARATOR};
use crossterm::cursor::MoveTo;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::queue;
use crossterm::terminal::{Clear, ClearType};
use std::collections::BTreeMap;
use std::io::Write;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// What one key press asks for.
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

/// The word that a table which holds where it stands writes under itself.
///
/// A held table looks like a table of a run that stopped, and a reader who
/// pressed the key a minute ago does not remember which of the two stands in
/// front of them. One word answers that.
const PAUSED: &str = "paused";

/// The number of keys that a live table reads.
const KEY_COUNT: usize = 5;

/// Each key of a live table, and what that key does.
///
/// This list is the one place that says what a key does, and the help builds
/// its lines out of it. A second list would name a key that [`classify`] no
/// longer holds, and a reader who pressed that key would then read a screen
/// which says that nothing happened.
const KEYS: [(&str, &str); KEY_COUNT] = [
    ("q, Ctrl-C", "stop the run"),
    ("p", "hold the table, or let it move"),
    ("n", "show the names, or show the addresses"),
    ("r", "empty the table"),
    ("?", "show these keys, or hide them"),
];

/// The text between a key and what that key does.
///
/// Two spaces, so the widest key of the list stands clear of its own text.
const KEY_GAP: &str = "  ";

/// The lines of the list of the keys.
///
/// Every key takes the columns of the widest key, so the text of each of them
/// starts at one column and the list reads as two columns.
fn key_lines() -> Vec<String> {
    let width = KEYS
        .iter()
        .map(|(key, _)| ui::display_width(key))
        .max()
        .unwrap_or(0);
    KEYS.iter()
        .map(|(key, does)| format!("{key:<width$}{KEY_GAP}{does}"))
        .collect()
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
    ///
    /// A turn that took no key draws nothing. The run loop takes ten turns each
    /// second, and a draw of every turn would clear the screen and paint it
    /// again at that rate.
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
/// fold. A run that draws this table holds the terminal in raw mode on the
/// alternate screen, so each draw clears the screen and moves the cursor to the
/// origin first, and every line ends with a carriage return and a line feed.
/// Raw mode returns no carriage on a bare line feed, and a frame of bare line
/// feeds walks down the screen one column further to the right for each line of
/// it.
pub(crate) struct Table<W: Write, K: Keys> {
    /// The facts of the run that the header line names.
    facts: RunFacts,
    /// The name of the recorded file, without its directory.
    file: String,
    /// The fold of every round that arrived.
    fold: HopTable,
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
            fold: HopTable::new(),
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
    ///
    /// The footer stands under the table, and one blank line holds it off the
    /// last row of the path: a word directly under the numbers of a hop reads
    /// as a part of the table.
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
            table: &self.fold,
            names: if self.named {
                &self.names
            } else {
                &self.nameless
            },
            destination: Some(self.facts.address),
        };
        let mut lines = frame.lines(self.width);
        if self.paused {
            lines.push(String::new());
            lines.push(PAUSED.to_owned());
        }
        if self.help {
            lines.push(String::new());
            lines.extend(key_lines());
        }
        lines
    }

    /// Acts on one command, and answers whether that command stops the run.
    fn apply(&mut self, command: Command) -> bool {
        match command {
            Command::Pause => self.paused = !self.paused,
            Command::Names => self.named = !self.named,
            Command::Reset => {
                // A new table, because the fold keeps no way back to the empty
                // one. The names stay: a name belongs to an address and not to
                // a round, and a reader who emptied the table did not ask to
                // resolve every address of the path again.
                self.fold = HopTable::new();
                self.rounds = 0;
            }
            Command::Help => self.help = !self.help,
            // No field of the table holds the stop. The run loop owns it,
            // because the loop is what closes the recorded file with its `end`
            // record, and a display that stopped the process itself would
            // leave the file without that record.
            Command::Quit => return true,
        }
        false
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
        let commands = self.keys.presses();
        // A turn of no key leaves the frame that stands. The run loop polls
        // ten times each second, and a draw of every turn would clear the
        // screen and paint it again at that rate. The table would flicker, and
        // no key of the reader asked for anything.
        if commands.is_empty() {
            return false;
        }
        let mut quit = false;
        for command in commands {
            // Every command of the turn acts, and the loop does not stop at
            // the stop: a turn that took `p` and then `q` holds the table and
            // stops the run.
            quit = self.apply(command) || quit;
        }
        // The draw stands outside the loop, and it draws while the table holds
        // as well: a table that took the key of the pause must put the mark of
        // that pause on the screen, and no round draws it while the table
        // holds.
        self.draw();
        quit
    }

    fn round(&mut self, round: &RoundRecord) {
        self.fold.observe(round);
        self.rounds += 1;
        if !self.paused {
            self.draw();
        }
    }

    fn names(&mut self, names: &[NameRecord]) {
        for name in names {
            // The map is keyed by the address, and not by the hop, because one
            // address answers at any number of TTLs.
            self.names.insert(name.addr, name.host.clone());
        }
        if !self.paused {
            self.draw();
        }
    }
}

/// Writes the one line that a run of no live table prints for one round.
///
/// The line holds the number of the round, the number of hops that answered,
/// whether the round reached the target, and the time that the round took. Two
/// spaces separate the fields, as they do in the closing line of the run.
///
/// A hop that did not answer is absent from the record, so the count is the
/// number of hops that answered and not the length of the path.
///
/// This line is the whole picture that a headless run gives of one round. A run
/// that holds a terminal draws the table above in the place of it.
pub(crate) fn status_line(round: &RoundRecord) -> String {
    let reached = if round.reached {
        REACHED
    } else {
        NEVER_REACHED
    };
    [
        format!("{ROUND} {}", round.seq),
        counted(round.hops.len(), HOP),
        reached.to_owned(),
        render_duration(Duration::from_millis(round.dur_ms)),
    ]
    .join(SUMMARY_SEPARATOR)
}

/// The clock that times the lines of a run which draws no table.
///
/// Such a run writes one line each minute. A test of that minute must not wait
/// one, so the clock comes from the caller and a test hands the screen a clock
/// that it moves by hand.
pub(crate) trait Clock {
    /// The moment now.
    fn now(&self) -> Instant;
}

/// The clock of a run, which reads the clock of the machine.
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// The time between two lines of a run that draws no table.
///
/// A run of one round each second writes one line each second, and an hour of
/// such a run fills a terminal with 3600 lines that say almost the same thing.
/// One line for each minute of the run says the same thing and leaves the
/// window of the terminal to the work of the reader.
const LINE_PERIOD: Duration = Duration::from_secs(60);

/// The display of a run that draws no table.
///
/// A run whose standard output is a pipe or a file holds no terminal. Such a
/// run clears no screen and reads no key. It writes one status line at its
/// first round, and one more each time a whole [`LINE_PERIOD`] passed since the
/// line before it. The first round stands on its own reason: a run that printed
/// nothing for a whole minute reads as a run that died.
pub(crate) struct Headless<W: Write, C: Clock> {
    /// Where the lines go.
    sink: W,
    /// The clock that times the lines.
    clock: C,
    /// The moment of the last line, and nothing before the first one.
    last: Option<Instant>,
}

impl<W: Write, C: Clock> Headless<W, C> {
    /// A headless screen that writes into `sink` and times its lines by
    /// `clock`.
    pub(crate) fn new(sink: W, clock: C) -> Self {
        Self {
            sink,
            clock,
            last: None,
        }
    }

    /// Whether the line of a round that arrives at `now` reaches the sink.
    ///
    /// The first round of a run prints, and every round after it prints when a
    /// whole [`LINE_PERIOD`] passed since the last line.
    fn due(&self, now: Instant) -> bool {
        self.last
            .is_none_or(|last| now.duration_since(last) >= LINE_PERIOD)
    }
}

impl<W: Write, C: Clock> Screen for Headless<W, C> {
    fn poll(&mut self) -> bool {
        // A headless run holds no terminal, so no key of it reaches this
        // process. The signal handler that `main.rs` registers stops such a
        // run, and the limits of the command line stop it too.
        false
    }

    fn round(&mut self, round: &RoundRecord) {
        let now = self.clock.now();
        if !self.due(now) {
            return;
        }
        self.last = Some(now);
        // A line that does not print stops nothing. The recording is the
        // purpose of the tool, and the line is one view of it, so a reader who
        // closes the pipe of the display loses the display and keeps the
        // recording.
        drop(writeln!(self.sink, "{}", status_line(round)));
    }

    fn names(&mut self, _names: &[NameRecord]) {
        todo!("a headless screen shows no name")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify, status_line, Clock, Command, Headless, Keys, NoKeys, RunFacts, Screen,
        SystemClock, Table,
    };
    use crate::record::{NameRecord, RoundRecord, RunId};
    use crate::testing::{address, round};
    use chrono::Utc;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

    /// The name that a name record gives that router.
    const ROUTER_NAME: &str = "router.lan";

    /// The host of the row of that router, while the names stand.
    ///
    /// The address stays beside the name, because a name is what a resolver
    /// said and an address is what answered.
    const NAMED_ROUTER: &str = "router.lan (10.0.0.1)";

    /// The round-trip time of the answer of that router, in milliseconds.
    const RTT: f64 = 0.87;

    /// The first TTL of every round below, which is also the last one.
    const TTL: u8 = 1;

    /// The identifier of the run that every name record below belongs to.
    const RUN: &str = "2026-08-18T12:00:00.000Z";

    /// The number of bytes of the recorded file at the first draw of the test
    /// of the size.
    const FIRST_BYTES: usize = 100;

    /// The number of bytes of that same file at the second draw.
    const SECOND_BYTES: usize = 142;

    /// The byte that fills that file.
    ///
    /// The header names the size of the recorded file and reads no line of it,
    /// so one byte serves as well as a record.
    const FILLER: u8 = b'x';

    /// The start of the header line of a table that folded one round.
    ///
    /// The name of the recorded file ends that line, and each test builds a
    /// path of its own, so the assertion reads the start of the line and not
    /// the whole of it.
    const ONE_ROUND_HEADER: &str =
        " krt  example.com → 93.184.216.34   src 1.2.3.4   round 1   1s   ";

    /// The start of the header line of a table that folded no round.
    const NO_ROUND_HEADER: &str =
        " krt  example.com → 93.184.216.34   src 1.2.3.4   round 0   1s   ";

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

    /// A file that one test makes. The file goes away when the test ends, and
    /// also when the test panics.
    struct TempFile {
        /// The path of the file.
        path: PathBuf,
    }

    impl TempFile {
        /// Holds a path that no file uses yet, and that no other run reaches.
        fn absent(label: &str) -> Self {
            Self {
                path: temp_path(label),
            }
        }

        /// The path of the file.
        fn path(&self) -> &Path {
            &self.path
        }

        /// Writes a file of `bytes` bytes at that path.
        ///
        /// # Panics
        ///
        /// Panics on a write that fails. Such a fault is a fault of the
        /// machine, not an answer of the code under test.
        fn of(&self, bytes: usize) {
            fs::write(&self.path, vec![FILLER; bytes]).expect("the test file must write");
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
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

    /// The text that ends a header line whose file holds this many bytes.
    ///
    /// A size below one kilobyte reads as whole bytes, so the text names the
    /// count of the bytes itself and no unit above the byte.
    fn size_of(bytes: usize) -> String {
        format!("({bytes} B)")
    }

    /// One round of one TTL, which the router answered.
    fn one_round() -> RoundRecord {
        round(TTL, TTL, &[(TTL, ROUTER, RTT)])
    }

    /// The TTL that the destination answered at.
    const TARGET_TTL: u8 = 2;

    /// The round-trip time of the answer of the destination, in milliseconds.
    const TARGET_RTT: f64 = 4.56;

    /// The number of a round that answered one hop and reached nothing.
    const LOST_ROUND_SEQ: u64 = 2;

    /// The time that such a round took, in milliseconds.
    const LOST_ROUND_MS: u64 = 1004;

    /// The status line of the first round of a run that answered the whole
    /// path.
    const A_WHOLE_PATH_LINE: &str = "round 1  2 hops  reached  1s";

    /// The status line of a round that answered one hop and reached nothing.
    const A_LOST_ROUND_LINE: &str = "round 2  1 hop  never reached  1004ms";

    /// One round that answered two hops and reached the target.
    fn a_whole_path_round() -> RoundRecord {
        let mut record = round(
            TTL,
            TARGET_TTL,
            &[
                (TTL, ROUTER, RTT),
                (TARGET_TTL, DESTINATION_ADDRESS, TARGET_RTT),
            ],
        );
        record.reached = true;
        record
    }

    /// One round that answered one hop and reached nothing.
    fn a_lost_round() -> RoundRecord {
        let mut record = round(TTL, TARGET_TTL, &[(TTL, ROUTER, RTT)]);
        record.seq = LOST_ROUND_SEQ;
        record.dur_ms = LOST_ROUND_MS;
        record
    }

    /// A clock that the test moves by hand.
    ///
    /// A headless screen writes one line each minute, and a test that waited a
    /// minute for the second line would take a minute of the suite. The moment
    /// sits behind a `Cell`, because [`Clock::now`] takes the clock by
    /// reference. The fake stays on one thread.
    struct FakeClock {
        /// The moment that the clock reads now.
        now: Cell<Instant>,
    }

    impl FakeClock {
        /// A clock that stands at the moment of its making.
        fn new() -> Rc<Self> {
            Rc::new(Self {
                now: Cell::new(Instant::now()),
            })
        }

        /// Moves the clock forward.
        fn advance(&self, by: Duration) {
            self.now.set(self.now.get() + by);
        }
    }

    impl Clock for Rc<FakeClock> {
        fn now(&self) -> Instant {
            self.now.get()
        }
    }

    /// A headless screen that writes into bytes and reads the clock of the
    /// test.
    fn headless(clock: &Rc<FakeClock>) -> Headless<Vec<u8>, Rc<FakeClock>> {
        Headless::new(Vec::new(), Rc::clone(clock))
    }

    /// The step that the clock of the test takes between two rounds.
    ///
    /// The rounds of [`ROUNDS_INSIDE_A_MINUTE`] at this step stand inside one
    /// minute, so a screen that holds to the minute prints one line for the
    /// whole of them.
    const ROUND_STEP: Duration = Duration::from_secs(10);

    /// The number of rounds that arrive inside one minute of the test.
    const ROUNDS_INSIDE_A_MINUTE: usize = 4;

    /// One whole minute.
    ///
    /// The test spells the minute, and the module spells it again. That is on
    /// purpose, as the word of the pause is: a test that read the constant of
    /// the module would agree with every period the module ever holds, and one
    /// line each minute is what a reader of a headless run gets.
    const ONE_MINUTE: Duration = Duration::from_secs(60);

    /// The lines that a headless screen wrote, in the order they arrived.
    fn printed(sink: &[u8]) -> Vec<String> {
        String::from_utf8_lossy(sink)
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The record of one name that a lookup answered.
    ///
    /// The identifier of the run and the moment of the record take the values
    /// of the moment of the test. No frame reads either of them: the map of the
    /// names is keyed by the address, because one address answers at any number
    /// of TTLs.
    fn name(addr: &str, host: &str) -> NameRecord {
        NameRecord {
            run: RunId::from(RUN),
            ts: Utc::now(),
            addr: address(addr),
            host: host.to_owned(),
        }
    }

    /// The word that a table which holds where it stands writes under its
    /// table.
    ///
    /// The test spells the word, and the module spells it again. That is on
    /// purpose: a test that read the constant of the module would agree with
    /// every word the module ever holds, and this word is what a reader of the
    /// screen sees.
    const PAUSED: &str = "paused";

    /// The list of the keys, one line for each key.
    ///
    /// The test spells every line, and the module builds the same lines out of
    /// one table of pairs. That is on purpose, as the word of the pause is: a
    /// test that read the table of the module would agree with every list the
    /// module ever holds, and this list is what a reader of the screen sees.
    const HELP_LINES: [&str; 5] = [
        "q, Ctrl-C  stop the run",
        "p          hold the table, or let it move",
        "n          show the names, or show the addresses",
        "r          empty the table",
        "?          show these keys, or hide them",
    ];

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
    fn the_names_key_swaps_a_name_for_the_address_of_it_and_back() {
        let mut screen = table(&[&[Command::Names], &[Command::Names]]);
        screen.round(&one_round());
        screen.names(&[name(ROUTER, ROUTER_NAME)]);
        let named = painted(&screen.sink);
        assert!(
            named.iter().any(|line| line.contains(NAMED_ROUTER)),
            "the name that arrived stands in the host of the row: {named:?}"
        );

        screen.sink.clear();
        screen.poll();
        let raw = painted(&screen.sink);
        assert!(
            !raw.iter().any(|line| line.contains(ROUTER_NAME)),
            "the key takes the names off the table: {raw:?}"
        );
        assert!(
            raw.iter().any(|line| line.contains(ROUTER)),
            "the host of the row then reads as the address that answered: {raw:?}"
        );

        screen.sink.clear();
        screen.poll();
        let again = painted(&screen.sink);
        assert!(
            again.iter().any(|line| line.contains(NAMED_ROUTER)),
            "a second press puts the names back, so the table keeps them: {again:?}"
        );
    }

    #[test]
    fn the_reset_key_empties_the_table_and_zeroes_the_counter_of_the_display() {
        let mut screen = table(&[&[Command::Reset]]);
        screen.round(&one_round());

        screen.sink.clear();
        screen.poll();
        let empty = painted(&screen.sink);
        assert!(
            empty
                .first()
                .is_some_and(|line| line.starts_with(NO_ROUND_HEADER)),
            "the header line then counts no round: {empty:?}"
        );
        assert!(
            !empty.iter().any(|line| line.contains(ROUTER)),
            "and no row of the path stands under it: {empty:?}"
        );

        screen.sink.clear();
        screen.round(&one_round());
        let first = painted(&screen.sink);
        assert!(
            first.iter().any(|line| line == ONE_ROUND_ROW),
            "the round that follows the reset counts as the first round: {first:?}"
        );
    }

    #[test]
    fn the_help_key_shows_the_list_of_the_keys_and_hides_it_again() {
        let mut screen = table(&[&[Command::Help], &[Command::Help]]);
        screen.poll();
        let shown = painted(&screen.sink);
        let mut wanted = vec![""];
        wanted.extend(HELP_LINES);
        assert_eq!(
            last_lines(&shown, wanted.len()),
            wanted,
            "one blank line and the list of the keys close the frame: {shown:?}"
        );

        screen.sink.clear();
        screen.poll();
        let hidden = painted(&screen.sink);
        assert!(
            !hidden
                .iter()
                .any(|line| HELP_LINES.contains(&line.as_str())),
            "a second press takes the list back off the screen: {hidden:?}"
        );
    }

    #[test]
    fn the_quit_key_stops_the_run_and_a_turn_of_no_key_stops_nothing() {
        let mut screen = table(&[&[], &[Command::Quit]]);
        assert!(
            !screen.poll(),
            "a turn that took no key asks for no stop of the run"
        );
        assert!(screen.poll(), "the quit key asks for the stop of the run");
    }

    #[test]
    fn a_turn_that_took_no_key_draws_nothing() {
        // The run loop polls ten times each second. A poll that drew every
        // turn would clear the screen and paint it again ten times each
        // second, and the table would flicker for nothing.
        let mut screen = table_at(temp_path("no-key"), NoKeys);
        screen.round(&one_round());
        let drawn = screen.sink.len();

        screen.poll();
        assert_eq!(
            screen.sink.len(),
            drawn,
            "a turn of no key leaves the frame that stands: {:?}",
            painted(&screen.sink)
        );
    }

    #[test]
    fn the_header_names_the_size_of_the_recorded_file_at_the_moment_of_the_draw() {
        let file = TempFile::absent("size");
        file.of(FIRST_BYTES);
        let mut screen = table_at(file.path().to_owned(), FakeKeys::of(&[]));

        screen.round(&one_round());
        let first = painted(&screen.sink);
        assert!(
            first
                .first()
                .is_some_and(|line| line.ends_with(&size_of(FIRST_BYTES))),
            "the header line names the size that the file holds: {first:?}"
        );

        // The run appends one record for each round, so the file grows while
        // the table stands. A header that measured the file one time would
        // name the size of an empty file for the whole of a long run.
        file.of(SECOND_BYTES);
        screen.sink.clear();
        screen.round(&one_round());
        let second = painted(&screen.sink);
        assert!(
            second
                .first()
                .is_some_and(|line| line.ends_with(&size_of(SECOND_BYTES))),
            "and the next draw names the size that the file holds then: {second:?}"
        );
    }

    #[test]
    fn the_clock_of_a_run_reads_the_moment_now() {
        let before = Instant::now();
        let read = SystemClock.now();
        let after = Instant::now();
        assert!(
            read >= before && read <= after,
            "the clock of a run reads the clock of the machine"
        );
    }

    #[test]
    fn a_round_that_answered_two_hops_and_reached_the_target_prints_one_line() {
        assert_eq!(status_line(&a_whole_path_round()), A_WHOLE_PATH_LINE);
    }

    /// One hop keeps the singular name, and a round that reached nothing says
    /// so.
    #[test]
    fn a_round_that_answered_one_hop_and_reached_nothing_prints_the_singular_name() {
        assert_eq!(status_line(&a_lost_round()), A_LOST_ROUND_LINE);
    }

    #[test]
    fn a_headless_screen_prints_a_line_at_its_first_round() {
        // A run that printed nothing for its first minute reads as a run that
        // died, so the first round always prints.
        let clock = FakeClock::new();
        let mut screen = headless(&clock);

        screen.round(&a_whole_path_round());
        assert_eq!(
            printed(&screen.sink),
            [A_WHOLE_PATH_LINE],
            "the first round of a run puts its status line on the screen"
        );
    }

    #[test]
    fn a_headless_screen_prints_no_second_line_inside_one_minute() {
        // A run of one round each second prints one line each second, and an
        // hour of such a run fills a terminal with the same line 3600 times.
        let clock = FakeClock::new();
        let mut screen = headless(&clock);

        for _ in 0..ROUNDS_INSIDE_A_MINUTE {
            screen.round(&a_whole_path_round());
            clock.advance(ROUND_STEP);
        }
        assert_eq!(
            printed(&screen.sink),
            [A_WHOLE_PATH_LINE],
            "the rounds that follow the first one inside the minute print nothing"
        );
    }

    #[test]
    fn a_headless_screen_prints_a_second_line_after_a_minute() {
        // The line of each minute is what says that the run still stands, and
        // it names the round that the file holds now.
        let clock = FakeClock::new();
        let mut screen = headless(&clock);

        screen.round(&a_whole_path_round());
        clock.advance(ONE_MINUTE);
        screen.round(&a_lost_round());
        assert_eq!(
            printed(&screen.sink),
            [A_WHOLE_PATH_LINE, A_LOST_ROUND_LINE],
            "the first round of the next minute puts its own line on the screen"
        );
    }

    #[test]
    fn a_headless_screen_reads_no_key_and_stops_nothing() {
        // A headless run holds no terminal, so no key of it reaches this
        // process. The signal handler of `main.rs` stops such a run, and the
        // limits of the command line stop it too.
        let clock = FakeClock::new();
        let mut screen = headless(&clock);

        assert!(
            !screen.poll(),
            "a turn of a headless screen asks for no stop of the run"
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

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
use crate::ui::render_duration;
use crate::{counted, HOP, NEVER_REACHED, REACHED, ROUND, ROW, SUMMARY_SEPARATOR};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, queue};
use std::collections::BTreeMap;
use std::io::Write;
use std::net::IpAddr;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
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

/// The key that shows the list of the keys, and hides it again.
///
/// The long help of `krt` names this key, and it names no other key of the
/// table, because a doc comment of `clap` is a string literal that reads
/// [`KEYS`] at no time. This name therefore stands here, beside the one list,
/// and `the_long_help_names_the_key_that_lists_the_keys` holds the two of them
/// together.
const KEY_THAT_LISTS_THE_KEYS: &str = "?";

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
    (KEY_THAT_LISTS_THE_KEYS, "show these keys, or hide them"),
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

/// The mark that opens the line which counts the rows that a frame left out.
///
/// The mark reads as "and more of the same". The rows behind it are rows of the
/// table above it, and the count says how many of them the window does not
/// hold.
const MORE_MARK: &str = "+";

/// The lines of one frame that a window holds, and the number of rows of the
/// path that reached that frame.
///
/// The count travels with the lines, because a caller that draws an image over
/// one row of the path must draw no image for a row that went out of the frame.
/// Such an image would stand over the footer of the frame, or over a line of
/// another frame, and nothing on the screen would say which row it belongs to.
/// A caller that counted the rows itself would count the rows of the table and
/// not the rows of the window.
struct Fitted {
    /// The lines of the frame, head first.
    lines: Vec<String>,
    /// The number of rows of the path that the frame holds.
    rows: usize,
}

/// The lines of one frame that fit a window of `rows` rows.
///
/// The head and the footer stand at every height. The head names the
/// destination, the count of the rounds, the size of the recorded file, and
/// every column of the table, and the footer holds the mark of the pause and
/// the list of the keys, which are what a key press of the reader asked for.
/// The rows of the path take the lines that those two leave, and one line then
/// says how many rows went out of the frame.
///
/// The frame drops the TTLs farthest from the source, which is the end that
/// `traceroute` and `mtr` drop as well. The numbering of the rows then starts
/// under the column header and runs down the window, so a reader of a short
/// frame still reads the hops that answer first.
///
/// A window that no probe measured holds every line. Such a run knows no height
/// to fit a frame to, and the terminal decides what it shows.
///
/// The last cut holds a window too short even for the head and the footer
/// together. A frame that ran past the foot of the window would scroll the
/// window by the lines that ran past it, and the head of the frame goes off the
/// top of an alternate screen that keeps no scrollback.
fn fitted(head: Vec<String>, body: Vec<String>, footer: Vec<String>, rows: Option<u16>) -> Fitted {
    let budget = rows.map_or(usize::MAX, usize::from);
    let mut lines = head;
    if lines.len() + body.len() + footer.len() <= budget {
        lines.extend(body);
        lines.extend(footer);
        return Fitted { lines, rows: 0 };
    }
    // The head, the footer, and the one line that counts the rows which went
    // out of the frame keep their lines. The rows of the path take what is
    // left.
    let kept = budget.saturating_sub(lines.len() + footer.len() + 1);
    let dropped = body.len() - kept.min(body.len());
    lines.extend(body.into_iter().take(kept));
    // A table of no row leaves no row out, and a line that counted zero rows
    // would name a table the reader can already see the whole of.
    if dropped > 0 {
        lines.push(format!("{MORE_MARK}{}", counted(dropped, ROW)));
    }
    lines.extend(footer);
    lines.truncate(budget);
    Fitted { lines, rows: 0 }
}

/// The text between two lines that a draw writes.
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
///
/// No run of the tool builds this source. A run that draws the table holds a
/// terminal and reads the keys of it, and a run that holds no terminal draws no
/// table and takes no key source at all. The source is therefore a part of a
/// test build and no part of a build that ships, and the tests of a turn that
/// took no key are what read it.
#[cfg(test)]
pub(crate) struct NoKeys;

#[cfg(test)]
impl Keys for NoKeys {
    fn presses(&mut self) -> Vec<Command> {
        Vec::new()
    }
}

/// The key source of a run that holds a terminal.
///
/// The source takes every key event that already arrived, and it waits for
/// none. The run loop asks ten times each second, and an ask that waited for a
/// key holds the recording where it stands until the user presses one.
///
/// No test of this file builds this source. `crossterm` reads the keys of a
/// terminal, and `cargo test` hands the test binary a pipe. A source that read
/// the keys of the terminal of `cargo test` takes the keys of the reader who
/// started it. The pseudo terminal of `tests/terminal.rs` is what drives this
/// source.
pub(crate) struct Keyboard;

impl Keys for Keyboard {
    fn presses(&mut self) -> Vec<Command> {
        let mut commands = Vec::new();
        // A poll of no time answers whether an event stands ready now, and it
        // waits for no event to arrive.
        //
        // A poll that fails, and a read that fails, end the gathering and leave
        // the commands that the gathering already holds. A terminal that will
        // not answer is no reason to stop the recording, which is the purpose
        // of the tool.
        while event::poll(Duration::ZERO).unwrap_or(false) {
            let Ok(arrived) = event::read() else {
                break;
            };
            // A resize of the window and a paste of text arrive at this same
            // reader, and neither of them is a key. The gathering goes on past
            // each of them, because the events behind one of them still hold
            // the keys of the user.
            if let Event::Key(key) = arrived {
                commands.extend(classify(key));
            }
        }
        commands
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

/// The window that a live table draws its frames in.
///
/// The two numbers travel as one value, because the table reads both of them
/// for one purpose: a frame stands in the window of the terminal, and a frame
/// that ran past either edge of that window would lose the part that ran past
/// it.
pub(crate) struct Window {
    /// The number of terminal columns that a frame draws in.
    columns: u16,
    /// The number of rows that the window holds, and `None` for a window that
    /// no probe measured.
    rows: Option<u16>,
}

impl Window {
    /// A window of these columns, and of these rows.
    ///
    /// `rows` is `None` for a window that no probe measured. Such a window
    /// holds every line of a frame, which is the rule of the columns applied to
    /// the height: a run that cannot measure the terminal draws the whole frame
    /// and lets the terminal decide what it shows.
    pub(crate) fn new(columns: u16, rows: Option<u16>) -> Self {
        Self { columns, rows }
    }
}

/// The live table of a run.
///
/// The table folds every round that arrives, and it draws the frame of that
/// fold. It also draws one frame at the moment it is built, in front of every
/// round, for the reason that [`Table::new`] states. A run that draws this
/// table holds the terminal in raw mode on the alternate screen, so each draw
/// clears the screen and moves the cursor to the origin first, and one carriage
/// return with one line feed stands between two lines of a frame. Raw mode
/// returns no carriage on a bare line feed, and a frame of bare line feeds
/// walks down the screen one column further to the right for each line of it.
///
/// The frame fits the window of the terminal. A frame of more lines than that
/// window holds keeps its head and its footer and drops the rows of the highest
/// TTLs, because a frame that ran past the foot of the window scrolls its own
/// header line off the top of an alternate screen that keeps no scrollback.
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
    /// The window that the frames draw in.
    window: Window,
    /// Whether the frames carry the color of a terminal.
    ///
    /// The reader of the terminal decides, and the caller reads that answer
    /// off the environment of the run. A reader who set `NO_COLOR` gets the
    /// frames with glyphs alone.
    paint: ui::Paint,
}

impl<W: Write, K: Keys> Table<W, K> {
    /// A table of one run, which draws into `sink` and reads `keys`.
    ///
    /// The names start on. A reader who wants the raw addresses asks for them
    /// with a key, and a run that resolves no name shows the addresses anyway,
    /// because the map of the names then stays empty.
    ///
    /// The table draws its opening frame here, and not at the first round. The
    /// caller takes the terminal in front of this call, so the alternate screen
    /// of the run already stands in front of the reader, and that screen hides
    /// every line which the run printed under it. A table that drew at the
    /// first round alone therefore leaves an empty screen for one whole
    /// period, and `--interval 2m` makes that two minutes with nothing on the
    /// screen that says the run started. The frame of no round answers it: the
    /// header line of such a frame names the destination, the address, the
    /// source, the period of one round, and the recorded file, and the column
    /// header stands under it.
    ///
    /// A table that exists holds a frame, and no caller can forget to ask for
    /// one.
    ///
    /// `paint` says whether the frames carry the color of a terminal. The
    /// caller reads that answer off the environment of the run, because a
    /// reader who wants no color says so with `NO_COLOR`.
    pub(crate) fn new(facts: RunFacts, sink: W, keys: K, window: Window, paint: ui::Paint) -> Self {
        let mut table = Self {
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
            window,
            paint,
        };
        table.draw();
        table
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
    ///
    /// The lines then fit the window, in the three parts that [`fitted`] takes:
    /// the head of the frame, the rows of the path, and that footer.
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
        let mut body = frame.lines(self.window.columns, self.paint);
        // The head comes off the front of the frame, because a frame that the
        // window does not hold keeps the head and drops rows of the path. A
        // frame holds those lines at every width, and the `min` says so anyway.
        let head: Vec<String> = body
            .drain(..usize::from(ui::HEAD_LINES).min(body.len()))
            .collect();
        fitted(head, body, self.footer(), self.window.rows).lines
    }

    /// The lines that stand under the rows of the path.
    ///
    /// The mark of the pause comes first, and the list of the keys comes under
    /// it. One blank line holds each of them off the lines above.
    fn footer(&self) -> Vec<String> {
        let mut lines = Vec::new();
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
    /// The whole frame stands in one buffer, and that buffer reaches the sink
    /// in one call. The sink of a live run is `std::io::Stdout`, which is a
    /// `LineWriter`, and such a writer flushes at every line feed. One call for
    /// each line of the frame is therefore one `write(2)` for each line of it,
    /// and the terminal draws what arrives, so the reader sees a half-drawn
    /// frame between two of those writes.
    ///
    /// One line end stands between two lines, and no line end closes the last
    /// line. A line end on the last row of the window scrolls the window by one
    /// line, so a frame of as many lines as the window holds would push its own
    /// header line off the top of a screen that keeps no scrollback. The
    /// position of the cursor between two draws says nothing, because every
    /// draw clears the screen and moves the cursor to the origin first.
    ///
    /// # Errors
    ///
    /// Answers the fault that the sink raised. The caller of this function
    /// drops that fault, for the reason that [`Table::draw`] states.
    fn paint(&mut self, lines: &[String]) -> std::io::Result<()> {
        let mut frame: Vec<u8> = Vec::new();
        queue!(frame, Clear(ClearType::All), MoveTo(0, 0))?;
        write!(frame, "{}", lines.join(LINE_END))?;
        self.sink.write_all(&frame)?;
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
const LINE_PERIOD: Duration = Duration::from_mins(1);

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
        // A name belongs to a row of a table, and this screen draws no table.
        // The run writes every name record to the recorded file before it
        // reaches this call, so a replay of that file names each address that
        // a lookup answered.
    }
}

/// A panic hook, of the kind that [`std::panic::take_hook`] answers.
///
/// The hook stands in an [`Arc`], because two holders read the one hook: the
/// hook that a live run installs chains to it, and the guard of that run puts
/// it back.
type PanicHook = Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send>;

/// The number of times that this process put the terminal back.
///
/// A test holds no terminal, so it reads nothing of what a restoration writes.
/// This count is what such a test reads in the place of that, and it says
/// whether the panic hook of a live run still stands. Only the tests of that
/// hook read the count, and they hold one lock while they do, so no two
/// readers of it race. The count is a part of a test build and no part of a
/// build that ships.
#[cfg(test)]
static RESTORATIONS: AtomicUsize = AtomicUsize::new(0);

/// Puts the terminal back the way it stood before a live run took it.
///
/// Each step stands on its own, and a step that fails leaves the steps after it
/// alone: a terminal that took one part of the entry and refused the next one
/// must come all the way back anyway. The function also runs two times on the
/// path of a panic, because the hook of the panic runs it and the unwind then
/// drops the guard, and each of those two runs is safe.
fn restore_terminal() {
    #[cfg(test)]
    RESTORATIONS.fetch_add(1, Ordering::SeqCst);
    drop(disable_raw_mode());
    drop(execute!(std::io::stdout(), Show, LeaveAlternateScreen));
}

/// Installs the hook that puts the terminal back before a panic message prints,
/// and answers the hook that stood before it.
///
/// A live run holds the terminal in raw mode on the alternate screen. A panic
/// of such a run prints its message on that alternate screen, and the process
/// then dies and takes the alternate screen away with it. The reader of that
/// terminal reads no message at all, and a raw terminal is what stays in front
/// of them. The hook therefore puts the terminal back first, and the message of
/// the panic then lands on the screen that the reader keeps.
///
/// The hook chains, and it puts the terminal back in front of that chain. The
/// hook of the machine prints the message of a panic, and a test binary holds a
/// hook that collects one. A hook that replaced either of them takes the report
/// of every panic away from the reader who needs it most.
///
/// # Panics
///
/// Panics on a thread that is already panicking, because
/// `std::panic::take_hook` refuses such a thread. The guard of a live run takes
/// the terminal at the start of that run, where no panic of the run stands.
/// [`restore_panic_hook`] reads the thread for the same rule, because the drop
/// of that guard does stand on the path of a panic.
fn install_panic_hook() -> PanicHook {
    let previous: PanicHook = Arc::from(std::panic::take_hook());
    let chained = Arc::clone(&previous);
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        (*chained)(info);
    }));
    previous
}

/// Puts a hook back.
///
/// The hook that [`install_panic_hook`] answers goes back here, at the end of
/// the live run that installed it. The hook of a live run puts a terminal back,
/// and no part of the process that follows that run holds one, so a hook that
/// outlived its run acts on a panic that it knows nothing about.
///
/// A thread that panics puts no hook back. `std::panic::set_hook` refuses such
/// a thread, and it refuses with a panic of its own, which aborts the process
/// on the spot. The guard of a live run reaches this call on the path of every
/// panic of that run, and the hook of the live run already put the terminal
/// back by then. That hook therefore stands on for the little of the process
/// that is left, and the message of the panic reaches the reader.
fn restore_panic_hook(previous: PanicHook) {
    if std::thread::panicking() {
        return;
    }
    std::panic::set_hook(Box::new(move |info| (*previous)(info)));
}

/// The hold of a live run on the terminal of that run.
///
/// The guard takes the terminal at the start of the live run and puts it back
/// at the end. It is a local of the run, so every way out of that run gives the
/// terminal back: the stop that the user asked for, the fault that ends the run
/// early, and the panic that nobody asked for.
///
/// No test of this file builds this guard. `crossterm` takes raw mode of the
/// terminal of the process, and a test that took the terminal of `cargo test`
/// takes the terminal of the reader who started it. The pseudo terminal of
/// `tests/terminal.rs` is what drives the guard. The panic hook of the guard
/// holds no terminal, and the tests above drive that hook on its own.
pub(crate) struct TerminalGuard {
    /// The panic hook that stood before [`TerminalGuard::enter`] wrapped it.
    ///
    /// An `Option`, so the drop moves the hook out of it and back into the
    /// process.
    previous_hook: Option<PanicHook>,
}

impl TerminalGuard {
    /// Takes the terminal for a live run.
    ///
    /// Raw mode sends the bytes of every key straight to this process, which is
    /// what lets a key of the reader reach the run. The alternate screen holds
    /// the frames of the run, so the lines that stood in the terminal come back
    /// at the end of it. The cursor goes away, because a table that redraws
    /// itself ten times each second drags a cursor across itself.
    ///
    /// The panic hook comes last. A panic of a live run prints its message on
    /// the alternate screen in raw mode, and the process then dies and takes
    /// that screen away with it, so the reader reads no message at all. The
    /// hook puts the terminal back in front of the message.
    ///
    /// # Errors
    ///
    /// Answers the fault of a terminal that refused raw mode, or that refused
    /// the alternate screen. A terminal that refused the second of the two
    /// comes all the way back before the fault answers, so a run that stops
    /// here leaves no terminal in raw mode.
    pub(crate) fn enter() -> std::io::Result<Self> {
        enable_raw_mode()?;
        if let Err(fault) = execute!(std::io::stdout(), EnterAlternateScreen, Hide) {
            restore_terminal();
            return Err(fault);
        }
        Ok(Self {
            previous_hook: Some(install_panic_hook()),
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
        // The hook of the live run goes off the process with the run that
        // installed it. A hook that stood on holds a terminal that no part of
        // the process holds.
        if let Some(previous) = self.previous_hook.take() {
            restore_panic_hook(previous);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify, install_panic_hook, restore_panic_hook, status_line, Clock, Command, Headless,
        Keys, NoKeys, PanicHook, RunFacts, Screen, SystemClock, Table, Window,
        KEY_THAT_LISTS_THE_KEYS,
    };
    use crate::record::{NameRecord, RoundRecord, RunId};
    use crate::testing::{address, round, FakeKeys};
    use crate::ui::Paint;
    use chrono::Utc;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use std::cell::Cell;
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
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

    /// The rows of a window that no probe measured.
    ///
    /// Such a window fits a frame to no height, so the table draws every line
    /// of it. Every table below takes this window, and the tests of the height
    /// name a window of their own.
    const NO_ROWS: Option<u16> = None;

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

    /// The mark that the Recent column draws for a probe that no hop answered.
    ///
    /// The test spells the glyph, and `ui.rs` spells it again. That is on
    /// purpose, as the word of the pause is: a test that read the constant of
    /// that module would agree with every mark the module ever holds, and this
    /// mark is what a reader of the table sees.
    const NO_ANSWER: &str = "╳";

    /// The control sequence that paints what follows it red.
    const RED: &str = "\u{1b}[31m";

    /// The control sequence that gives the foreground of the terminal back.
    const PLAIN: &str = "\u{1b}[39m";

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

    /// The facts of a run whose recorded file stands at `path`.
    ///
    /// Every table below heads its frames with these facts, and each of those
    /// tables holds a sink of its own, so the facts stand apart from the sink.
    fn facts_at(path: PathBuf) -> RunFacts {
        RunFacts {
            destination: DESTINATION.to_owned(),
            address: address(DESTINATION_ADDRESS),
            source: address(SOURCE),
            interval: INTERVAL,
            path,
        }
    }

    /// A table that draws into bytes, reads `keys`, and heads its frames with
    /// the recorded file at `path`.
    ///
    /// The sink comes back empty. A table draws its opening frame at the moment
    /// it is built, and every test below reads the frames that its own calls
    /// drew.
    fn table_at<K: Keys>(path: PathBuf, keys: K) -> Table<Vec<u8>, K> {
        let mut table = Table::new(
            facts_at(path),
            Vec::new(),
            keys,
            Window::new(WIDTH, NO_ROWS),
            Paint::Colored,
        );
        table.sink.clear();
        table
    }

    /// A sink that counts the calls of [`Write::write`] that reach it.
    ///
    /// A test holds no terminal, so it reads nothing of how many times a draw
    /// crossed into the kernel. This count is what such a test reads in the
    /// place of that. The sink keeps none of the bytes, because the tests that
    /// read the glyphs of a frame draw into bytes above.
    struct Counted {
        /// The number of calls that reached this sink.
        writes: usize,
    }

    impl Write for Counted {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.writes += 1;
            // The whole buffer counts as taken, so `write_all` hands one
            // buffer over in one call. A sink that took less of it would count
            // the calls of its own refusal.
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A table that draws into a sink which counts the calls, and that takes
    /// the keys of an empty script.
    ///
    /// The count comes back at zero. A table draws its opening frame at the
    /// moment it is built, and the test below counts the calls of one draw.
    fn counted_table() -> Table<Counted, FakeKeys> {
        let mut table = Table::new(
            facts_at(temp_path("writes")),
            Counted { writes: 0 },
            FakeKeys::of(&[]),
            Window::new(WIDTH, NO_ROWS),
            Paint::Colored,
        );
        table.sink.writes = 0;
        table
    }

    /// A table that draws into bytes and takes the keys of a script.
    ///
    /// The path of its recorded file names no file, so the header line of it
    /// names no size. The one test that reads a size writes a file of its own.
    fn table(script: &[&[Command]]) -> Table<Vec<u8>, FakeKeys> {
        table_at(temp_path("frame"), FakeKeys::of(script))
    }

    /// A table that draws into bytes with no color at all, and that reads no
    /// key.
    ///
    /// This is the table of a reader who set `NO_COLOR`. It folds the same
    /// rounds as the table above, and it writes the glyphs of them alone.
    ///
    /// The sink comes back empty, for the reason that [`table_at`] states.
    fn plain_table() -> Table<Vec<u8>, FakeKeys> {
        let mut table = Table::new(
            facts_at(temp_path("plain")),
            Vec::new(),
            FakeKeys::of(&[]),
            Window::new(WIDTH, NO_ROWS),
            Paint::Plain,
        );
        table.sink.clear();
        table
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

    /// One round of the same TTL that no hop answered.
    fn one_lost_round() -> RoundRecord {
        round(TTL, TTL, &[])
    }

    /// The number of lines that stand above the rows of the path: the header
    /// line, the blank line under it, and the column header.
    ///
    /// The test spells the count, and the module reads it off the two
    /// constants of `ui.rs`. That is on purpose, as the word of the pause is:
    /// these three lines are what a reader of a short frame keeps.
    const HEAD_LINES: usize = 3;

    /// The start of the column header of every frame.
    ///
    /// The TTL column and the Host column never drop, so this text starts the
    /// column header at every width.
    const COLUMN_HEADER_START: &str = " TTL  Host";

    /// The number of columns that the TTL of a row takes, with the one column
    /// that stands to the left of it.
    const TTL_COLUMNS: usize = 4;

    /// The text between two columns of a frame.
    const COLUMN_GAP: &str = "  ";

    /// The number of TTLs of the path that the tall table below folds.
    ///
    /// The head takes three lines and this path takes twenty, so the frame of
    /// it stands inside a window of [`WINDOW_ROWS`] rows on its own, and it
    /// runs past the foot of that window the moment a key asks for the pause
    /// and the list of the keys.
    const TALL_PATH: u8 = 20;

    /// The network of the address that answers at each TTL of that path.
    const TALL_NETWORK: &str = "10.0.0";

    /// The number of rows of a window too short for that path.
    ///
    /// The head takes three of these rows, and the line that counts the rows
    /// which went out of the frame takes one, so six rows of the path stand.
    const SHORT_ROWS: u16 = 10;

    /// The last TTL of the tall path that a window of [`SHORT_ROWS`] rows
    /// holds.
    const LAST_KEPT_TTL: u8 = 6;

    /// The first TTL of that path that such a window leaves out.
    const FIRST_DROPPED_TTL: u8 = 7;

    /// The line that closes the rows of a frame in a window of [`SHORT_ROWS`]
    /// rows.
    ///
    /// Six of the twenty rows of the path stand, so fourteen of them go out of
    /// the frame.
    ///
    /// The test spells the line, and the module builds the same line out of a
    /// count and the name of a row. That is on purpose, as the word of the
    /// pause is: this line is what a reader of the screen sees.
    const DROPPED_LINE: &str = "+14 rows";

    /// The number of rows of the window of a common terminal.
    ///
    /// The head and the tall path take 23 of these 24 rows. The pause and the
    /// list of the keys take eight lines more, so a frame that carries both of
    /// them runs past the foot of this window.
    const WINDOW_ROWS: u16 = 24;

    /// The line that closes the rows of that crowded frame.
    ///
    /// The head takes three rows, the pause and the list of the keys take
    /// eight, and this line takes one, so twelve of the twenty rows of the path
    /// stand and eight go out of the frame.
    const CROWDED_LINE: &str = "+8 rows";

    /// The mark that opens the line which counts the rows that a frame left
    /// out.
    ///
    /// No other line of a frame opens with it: the header line opens with the
    /// name of the tool, and a row of the path opens with the number of a TTL.
    const MORE_MARK: char = '+';

    /// The address that answers at one TTL of the tall path.
    fn tall_host(ttl: u8) -> String {
        format!("{TALL_NETWORK}.{ttl}")
    }

    /// The start of the row of one TTL of that path: the number of the TTL, and
    /// the address that answered at it.
    fn tall_row(ttl: u8) -> String {
        format!("{ttl:>TTL_COLUMNS$}{COLUMN_GAP}{}", tall_host(ttl))
    }

    /// One round that answered at every TTL of the tall path.
    fn a_tall_round() -> RoundRecord {
        let hosts: Vec<String> = (TTL..=TALL_PATH).map(tall_host).collect();
        let hops: Vec<(u8, &str, f64)> = (TTL..=TALL_PATH)
            .zip(hosts.iter())
            .map(|(ttl, host)| (ttl, host.as_str(), RTT))
            .collect();
        round(TTL, TALL_PATH, &hops)
    }

    /// A table that draws into bytes in a window of `rows` rows, and that takes
    /// the keys of a script.
    ///
    /// The sink comes back empty, for the reason that [`table_at`] states.
    fn table_in(rows: Option<u16>, script: &[&[Command]]) -> Table<Vec<u8>, FakeKeys> {
        let mut table = Table::new(
            facts_at(temp_path("window")),
            Vec::new(),
            FakeKeys::of(script),
            Window::new(WIDTH, rows),
            Paint::Colored,
        );
        table.sink.clear();
        table
    }

    /// A table of the tall path in a window of `rows` rows, which drew the
    /// frame of that path already.
    fn tall_table(rows: Option<u16>, script: &[&[Command]]) -> Table<Vec<u8>, FakeKeys> {
        let mut screen = table_in(rows, script);
        screen.round(&a_tall_round());
        screen
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
    const ONE_MINUTE: Duration = Duration::from_mins(1);

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

    /// The lines that stand above the rows of the path of a fitted frame.
    fn a_head() -> Vec<String> {
        (0..HEAD_LINES).map(|line| format!("head {line}")).collect()
    }

    /// One row of the path for each of `count` TTLs.
    fn a_body(count: usize) -> Vec<String> {
        (0..count).map(|row| format!("row {row}")).collect()
    }

    /// The number of rows of the path that the frames below hold.
    const A_PATH: usize = 8;

    /// The number of rows of a window that holds the whole of that path.
    const A_TALL_WINDOW: u16 = 24;

    /// The number of rows of a window too short for that path.
    ///
    /// The head takes three of these rows and the line that counts the rows
    /// which went out of the frame takes one, so two rows of the path stand.
    const A_SHORT_WINDOW: u16 = 6;

    /// The number of rows of the path that a window of [`A_SHORT_WINDOW`] rows
    /// holds.
    const KEPT_ROWS: usize = 2;

    #[test]
    fn a_frame_that_its_window_holds_whole_keeps_every_row_of_the_path() {
        let fitted = super::fitted(a_head(), a_body(A_PATH), Vec::new(), Some(A_TALL_WINDOW));

        assert_eq!(
            fitted.lines.len(),
            HEAD_LINES + A_PATH,
            "the frame holds its head and every row of the path: {:?}",
            fitted.lines
        );
        assert_eq!(
            fitted.rows, A_PATH,
            "and it says that every row of the path reached it"
        );
    }

    #[test]
    fn a_frame_that_dropped_rows_counts_the_rows_that_reached_it() {
        // A caller that draws an image over one row of the path must draw no
        // image for a row that went out of the frame. Such an image would stand
        // over the footer of the frame, and nothing on the screen would say
        // which row it belongs to.
        let fitted = super::fitted(a_head(), a_body(A_PATH), Vec::new(), Some(A_SHORT_WINDOW));

        assert_eq!(
            fitted.rows, KEPT_ROWS,
            "the count names the rows of the path that the window held: {:?}",
            fitted.lines
        );
    }

    #[test]
    fn a_table_draws_a_frame_of_no_round_at_the_moment_it_is_built() {
        // The run takes the terminal in front of this call, so the alternate
        // screen of that run already stands in front of the reader, and it
        // hides every line which the run printed under it. A table that drew at
        // the first round alone leaves an empty screen for one whole period,
        // and a period of two minutes is two minutes of nothing that says the
        // run started.
        let screen = Table::new(
            facts_at(temp_path("opening")),
            Vec::new(),
            FakeKeys::of(&[]),
            Window::new(WIDTH, NO_ROWS),
            Paint::Colored,
        );
        let lines = painted(&screen.sink);

        assert!(
            lines
                .first()
                .is_some_and(|line| line.starts_with(NO_ROUND_HEADER)),
            "the header line names the target, the source, the interval, and the round that no probe took yet: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with(COLUMN_HEADER_START)),
            "and the column header stands under it: {lines:?}"
        );
    }

    #[test]
    fn a_lost_probe_reaches_the_terminal_in_red() {
        // A live table draws on a terminal: the run holds that terminal in raw
        // mode on the alternate screen in front of the first draw. The loss of
        // a probe is the one thing the table paints, and the color says at a
        // glance which row of the path drops its probes. The table below takes
        // the color, as the table of a reader who set nothing does.
        //
        // The test reads the bytes of the sink and not the glyphs of them,
        // because the codes of the color are what it is about.
        let mut screen = table(&[]);
        screen.round(&one_lost_round());
        let drawn = String::from_utf8_lossy(&screen.sink).into_owned();
        assert!(
            drawn.contains(&format!("{RED}{NO_ANSWER}")),
            "the mark of the lost probe carries the code that paints it red: {drawn:?}"
        );
        assert!(
            drawn.contains(&format!("{NO_ANSWER}{PLAIN}")),
            "and the code that gives the foreground of the terminal back stands behind that mark: {drawn:?}"
        );
    }

    #[test]
    fn a_table_of_no_color_paints_a_lost_probe_with_no_code_at_all() {
        // A reader who set `NO_COLOR` asks every tool for the glyphs alone,
        // and this table is what such a reader gets. The mark of the loss
        // stays, because the mark is no bar of a time, and the codes that
        // paint it red go away.
        //
        // The test reads the bytes of the sink and not the glyphs of them,
        // because the codes of the color are what it is about.
        let mut screen = plain_table();
        screen.round(&one_lost_round());
        let drawn = String::from_utf8_lossy(&screen.sink).into_owned();
        assert!(
            !drawn.contains(RED),
            "no code of the red reaches a table that the reader asked for no color in: {drawn:?}"
        );
        assert!(
            drawn.contains(NO_ANSWER),
            "and the mark of the lost probe stands in that table anyway: {drawn:?}"
        );
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
    fn a_frame_taller_than_the_window_keeps_the_near_hops_and_counts_the_rest() {
        // The alternate screen keeps no scrollback. A frame of more lines than
        // the window holds scrolls its own header line off the top of that
        // window at every draw, and nothing brings the line back.
        let screen = tall_table(Some(SHORT_ROWS), &[]);
        let lines = painted(&screen.sink);

        assert!(
            lines.len() <= usize::from(SHORT_ROWS),
            "the frame stands inside the {SHORT_ROWS} rows of the window: {lines:?}"
        );
        assert!(
            lines
                .first()
                .is_some_and(|line| line.starts_with(ONE_ROUND_HEADER)),
            "the header line stands, so the reader keeps the destination, the round, and the file: {lines:?}"
        );
        assert!(
            lines
                .get(HEAD_LINES - 1)
                .is_some_and(|line| line.starts_with(COLUMN_HEADER_START)),
            "the column header stands under it, so every row that is left reads: {lines:?}"
        );
        for (index, ttl) in (TTL..=LAST_KEPT_TTL).enumerate() {
            assert!(
                lines
                    .get(HEAD_LINES + index)
                    .is_some_and(|line| line.starts_with(&tall_row(ttl))),
                "the hops nearest the source stand under that header, in TTL order: {lines:?}"
            );
        }
        for ttl in FIRST_DROPPED_TTL..=TALL_PATH {
            assert!(
                !lines.iter().any(|line| line.starts_with(&tall_row(ttl))),
                "the TTLs that the window does not hold go out of the frame: {lines:?}"
            );
        }
        assert_eq!(
            last_lines(&lines, 1),
            [DROPPED_LINE],
            "and one line says how many rows the frame left out: {lines:?}"
        );
    }

    #[test]
    fn a_window_that_no_probe_measured_leaves_every_row_standing() {
        // A run that cannot measure the terminal knows no height to fit a
        // frame to. It draws the whole frame and lets the terminal decide what
        // it shows, which is the rule of the columns applied to the height.
        let screen = tall_table(NO_ROWS, &[]);
        let lines = painted(&screen.sink);

        for ttl in TTL..=TALL_PATH {
            assert!(
                lines.iter().any(|line| line.starts_with(&tall_row(ttl))),
                "the row of TTL {ttl} stands: {lines:?}"
            );
        }
        assert!(
            !lines.iter().any(|line| line.starts_with(MORE_MARK)),
            "and no line counts a row that went out of the frame: {lines:?}"
        );
    }

    #[test]
    fn a_frame_that_fits_its_window_counts_no_row() {
        let mut screen = table_in(Some(WINDOW_ROWS), &[]);
        screen.round(&one_round());
        let lines = painted(&screen.sink);

        assert!(
            lines.iter().any(|line| line == ONE_ROUND_ROW),
            "the one row of the path stands: {lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.starts_with(MORE_MARK)),
            "and a frame that the window holds whole leaves nothing out: {lines:?}"
        );
    }

    #[test]
    fn a_clamped_frame_keeps_the_pause_and_the_list_of_the_keys() {
        // The two of them are what the reader asked for with a key. A frame
        // that dropped them answers that key press with nothing.
        let mut screen = tall_table(Some(WINDOW_ROWS), &[&[Command::Pause, Command::Help]]);
        screen.sink.clear();

        screen.poll();
        let lines = painted(&screen.sink);
        let mut wanted = vec!["", PAUSED, ""];
        wanted.extend(HELP_LINES);
        assert_eq!(
            last_lines(&lines, wanted.len()),
            wanted,
            "the mark of the pause and the list of the keys close the frame: {lines:?}"
        );
        assert!(
            lines.len() <= usize::from(WINDOW_ROWS),
            "the frame stands inside the {WINDOW_ROWS} rows of the window: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line == CROWDED_LINE),
            "and the rows of the path give up the lines that those two take: {lines:?}"
        );
    }

    #[test]
    fn a_frame_writes_no_line_end_under_its_last_line() {
        // A frame of as many lines as the window holds ends with a line feed
        // on the last row of that window, and the terminal then scrolls the
        // whole window by one line. The header line goes off the top of an
        // alternate screen that keeps no scrollback.
        let mut screen = table(&[]);
        screen.round(&one_round());
        let text = glyphs(&screen.sink);
        let lines = painted(&screen.sink);

        assert!(
            !text.ends_with(super::LINE_END),
            "no line end closes the last line of a frame: {text:?}"
        );
        assert_eq!(
            text.matches(super::LINE_END).count(),
            lines.len().saturating_sub(1),
            "and one line end stands between two lines of it: {lines:?}"
        );
    }

    #[test]
    fn one_frame_of_the_live_table_reaches_the_sink_in_one_write() {
        // The sink of a live run is `std::io::Stdout`, which is a
        // `LineWriter`. Such a writer flushes at every line feed, so one call
        // for each line of a frame is one `write(2)` for each line of it. The
        // terminal draws what arrives, and the reader then sees a half-drawn
        // frame between two of those writes.
        let mut screen = counted_table();
        screen.round(&one_round());
        assert_eq!(
            screen.sink.writes, 1,
            "the whole frame reaches the sink in one call"
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

    /// The long help of `krt` names the key that opens the list of the keys.
    ///
    /// [`KEYS`] is the one list that says what a key does, and a doc comment
    /// of `clap` is a string literal that reads that list at no time. The long
    /// help therefore writes the name of one key by hand. This test holds that
    /// name and the list together: a list that gives the work to another key
    /// fails here, and no reader of the help then presses a key which does
    /// nothing.
    #[test]
    fn the_long_help_names_the_key_that_lists_the_keys() {
        use clap::CommandFactory;

        let help = crate::Cli::command().render_long_help().to_string();
        let name = format!("`{KEY_THAT_LISTS_THE_KEYS}`");
        assert!(
            help.contains(&name),
            "the long help names the {name} key, which opens the list of the keys: {help}"
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
    fn a_headless_screen_shows_no_name() {
        // A name belongs to a row of a table, and a headless run draws no
        // table. The name record reaches the recorded file whatever the screen
        // does, so a replay of that file names every address that a lookup
        // answered.
        let clock = FakeClock::new();
        let mut screen = headless(&clock);
        screen.round(&a_whole_path_round());
        screen.sink.clear();

        screen.names(&[name(ROUTER, ROUTER_NAME)]);
        assert!(
            screen.sink.is_empty(),
            "the names that arrived print nothing: {:?}",
            printed(&screen.sink)
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

    /// The lock of every test that changes the panic hook.
    ///
    /// The panic hook is one setting of the whole process, and `cargo test`
    /// runs the tests of one binary on many threads. A test that set the hook
    /// while another test held it takes the hook of that test away, and that
    /// test then reads the answer of a hook it never installed. The lock keeps
    /// the tests below apart from each other, and it keeps each of them apart
    /// from a second run of itself. The count of the restorations reads true
    /// under the same lock, for the same reason.
    static HOOK_LOCK: Mutex<()> = Mutex::new(());

    /// The message of the panic that each test of the hook raises.
    const THE_TEST_PANIC: &str = "the panic that a test of the hook raises";

    /// Holds the panic hook of the process while one test changes it.
    ///
    /// The guard takes the lock of the hook, and it takes the hook that stood
    /// in front of the test. The drop puts that hook back and lets the lock go
    /// after it.
    ///
    /// Each test below reads its answers into locals and drops this guard
    /// before it asserts on them. `std::panic::set_hook` refuses a thread that
    /// is panicking, and it refuses with a panic of its own, which aborts the
    /// process and takes the report of every other test with it. A failed
    /// assertion of a test that still held the guard is such a thread.
    struct HookGuard {
        /// The hook that stood before the test. An `Option`, so the drop moves
        /// the hook out of it.
        previous: Option<PanicHook>,
        /// The lock, which the drop lets go last of all.
        _lock: MutexGuard<'static, ()>,
    }

    impl HookGuard {
        /// Takes the lock, and the hook that stands now.
        ///
        /// A poisoned lock still guards. The state under it is the panic hook
        /// of the process, and this guard puts a whole hook back whatever the
        /// test in front of it did.
        fn take() -> Self {
            let lock = HOOK_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
            Self {
                previous: Some(Arc::from(std::panic::take_hook())),
                _lock: lock,
            }
        }
    }

    impl Drop for HookGuard {
        fn drop(&mut self) {
            // The panic that no test asked for reaches this drop while the
            // thread panics, and a hook that goes back there aborts the
            // process. The hook of the test stands on for that run, which
            // costs the report of a panic and keeps the report of every test.
            if std::thread::panicking() {
                return;
            }
            if let Some(previous) = self.previous.take() {
                std::panic::set_hook(Box::new(move |info| (*previous)(info)));
            }
        }
    }

    /// What the hook of a test read.
    struct Marked {
        /// Whether the hook read a panic.
        read: Arc<AtomicBool>,
        /// Whether the terminal came back before the hook read that panic.
        restored_first: Arc<AtomicBool>,
    }

    /// The number of times that this process put the terminal back.
    fn restorations() -> usize {
        super::RESTORATIONS.load(Ordering::SeqCst)
    }

    /// Installs the hook of a test, and answers what that hook reads.
    ///
    /// The hook prints nothing. Each test below raises a panic on purpose, and
    /// the hook of the machine prints the message of every panic it reads, so a
    /// test that left that hook standing writes the message of a panic that the
    /// test asked for.
    fn mark() -> Marked {
        let read = Arc::new(AtomicBool::new(false));
        let restored_first = Arc::new(AtomicBool::new(false));
        let read_in_hook = Arc::clone(&read);
        let restored_in_hook = Arc::clone(&restored_first);
        let before = restorations();
        std::panic::set_hook(Box::new(move |_info| {
            restored_in_hook.store(restorations() > before, Ordering::SeqCst);
            read_in_hook.store(true, Ordering::SeqCst);
        }));
        Marked {
            read,
            restored_first,
        }
    }

    #[test]
    fn the_panic_hook_of_a_live_run_chains_to_the_hook_it_replaced() {
        // A hook that replaced the hook of the process takes the report of
        // every panic away from the reader of it. The hook of a live run
        // therefore puts the terminal back and hands the panic on.
        let hooks = HookGuard::take();
        let marked = mark();

        // The guard of the test puts a whole hook back, so the answer of the
        // installation goes nowhere here. The test below reads that answer.
        drop(install_panic_hook());
        let outcome = std::panic::catch_unwind(|| panic!("{THE_TEST_PANIC}"));
        let read = marked.read.load(Ordering::SeqCst);
        let restored_first = marked.restored_first.load(Ordering::SeqCst);
        drop(hooks);

        assert!(outcome.is_err(), "the body of the test raised its panic");
        assert!(
            read,
            "the hook of a live run chains to the hook that stood before it"
        );
        assert!(
            restored_first,
            "and it puts the terminal back first, so the message of the panic lands on the screen that the reader keeps"
        );
    }

    #[test]
    fn a_live_run_puts_back_the_hook_that_stood_before_it() {
        // The panic hook is one setting of the whole process. A live run that
        // ended and left its own hook standing puts a terminal back for every
        // panic of every part of the process that follows it, and no part of
        // that process holds a terminal.
        let hooks = HookGuard::take();
        let marked = mark();

        let previous = install_panic_hook();
        restore_panic_hook(previous);
        let before = restorations();
        let outcome = std::panic::catch_unwind(|| panic!("{THE_TEST_PANIC}"));
        let read = marked.read.load(Ordering::SeqCst);
        let after = restorations();
        drop(hooks);

        assert!(outcome.is_err(), "the body of the test raised its panic");
        assert!(
            read,
            "the hook that stood in front of the live run reads the panic on its own"
        );
        assert_eq!(
            after, before,
            "and the hook of the live run puts back no terminal, because that hook no longer stands"
        );
    }

    /// Puts a hook back on the way out, as the guard of a live run does.
    struct Restorer {
        /// The hook to put back. An `Option`, so the drop moves it out.
        previous: Option<PanicHook>,
        /// Whether the restoration raised a panic of its own.
        refused: Arc<AtomicBool>,
    }

    impl Drop for Restorer {
        fn drop(&mut self) {
            let Some(previous) = self.previous.take() else {
                return;
            };
            // This drop runs while the thread panics, and a panic of the
            // restoration on that path ends the whole process. The catch holds
            // such a panic long enough for the test to read that it happened.
            // The test asserts that there is nothing to catch.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                restore_panic_hook(previous);
            }));
            self.refused.store(outcome.is_err(), Ordering::SeqCst);
        }
    }

    #[test]
    fn a_live_run_that_panics_puts_back_no_hook_and_the_process_lives() {
        // `std::panic::set_hook` refuses a thread that panics, and it refuses
        // with a panic of its own. A second panic on the path of a first one
        // aborts the process. The guard of a live run reaches its drop on the
        // path of every panic of that run, so a restoration that reads the
        // thread of no such panic turns each of them into an abort. The hook
        // of the live run already put the terminal back by then, and the
        // message of the panic is what the reader waits for.
        let hooks = HookGuard::take();
        let marked = mark();
        let refused = Arc::new(AtomicBool::new(false));
        let refused_in_run = Arc::clone(&refused);

        let outcome = std::panic::catch_unwind(move || {
            let _restorer = Restorer {
                previous: Some(install_panic_hook()),
                refused: refused_in_run,
            };
            panic!("{THE_TEST_PANIC}");
        });
        let read = marked.read.load(Ordering::SeqCst);
        let refused_now = refused.load(Ordering::SeqCst);
        drop(hooks);

        assert!(outcome.is_err(), "the body of the test raised its panic");
        assert!(
            read,
            "the hook of the live run read that panic and handed it on"
        );
        assert!(
            !refused_now,
            "and the restoration of the hook raises no panic of its own on the path of a panic"
        );
    }
}

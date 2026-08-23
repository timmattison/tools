//! Black-box coverage for the parts of `krt` that a pipe cannot reach.
//!
//! `cargo test` hands a test binary a pipe, and three parts of `krt` answer
//! nothing without a terminal: the width that a frame draws in, the keys that a
//! live run reads, and the hold that a live run takes on the terminal it draws
//! on. A pseudo terminal is a terminal, so these tests give the binary one and
//! drive it through that.
//!
//! Every pseudo terminal below carries a size. A pseudo terminal that nobody
//! sized answers the `TIOCGWINSZ` ioctl with zero columns, and that ioctl
//! succeeds. `termsize` reads the zero as no answer, and `krt` then draws the
//! nominal frame of 97 columns. A test of the width under such a terminal
//! therefore passes or fails for a reason that has nothing to do with the code
//! it reads.
//!
//! Every wait below carries a deadline. A read of a pseudo terminal whose child
//! writes nothing waits for ever, and a test that waits for ever holds every
//! commit of the repository. A wait that runs out fails with the text that the
//! terminal showed, so a reader of the failure sees how far the run got.
//!
//! The tests of a live run stand on macOS alone. macOS sends its probes without
//! privileges, so a run there reaches the loop that draws the table. Linux and
//! Windows stop such a run at the privilege gate, in front of every line that
//! these tests read.
//!
//! Those runs stay offline. The destination is the loopback, `--source` names
//! the loopback, and `--no-dns` looks nothing up. A run that names no source
//! asks a public service for the address that the internet sees, and every
//! `cargo test` then reaches the internet.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "each unwrap here is an assertion about the harness, not an unhandled error: the open of the pseudo terminal, the spawn of the freshly built binary, the write of one key into that terminal, and the read of the file that the run recorded. A failure of any one is a broken harness, and a panic names it at once"
)]

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(target_os = "macos")]
use std::{env, fs, process};

/// The number of rows of every pseudo terminal below.
///
/// The frames of these tests hold six lines at the most, so no run of them
/// scrolls. The number is a part of the size that the terminal reports, and a
/// terminal that reports a zero in either number reports no window at all.
const ROWS: u16 = 40;

/// The number of columns of a terminal that holds the whole frame.
///
/// The nominal frame takes 97 columns, so this terminal is wider than every
/// column of the table needs. Nothing drops, and the Host column takes the 23
/// columns that are left over.
const WIDE: u16 = 120;

/// The number of columns of a terminal too narrow for the whole frame.
///
/// The frame drops its columns in the order that `src/krt/src/ui.rs` states,
/// first dropped first: `Recent`, `StDev`, `Max`, `Min`, `Last`, `Sent`,
/// `Loss%`. Sixty columns take the first three of that order away and leave the
/// rest standing.
const NARROW: u16 = 60;

/// The longest that any wait below runs.
///
/// A run of the loopback takes one round each second, and the first frame draws
/// at the first round. Thirty seconds is far longer than that, and it is short
/// enough that a run which stopped drawing fails the suite in the place of
/// holding it.
const PATIENCE: Duration = Duration::from_secs(30);

/// The time between two reads of the state that a wait waits for.
const GLANCE: Duration = Duration::from_millis(20);

/// The recorded file that the repository holds, which carries two runs.
const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/two-runs.jsonl");

/// The name of the command that folds a recorded file.
const REPLAY: &str = "replay";

/// The start of the header line of a frame.
///
/// A replay writes its frame to standard output, and it writes the note about
/// the run it picked to standard error. One pseudo terminal carries both of
/// them, so the reader of the terminal takes the frame from this line onward.
const FRAME_START: &str = " krt  ";

/// The column header of the frame at [`WIDE`] columns.
///
/// The nominal header takes 94 columns with a Host column of 30. This terminal
/// holds 120 columns where the nominal frame takes 97, so the Host column takes
/// 23 more of them and the header takes 117.
///
/// The test spells the header, and the render draws it from the list of columns
/// that it holds. That is on purpose: a heading that moved would otherwise
/// agree with itself, and these headings are what a reader of the table sees.
const WIDE_COLUMN_HEADER: &str = " TTL  Host                                                    Loss%   Sent   Last    Min    Avg    Max  StDev  Recent";

/// The heading of the column of the recent round-trip times.
const RECENT_HEADING: &str = "Recent";

/// The heading of the column of the standard deviation.
const STDEV_HEADING: &str = "StDev";

/// The heading of the column of the TTL.
const TTL_HEADING: &str = "TTL";

/// The heading of the column of the router that answered.
const HOST_HEADING: &str = "Host";

/// The heading of the column of the mean round-trip time.
const AVG_HEADING: &str = "Avg";

/// The start of the column header of every frame.
///
/// The TTL column and the Host column never drop, so this text starts the
/// column header at every width. A wait for a frame of a live run waits for it.
const COLUMN_HEADER_START: &str = " TTL  Host";

/// The loopback address of ip version 4. Every machine holds a route to it.
#[cfg(target_os = "macos")]
const LOOPBACK: &str = "127.0.0.1";

/// The flag that names the address the probes leave from.
#[cfg(target_os = "macos")]
const FLAG_SOURCE: &str = "--source";

/// The flag that turns the reverse lookups off.
///
/// A lookup of the loopback answers inside the machine, and this flag makes no
/// lookup at all. It is what says, at the command line of the test, that the
/// run reaches no name server.
#[cfg(target_os = "macos")]
const FLAG_NO_DNS: &str = "--no-dns";

/// The flag that names the recorded file.
#[cfg(target_os = "macos")]
const FLAG_OUTPUT: &str = "--output";

/// The byte that a terminal sends for Ctrl-C.
///
/// Raw mode clears `ISIG`, so this byte reaches the process as a key press and
/// never as a `SIGINT`.
#[cfg(target_os = "macos")]
const CTRL_C: u8 = 0x03;

/// The byte that a terminal sends for the `q` key.
#[cfg(target_os = "macos")]
const Q: u8 = b'q';

/// The exit code of a run that stopped the way the user asked it to.
#[cfg(target_os = "macos")]
const EXIT_SUCCESS: u32 = 0;

/// The control sequence that enters the alternate screen.
#[cfg(target_os = "macos")]
const ENTER_ALTERNATE_SCREEN: &str = "\u{1b}[?1049h";

/// The control sequence that leaves the alternate screen.
#[cfg(target_os = "macos")]
const LEAVE_ALTERNATE_SCREEN: &str = "\u{1b}[?1049l";

/// The control sequence that shows the cursor.
#[cfg(target_os = "macos")]
const SHOW_CURSOR: &str = "\u{1b}[?25h";

/// The start of the line that a run prints when it stops.
#[cfg(target_os = "macos")]
const RECORDED: &str = "recorded ";

/// The name of the field that names what kind of record one line holds.
#[cfg(target_os = "macos")]
const TYPE_FIELD: &str = "type";

/// The kind of the record that closes a run.
#[cfg(target_os = "macos")]
const END_RECORD: &str = "end";

/// The name of the field that says why a run stopped.
#[cfg(target_os = "macos")]
const REASON_FIELD: &str = "reason";

/// The reason of a run that the user stopped.
#[cfg(target_os = "macos")]
const QUIT_REASON: &str = "quit";

/// One run of `krt` under a pseudo terminal, and the text that terminal showed.
///
/// The terminal carries standard output, standard error, and standard input of
/// the run, and it is the controlling terminal of that run. `krt` therefore
/// measures it, holds it, and reads the keys that this harness writes into it.
///
/// A thread reads the terminal, because a read of a terminal waits for the
/// child to write and the test has other work while it waits. Every wait of the
/// test reads the bytes that the thread collected, under the lock.
struct Terminal {
    /// The hold on the terminal. The drop of it closes the terminal.
    _master: Box<dyn MasterPty + Send>,
    /// Where a key of the test goes.
    keys: Box<dyn Write + Send>,
    /// Every byte that the terminal showed.
    shown: Arc<Mutex<Vec<u8>>>,
    /// Whether the terminal closed, which is the end of the text it showed.
    closed: Arc<AtomicBool>,
    /// The run under the terminal.
    child: Box<dyn Child + Send + Sync>,
}

impl Terminal {
    /// Starts `krt` with `arguments` under a terminal of `columns` columns.
    fn open(columns: u16, arguments: &[&str]) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("the pseudo terminal must open");
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_krt"));
        for argument in arguments {
            command.arg(argument);
        }
        // The name of the terminal reaches the child, so the run reads one
        // terminal and not whichever one started `cargo test`.
        command.env("TERM", "xterm-256color");
        let child = pair
            .slave
            .spawn_command(command)
            .expect("the binary must start under the pseudo terminal");
        // The child holds the one side of the terminal that is left. The read
        // of the other side then ends when the run ends, and a wait for that
        // end waits for the whole of the text.
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("the pseudo terminal must answer with a reader");
        let keys = pair
            .master
            .take_writer()
            .expect("the pseudo terminal must answer with a writer");
        let shown = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let filled = Arc::clone(&shown);
        let ended = Arc::clone(&closed);
        thread::spawn(move || {
            let mut chunk = [0_u8; 4096];
            // A closed terminal answers a read with no bytes, and it answers
            // one with a fault on some platforms. Both of them are the end of
            // the text, so both of them end this thread.
            while let Ok(read) = reader.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                filled
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend_from_slice(&chunk[..read]);
            }
            ended.store(true, Ordering::SeqCst);
        });
        Self {
            _master: pair.master,
            keys,
            shown,
            closed,
            child,
        }
    }

    /// The text that the terminal showed so far.
    ///
    /// A poisoned lock still holds bytes. The state under it is a buffer that
    /// one thread appends to, and a panic of a test never leaves half a byte in
    /// it.
    fn shown(&self) -> String {
        String::from_utf8_lossy(&self.shown.lock().unwrap_or_else(PoisonError::into_inner))
            .into_owned()
    }

    /// The lines of the frame, from the header line to the end of the text.
    ///
    /// The terminal returns a carriage on each line of its own, and the live
    /// table writes one of those itself, so each line ends with one carriage or
    /// with two. The reader takes every carriage off the end of a line, and
    /// what stays is the text that a reader of the terminal sees.
    fn frame(&self) -> Vec<String> {
        let shown = self.shown();
        shown
            .split('\n')
            .skip_while(|line| !line.starts_with(FRAME_START))
            .map(|line| line.trim_end_matches('\r').to_owned())
            .collect()
    }

    /// Waits until the terminal shows `text`.
    ///
    /// # Panics
    ///
    /// Panics when [`PATIENCE`] runs out, and names the text that the terminal
    /// showed. A wait of no deadline holds every commit of the repository.
    fn wait_for(&self, text: &str) {
        let deadline = Instant::now() + PATIENCE;
        while !self.shown().contains(text) {
            assert!(
                Instant::now() < deadline,
                "the run must show {text:?} inside {PATIENCE:?}, and it showed {:?}",
                self.shown()
            );
            thread::sleep(GLANCE);
        }
    }

    /// Writes one key into the terminal.
    #[cfg(target_os = "macos")]
    fn press(&mut self, key: u8) {
        self.keys
            .write_all(&[key])
            .expect("the pseudo terminal must take the key");
        self.keys.flush().expect("the key must reach the run");
    }

    /// Waits for the run to stop, and answers the code it stopped with.
    ///
    /// The wait runs on past the stop, until the terminal closes. The run
    /// writes its closing line last of all, and a test that read the text at
    /// the moment of the stop reads a text that misses it.
    ///
    /// # Panics
    ///
    /// Panics when [`PATIENCE`] runs out, and names the text that the terminal
    /// showed. A run that never stops holds every commit of the repository.
    fn wait_for_exit(&mut self) -> u32 {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                while !self.closed.load(Ordering::SeqCst) && Instant::now() < deadline {
                    thread::sleep(GLANCE);
                }
                return status.exit_code();
            }
            assert!(
                Instant::now() < deadline,
                "the run must stop inside {PATIENCE:?}, and it showed {:?}",
                self.shown()
            );
            thread::sleep(GLANCE);
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // A run that the test left standing takes a probe each second for as
        // long as the machine stands. The kill reaches the run that stopped as
        // well, and it does nothing there.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_replay_under_a_terminal_draws_the_table_at_the_width_of_that_terminal() {
    let mut terminal = Terminal::open(WIDE, &[REPLAY, FIXTURE]);
    terminal.wait_for_exit();
    let frame = terminal.frame();

    assert!(
        frame.iter().any(|line| line == WIDE_COLUMN_HEADER),
        "the frame draws at the width of the terminal, and the Host column takes the columns that are left over: {frame:?}"
    );
    for line in &frame {
        assert!(
            line.chars().count() <= usize::from(WIDE),
            "no line of the frame runs past the {WIDE} columns of the terminal: {line:?}"
        );
    }
}

#[test]
fn a_replay_under_a_narrow_terminal_drops_the_columns_that_drop_first() {
    let mut terminal = Terminal::open(NARROW, &[REPLAY, FIXTURE]);
    terminal.wait_for_exit();
    let frame = terminal.frame();
    let header = frame
        .iter()
        .find(|line| line.starts_with(COLUMN_HEADER_START))
        .unwrap_or_else(|| panic!("the frame must carry a column header: {frame:?}"))
        .clone();

    for gone in [RECENT_HEADING, STDEV_HEADING] {
        assert!(
            !header.contains(gone),
            "a terminal of {NARROW} columns drops the {gone} column: {header:?}"
        );
    }
    for stands in [TTL_HEADING, HOST_HEADING, AVG_HEADING] {
        assert!(
            header.contains(stands),
            "and it keeps the {stands} column, which never drops: {header:?}"
        );
    }
}

/// A path under the temporary directory of the machine.
///
/// The name keys on the process and on the moment, so two copies of one test
/// that run at the same time never share a file. `CLAUDE.md` demands it.
#[cfg(target_os = "macos")]
fn temp_path(name: &str) -> PathBuf {
    let moment = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock must sit after the epoch")
        .as_nanos();
    env::temp_dir().join(format!("{name}-{}-{moment}.jsonl", process::id()))
}

/// The file that one run recorded. The file goes away when the test ends, and
/// also when the test panics.
#[cfg(target_os = "macos")]
struct Recording {
    /// The path of the file.
    path: PathBuf,
}

#[cfg(target_os = "macos")]
impl Recording {
    /// Holds a path that no file uses yet, and that no other run reaches.
    fn at(name: &str) -> Self {
        Self {
            path: temp_path(name),
        }
    }

    /// The path of the file, as the command line of a run spells it.
    fn argument(&self) -> String {
        self.path
            .to_str()
            .expect("the temporary path must be text")
            .to_owned()
    }

    /// Why the `end` record of the file says the run stopped, and `None` when
    /// the file holds no such record.
    ///
    /// The file holds one JSON object for each line, so the reader parses each
    /// line and reads the field that names its kind. A search of the text for
    /// the two words would pass on a file whose `end` record names one reason
    /// and whose other records name the word beside it.
    fn end_reason(&self) -> Option<String> {
        let text = fs::read_to_string(&self.path).expect("the recorded file must read");
        text.lines()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line)
                    .expect("each line of the recorded file must parse")
            })
            .find(|record| {
                record.get(TYPE_FIELD).and_then(serde_json::Value::as_str) == Some(END_RECORD)
            })
            .and_then(|record| {
                record
                    .get(REASON_FIELD)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
    }
}

#[cfg(target_os = "macos")]
impl Drop for Recording {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// The name of the file that holds the one tracer of the machine.
///
/// The name is fixed, where every other path of this file keys on the process
/// and on the moment. That is the purpose of it: a lock that two processes
/// spell differently locks nothing.
#[cfg(target_os = "macos")]
const TRACER_LOCK: &str = "krt-live-tracer.lock";

/// The longest that a wait for the tracer of the machine runs.
///
/// One holder takes a few seconds, and [`PATIENCE`] bounds the longest one. Two
/// test binaries of three live runs each therefore take well under this bound,
/// and a wait that reaches it says that something outside these tests holds the
/// lock.
#[cfg(target_os = "macos")]
const LOCK_PATIENCE: Duration = Duration::from_mins(2);

/// The age at which a lock file belongs to a run that is no longer there.
///
/// A holder gives the lock back at the end of its test, and the panic of a test
/// gives it back too. A file that a killed process left behind never comes
/// back, so a file older than any holder ever lives is a file to take over.
#[cfg(target_os = "macos")]
const STALE_LOCK: Duration = Duration::from_mins(2);

/// The hold of one test on the tracer of the machine.
///
/// Two tracers of one machine collide. macOS hands the ICMP replies of one
/// process to the socket of every other process that reads that protocol, so a
/// tracer reads the answer of a probe that another tracer sent. The probe of
/// that answer stands in no state that an answer belongs to, and the tracer
/// then stops with a fault. Three live runs of this file collided that way, and
/// two of the three failed.
///
/// The lock is a file, and not a mutex of the process. `cargo test` runs the
/// tests of one binary on many threads, and more than one `cargo test` can run
/// at once, so a mutex holds one of the two cases and the file holds both.
#[cfg(target_os = "macos")]
struct TracerLock {
    /// The path of the lock file.
    path: PathBuf,
}

#[cfg(target_os = "macos")]
impl TracerLock {
    /// Waits for the tracer of the machine, and takes it.
    ///
    /// # Panics
    ///
    /// Panics when [`LOCK_PATIENCE`] runs out. A test that waited with no
    /// deadline holds every commit of the repository.
    fn take() -> Self {
        let path = env::temp_dir().join(TRACER_LOCK);
        let deadline = Instant::now() + LOCK_PATIENCE;
        loop {
            if fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .is_ok()
            {
                return Self { path };
            }
            // A file that no run holds any more goes away, and the next turn of
            // this loop takes the lock. Two waiters can both read the same file
            // as stale and both take it away, which costs one turn of the loop
            // and no more: the taker of the lock is the one that makes the file,
            // and one process alone can make a file that is not there.
            if stale(&path) {
                let _ = fs::remove_file(&path);
            }
            assert!(
                Instant::now() < deadline,
                "the tracer of the machine must come free inside {LOCK_PATIENCE:?}: {}",
                path.display()
            );
            thread::sleep(GLANCE);
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for TracerLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Whether a lock file belongs to a run that is no longer there.
///
/// A file whose age this machine cannot read counts as fresh. A lock that a
/// test takes away for a reason it could not check is worse than a lock that a
/// test waits for.
#[cfg(target_os = "macos")]
fn stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|data| data.modified())
        .ok()
        .and_then(|made| SystemTime::now().duration_since(made).ok())
        .is_some_and(|age| age > STALE_LOCK)
}

/// One live run of the loopback, and the hold that run takes on the machine.
#[cfg(target_os = "macos")]
struct LiveRun {
    /// The terminal of the run.
    terminal: Terminal,
    /// The hold on the tracer of the machine.
    ///
    /// The field stands under the terminal, because a field drops in the order
    /// it stands: the drop of the terminal stops the run, and the lock then
    /// goes back to the next test. A lock that went back first would let that
    /// test start its tracer beside a tracer that still stands.
    _lock: TracerLock,
}

/// Starts a live run of the loopback under a terminal of [`WIDE`] columns, and
/// waits until the table draws its first frame.
///
/// The run stays offline for the reason that the file documentation states: the
/// destination is the loopback, [`FLAG_SOURCE`] names the loopback, and
/// [`FLAG_NO_DNS`] looks nothing up.
#[cfg(target_os = "macos")]
fn live_run(recording: &Recording) -> LiveRun {
    let lock = TracerLock::take();
    let output = recording.argument();
    let terminal = Terminal::open(
        WIDE,
        &[
            LOOPBACK,
            FLAG_SOURCE,
            LOOPBACK,
            FLAG_NO_DNS,
            FLAG_OUTPUT,
            &output,
        ],
    );
    // The table draws its first frame at the first round. A key that arrived in
    // front of the terminal going into raw mode reaches the line discipline and
    // not the run, so every test below presses its key after this wait.
    terminal.wait_for(COLUMN_HEADER_START);
    LiveRun {
        terminal,
        _lock: lock,
    }
}

/// Asserts that a live run which one key stopped exits with success and closes
/// its recorded file with the reason of a quit.
#[cfg(target_os = "macos")]
fn a_key_stops_a_live_run(name: &str, key: u8) {
    let recording = Recording::at(name);
    let mut run = live_run(&recording);

    run.terminal.press(key);
    let code = run.terminal.wait_for_exit();

    assert_eq!(
        code,
        EXIT_SUCCESS,
        "the run stops the way the user asked it to, and it showed {:?}",
        run.terminal.shown()
    );
    assert_eq!(
        recording.end_reason().as_deref(),
        Some(QUIT_REASON),
        "the recorded file closes with the reason of a quit, and the run showed {:?}",
        run.terminal.shown()
    );
}

/// The `q` key stops a live run, and the run closes its file.
///
/// The run holds the terminal in raw mode, so the byte of the key reaches the
/// run and the line discipline holds none of it.
#[cfg(target_os = "macos")]
#[test]
fn the_q_key_stops_a_live_run_and_writes_the_end_record_of_a_quit() {
    a_key_stops_a_live_run("krt-terminal-q", Q);
}

/// Ctrl-C stops a live run, and the run closes its file.
///
/// This is the test of the design of the live table. Raw mode clears `ISIG`, so
/// the terminal sends the byte of Ctrl-C and the process takes no `SIGINT`. The
/// key handler of the run is therefore the one thing that can stop it, and a
/// run that lost that handler would take Ctrl-C and go on recording.
#[cfg(target_os = "macos")]
#[test]
fn ctrl_c_stops_a_live_run_and_writes_the_end_record_of_a_quit() {
    a_key_stops_a_live_run("krt-terminal-ctrl-c", CTRL_C);
}

/// A live run holds the terminal until it stops, and then gives it back.
///
/// The test reads the order of four things in one stream of bytes: the run
/// enters the alternate screen, it draws its table there, it leaves that screen
/// and shows the cursor again, and it prints its closing line last of all.
///
/// The order is the whole of what this test covers. A run that gave the
/// terminal back at the start of itself writes every one of the four, and it
/// draws its table over the lines of the reader and hides none of the cursor
/// that drags across it. A run that gave the terminal back at the end of itself
/// but printed its closing line in front of that puts the one line a reader
/// waits for on a screen that the terminal then takes away.
#[cfg(target_os = "macos")]
#[test]
fn a_live_run_gives_the_terminal_back_when_it_stops() {
    let recording = Recording::at("krt-terminal-restore");
    let mut run = live_run(&recording);

    run.terminal.press(Q);
    run.terminal.wait_for_exit();
    let shown = run.terminal.shown();

    let entered = shown
        .find(ENTER_ALTERNATE_SCREEN)
        .unwrap_or_else(|| panic!("the run must enter the alternate screen: {shown:?}"));
    // The last frame and the last restoration, because the run draws one frame
    // for each round and the table draws one more for the key that stopped it.
    let drawn = shown
        .rfind(COLUMN_HEADER_START)
        .unwrap_or_else(|| panic!("the run must draw its table: {shown:?}"));
    let left = shown
        .rfind(LEAVE_ALTERNATE_SCREEN)
        .unwrap_or_else(|| panic!("the run must leave the alternate screen: {shown:?}"));
    let cursor = shown
        .rfind(SHOW_CURSOR)
        .unwrap_or_else(|| panic!("the run must show the cursor again: {shown:?}"));
    let closing = shown
        .find(RECORDED)
        .unwrap_or_else(|| panic!("the run must print its closing line: {shown:?}"));

    assert!(
        entered < drawn,
        "the table draws on the alternate screen, and not over the lines of the reader: {shown:?}"
    );
    assert!(
        drawn < left,
        "and the run holds that screen for the whole of itself: {shown:?}"
    );
    assert!(
        left < closing,
        "the closing line stands after the alternate screen went away, so the reader keeps it: {shown:?}"
    );
    assert!(
        cursor < closing,
        "and the cursor stands again in front of that line: {shown:?}"
    );
}

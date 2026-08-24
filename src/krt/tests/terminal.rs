//! Black-box coverage for the parts of `krt` that need a real terminal, and
//! for the parts that need a known count of the live runs of the machine.
//!
//! `cargo test` hands a test binary a pipe, and three parts of `krt` answer
//! nothing without a terminal: the width that a frame draws in, the keys that a
//! live run reads, and the hold that a live run takes on the terminal it draws
//! on. A pseudo terminal is a terminal, so these tests give the binary one and
//! drive it through that.
//!
//! Some tests below give the binary a pipe on purpose. One covers the answer a
//! pipe produces, which is that such a run draws no table. The others read a
//! recorded file in place of a drawn table, and a terminal gives them nothing.
//! They all stand here beside the runs of a terminal for the other reason of
//! this file. Every test of a live run holds the lock of the live runs of the
//! machine while that run stands. A second lock in a second file locks nothing,
//! so a live run belongs here whatever its standard output is.
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
use std::net;
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(target_os = "macos")]
use std::{env, fs, process};

/// The number of rows of every pseudo terminal below.
///
/// The frames of these tests hold six lines at the most, so no run of them
/// scrolls and the live table leaves no row of a path out of a frame. The
/// number is a part of the size that the terminal reports, and a terminal that
/// reports a zero in either number reports no window at all.
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
/// A run draws its first frame at the moment it takes the terminal, and a run
/// of the loopback takes one round each second after that. Thirty seconds is
/// far longer than either of them, and it is short enough that a run which
/// stopped drawing fails the suite in the place of holding it.
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

/// A second address of the loopback network, which no test expects an answer
/// from.
///
/// The test of two runs at one time needs the answers of the two runs to be
/// different, and it needs no interface to answer as [`LOOPBACK`] for a probe
/// that went here. Both hold: this machine answers a probe of this address as
/// this address, or it answers nothing at all. A [`LOOPBACK`] hop in the file
/// of a run that probed this address is therefore the answer of the other run,
/// on every machine that runs the test.
#[cfg(target_os = "macos")]
const OTHER_LOOPBACK: &str = "127.0.0.2";

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

/// The flag that names the period of one round.
#[cfg(target_os = "macos")]
const FLAG_INTERVAL: &str = "--interval";

/// The flag that names the number of rounds which stops a run.
#[cfg(target_os = "macos")]
const FLAG_ROUNDS: &str = "--rounds";

/// The flag that names the last TTL that a round probes.
#[cfg(target_os = "macos")]
const FLAG_MAX_TTL: &str = "--max-ttl";

/// The flag that asks for the status lines in the place of the table.
#[cfg(target_os = "macos")]
const FLAG_HEADLESS: &str = "--headless";

/// The flag that names the protocol of a probe.
#[cfg(target_os = "macos")]
const FLAG_PROTOCOL: &str = "--protocol";

/// The protocol that `krt` probes with by default.
#[cfg(target_os = "macos")]
const ICMP: &str = "icmp";

/// The protocol of a trace that holds its source port and varies the
/// destination port.
#[cfg(target_os = "macos")]
const UDP: &str = "udp";

/// The protocol of a trace that holds its destination port and varies the
/// source port.
#[cfg(target_os = "macos")]
const TCP: &str = "tcp";

/// The first port of the range that a traceroute probes.
///
/// Every UDP run of `krt` fixed this port as its source port once, and the
/// unprivileged path of macOS binds the source port for each probe it sends. So
/// one program that held this port stopped every UDP run of the machine, and
/// two runs stopped each other.
#[cfg(target_os = "macos")]
const THE_CLASSIC_SOURCE_PORT: u16 = 33_434;

/// The number one, as a limit of rounds and as a limit of TTLs.
#[cfg(target_os = "macos")]
const ONE: &str = "1";

/// The number of rounds of the run that measures a UDP source port.
#[cfg(target_os = "macos")]
const TWO: &str = "2";

/// The number of rounds that each run of the collision test records.
///
/// The two runs start at one moment and each round of each of them sends one
/// probe, so five rounds give the two runs five chances each to read the
/// answer of the other. One chance is enough to show the defect, and five
/// stand well clear of a machine that scheduled the two runs apart.
#[cfg(target_os = "macos")]
const FIVE: &str = "5";

/// The period of one round of the collision test.
///
/// The two runs of that test record [`FIVE`] rounds each, so this period puts
/// the whole test inside one second. Every other live test of this file takes
/// the period that `krt` holds by default.
#[cfg(target_os = "macos")]
const A_SHORT_INTERVAL: &str = "200ms";

/// The flags of a run that records one round of one TTL and then stops.
///
/// The round limit is what stops such a run, and it is what bounds the wait of
/// each test that takes these flags. One of those waits carries no deadline of
/// its own, so a reader who takes the round limit away leaves a test that holds
/// every commit of the repository.
///
/// The TTL limit is what makes the status line of that round read the same in
/// each run. The tracer holds more than one TTL in the air at a time, and the
/// loopback answers every one of them, so a round of the whole range reports
/// one hop in one run and two hops in the next. A round of one TTL reports one
/// hop always.
#[cfg(target_os = "macos")]
const ONE_ROUND_OF_ONE_TTL: [&str; 4] = [FLAG_ROUNDS, ONE, FLAG_MAX_TTL, ONE];

/// The period of one round of a run whose first round no test waits for.
///
/// `src/krt/src/trace.rs` hands this period to the tracer as the shortest round
/// and as the longest one, so the first round of such a run lands two minutes
/// after the start. That is four times [`PATIENCE`], so every frame that a test
/// of this period reads is a frame that stands in front of the first round.
#[cfg(target_os = "macos")]
const A_LONG_INTERVAL: &str = "2m";

/// The start of the header line of a live table that folded no round.
///
/// The line names the destination, the address it resolved to, the source, the
/// count of the rounds, and the period of one round. The name of the recorded
/// file ends it, and each run of a test names a file of its own, so the text
/// holds the start of the line and not the whole of it.
///
/// The test spells the line, and the table builds it out of the fields it
/// holds. That is on purpose: `round 0` is what says that the frame stands in
/// front of the first round of the run, and this line is what a reader of the
/// terminal sees.
#[cfg(target_os = "macos")]
const NO_ROUND_HEADER: &str = " krt  127.0.0.1 → 127.0.0.1   src 127.0.0.1   round 0   2m   ";

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

/// The kind of the record that holds one round.
#[cfg(target_os = "macos")]
const ROUND_RECORD: &str = "round";

/// The name of the field of a `round` record that holds the hops which
/// answered.
#[cfg(target_os = "macos")]
const HOPS_FIELD: &str = "hops";

/// The name of the field of a hop that holds the address which answered.
#[cfg(target_os = "macos")]
const ADDR_FIELD: &str = "addr";

/// The reason of a run that the user stopped.
#[cfg(target_os = "macos")]
const QUIT_REASON: &str = "quit";

/// The reason of a run that the round limit stopped.
#[cfg(target_os = "macos")]
const ROUNDS_REASON: &str = "rounds";

/// The start of the status line of the first round of a run of the loopback.
///
/// The line names the round, the number of the TTLs that answered, whether the
/// round reached the target, and the time that round took. A probe of the
/// loopback answers at the first TTL and reaches the target, so the first three
/// fields of the line stand as this text spells them. The time is different in
/// each run, so the text stops in front of it.
///
/// The test spells the line, and `src/krt/src/live.rs` builds it out of the
/// fields of the round. That is on purpose: this line is the whole picture that
/// a run without a table gives of one round, and it is what a reader of such a
/// run sees.
#[cfg(target_os = "macos")]
const STATUS_LINE_START: &str = "round 1  1 hop  reached  ";

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
    /// table writes one of those between two lines of a frame, so a line ends
    /// with one carriage or with two. The last line of a live frame carries no
    /// line end at all, because a line end there scrolls the window. The reader
    /// takes every carriage off the end of a line, and what stays is the text
    /// that a reader of the terminal sees.
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

    /// Every record of the file, in the order that the run appended them.
    ///
    /// The file holds one JSON object for each line, so the reader parses each
    /// line. A search of the text for a word would pass on a file whose record
    /// of one kind names the word that a record of another kind carries.
    fn records(&self) -> Vec<serde_json::Value> {
        let text = fs::read_to_string(&self.path).expect("the recorded file must read");
        text.lines()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line)
                    .expect("each line of the recorded file must parse")
            })
            .collect()
    }

    /// Why the `end` record of the file says the run stopped, and `None` when
    /// the file holds no such record.
    fn end_reason(&self) -> Option<String> {
        self.records()
            .into_iter()
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

    /// The address of every hop that the `round` records of the file report.
    ///
    /// A hop that did not answer is absent from a `round` record, so this list
    /// names the routers that answered a probe of the run, one entry for each
    /// answer and in the order that the rounds recorded them.
    fn hop_addresses(&self) -> Vec<String> {
        self.records()
            .iter()
            .filter(|record| {
                record.get(TYPE_FIELD).and_then(serde_json::Value::as_str) == Some(ROUND_RECORD)
            })
            .filter_map(|record| record.get(HOPS_FIELD).and_then(serde_json::Value::as_array))
            .flatten()
            .filter_map(|hop| hop.get(ADDR_FIELD).and_then(serde_json::Value::as_str))
            .map(str::to_owned)
            .collect()
    }
}

#[cfg(target_os = "macos")]
impl Drop for Recording {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// The name of the file that holds the live runs of the machine.
///
/// The name is fixed, where every other path of this file keys on the process
/// and on the moment. That is the purpose of it: a lock that two processes
/// spell differently locks nothing.
#[cfg(target_os = "macos")]
const TRACER_LOCK: &str = "krt-live-tracer.lock";

/// The longest that a wait for the live runs of the machine runs.
///
/// One holder takes a few seconds. A live test carries two waits of
/// [`PATIENCE`], one for the first frame and one for the stop, so twice
/// PATIENCE bounds a holder that passes. The test of two runs at one time waits
/// for no frame, and the round limit of its two runs stops both of them inside
/// a second. Two test binaries of the live tests of this file therefore take
/// well under this bound, and a wait that reaches it says that something
/// outside these tests holds the lock.
///
/// The bound is more than twice [`STALE_LOCK`], so a waiter that pays the whole
/// wait for a stale file keeps most of its patience for the lock it then takes.
#[cfg(target_os = "macos")]
const LOCK_PATIENCE: Duration = Duration::from_mins(4);

/// The age at which a lock file belongs to a run that is no longer there.
///
/// A holder gives the lock back at the end of its test, and the panic of a test
/// gives it back too. A file that a killed process left behind never comes
/// back, so a file older than any holder ever lives is a file to take over.
///
/// Ninety seconds is three times [`PATIENCE`], and twice PATIENCE bounds the
/// longest holder that passes, so no live test takes the lock of another one.
/// The age also stands well inside [`LOCK_PATIENCE`]: a waiter pays this
/// age at the most for the file of a killed run, and the rest of its patience
/// then holds the lock that it took.
#[cfg(target_os = "macos")]
const STALE_LOCK: Duration = Duration::from_secs(90);

/// The hold of one test on the live runs of the machine.
///
/// A test that holds this lock knows the live runs that `cargo test` started.
/// They are the runs that the test itself started, and every other test of this
/// file waits. A `krt` that a user started by hand stands outside that count.
///
/// The lock started as a workaround, and that is no longer the reason it
/// stands. macOS hands the ICMP replies of one process to the socket of every
/// other process that reads that protocol, so a tracer reads the answer of a
/// probe that another tracer sent. `krt` named no identifier of its own once,
/// so every run took every answer. Three live runs of this file collided that
/// way, and two of the three failed. `src/krt/src/trace.rs` closes that defect:
/// each run carries the identifier of its process, and the tracer drops every
/// answer that carries another one.
///
/// What the lock gives now is the count above, and three tests read it. Each of
/// them calls [`two_live_runs_of`], which starts two runs of one protocol and
/// asks what each of them recorded. A third run of this file beside them can
/// make a failure read as a defect of `krt`. The cause is then a test of this
/// file that stands at the same moment.
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
    /// Waits for the live runs of the machine, and takes the hold on them.
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
                "the live runs of the machine must come free inside {LOCK_PATIENCE:?}: {}",
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

/// A waiter that takes a stale lock over keeps patience for the run it starts.
///
/// The waiter of a file that a killed run left behind pays [`STALE_LOCK`] in
/// front of the moment that file counts as stale. It removes the file then, and
/// it takes the lock on the turn after that, so the patience it has left is
/// [`LOCK_PATIENCE`] less the age. [`TracerLock::take`] reads its deadline after
/// the removal, so a waiter of no patience left fails on the very turn that
/// freed the lock, and the turn it fails on is one it never controlled: the
/// clock of the age starts when the dead holder made the file, and the clock of
/// the patience starts when the waiter arrives.
///
/// Twice the age is therefore the bound. A waiter that pays the whole wait for
/// a stale file keeps half of its patience at the least, and that half is what
/// it holds the lock with.
#[cfg(target_os = "macos")]
#[test]
fn a_waiter_that_takes_a_stale_lock_over_keeps_patience_for_the_run_it_starts() {
    assert!(
        LOCK_PATIENCE >= STALE_LOCK * 2,
        "the patience of a waiter must stand clear of the age of a stale lock: {LOCK_PATIENCE:?} against {STALE_LOCK:?}"
    );
}

/// One live run of the loopback, and the hold that run takes on the machine.
#[cfg(target_os = "macos")]
struct LiveRun {
    /// The terminal of the run.
    terminal: Terminal,
    /// The hold on the live runs of the machine.
    ///
    /// The field stands under the terminal, because a field drops in the order
    /// it stands: the drop of the terminal stops the run, and the lock then
    /// goes back to the next test. A lock that went back first would let that
    /// test start its run beside a run that still stands.
    _lock: TracerLock,
}

/// The command line of a live run of `destination`, with `flags` behind the
/// flags that every live run of this file takes.
///
/// A test that needs one more flag names that flag alone. The flags of the run
/// which keep it offline stand here, in one place, so no test of this file can
/// start a run that reaches the network. The run stays offline for the reason
/// that the file documentation states: the destination is of the loopback
/// network, [`FLAG_SOURCE`] names the loopback, and [`FLAG_NO_DNS`] looks
/// nothing up.
#[cfg(target_os = "macos")]
fn live_arguments<'a>(destination: &'a str, output: &'a str, flags: &[&'a str]) -> Vec<&'a str> {
    let mut arguments = vec![
        destination,
        FLAG_SOURCE,
        LOOPBACK,
        FLAG_NO_DNS,
        FLAG_OUTPUT,
        output,
    ];
    arguments.extend_from_slice(flags);
    arguments
}

/// Starts a live run of the loopback under a terminal of [`WIDE`] columns, and
/// waits until the table draws its first frame.
#[cfg(target_os = "macos")]
fn live_run(recording: &Recording) -> LiveRun {
    live_run_with(recording, &[])
}

/// Starts such a run with `flags`, and waits until the table draws its first
/// frame.
#[cfg(target_os = "macos")]
fn live_run_with(recording: &Recording, flags: &[&str]) -> LiveRun {
    let run = live_run_under(recording, flags);
    // The table draws its first frame at the moment the run takes the
    // terminal, so a frame on the screen says that raw mode stands. A key that
    // arrived in front of raw mode reaches the line discipline and not the run,
    // so every test below presses its key after this wait.
    run.terminal.wait_for(COLUMN_HEADER_START);
    run
}

/// Starts such a run with `flags`, and waits for nothing.
///
/// A run that draws no table shows no frame, so a test of that run waits for a
/// text of its own. Every other caller takes [`live_run_with`], which waits for
/// the first frame of the table.
#[cfg(target_os = "macos")]
fn live_run_under(recording: &Recording, flags: &[&str]) -> LiveRun {
    let lock = TracerLock::take();
    let output = recording.argument();
    let terminal = Terminal::open(WIDE, &live_arguments(LOOPBACK, &output, flags));
    LiveRun {
        terminal,
        _lock: lock,
    }
}

/// The number of status lines that `text` holds.
///
/// A terminal returns a carriage at the end of each line it shows, and a pipe
/// returns none, so the reader takes every carriage off the end of a line. What
/// stays is the line that a reader of either one sees.
#[cfg(target_os = "macos")]
fn status_lines(text: &str) -> usize {
    text.split('\n')
        .filter(|line| line.trim_end_matches('\r').starts_with(STATUS_LINE_START))
        .count()
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

/// A live run draws its table when it takes the terminal, in front of the first
/// round of that run.
///
/// The period of the run is what makes this a test. A run of
/// [`A_LONG_INTERVAL`] takes its first round two minutes after the start, which
/// is four times [`PATIENCE`], so the frame that this test reads is a frame
/// that no round drew. A table that drew at the first round alone leaves the
/// reader in front of an empty alternate screen for those two minutes, and that
/// screen hides the lines which the run printed in front of it, so nothing at
/// all says that the run started.
#[cfg(target_os = "macos")]
#[test]
fn a_live_run_draws_its_table_in_front_of_the_first_round() {
    let recording = Recording::at("krt-terminal-opening-frame");
    let mut run = live_run_with(&recording, &[FLAG_INTERVAL, A_LONG_INTERVAL]);
    let shown = run.terminal.shown();

    assert!(
        shown.contains(NO_ROUND_HEADER),
        "the header line of that frame names the run and the round it stands in front of: {shown:?}"
    );

    run.terminal.press(Q);
    assert_eq!(
        run.terminal.wait_for_exit(),
        EXIT_SUCCESS,
        "and the key of the reader stops the run, so the test leaves no tracer behind: {:?}",
        run.terminal.shown()
    );
}

/// A live run whose standard output is a pipe draws no table.
///
/// This is the one test of this file that reads what a pipe holds. The run has
/// no terminal to hold, no key to read, and no screen to clear, so it writes
/// one status line for the round it made and nothing else. A table there would
/// write a whole frame of control sequences into the file of the reader for
/// each round, and the alternate screen is the first of those sequences.
///
/// The recorded file carries the other half of the answer. Standard output says
/// what the reader of a pipe sees, and the reason in that file says how the run
/// stopped: the round limit stopped it, and no fault did.
#[cfg(target_os = "macos")]
#[test]
fn a_live_run_whose_standard_output_is_a_pipe_draws_no_table() {
    let recording = Recording::at("krt-terminal-piped");
    // The lock stands under the recording, so the drop of the lock comes first
    // and the removal of the file comes after it. That is the order of every
    // other live test of this file.
    let _lock = TracerLock::take();
    let output = recording.argument();
    // `wait_with_output` waits with no deadline, and the round limit of
    // [`ONE_ROUND_OF_ONE_TTL`] is what bounds it: the run stops on its own.
    let finished = process::Command::new(env!("CARGO_BIN_EXE_krt"))
        .args(live_arguments(LOOPBACK, &output, &ONE_ROUND_OF_ONE_TTL))
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped())
        .spawn()
        .expect("the binary must start with its standard output on a pipe")
        .wait_with_output()
        .expect("the run must answer with the text it wrote");
    let shown = String::from_utf8_lossy(&finished.stdout).into_owned();

    assert!(
        finished.status.success(),
        "the round limit stops the run, and it showed {shown:?}"
    );
    assert_eq!(
        status_lines(&shown),
        1,
        "the run writes one status line for the one round it made: {shown:?}"
    );
    for sequence in [ENTER_ALTERNATE_SCREEN, LEAVE_ALTERNATE_SCREEN] {
        assert!(
            !shown.contains(sequence),
            "and it writes no control sequence of the alternate screen into a pipe: {shown:?}"
        );
    }
    assert_eq!(
        recording.end_reason().as_deref(),
        Some(ROUNDS_REASON),
        "the recorded file closes with the reason of the round limit, and the run showed {shown:?}"
    );
}

/// The `--headless` flag draws no table under a terminal either.
///
/// The terminal of this run is a real one, so the flag is the only thing that
/// keeps the table away. That is what makes this a test of the flag: a run
/// which read the terminal alone would draw its table here, and the reader who
/// asked for the status lines would get a screen that clears itself instead.
#[cfg(target_os = "macos")]
#[test]
fn the_headless_flag_draws_no_table_under_a_terminal() {
    let recording = Recording::at("krt-terminal-headless");
    // The test presses no key, so the round limit of [`ONE_ROUND_OF_ONE_TTL`]
    // is what stops this run.
    let mut flags = ONE_ROUND_OF_ONE_TTL.to_vec();
    flags.push(FLAG_HEADLESS);
    let mut run = live_run_under(&recording, &flags);

    let code = run.terminal.wait_for_exit();
    let shown = run.terminal.shown();

    assert_eq!(
        code, EXIT_SUCCESS,
        "the round limit stops the run, and it showed {shown:?}"
    );
    assert_eq!(
        status_lines(&shown),
        1,
        "the run writes one status line for the one round it made: {shown:?}"
    );
    for sequence in [ENTER_ALTERNATE_SCREEN, LEAVE_ALTERNATE_SCREEN] {
        assert!(
            !shown.contains(sequence),
            "and the flag keeps it off the alternate screen of the terminal it holds: {shown:?}"
        );
    }
}

/// Asserts that two live runs of `protocol`, which stand at one moment, each
/// record the answers of its own probes and no answer of the other run.
///
/// One run probes [`LOOPBACK`], which answers every probe, and the other probes
/// [`OTHER_LOOPBACK`], which answers none as [`LOOPBACK`]. That is what makes
/// the answers tell the two runs apart: a [`LOOPBACK`] hop in the file of the
/// second run is an answer that the first run earned.
///
/// The caller holds the lock of the live runs of the machine, so the number of
/// runs while this helper measures is the two that it starts.
#[cfg(target_os = "macos")]
fn two_live_runs_of(name: &str, protocol: &str) {
    let answered = Recording::at(&format!("krt-collide-{name}-answered"));
    let silent = Recording::at(&format!("krt-collide-{name}-silent"));
    let flags = [
        FLAG_PROTOCOL,
        protocol,
        FLAG_ROUNDS,
        FIVE,
        FLAG_MAX_TTL,
        ONE,
        FLAG_INTERVAL,
        A_SHORT_INTERVAL,
        FLAG_HEADLESS,
    ];

    // The two runs start one after the other and then stand together. The round
    // limit of each one stops it, so neither wait below carries a deadline of
    // its own. The output of a headless run of five rounds is a few hundred
    // bytes, which is far under the buffer of a pipe, so the run that this
    // thread does not wait for never stalls on a full pipe.
    let answered_output = answered.argument();
    let silent_output = silent.argument();
    let mut running: Vec<process::Child> = [
        (LOOPBACK, &answered_output),
        (OTHER_LOOPBACK, &silent_output),
    ]
    .iter()
    .map(|(destination, output)| {
        process::Command::new(env!("CARGO_BIN_EXE_krt"))
            .args(live_arguments(destination, output, &flags))
            .stdin(process::Stdio::null())
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::piped())
            .spawn()
            .expect("the binary must start")
    })
    .collect();
    let finished: Vec<process::Output> = running
        .drain(..)
        .map(|child| {
            child
                .wait_with_output()
                .expect("the run must stop on its round limit")
        })
        .collect();

    for (finished, destination) in finished.iter().zip([LOOPBACK, OTHER_LOOPBACK]) {
        assert!(
            finished.status.success(),
            "the round limit stops the {protocol} run of {destination}, and no answer of the other run does: {} said {:?}",
            finished.status,
            String::from_utf8_lossy(&finished.stderr)
        );
    }
    for (recording, destination) in [(&answered, LOOPBACK), (&silent, OTHER_LOOPBACK)] {
        assert_eq!(
            recording.end_reason().as_deref(),
            Some(ROUNDS_REASON),
            "the file of the {protocol} run of {destination} closes with the reason of the round limit"
        );
    }
    assert!(
        answered.hop_addresses().iter().any(|hop| hop == LOOPBACK),
        "the {protocol} run of {LOOPBACK} records the answers of its own probes: {:?}",
        answered.hop_addresses()
    );
    assert!(
        !silent.hop_addresses().iter().any(|hop| hop == LOOPBACK),
        "and the {protocol} run of {OTHER_LOOPBACK} records no answer that the other run earned: {:?}",
        silent.hop_addresses()
    );
}

/// Two live ICMP runs of one machine each record the answers of its own probes.
///
/// This is the test of the identifier that `src/krt/src/trace.rs` gives each
/// run. macOS hands the ICMP answers of one process to the socket of every
/// other process that reads that protocol, so each of these two runs reads
/// every answer that the machine took. A run that reads a foreign answer as its
/// own records the path of another run, and the tracer of a debug build stops
/// with a fault on the way, because that answer belongs to no probe of the
/// state which the tracer holds.
#[cfg(target_os = "macos")]
#[test]
fn two_live_icmp_runs_of_one_machine_each_record_only_the_answers_of_its_own_probes() {
    // The lock stands over the runs of the helper, and the recordings of that
    // helper drop inside it, so the drop of the lock comes after the runs stop
    // and before the next test starts one.
    let _lock = TracerLock::take();
    two_live_runs_of("icmp", ICMP);
}

/// Two live UDP runs of one machine each record the answers of its own probes.
///
/// The source port is what tells two UDP runs apart. A UDP trace holds its
/// source port while the destination port varies, and the unprivileged path of
/// macOS binds that port for each probe, so two runs of one port cannot both
/// send. The tracer of the second one stops on the port it cannot take, and the
/// answers of the two would carry one port besides.
#[cfg(target_os = "macos")]
#[test]
fn two_live_udp_runs_of_one_machine_each_record_only_the_answers_of_its_own_probes() {
    let _lock = TracerLock::take();
    two_live_runs_of("udp", UDP);
}

/// Two live TCP runs of one machine each record the answers of its own probes.
///
/// A TCP trace holds its destination port while the source port varies, and the
/// tracer takes the next source port when one is in use, so two TCP runs stand
/// beside each other already. The test holds that answer in place.
#[cfg(target_os = "macos")]
#[test]
fn two_live_tcp_runs_of_one_machine_each_record_only_the_answers_of_its_own_probes() {
    let _lock = TracerLock::take();
    two_live_runs_of("tcp", TCP);
}

/// A live UDP run stands while another program holds the port that a traceroute
/// probes first.
///
/// This is the answer of the test above, said once and for certain. The two
/// runs of that test meet on a port that each of them binds for a moment and
/// then gives back, so they meet on it often and not on every run. This test
/// holds the port for the whole run, so a run that wanted that one port stops
/// every time.
///
/// The port that the run takes in its place is a port of the range which stands
/// above the ports a traceroute probes and under the ports that macOS hands to
/// a socket asking for any port. `src/krt/src/trace.rs` states the range and
/// the fold onto it.
///
/// The run records two rounds, and the first of them reports no hop. A UDP
/// trace carries the sequence of a probe in the destination port, and the first
/// sequence of a run is 33434, so the first probe of the run arrives at the
/// socket that this test holds. That socket takes the datagram, the machine
/// answers no port unreachable for it, and the hop of that round is therefore
/// absent. The second probe carries 33435, which nothing holds.
#[cfg(target_os = "macos")]
#[test]
fn a_live_udp_run_stands_while_another_program_holds_the_port_of_a_traceroute() {
    let recording = Recording::at("krt-udp-classic-port");
    // The lock stands under the recording, so the drop of the lock comes first
    // and the removal of the file comes after it.
    let _lock = TracerLock::take();
    let held = net::UdpSocket::bind((LOOPBACK, THE_CLASSIC_SOURCE_PORT))
        .expect("the port that a traceroute probes first must be free for this test to hold");
    let output = recording.argument();
    let flags = [
        FLAG_PROTOCOL,
        UDP,
        FLAG_ROUNDS,
        TWO,
        FLAG_MAX_TTL,
        ONE,
        FLAG_INTERVAL,
        A_SHORT_INTERVAL,
        FLAG_HEADLESS,
    ];

    let finished = process::Command::new(env!("CARGO_BIN_EXE_krt"))
        .args(live_arguments(LOOPBACK, &output, &flags))
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped())
        .spawn()
        .expect("the binary must start")
        .wait_with_output()
        .expect("the run must stop on its round limit");
    drop(held);

    assert!(
        finished.status.success(),
        "the round limit stops the run, and the held port does not: {} said {:?}",
        finished.status,
        String::from_utf8_lossy(&finished.stderr)
    );
    assert_eq!(
        recording.end_reason().as_deref(),
        Some(ROUNDS_REASON),
        "the file closes with the reason of the round limit"
    );
    assert!(
        recording.hop_addresses().iter().any(|hop| hop == LOOPBACK),
        "and the run records the answer of the probe that the held socket did not take: {:?}",
        recording.hop_addresses()
    );
}

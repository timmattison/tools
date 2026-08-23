//! Black-box coverage for the parts of `krt` that a pipe cannot reach.
//!
//! `cargo test` hands a test binary a pipe, and `krt` reads a terminal for the
//! width that a frame draws in. A pseudo terminal is a terminal, so these tests
//! give the binary one and drive it through that.
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

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "each unwrap here is an assertion about the harness, not an unhandled error: the open of the pseudo terminal, and the spawn of the freshly built binary under it. A failure of either one is a broken harness, and a panic names it at once"
)]

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

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
/// A replay reads one file and prints one frame, which takes a moment. Thirty
/// seconds is far longer than that, and it is short enough that a run which
/// stopped writing fails the suite in the place of holding it.
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
/// column header at every width.
const COLUMN_HEADER_START: &str = " TTL  Host";

/// One run of `krt` under a pseudo terminal, and the text that terminal showed.
///
/// The terminal carries standard output, standard error, and standard input of
/// the run, and it is the controlling terminal of that run. `krt` therefore
/// measures it, and it draws in the columns that the terminal reports.
///
/// A thread reads the terminal, because a read of a terminal waits for the
/// child to write and the test has other work while it waits. Every wait of the
/// test reads the bytes that the thread collected, under the lock.
struct Terminal {
    /// The hold on the terminal. The drop of it closes the terminal.
    _master: Box<dyn MasterPty + Send>,
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

//! Black-box tests that hold `ic` to the character cell the terminal named, and
//! not to the estimate of a cell that a run guesses when nothing answers.
//!
//! `termgfx` lays a picture out in character cells and it converts cells to
//! pixels, so it needs the size of one cell. The `TIOCGWINSZ` ioctl carries
//! that measure, and a mosh session carries none: the mosh wire protocol
//! resizes with a width and a height in cells and nothing else, so the server
//! writes a zero into both pixel fields of the pseudo terminal. A pane of
//! Zellij reports none and a ttyd panel reports none.
//!
//! `ic` then fell back to an estimate of 10 pixels by 20, and a picture bound
//! by its height takes a column count of `rows × cell_aspect × image_aspect`.
//! The column count is thus directly proportional to the shape of the cell, so
//! the guess decided what the user saw: a picture about 7 percent too narrow
//! over mosh, beside the same picture at the right width over ssh. GitHub issue
//! #468 reports that.
//!
//! # What each test builds
//!
//! The child takes a pseudo-terminal as its controlling terminal and writes its
//! picture to a pipe, which is the shape of a captured run. **The window of that
//! pseudo-terminal reports no pixel size**, which is the shape of a mosh
//! session. This file stands where the terminal stands: it reads the query off
//! the master end and it writes the answer of a terminal back.
//!
//! `--height 10` states one axis and leaves the other open, so the aspect ratio
//! decides the open one and the column count is the cell shape times ten. The
//! test image is one pixel by one, so the image contributes a ratio of one and
//! the whole of the column count comes off the cell. Each answer below
//! therefore names a column count that no other answer of this file names, and
//! none of them names the count that the estimate gives.
//!
//! # The two things that a test of this shape gets wrong
//!
//! `ic` writes the whole picture to standard output before it asks the terminal
//! about a refusal, and a pipe that nobody reads fills and holds the child there
//! for good. So the picture leaves on a thread of its own, and a defect fails
//! this file instead of hanging it.
//!
//! Every wait carries a deadline for the same reason. A change that took the
//! query away would leave a test with no deadline waiting for a byte that
//! nothing writes.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

mod common;

use common::pty::{Pty, Window};
use common::{find, unreachable_path_dir, TEST_IMAGE};

/// The name that this target puts in the unreachable `PATH` of its children.
const TARGET_NAME: &str = "cell-size";

/// The terminal type that names a Kitty terminal, so the Kitty writer runs and
/// the picture carries a `c=` key.
///
/// **The name is the point of this file.** A named terminal answered the
/// question about the protocol already, and it answered nothing about the size
/// of a cell. Every run of this file draws a picture and therefore reads that
/// size, so a run under this name must still ask, and the trigger of the
/// question must be the pixel size that the window reports. A run that draws
/// nothing reads no cell and asks such a terminal nothing at all, which
/// `src/ic/tests/will-display.rs` holds.
const TERM_XTERM_KITTY: &str = "xterm-kitty";

/// The window that the pseudo-terminal of these tests reports.
///
/// **The pixel size is zero on both axes**, which is what a mosh session, a
/// pane of Zellij and a ttyd panel all report. The ioctl therefore measures no
/// cell, and the answer of the terminal is the only measure a run has.
const WINDOW: Window = Window {
    columns: 80,
    rows: 24,
    width_px: 0,
    height_px: 0,
};

/// The height in character cells that every run of this file asks for.
///
/// One axis stated and the other open, so the aspect ratio of the picture
/// decides the open one and the column count is this number times the shape of
/// the cell.
const PICTURE_ROWS: u32 = 10;

/// The request of the primary device attributes.
///
/// Every terminal answers it, so its answer is what ends a read of the terminal.
/// It stands at the end of the query of `ic`, and `ic` writes it alone after a
/// still picture to ask whether the terminal refused that picture.
const ATTRIBUTES_REQUEST: &[u8] = b"\x1b[c";

/// The request of the size of one character cell in pixels.
///
/// This is window operation 16 of xterm. A run that asks it gets the size of a
/// cell from the one party that knows.
const CELL_SIZE_REQUEST: &[u8] = b"\x1b[16t";

/// The request of the size of the text area in pixels.
///
/// This is window operation 14 of xterm, and it is the fallback of
/// [`CELL_SIZE_REQUEST`] for a terminal that reads the older operation alone.
const TEXT_AREA_REQUEST: &[u8] = b"\x1b[14t";

/// The answer of the primary device attributes that ends every read.
const ATTRIBUTES_ANSWER: &[u8] = b"\x1b[?62;4c";

/// How long a test waits for `ic` to ask the terminal anything.
///
/// A run that works spends almost none of this. The budget is here for a run
/// that asks nothing at all, and it is generous because a loaded machine starts
/// a process late.
const QUESTION_BUDGET: Duration = Duration::from_secs(10);

/// One answer of a terminal, and the picture that `ic` must draw for it.
struct Case {
    /// The bytes that the terminal writes for the query, in front of
    /// [`ATTRIBUTES_ANSWER`].
    answer: &'static [u8],
    /// The column count that the picture must carry in its `c=` key.
    columns: u32,
    /// What the answer says, for the message of a failed assertion.
    says: &'static str,
}

/// Run `ic --stdin --height 10` under a terminal that answers `answer`, and
/// give back the picture it drew.
///
/// The order of the steps is the order that keeps the run from stopping: the
/// picture leaves on a thread of its own, then this thread answers the query,
/// then it answers the question about a refusal, then it reads standard error
/// to the end, and only then does it wait for the child.
///
/// # Arguments
/// * `answer` - The bytes that the terminal writes for the query, in front of
///   [`ATTRIBUTES_ANSWER`].
///
/// # Returns
/// The query that reached the terminal and the picture that `ic` wrote to
/// standard output.
///
/// # Panics
/// Panics when the child does not start, does not take the image, does not
/// report, or exits with a failure.
fn run_ic_against(answer: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let pty = Pty::open(WINDOW);

    // The environment is empty except for the two variables below, so nothing
    // the test runner inherited can pick another display routine. The `PATH`
    // points at a directory that does not exist, which keeps `ps` out of reach
    // of the remote transport detection.
    let mut command = Command::new(env!("CARGO_BIN_EXE_ic"));
    command
        .arg("--stdin")
        .arg("--height")
        .arg(PICTURE_ROWS.to_string())
        .env_clear()
        .env("PATH", unreachable_path_dir(TARGET_NAME))
        .env("TERM", TERM_XTERM_KITTY)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    pty.hand_to(&mut command);

    let mut child = command.spawn().expect("failed to start ic");

    let mut stdin = child.stdin.take().expect("ic has no stdin pipe");
    stdin
        .write_all(TEST_IMAGE)
        .expect("failed to send the image to ic");
    drop(stdin);

    // A pipe that nobody reads fills at about 64 kilobytes and holds the child
    // there. This thread stays free to answer the terminal.
    let mut picture = child.stdout.take().expect("ic has no stdout pipe");
    let drawing = thread::spawn(move || {
        let mut drawn = Vec::new();
        picture
            .read_to_end(&mut drawn)
            .expect("failed to read the picture that ic drew");
        drawn
    });

    // The query ends with the request of the attributes, so that request is
    // what says the whole of it arrived.
    let query = pty.read_until(ATTRIBUTES_REQUEST, QUESTION_BUDGET);
    let mut said = answer.to_vec();
    said.extend_from_slice(ATTRIBUTES_ANSWER);
    pty.answer(&said);

    // A still picture asks the terminal whether it refused. The answer of the
    // attributes alone reports no refusal, so `ic` exits with a success.
    pty.read_until(ATTRIBUTES_REQUEST, QUESTION_BUDGET);
    pty.answer(ATTRIBUTES_ANSWER);

    // The read ends when the child closes standard error, which is when the
    // child exits. So this read is the wait, and the status comes back after it.
    let mut reported = String::new();
    child
        .stderr
        .take()
        .expect("ic has no stderr pipe")
        .read_to_string(&mut reported)
        .expect("failed to read the standard error of ic");
    let status = child.wait().expect("failed to wait for ic");
    let drawn = drawing.join().expect("the reader of the picture panicked");

    assert!(
        status.success(),
        "ic exited with {status}, and it must draw a picture for every terminal: {reported}"
    );

    (query, drawn)
}

/// The opener of a Kitty graphics command.
const KITTY_OPENER: &[u8] = b"\x1b_G";

/// The byte that divides the control keys of a Kitty command from its payload.
const KITTY_SEPARATOR: char = ';';

/// The byte that divides one control key from the next.
const KEY_SEPARATOR: char = ',';

/// The name of the key that carries the column count, and the equals sign
/// behind it.
///
/// This is the key that decides how wide the picture is on the screen. The
/// terminal draws the picture into that many real cells, so this number is what
/// the user sees.
const COLUMNS_PREFIX: &str = "c=";

/// The column count that the picture in `drawn` states, or [`None`] for a
/// picture that states none.
///
/// The picture is `ESC _ G <control keys> ; <payload> ESC \`, and the control
/// keys are `<name>=<value>` pairs that a comma divides.
///
/// # Arguments
/// * `drawn` - Every byte that `ic` wrote to standard output.
///
/// # Returns
/// The value of the `c` key of the first Kitty command in `drawn`.
fn columns_of(drawn: &[u8]) -> Option<u32> {
    let start = find(drawn, KITTY_OPENER)? + KITTY_OPENER.len();
    let command = String::from_utf8_lossy(drawn.get(start..)?).into_owned();
    let keys = command.split(KITTY_SEPARATOR).next()?;
    keys.split(KEY_SEPARATOR)
        .find_map(|pair| pair.strip_prefix(COLUMNS_PREFIX))?
        .parse()
        .ok()
}

#[test]
fn the_query_asks_a_named_terminal_that_reports_no_pixel_size() {
    // The name of the terminal answers the question about the protocol and says
    // nothing about the size of a cell. A named terminal reached through a
    // proxy that strips the pixel size therefore still owes that answer, and a
    // trigger that read the name alone would never ask it.
    let (query, _) = run_ic_against(b"\x1b[6;30;14t");

    assert!(
        find(&query, CELL_SIZE_REQUEST).is_some(),
        "ic must ask a named terminal how big one cell is when the window reports no pixel size"
    );
    assert!(
        find(&query, TEXT_AREA_REQUEST).is_some(),
        "ic must ask for the text area as well, for a terminal that reads the older window operation alone"
    );
    assert!(
        query.ends_with(ATTRIBUTES_REQUEST),
        "the request of the attributes must stand last, because its answer is what ends the read"
    );
}

#[test]
fn the_cell_the_terminal_named_decides_how_wide_the_picture_is() {
    // This is the test that ties the measure to what the user sees. Each answer
    // names one cell, and the column count of the picture is ten rows times the
    // shape of that cell. The image is one pixel by one, so it contributes a
    // ratio of one and the whole of the count comes off the cell.
    for case in [
        Case {
            // A cell of 14 pixels by 30 is the shape of a mosh session on the
            // display that issue #468 measured. 30 over 14 is about 2.1429, and
            // ten rows of it round to 21 columns.
            answer: b"\x1b[6;30;14t",
            columns: 21,
            says: "a cell of 14 pixels by 30",
        },
        Case {
            // A cell of 10 pixels by 40 is four times as tall as it is wide, so
            // ten rows of it are exactly 40 columns.
            answer: b"\x1b[6;40;10t",
            columns: 40,
            says: "a cell of 10 pixels by 40",
        },
        Case {
            // The text area alone, for a terminal that reads the older window
            // operation. 640 pixels over 80 columns is a cell 8 pixels wide,
            // and 672 over 24 rows is a cell 28 pixels tall. 28 over 8 is 3.5,
            // and ten rows of it are 35 columns.
            answer: b"\x1b[4;672;640t",
            columns: 35,
            says: "a text area of 640 pixels by 672 over a window of 80 columns by 24",
        },
    ] {
        let (_, drawn) = run_ic_against(case.answer);

        assert_eq!(
            columns_of(&drawn),
            Some(case.columns),
            "a terminal that answers {} must get a picture of {} columns, because the terminal draws the picture into that many real cells",
            case.says,
            case.columns
        );
    }
}

#[test]
fn a_terminal_that_answers_no_window_operation_still_gets_a_picture() {
    // The estimate of 10 pixels by 20 is not wrong to exist. A run of `ic` has
    // no second way to show a picture, so a picture at a guessed size beats no
    // picture at all. Ten rows of a cell twice as tall as it is wide are 20
    // columns, and no answer of this file names that count.
    let (_, drawn) = run_ic_against(b"");

    assert_eq!(
        columns_of(&drawn),
        Some(20),
        "a terminal that names no cell gets the estimate of 10 pixels by 20, and it still gets a picture"
    );
}

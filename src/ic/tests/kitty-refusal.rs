//! Black-box tests that hold `ic` to the failure a Kitty terminal reports, and
//! not to the bytes that `ic` wrote.
//!
//! A Kitty terminal answers a transmission that it refused, and it names a code
//! such as `ENOSPC` in that answer. The image store of a mosh session holds a
//! fixed number of bytes, and a picture above that number arrives as exactly
//! such a refusal. `ic` used to write the picture, read nothing, and exit 0. A
//! shell then reported success under an empty screen, and nothing anywhere said
//! why the picture was missing. GitHub issue #465 reports that.
//!
//! Each test here builds the whole round trip. The child takes a
//! pseudo-terminal as its controlling terminal and writes its picture to a
//! pipe, which is the shape of a captured run. This file stands where the
//! terminal stands: it reads the question off the master end of that
//! pseudo-terminal, and it writes the answer of a terminal back.
//!
//! # What the environment of the child has to say
//!
//! `TERM=xterm-kitty` names the terminal Kitty, so the Kitty writer runs and
//! the still picture asks for the failures. No test here sets `MUXIAVELLI`,
//! which would select Sixel.
//!
//! No test here passes `--no-newline` either, and that flag would change
//! nothing: it states who moves the cursor, so a run that carries it draws one
//! still picture, asks for the failures, and reads the answer.
//! `no_newline_still_draws_one_still_picture` in `cursor_contract.rs` holds it
//! to that. Video playback is the path that asks for no answer at all: it draws
//! frame after frame and holds the terminal in raw mode for the key presses of
//! the user, where an answer would arrive as a key press.
//!
//! # Which picture a refusal is about
//!
//! The answer of a terminal reaches whoever reads that terminal next. So a run
//! reads the answer of the run before it, when that answer arrived late, and it
//! reads the answer of a second program that draws Kitty pictures on the same
//! terminal. Both name a picture that this run never sent, and a run that
//! reported them would fail for a picture that drew.
//!
//! The image number tells them apart, and two tests here hold `ic` to it: one
//! answers a refusal of another image number and asks for a success, and one
//! runs `ic` twice and asks the two pictures for two numbers.
//!
//! # The two things that a test of this shape gets wrong
//!
//! `ic` writes the whole picture to standard output before it asks the terminal
//! anything, and a pipe that nobody reads fills and holds the child there for
//! good. So the picture leaves on a thread of its own, and a defect fails this
//! file instead of hanging it.
//!
//! Every wait carries a deadline for the same reason. A change that took the
//! question away would leave a test with no deadline waiting for a byte that
//! nothing writes.

use std::io::{Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;

mod common;

use common::pty::{Pty, Window};
use common::{find, unreachable_path_dir, TEST_IMAGE};

/// The name that this target puts in the unreachable `PATH` of its children.
const TARGET_NAME: &str = "kitty-refusal";

/// The terminal type that names a Kitty terminal, so the Kitty writer runs.
const TERM_XTERM_KITTY: &str = "xterm-kitty";

/// The window that the pseudo-terminal of these tests reports.
///
/// No expectation here rests on the size, because a refusal names no pixel. The
/// numbers are a plain terminal of 80 columns by 24 over a window that measures
/// a character cell of 20 pixels by 40.
const WINDOW: Window = Window {
    columns: 80,
    rows: 24,
    width_px: 1600,
    height_px: 960,
};

/// The request of the primary device attributes.
///
/// `ic` writes this to the terminal after a still picture. Every terminal
/// answers it, so its answer is what ends the read, and a terminal that refused
/// the picture writes the refusal in front of it.
const ATTRIBUTES_REQUEST: &[u8] = b"\x1b[c";

/// The answer of a terminal that refused the picture.
///
/// This is the shape that mosh writes: the refusal of the image store, and then
/// the answer of the attributes that ends the read.
const REFUSAL_ANSWER: &[u8] = b"\x1b_Gi=31;ENOSPC:the image store is full\x1b\\\x1b[?62;4c";

/// The answer of a terminal that drew the picture.
///
/// A terminal that carried the transmission out answers nothing for it, because
/// a still picture asks for the failures alone. So the answer of the attributes
/// stands here by itself.
const ATTRIBUTES_ANSWER: &[u8] = b"\x1b[?62;4c";

/// The answer of a terminal that refused a picture of another run.
///
/// The image number is the one key that says which picture a terminal speaks
/// about, and `I=999999` names a picture that this run never sent. Such an
/// answer reaches a run in two ways: a terminal that answered the run before
/// this one late, and a second program that draws Kitty pictures on the same
/// terminal.
const ANOTHER_PICTURES_REFUSAL: &[u8] =
    b"\x1b_Gi=31,I=999999;ENOSPC:the image store is full\x1b\\\x1b[?62;4c";

/// The code that [`REFUSAL_ANSWER`] carries.
const REFUSED_CODE: &str = "ENOSPC";

/// The detailed message that [`REFUSAL_ANSWER`] carries behind that code.
const REFUSED_DETAIL: &str = "the image store is full";

/// How long a test waits for `ic` to ask the terminal anything.
///
/// A run that works spends almost none of this: the question follows the
/// picture, and the picture of a one-pixel image is a few hundred bytes. The
/// budget is here for a run that asks nothing at all, and it is generous
/// because a loaded machine starts a process late.
const QUESTION_BUDGET: Duration = Duration::from_secs(10);

/// What one run of `ic` under a terminal that answers came to.
struct Run {
    /// True when the request of the primary device attributes reached the
    /// terminal, which is how `ic` asks whether the terminal refused.
    asked: bool,
    /// The exit status of `ic`.
    status: ExitStatus,
    /// What `ic` wrote to standard error.
    stderr: String,
    /// The picture that `ic` wrote to standard output.
    drawn: Vec<u8>,
}

/// Run `ic --stdin` under a terminal that answers `answer`, and report what
/// happened.
///
/// The order of the steps is the order that keeps the run from stopping: the
/// picture leaves on a thread of its own, then this thread waits for the
/// question with a deadline, then it answers, then it reads standard error to
/// the end, and only then does it wait for the child. A read of standard error
/// that stood in front of the answer would wait for a child that waits for its
/// terminal.
///
/// # Arguments
/// * `answer` - The bytes that the terminal says when the question arrives.
///
/// # Returns
/// Whether the question arrived, the exit status of `ic`, its standard error,
/// and the picture that it drew.
///
/// # Panics
/// Panics when the child does not start, does not take the image, or does not
/// report.
fn run_ic_against(answer: &[u8]) -> Run {
    let pty = Pty::open(WINDOW);

    // The environment is empty except for the two variables below, so nothing
    // the test runner inherited can pick another display routine. The `PATH`
    // points at a directory that does not exist, which keeps `ps` out of reach
    // of the remote transport detection.
    let mut command = Command::new(env!("CARGO_BIN_EXE_ic"));
    command
        .arg("--stdin")
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

    // The picture reaches standard output before the question reaches the
    // terminal, and a pipe that nobody reads fills at about 64 kilobytes and
    // holds the child there. This thread stays free to answer the terminal.
    let mut picture = child.stdout.take().expect("ic has no stdout pipe");
    let drawing = thread::spawn(move || {
        let mut drawn = Vec::new();
        picture
            .read_to_end(&mut drawn)
            .expect("failed to read the picture that ic drew");
        drawn
    });

    let question = pty.read_until(ATTRIBUTES_REQUEST, QUESTION_BUDGET);
    let asked = find(&question, ATTRIBUTES_REQUEST).is_some();
    // A child that asked nothing has already exited, and the kernel revokes the
    // terminal of a session leader that exits. Every call on the master end then
    // fails with EIO, so an answer written here would report the revoked
    // terminal instead of the question that never arrived.
    if asked {
        pty.answer(answer);
    }

    // The read ends when the child closes standard error, which is when the
    // child exits. So this read is the wait, and the status comes back after
    // it.
    let mut reported = String::new();
    child
        .stderr
        .take()
        .expect("ic has no stderr pipe")
        .read_to_string(&mut reported)
        .expect("failed to read the standard error of ic");
    let status = child.wait().expect("failed to wait for ic");
    let drawn = drawing.join().expect("the reader of the picture panicked");

    Run {
        asked,
        status,
        stderr: reported,
        drawn,
    }
}

/// The opener of a Kitty graphics command.
const KITTY_OPENER: &[u8] = b"\x1b_G";

/// The byte that divides the control keys of a Kitty command from its payload.
const KITTY_SEPARATOR: char = ';';

/// The byte that divides one control key from the next.
const KEY_SEPARATOR: char = ',';

/// The name of the key that carries an image number, and the equals sign
/// behind it.
///
/// The name is a capital `I`. A lower case `i` is an image id, which names a
/// different thing.
const IMAGE_NUMBER_PREFIX: &str = "I=";

/// The image number that the picture in `drawn` carries, or [`None`] for a
/// picture that names none.
///
/// The picture is `ESC _ G <control keys> ; <payload> ESC \`, and the control
/// keys are `<name>=<value>` pairs that a comma divides. This reads the whole
/// value behind `I=`, so a picture of number 12 reads as 12 and not as 1.
///
/// # Arguments
/// * `drawn` - Every byte that `ic` wrote to standard output.
///
/// # Returns
/// The value of the `I` key of the first Kitty command in `drawn`.
fn image_number_of(drawn: &[u8]) -> Option<String> {
    let start = find(drawn, KITTY_OPENER)? + KITTY_OPENER.len();
    let command = String::from_utf8_lossy(drawn.get(start..)?).into_owned();
    let keys = command.split(KITTY_SEPARATOR).next()?;
    keys.split(KEY_SEPARATOR)
        .find_map(|pair| pair.strip_prefix(IMAGE_NUMBER_PREFIX))
        .map(str::to_owned)
}

/// `ic` must report the refusal that the terminal wrote, and it must exit with
/// a failure.
///
/// The picture did not draw. A run that ends with a success there tells the
/// user that the empty screen is what `ic` meant to leave, and it tells a
/// script the same thing.
#[test]
fn a_refused_picture_names_the_refusal_and_fails() {
    let run = run_ic_against(REFUSAL_ANSWER);

    assert!(
        run.asked,
        "a still picture must ask the terminal whether it refused, and no request of the primary device attributes arrived inside {QUESTION_BUDGET:?}"
    );
    assert!(
        !run.status.success(),
        "ic must fail for a picture that the terminal refused, and it exited with {}. It wrote {:?} to standard error",
        run.status,
        run.stderr
    );
    assert!(
        run.stderr.contains(REFUSED_CODE),
        "the failure must name the code that the terminal wrote, {REFUSED_CODE}, and ic wrote {:?}",
        run.stderr
    );
    assert!(
        run.stderr.contains(REFUSED_DETAIL),
        "the code alone sends the user to a search engine, so the failure must carry the detail of the terminal, {REFUSED_DETAIL:?}, and ic wrote {:?}",
        run.stderr
    );
}

/// A terminal that refused nothing must leave `ic` with a success.
///
/// A read that took every answer for a refusal would fail every picture on
/// every terminal, and the tests above would pass while it did.
#[test]
fn a_picture_that_drew_leaves_ic_with_a_success() {
    let run = run_ic_against(ATTRIBUTES_ANSWER);

    assert!(
        run.asked,
        "a still picture must ask the terminal whether it refused, and no request of the primary device attributes arrived inside {QUESTION_BUDGET:?}"
    );
    assert!(
        run.status.success(),
        "a terminal that refused nothing must leave ic with a success, and it exited with {}. It wrote {:?} to standard error",
        run.status,
        run.stderr
    );
}

/// A refusal that names another picture must leave `ic` with a success.
///
/// The answer of a terminal reaches whoever reads the terminal next. A terminal
/// that answers the run before this one late writes that answer into the input
/// queue, and this run drains it. A second program that draws Kitty pictures on
/// the same terminal writes one there as well. Both of them refuse a picture
/// that this run never sent, and a run that reported them would fail for a
/// picture that drew. A false failure over a good picture is worse than the
/// silence that issue #465 reports.
#[test]
fn a_refusal_for_another_picture_leaves_ic_with_a_success() {
    let run = run_ic_against(ANOTHER_PICTURES_REFUSAL);

    assert!(
        run.asked,
        "a still picture must ask the terminal whether it refused, and no request of the primary device attributes arrived inside {QUESTION_BUDGET:?}"
    );
    assert!(
        run.status.success(),
        "a refusal that names another image number belongs to another picture, so ic must exit with a success, and it exited with {}. It wrote {:?} to standard error",
        run.status,
        run.stderr
    );
    assert!(
        run.stderr.is_empty(),
        "ic must report nothing for a refusal of another picture, and it wrote {:?} to standard error",
        run.stderr
    );
}

/// Two runs of `ic` must carry two image numbers.
///
/// The image number is the one key that says which picture a terminal speaks
/// about. One fixed number gives every run of `ic` the same name, so the
/// refusal of the run before this one names this picture as well and this run
/// reports it. Two numbers are what separate the two runs.
#[test]
fn two_runs_carry_two_image_numbers() {
    let first = run_ic_against(ATTRIBUTES_ANSWER);
    let second = run_ic_against(ATTRIBUTES_ANSWER);

    let first_number = image_number_of(&first.drawn)
        .expect("a still picture must carry an image number, and the first run carried none");
    let second_number = image_number_of(&second.drawn)
        .expect("a still picture must carry an image number, and the second run carried none");

    assert!(
        first_number.parse::<u32>().is_ok_and(|number| number != 0),
        "an image number is a 32-bit number above zero, because zero names no image, and the first run carried {first_number:?}"
    );
    assert_ne!(
        first_number, second_number,
        "two runs of ic must carry two image numbers, and both of them carried {first_number:?}"
    );
}

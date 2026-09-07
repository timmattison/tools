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
//! which would select Sixel, and no test here passes `--no-newline`. That flag
//! draws one frame of a video, which asks the terminal for no answer at all: a
//! caller of that path holds the terminal in raw mode for the key presses of
//! the user, and an answer would arrive there as a key press.
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
/// Whether the question arrived, the exit status of `ic`, and its standard
/// error.
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
    let _drawn = drawing.join().expect("the reader of the picture panicked");

    Run {
        asked,
        status,
        stderr: reported,
    }
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

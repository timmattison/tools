//! Cursor contract tests for the Sixel display routine of `ic`.
//!
//! Sixel gives no contract for the position of the cursor after the string
//! terminator. Each renderer makes its own decision. `ic` must therefore state
//! where the cursor ends, instead of a guess of one newline.
//!
//! These tests drive the real binary and look at the bytes it writes. They
//! measure the cursor movement that the stream *requests*, not the exact escape
//! sequences, so a change of spelling keeps them alive.

use std::io::Write;
use std::process;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// The escape byte that starts every escape sequence.
const ESC: u8 = 0x1b;

/// The second byte of a control sequence introducer.
const CSI_BRACKET: u8 = b'[';

/// The final byte of a CUU (cursor up) sequence.
const CURSOR_UP_FINAL: u8 = b'A';

/// The final byte of a CUD (cursor down) sequence.
const CURSOR_DOWN_FINAL: u8 = b'B';

/// The device control string introducer that opens the Sixel payload.
const SIXEL_START: &[u8] = b"\x1bP";

/// The string terminator that closes the Sixel payload.
const STRING_TERMINATOR: &[u8] = b"\x1b\\";

/// DECSC. It saves the position of the cursor.
const SAVE_CURSOR: &[u8] = b"\x1b7";

/// DECRC. It restores the saved position of the cursor.
const RESTORE_CURSOR: &[u8] = b"\x1b8";

/// The image that the tests send to `ic` on stdin.
const TEST_IMAGE: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/test_image.png"));

/// The terminal type for the child process.
const TEST_TERM: &str = "xterm-256color";

/// A directory that does not exist, unique to this process.
///
/// The `PATH` of the child points here, which keeps `ps` out of reach. The
/// remote transport detection then finds no process tree to walk, so a test
/// runner that is itself under a remote transport cannot change the bytes that
/// `ic` writes. The directory holds the process id and a nanosecond stamp, so
/// two concurrent runs of this file never name the same directory.
///
/// # Returns
/// The path of a directory that no process creates.
fn unreachable_path_dir() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock must be after the epoch")
        .as_nanos();
    format!("/nonexistent-ic-cursor-contract-{}-{nanos}", process::id())
}

/// The number of terminal rows that the test image occupies.
///
/// The test invocation makes a Sixel image of 100 pixels by 100 pixels. One
/// character cell is 20 pixels high, so the image fills 5 rows.
const EXPECTED_ROWS: i64 = 5;

/// The cursor movement that a byte stream requests, in rows.
struct CursorMovement {
    /// The total number of rows of downward movement.
    down: i64,
    /// The total number of rows of upward movement.
    up: i64,
}

impl CursorMovement {
    /// Give the net movement in rows. A positive result is downward.
    fn net(&self) -> i64 {
        self.down - self.up
    }
}

/// Scan a byte slice and measure the cursor movement that it requests.
///
/// A newline moves the cursor down one row. A CUD sequence (`ESC [ n B`) moves
/// the cursor down `n` rows and a CUU sequence (`ESC [ n A`) moves it up `n`
/// rows. A missing or zero parameter means one row, which is what a terminal
/// does. The scan ignores all other bytes.
///
/// # Arguments
/// * `bytes` - The byte stream to scan.
///
/// # Returns
/// The total downward and upward movement in rows.
fn scan_cursor_movement(bytes: &[u8]) -> CursorMovement {
    let mut movement = CursorMovement { down: 0, up: 0 };
    let mut index = 0_usize;

    while index < bytes.len() {
        if bytes[index] == b'\n' {
            movement.down += 1;
            index += 1;
            continue;
        }

        if bytes[index] != ESC || bytes.get(index + 1) != Some(&CSI_BRACKET) {
            index += 1;
            continue;
        }

        let mut end = index + 2;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }

        let parameter: i64 = std::str::from_utf8(&bytes[index + 2..end])
            .ok()
            .and_then(|digits| digits.parse().ok())
            .unwrap_or(0);
        // A missing or zero parameter means one row.
        let rows = if parameter == 0 { 1 } else { parameter };

        match bytes.get(end) {
            Some(&CURSOR_DOWN_FINAL) => movement.down += rows,
            Some(&CURSOR_UP_FINAL) => movement.up += rows,
            _ => {}
        }

        index = end.saturating_add(1).min(bytes.len());
    }

    movement
}

/// Find the first position of a byte pattern in a byte slice.
///
/// # Arguments
/// * `haystack` - The byte slice to search.
/// * `needle` - The byte pattern to look for.
///
/// # Returns
/// The index of the first byte of the match, or `None` when there is no match.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Run `ic` through the Sixel display routine and give back its stdout.
///
/// `MUXIAVELLI` selects the Sixel routine. `ZELLIJ` selects the same routine
/// but it also turns on a process tree heuristic that reads the real process
/// list of the host, which makes the result depend on the machine.
///
/// # Panics
/// Panics when the child process does not start, does not accept the image, or
/// exits with a failure.
fn run_ic_sixel() -> Vec<u8> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ic"))
        .args(["--stdin", "--width", "10", "--height", "5"])
        .env_clear()
        .env("PATH", unreachable_path_dir())
        .env("TERM", TEST_TERM)
        .env("MUXIAVELLI", "1")
        .env("MUXIAVELLI_IMAGE_PROTOCOLS", "sixel")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start ic");

    let mut stdin = child.stdin.take().expect("ic has no stdin pipe");
    stdin
        .write_all(TEST_IMAGE)
        .expect("failed to send the image to ic");
    drop(stdin);

    let output = child.wait_with_output().expect("failed to wait for ic");
    assert!(
        output.status.success(),
        "ic exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    output.stdout
}

/// The bytes after the Sixel string terminator must move the cursor down by the
/// row count of the image, and must put it at column 1. This implementation
/// ends the line with a carriage return.
#[test]
fn sixel_advances_the_cursor_below_the_image() {
    let stdout = run_ic_sixel();

    let terminator =
        find(&stdout, STRING_TERMINATOR).expect("the output must hold a Sixel string terminator");
    let tail = &stdout[terminator + STRING_TERMINATOR.len()..];

    assert_eq!(
        scan_cursor_movement(tail).net(),
        EXPECTED_ROWS,
        "the bytes after the payload must move the cursor down {EXPECTED_ROWS} rows"
    );
    assert!(
        tail.ends_with(b"\r") || tail.ends_with(b"\n"),
        "the stream must end the line, to put the cursor at column 1"
    );
}

/// DECSC must come immediately before the payload and DECRC immediately after
/// it. The brackets make sure that cursor motion inside the payload cannot
/// change the final position of the cursor.
#[test]
fn sixel_brackets_the_payload_against_renderer_cursor_motion() {
    let stdout = run_ic_sixel();

    let payload_start = find(&stdout, SIXEL_START).expect("the output must hold a Sixel payload");
    let terminator =
        find(&stdout, STRING_TERMINATOR).expect("the output must hold a Sixel string terminator");

    assert!(
        payload_start >= SAVE_CURSOR.len(),
        "the stream must save the cursor before the payload"
    );
    assert_eq!(
        &stdout[payload_start - SAVE_CURSOR.len()..payload_start],
        SAVE_CURSOR,
        "DECSC must come immediately before the payload"
    );

    let payload_end = terminator + STRING_TERMINATOR.len();
    assert!(
        stdout.len() >= payload_end + RESTORE_CURSOR.len(),
        "the stream must restore the cursor after the payload"
    );
    assert_eq!(
        &stdout[payload_end..payload_end + RESTORE_CURSOR.len()],
        RESTORE_CURSOR,
        "DECRC must come immediately after the payload"
    );
}

/// The stream must reserve the rows of the image before it draws. It asks for
/// the rows and then takes them back, so an image at the bottom of the screen
/// scrolls the terminal instead of running off it.
#[test]
fn sixel_reserves_the_rows_before_it_draws() {
    let stdout = run_ic_sixel();

    let save = find(&stdout, SAVE_CURSOR).expect("the stream must save the cursor");
    let payload_start = find(&stdout, SIXEL_START).expect("the output must hold a Sixel payload");
    assert!(
        save < payload_start,
        "the reservation must come before the payload"
    );

    let reservation = &stdout[..save];
    let movement = scan_cursor_movement(reservation);
    assert_eq!(
        movement.down, EXPECTED_ROWS,
        "the reservation must ask for {EXPECTED_ROWS} rows before the payload"
    );
    assert_eq!(
        movement.net(),
        0,
        "the reservation must take the rows back, so the image starts at the top of them"
    );
}

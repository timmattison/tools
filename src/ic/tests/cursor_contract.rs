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
use std::process::{Command, Stdio};

/// The escape byte that starts every escape sequence.
const ESC: u8 = 0x1b;

/// The second byte of a control sequence introducer.
const CSI_BRACKET: u8 = b'[';

/// The final byte of a CUU (cursor up) sequence.
const CURSOR_UP_FINAL: u8 = b'A';

/// The final byte of a CUD (cursor down) sequence.
const CURSOR_DOWN_FINAL: u8 = b'B';

/// The string terminator that closes the Sixel payload.
const STRING_TERMINATOR: &[u8] = b"\x1b\\";

/// The image that the tests send to `ic` on stdin.
const TEST_IMAGE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/test_image.png"
));

/// The search path for the child process. `ic` starts `ps` to look at the
/// process tree, so the child needs a path.
const TEST_PATH: &str = "/usr/bin:/bin:/usr/sbin";

/// The terminal type for the child process.
const TEST_TERM: &str = "xterm-256color";

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
        .env("PATH", TEST_PATH)
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

    let terminator = find(&stdout, STRING_TERMINATOR)
        .expect("the output must hold a Sixel string terminator");
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

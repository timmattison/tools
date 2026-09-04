//! The code that the test targets of `ic` share.
//!
//! `cursor_contract` and `controlling-terminal` both read the byte stream that
//! `ic` writes, and both measure the cursor movement that the stream requests.
//! The scanner of that stream is here, so a change to it lands one time. Two
//! copies of a scanner can part company, and no test says so.
//!
//! Only the items that both targets use are here. A name that means two things
//! in the two targets stays in each target, with its own type and its own doc
//! comment. `TERMINAL_ROWS` and `EXPECTED_ROWS` are two such names: one target
//! measures a terminal that it made, and the other counts on a fallback.
//!
//! Cargo makes no test binary out of a subdirectory of `tests`. This file
//! therefore compiles into each target that writes `mod common;`, and cargo
//! builds no third binary from it. Each target compiles its own copy, so an
//! item that one target does not use raises `dead_code` in that target.

use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

/// The escape byte that starts every escape sequence.
pub const ESC: u8 = 0x1b;

/// The second byte of a control sequence introducer.
pub const CSI_BRACKET: u8 = b'[';

/// The final byte of a CUU (cursor up) sequence.
pub const CURSOR_UP_FINAL: u8 = b'A';

/// The final byte of a CUD (cursor down) sequence.
pub const CURSOR_DOWN_FINAL: u8 = b'B';

/// The device control string introducer that opens the Sixel payload.
pub const SIXEL_START: &[u8] = b"\x1bP";

/// The terminal type of a child process that must not look like Kitty.
pub const TERM_XTERM_256COLOR: &str = "xterm-256color";

/// The image that the tests send to `ic` on stdin. It is 1 pixel by 1, so it
/// is square and it holds no aspect ratio of its own to argue with.
pub const TEST_IMAGE: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/test_image.png"));

/// A directory that does not exist, unique to this process.
///
/// The `PATH` of the child points here, which keeps `ps` out of reach. The
/// remote transport detection then finds no process tree to walk, so a test
/// runner that is itself under a remote transport cannot change the bytes that
/// `ic` writes. The directory holds the name of the caller, the process id and
/// a nanosecond stamp, so two concurrent runs never name the same directory.
///
/// # Arguments
/// * `target` - The name of the test target that asks for the directory. It
///   goes into the name of the directory, so a path that reaches a person says
///   which target made it.
///
/// # Returns
/// The path of a directory that no process creates.
///
/// # Panics
/// Panics when the clock of the machine is before the epoch.
pub fn unreachable_path_dir(target: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock must be after the epoch")
        .as_nanos();
    format!("/nonexistent-ic-{target}-{}-{nanos}", process::id())
}

/// The cursor movement that a byte stream requests, in rows.
pub struct CursorMovement {
    /// The total number of rows of downward movement.
    pub down: i64,
    /// The total number of rows of upward movement.
    pub up: i64,
}

impl CursorMovement {
    /// Give the net movement in rows. A positive result is downward.
    ///
    /// # Returns
    /// The downward movement less the upward movement.
    pub fn net(&self) -> i64 {
        self.down - self.up
    }
}

/// Scan a byte slice and measure the cursor movement that it requests.
///
/// A newline moves the cursor down one row. A CUD sequence (`ESC [ n B`) moves
/// the cursor down `n` rows and a CUU sequence (`ESC [ n A`) moves it up `n`
/// rows. A missing or zero parameter means one row, which is what a terminal
/// does. The scan ignores all other bytes, and a Sixel payload holds none of
/// the three, so the payload adds nothing to the count.
///
/// # Arguments
/// * `bytes` - The byte stream to scan.
///
/// # Returns
/// The total downward and upward movement in rows.
pub fn scan_cursor_movement(bytes: &[u8]) -> CursorMovement {
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
pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

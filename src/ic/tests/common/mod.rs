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
//! Cargo makes no test binary out of a subdirectory of `tests`. This module
//! therefore compiles into each target that writes `mod common;`, and cargo
//! builds no binary of its own from it. Each target compiles its own copy, and
//! **no target uses every item here**, so each copy holds items that the target
//! around it never calls.
//!
//! `dead_code` is off for that reason, and the reason is the compile model and
//! not a habit. `cursor_contract` takes the controlling terminal away from its
//! children and gives them no terminal of its own, so [`pty::Pty`] is dead
//! there and [`pty::take_the_terminal_away`] is not. `kitty-refusal` reads the
//! failure that a terminal reports and no picture at all, so
//! [`scan_cursor_movement`] is dead there. The alternative is a copy of each
//! item in each target that wants it, and two copies of sixty lines of
//! `openpty` and `TIOCSCTTY` part company on the day either changes.
#![allow(
    dead_code,
    reason = "each target compiles its own copy of this module and no target calls every item of it, so the lint reports the compile model instead of an unused item"
)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

pub mod pty;

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

/// A directory holding a `ps` that reports this session as a Mosh session.
///
/// # Why a test needs one
///
/// `ic` names the remote transport from the process tree, which it reads by
/// running `ps`. A test cannot put a real `mosh-server` above itself, and a
/// test that reaches the real `ps` reads the machine of whoever runs the suite:
/// the verdict then turns on whether that person is under Mosh, which is the
/// same class of defect as a test that reads the terminal of the runner.
///
/// So the test states the process tree instead. This directory stands first on
/// the `PATH` of the child and holds a `ps` that prints one table. **The shell
/// that runs the script reads its own parent as `PPID`, and that parent is
/// `ic`**, so the table names the exact process that reads it and no test has
/// to guess a process id.
///
/// The directory carries the process id and a nanosecond stamp, so two
/// concurrent runs never name the same one, and [`Drop`] takes it away again.
pub struct MoshProcessTable {
    directory: PathBuf,
}

impl MoshProcessTable {
    /// Build the directory and write the `ps` into it.
    ///
    /// # Arguments
    /// * `target` - The name of the test target that asks for it, which goes
    ///   into the name of the directory.
    ///
    /// # Panics
    /// Panics when the directory or the script cannot be written, because a
    /// test that ran without them would report on the machine of the runner.
    #[must_use]
    pub fn new(target: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock must be after the epoch")
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("ic-{target}-ps-{}-{nanos}", process::id()));
        fs::create_dir_all(&directory).expect("the fake ps needs a directory to stand in");

        let script = directory.join("ps");
        fs::write(
            &script,
            "#!/bin/sh\n\
             # The comm snapshot names mosh-server as the parent of whoever ran this\n\
             # script, which is `ic`. Every other question gets an empty table: only\n\
             # a Zellij session reads the argument snapshot, and the rule about a\n\
             # direct ancestor answers before that snapshot is read.\n\
             case \"$*\" in\n\
             *comm=*)\n\
             \techo \"    1     0 /sbin/launchd\"\n\
             \techo \"  100     1 /usr/bin/mosh-server\"\n\
             \techo \"$PPID 100 /usr/local/bin/ic\"\n\
             \t;;\n\
             esac\n",
        )
        .expect("the fake ps must be written");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .expect("the fake ps must be executable");

        Self { directory }
    }

    /// The `PATH` that reaches this `ps` and reaches no other program.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.directory
    }
}

impl Drop for MoshProcessTable {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

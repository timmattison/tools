//! Black-box tests that measure `ic` against the terminal of the session, and
//! not against the file that standard output points at.
//!
//! A caller that captures the standard output of `ic` takes away the one file
//! descriptor that `ic` once read for every probe of the size of the terminal.
//! `ic` then fell back to 80 columns by 24 rows and to a character cell of 10
//! pixels by 20, and it drew the image at that guessed size. The guess is
//! wrong on every display of a high pixel density, so the image came out too
//! small and `ic` reserved more rows than the image covered. GitHub issue #350
//! reports that.
//!
//! The probe now reads standard output, then standard error, then standard
//! input, and then `/dev/tty`. A captured run therefore measures the terminal
//! of the session and draws the image at the size that terminal gives. These
//! tests hold `ic` to that behavior.
//!
//! Each test here gives the child a pipe for standard output and a
//! pseudo-terminal of a known size as its controlling terminal. The session
//! therefore holds a terminal that `ic` can measure through `/dev/tty`, and
//! standard output holds none. That pair is the shape of a captured run, and
//! it is the shape that these tests hold `ic` to.
//!
//! The pseudo-terminal reports 80 columns and 24 rows over a window of 1600
//! pixels by 960, which measures a character cell of 20 pixels by 40. Neither
//! number is a multiple of the estimate of 10 pixels by 20, so an answer that
//! comes from the estimate can never look like an answer that comes from the
//! terminal.

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process;
use std::process::{Command, Stdio};
use std::ptr;
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

/// The byte that opens the raster attributes of a Sixel payload.
const RASTER_INTRODUCER: u8 = b'"';

/// The number of raster attributes that a Sixel payload carries.
///
/// They are `Pan`, `Pad`, `Ph` and `Pv`. The first two give the aspect ratio
/// of one Sixel, and the last two give the width and the height of the image
/// in pixels.
const RASTER_ATTRIBUTE_COUNT: usize = 4;

/// The position of `Ph`, the width of the image in pixels, in the raster
/// attributes.
const RASTER_WIDTH_INDEX: usize = 2;

/// The position of `Pv`, the height of the image in pixels, in the raster
/// attributes.
const RASTER_HEIGHT_INDEX: usize = 3;

/// The terminal type of a child that must not look like Kitty.
const TERM_XTERM_256COLOR: &str = "xterm-256color";

/// The width of the pseudo-terminal of the tests, in columns.
const TERMINAL_COLUMNS: u16 = 80;

/// The height of the pseudo-terminal of the tests, in rows.
const TERMINAL_ROWS: u16 = 24;

/// The width of the window of the pseudo-terminal, in pixels.
///
/// 1600 pixels over 80 columns measures a character cell 20 pixels wide, which
/// is twice the estimate of 10 pixels.
const TERMINAL_WIDTH_PX: u16 = 1600;

/// The height of the window of the pseudo-terminal, in pixels.
///
/// 960 pixels over 24 rows measures a character cell 40 pixels high, which is
/// twice the estimate of 20 pixels.
const TERMINAL_HEIGHT_PX: u16 = 960;

/// The width of one character cell of the pseudo-terminal, in pixels.
///
/// The measure comes off the two constants above, so a change to the window
/// carries through to every number below it.
const CELL_WIDTH_PX: u32 = TERMINAL_WIDTH_PX as u32 / TERMINAL_COLUMNS as u32;

/// The height of one character cell of the pseudo-terminal, in pixels.
const CELL_HEIGHT_PX: u32 = TERMINAL_HEIGHT_PX as u32 / TERMINAL_ROWS as u32;

/// The width of the image that the tests ask for, in character cells.
const IMAGE_COLUMNS: u32 = 20;

/// The height of the image that the tests ask for, in character cells.
const IMAGE_ROWS: u32 = 10;

/// The width in pixels that the Sixel image must have.
///
/// 20 columns of 20 pixels is 400 pixels. The horizontal margin of 95 percent
/// of 1600 pixels is 1520 pixels, which is larger, so the budget of the caller
/// binds. The test image is square, so the height matches the width.
const EXPECTED_SIXEL_WIDTH_PX: u32 = IMAGE_COLUMNS * CELL_WIDTH_PX;

/// The height in pixels that the Sixel image must have.
const EXPECTED_SIXEL_HEIGHT_PX: u32 = EXPECTED_SIXEL_WIDTH_PX;

/// The number of terminal rows that the Sixel image must cover.
///
/// 400 pixels over a character cell 40 pixels high is 10 rows. `ic` reserves
/// one row for each row of the image, so this is also the number of rows that
/// the stream must move the cursor down. The type is `i64`, because
/// [`scan_cursor_movement`] counts a movement up as well as a movement down.
const EXPECTED_ROWS: i64 = (EXPECTED_SIXEL_HEIGHT_PX / CELL_HEIGHT_PX) as i64;

/// The image that the tests send to `ic` on stdin. It is 1 pixel by 1, so it
/// is square and it holds no aspect ratio of its own to argue with.
const TEST_IMAGE: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/test_image.png"));

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
    format!(
        "/nonexistent-ic-controlling-terminal-{}-{nanos}",
        process::id()
    )
}

/// A pseudo-terminal of the size that the tests measure `ic` against.
///
/// The pair of file descriptors stays open for the life of the child. The
/// master end holds the pseudo-terminal alive, and the slave end is the
/// terminal that the child takes as its own.
///
/// The size arrives with the pseudo-terminal, so no second ioctl sets it and
/// no window of the wrong size ever exists.
struct Pty {
    /// The master end. The parent holds it open and reads nothing from it,
    /// because the child writes its image to a pipe and not to the terminal.
    master: libc::c_int,
    /// The slave end. It becomes the controlling terminal of the child.
    slave: libc::c_int,
}

impl Pty {
    /// Open a pseudo-terminal of the size of the tests.
    ///
    /// # Returns
    /// The two ends of a pseudo-terminal that reports [`TERMINAL_COLUMNS`] by
    /// [`TERMINAL_ROWS`] over a window of [`TERMINAL_WIDTH_PX`] by
    /// [`TERMINAL_HEIGHT_PX`].
    ///
    /// # Panics
    /// Panics when the system opens no pseudo-terminal.
    fn open() -> Self {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        let mut size = libc::winsize {
            ws_row: TERMINAL_ROWS,
            ws_col: TERMINAL_COLUMNS,
            ws_xpixel: TERMINAL_WIDTH_PX,
            ws_ypixel: TERMINAL_HEIGHT_PX,
        };

        // SAFETY: `openpty` writes one file descriptor to each of the first two
        // pointers, and both point at a live local variable. The two null
        // pointers are the documented way to ask for the default terminal modes
        // and to ask for no name of the slave device. The last pointer is the
        // size of the window, and it points at a live local variable that
        // outlives the call.
        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut size,
            )
        };
        assert_eq!(
            result,
            0,
            "openpty must give a pseudo-terminal: {}",
            std::io::Error::last_os_error()
        );

        Pty { master, slave }
    }
}

impl Drop for Pty {
    /// Close both ends of the pseudo-terminal.
    ///
    /// A test that leaks a file descriptor for each run empties the table of
    /// the process, and the runs of this file share one process.
    fn drop(&mut self) {
        // SAFETY: each descriptor came from the one `openpty` call of
        // [`Pty::open`], nothing else closes them, and `Drop` runs one time.
        unsafe {
            libc::close(self.slave);
            libc::close(self.master);
        }
    }
}

/// Make a command that runs `ic` with a pipe for standard output and a
/// pseudo-terminal for the session.
///
/// The child starts a new session and then claims the slave end of the
/// pseudo-terminal as its controlling terminal. `/dev/tty` in the child
/// therefore resolves to that pseudo-terminal, while standard output stays a
/// pipe. That is the shape of a captured run: the terminal of the session is
/// there to measure, and standard output cannot measure it.
///
/// `pre_exec` runs between the fork and the exec, so the slave descriptor is
/// still open at that moment and the close-on-exec flag changes nothing.
///
/// The environment is empty except for the four variables below, so nothing
/// the test runner inherited can pick a different display routine. `MUXIAVELLI`
/// selects the Sixel routine, and the `PATH` points at a directory that does
/// not exist, which keeps `ps` out of reach of the remote transport detection.
///
/// # Arguments
/// * `pty` - The pseudo-terminal that the child takes as its own.
/// * `args` - The full command line for `ic`.
///
/// # Returns
/// A command with the environment, the pipes and the session of the tests
/// already set.
fn ic_command(pty: &Pty, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ic"));
    command
        .args(args)
        .env_clear()
        .env("PATH", unreachable_path_dir())
        .env("TERM", TERM_XTERM_256COLOR)
        .env("MUXIAVELLI", "1")
        .env("MUXIAVELLI_IMAGE_PROTOCOLS", "sixel")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let slave = pty.slave;
    // SAFETY: the closure runs in the child between the fork and the exec, and
    // it calls two functions. `setsid` and `ioctl` are both async-signal-safe,
    // and neither one touches memory of this process: the ioctl takes the
    // request `TIOCSCTTY`, which reads no pointer. The child is never a process
    // group leader in that window, because the fork gave it a new process id
    // and the process group is still the one of the parent, so the one
    // documented failure of `setsid` cannot happen.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }

            #[allow(
                clippy::disallowed_methods,
                reason = "the ban covers the read of a window, and `TIOCSCTTY` reads none. It claims the pseudo-terminal as the controlling terminal of the child, and termsize offers no call for that"
            )]
            if libc::ioctl(slave, libc::c_ulong::from(libc::TIOCSCTTY), 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }

            Ok(())
        });
    }

    command
}

/// Run `ic` with the image on stdin and give back the bytes it wrote to
/// standard output.
///
/// The pseudo-terminal lives for the whole call, so it is still the terminal of
/// the child while the child runs, and both of its ends close when the call
/// ends.
///
/// # Arguments
/// * `args` - The full command line for `ic`.
///
/// # Returns
/// The bytes that `ic` wrote to stdout.
///
/// # Panics
/// Panics when the child process does not start, does not accept the image, or
/// exits with a failure.
fn run_ic(args: &[&str]) -> Vec<u8> {
    let pty = Pty::open();
    let mut child = ic_command(&pty, args).spawn().expect("failed to start ic");

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
/// does. The scan ignores all other bytes, and a Sixel payload holds none of
/// the three, so the payload adds nothing to the count.
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

/// Read the size in pixels of the Sixel image in a byte stream.
///
/// A Sixel payload opens with `ESC P`, and the raster attributes follow as
/// `" Pan ; Pad ; Ph ; Pv`. `Pan` and `Pad` give the aspect ratio of one Sixel,
/// `Ph` gives the width of the image in pixels and `Pv` gives the height. The
/// size that `ic` chose is therefore in the stream itself, and this test needs
/// no second opinion about it.
///
/// # Arguments
/// * `bytes` - The byte stream to read.
///
/// # Returns
/// The width and the height of the Sixel image in pixels.
///
/// # Panics
/// Panics when the stream holds no Sixel payload, when the payload carries no
/// raster attributes, or when the attributes hold fewer than four numbers.
fn sixel_raster_size(bytes: &[u8]) -> (u32, u32) {
    let payload_start =
        find(bytes, SIXEL_START).expect("the output must hold a Sixel payload") + SIXEL_START.len();
    let payload = &bytes[payload_start..];
    let mut index = payload
        .iter()
        .position(|byte| *byte == RASTER_INTRODUCER)
        .expect("a Sixel payload must carry raster attributes")
        + 1;

    let mut attributes = Vec::with_capacity(RASTER_ATTRIBUTE_COUNT);
    while attributes.len() < RASTER_ATTRIBUTE_COUNT {
        let mut end = index;
        while end < payload.len() && payload[end].is_ascii_digit() {
            end += 1;
        }
        assert!(
            end > index,
            "raster attribute {} must be a number",
            attributes.len() + 1
        );

        let digits = std::str::from_utf8(&payload[index..end])
            .expect("a run of ASCII digits is always UTF-8");
        attributes.push(
            digits
                .parse::<u32>()
                .expect("a raster attribute must fit in a u32"),
        );

        index = end;
        if payload.get(index) == Some(&b';') {
            index += 1;
        }
    }

    (
        attributes[RASTER_WIDTH_INDEX],
        attributes[RASTER_HEIGHT_INDEX],
    )
}

/// `ic` must measure the terminal of the session when standard output is a
/// pipe, and it must draw the image at the size that terminal gives.
///
/// The arithmetic of `src/termgfx/src/geometry.rs` for
/// `ic --stdin --width 20 --height 10` in this pseudo-terminal:
///
/// * The terminal reports 1600 pixels by 960 over 80 columns by 24 rows, so
///   one character cell is 20 pixels by 40.
/// * The budget of the caller is 20 columns by 10 rows, which is 400 pixels by
///   400.
/// * The margins are 95 percent of 1600 pixels, which is 1520, and 90 percent
///   of 960 pixels, which is 864. Both are larger than the budget, so the
///   budget binds on both axes.
/// * The image is square, so the Sixel image is 400 pixels by 400.
/// * 400 pixels over a cell 40 pixels high is 10 rows, and `ic` reserves one
///   row for each row of the image.
///
/// A probe that read standard output alone would measure nothing here,
/// because standard output is a pipe. It would fall back to a character cell
/// of 10 pixels by 20 and draw 200 pixels by 200, so this test fails on the
/// raster attributes if `ic` ever stops reading the terminal of the session.
#[test]
fn a_sized_terminal_gives_the_pixel_size_of_the_image() {
    let stdout = run_ic(&["--stdin", "--width", "20", "--height", "10"]);

    assert_eq!(
        sixel_raster_size(&stdout),
        (EXPECTED_SIXEL_WIDTH_PX, EXPECTED_SIXEL_HEIGHT_PX),
        "a cell of {CELL_WIDTH_PX} pixels by {CELL_HEIGHT_PX} gives {IMAGE_COLUMNS} columns by {IMAGE_ROWS} rows a Sixel image of {EXPECTED_SIXEL_WIDTH_PX} pixels by {EXPECTED_SIXEL_HEIGHT_PX}"
    );
    assert_eq!(
        scan_cursor_movement(&stdout).net(),
        EXPECTED_ROWS,
        "an image of {EXPECTED_SIXEL_HEIGHT_PX} pixels covers {EXPECTED_ROWS} rows of {CELL_HEIGHT_PX} pixels, so the stream must move the cursor down {EXPECTED_ROWS} rows"
    );
}

/// `ic` must measure the terminal of the session for the axis that the user
/// leaves out, and not fall back to a default size of the image.
///
/// The arithmetic of `src/termgfx/src/geometry.rs` for
/// `ic --stdin --width 20` in this pseudo-terminal:
///
/// * One character cell is 20 pixels by 40, as above.
/// * The budget of the caller is 20 columns, which is 400 pixels. It has no
///   row count, so the vertical margin of 90 percent of 960 pixels, which is
///   864, is the only bound on the height.
/// * The image is square, so the smaller side binds and the Sixel image is 400
///   pixels by 400.
/// * That is 10 rows of 40 pixels, the same as the test above.
///
/// A probe that read standard output alone would measure nothing here. With
/// no pixel size and only one axis of the budget, `sixel_pixel_budget` gives
/// its default of 800 pixels by 600, a square image turns that into 600 pixels
/// by 600, and 600 pixels over the estimated cell of 20 pixels is 30 rows. The
/// height of the fallback terminal then bounds the reservation down to 23
/// rows. This test therefore fails on the raster attributes and on the row
/// count if `ic` ever stops reading the terminal of the session.
#[test]
fn a_sized_terminal_bounds_the_axis_that_the_user_leaves_out() {
    let stdout = run_ic(&["--stdin", "--width", "20"]);

    assert_eq!(
        sixel_raster_size(&stdout),
        (EXPECTED_SIXEL_WIDTH_PX, EXPECTED_SIXEL_HEIGHT_PX),
        "a width of {IMAGE_COLUMNS} columns is {EXPECTED_SIXEL_WIDTH_PX} pixels, and a square image inside a height of 864 pixels keeps that width on both axes"
    );
    assert_eq!(
        scan_cursor_movement(&stdout).net(),
        EXPECTED_ROWS,
        "an image of {EXPECTED_SIXEL_HEIGHT_PX} pixels covers {EXPECTED_ROWS} rows of {CELL_HEIGHT_PX} pixels, so the stream must move the cursor down {EXPECTED_ROWS} rows"
    );
}

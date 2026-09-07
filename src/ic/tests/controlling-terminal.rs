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
use std::process::{Command, Stdio};

mod common;

use common::pty::{Pty, Window};
use common::{
    find, scan_cursor_movement, unreachable_path_dir, SIXEL_START, TERM_XTERM_256COLOR, TEST_IMAGE,
};

/// The name that this target puts in the unreachable `PATH` of its children.
const TARGET_NAME: &str = "controlling-terminal";

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

/// The window that the pseudo-terminal of these tests reports.
///
/// The four numbers are the ones that every expectation below is built on, so
/// they arrive at [`Pty::open`] from the same constants that the arithmetic
/// reads.
const WINDOW: Window = Window {
    columns: TERMINAL_COLUMNS,
    rows: TERMINAL_ROWS,
    width_px: TERMINAL_WIDTH_PX,
    height_px: TERMINAL_HEIGHT_PX,
};

/// Make a command that runs `ic` with a pipe for standard output and a
/// pseudo-terminal for the session.
///
/// [`Pty::hand_to`] gives the child a session of its own and the slave end of
/// the pseudo-terminal as its controlling terminal. `/dev/tty` in the child
/// therefore resolves to that pseudo-terminal, while standard output stays a
/// pipe. That is the shape of a captured run: the terminal of the session is
/// there to measure, and standard output cannot measure it.
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
        .env("PATH", unreachable_path_dir(TARGET_NAME))
        .env("TERM", TERM_XTERM_256COLOR)
        .env("MUXIAVELLI", "1")
        .env("MUXIAVELLI_IMAGE_PROTOCOLS", "sixel")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    pty.hand_to(&mut command);

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
    let pty = Pty::open(WINDOW);
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

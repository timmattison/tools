//! Black-box tests of `MOSH_IMAGE_BUDGETS`, the variable that states what one
//! picture can spend in each protocol that a mosh session carries.
//!
//! # Why the name of a variable needs a test of its own
//!
//! The name is the whole contract between this tool and mosh. `termgfx` reads
//! it in `MoshImages::detect`, and a typo there fails in silence: every
//! protocol falls back to the careful number that `termgfx` holds, every
//! picture still draws, and the user only gets a smaller picture than the
//! transport allows. Nothing reports that. No unit test of `termgfx` sees it
//! either, because a unit test states the value of the variable and reads no
//! environment at all.
//!
//! That silence is the defect of
//! <https://github.com/timmattison/tools/issues/480>, read one level up: a
//! number that stands under the real cap draws a smaller picture than the
//! session allows, and nothing says so. The sibling name `MOSH_IMAGES` is
//! pinned already, because `will-display` states it on the real binary and
//! asserts the refusal that it lifts. This target pins the third name the same
//! way.
//!
//! # What one run proves and what two runs prove
//!
//! One run under a small cap proves nothing on its own. A picture that always
//! stood under the cap passes that assertion whatever the tool read, so this
//! file states the cap **and** runs the same picture without it. The run
//! without the variable writes a transmission far above the cap, so the pair of
//! assertions holds the tool to reading the name and to reading it correctly.
//!
//! Every run drives the real binary with a cleared environment and no
//! controlling terminal, so the numbers come from the variables this file
//! states and from nothing the test runner inherited. The `PATH` reaches the
//! stated `ps` of [`MoshProcessTable`] and reaches nothing else, so the
//! transport comes from that table and not from the machine of whoever runs the
//! suite.

use std::io::{Cursor, Write};
use std::process::{Command, Stdio};

mod common;

use common::pty::take_the_terminal_away;
use common::{find, MoshProcessTable};

/// The name that this target puts in the temporary directory of its `ps`.
const TARGET_NAME: &str = "mosh-budget";

/// The terminal type that names a Kitty terminal.
const TERM_XTERM_KITTY: &str = "xterm-kitty";

/// The window id that a Kitty terminal writes into the environment of a pane.
const KITTY_WINDOW: &str = "1";

/// The environment variable that names what the transport of the session
/// carries.
const TRANSPORT_VARIABLE: &str = "MOSH_IMAGES";

/// The environment variable that states what one picture can spend in each
/// protocol that the transport carries.
///
/// **This file states the name, and `termgfx` states it again.** That is the
/// point of this target: the two spellings have to agree, and a run of the real
/// binary is the one place where a disagreement shows itself.
const BUDGETS_VARIABLE: &str = "MOSH_IMAGE_BUDGETS";

/// The name that both variables give the Kitty graphics protocol.
///
/// `MOSH_IMAGES` names the protocol on its own, and `MOSH_IMAGE_BUDGETS` puts
/// the same name in front of the cap of it.
const KITTY_NAME: &str = "kitty";

/// The application program command that opens a Kitty graphics command.
const KITTY_START: &[u8] = b"\x1b_G";

/// The string terminator that closes a Kitty graphics command.
const STRING_TERMINATOR: &[u8] = b"\x1b\\";

/// The width of the picture in character cells.
const PICTURE_COLUMNS: u32 = 40;

/// The height of the picture in character cells.
const PICTURE_ROWS: u32 = 12;

/// The width of one character cell in pixels.
///
/// A child with no controlling terminal measures no cell, and `termgfx` falls
/// back to an estimate of 10 pixels by 20. Every run of this file takes that
/// fallback, so the picture reaches the writer at the size that this file
/// states and at no size of a window of a person.
const CELL_WIDTH_PIXELS: u32 = 10;

/// The height of one character cell in pixels.
const CELL_HEIGHT_PIXELS: u32 = 20;

/// The width of the picture in pixels.
const PICTURE_WIDTH_PIXELS: u32 = PICTURE_COLUMNS * CELL_WIDTH_PIXELS;

/// The height of the picture in pixels.
const PICTURE_HEIGHT_PIXELS: u32 = PICTURE_ROWS * CELL_HEIGHT_PIXELS;

/// The cap that this file states for the Kitty graphics protocol.
///
/// **The number comes from a measurement and not from a guess.** The grainy
/// picture of [`grainy_picture`] wrote a transmission of 385327 characters on
/// 2026-09-11 in a run that stated no cap, and it wrote 109608 characters in a
/// run that stated this one. So the cap stands well under the picture, which
/// gives the fit real work to do, and the picture that arrives still spends
/// nine tenths of the cap, which is a picture a reader sees.
///
/// The number is far below `PayloadBudget::MOSH`, the careful fallback of
/// 1044480 characters, which is what makes the second assertion of
/// [`a_stated_kitty_cap_bounds_the_whole_transmission`] catch a name that `ic`
/// never read.
const KITTY_CAP: usize = 120_000;

/// The share of the cap that the picture under the cap has to spend.
///
/// A fit that threw the picture away would stand under the cap as well, and the
/// first assertion alone reads that as a pass. A picture that spends more than
/// half of the room it was given is a picture, and not the one pixel that a
/// budget of zero leaves.
const SPENT_SHARE_DIVISOR: usize = 2;

/// One channel of one pixel of the grainy picture.
///
/// The value comes from the position alone, so the picture is the same on every
/// run and on every machine, and the function holds no state that a second call
/// could disturb.
///
/// # Arguments
/// * `x` - The column of the pixel.
/// * `y` - The row of the pixel.
/// * `plane` - The number of the channel, which keeps the three channels of one
///   pixel apart.
///
/// # Returns
/// A value that carries no pattern a compressor can find.
fn channel(x: u32, y: u32, plane: u32) -> u8 {
    let mut hash = x
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(y.wrapping_mul(0x85EB_CA6B))
        .wrapping_add(plane.wrapping_mul(0xC2B2_AE35));
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2545_F491);
    hash ^= hash >> 13;
    hash.to_le_bytes()[0]
}

/// A PNG file of a grainy picture, at the exact pixel size that a run draws.
///
/// **The grain is what makes these tests measure anything.** `ic` sends a still
/// picture as a PNG, and a PNG of flat color costs a few hundred characters
/// whatever its size, so a flat picture stands under every cap already and a run
/// of it proves nothing. This picture carries no pattern that the encoder can
/// compress, so it costs about four characters for each pixel and it reaches a
/// size that a stated cap has to cut.
///
/// The picture arrives at the size that the run draws at, so no resize stands
/// between the grain and the encoder.
///
/// # Returns
/// The bytes of the PNG file.
///
/// # Panics
/// Panics when the PNG encoder refuses the picture.
fn grainy_picture() -> Vec<u8> {
    let pixels = image::RgbImage::from_fn(PICTURE_WIDTH_PIXELS, PICTURE_HEIGHT_PIXELS, |x, y| {
        image::Rgb([channel(x, y, 0), channel(x, y, 1), channel(x, y, 2)])
    });

    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(pixels)
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("the PNG encoder must accept an RGB picture");

    png.into_inner()
}

/// Run the real `ic` inside a session that the process tree reports as mosh,
/// and give back the bytes that it wrote to standard output.
///
/// The child gets a cleared environment, the `PATH` of `table` and no
/// controlling terminal. So it reads the stated `ps`, it measures no terminal,
/// and it reaches no terminal of a person.
///
/// # Arguments
/// * `table` - The stated process tree that names mosh above `ic`.
/// * `budgets` - The value of `MOSH_IMAGE_BUDGETS`, or `None` for a run that
///   states no cap at all.
///
/// # Returns
/// The bytes that `ic` wrote to standard output.
///
/// # Panics
/// Panics when `ic` does not start, when it does not accept the picture, or
/// when it exits with a failure.
fn run_ic(table: &MoshProcessTable, budgets: Option<&str>) -> Vec<u8> {
    let columns = PICTURE_COLUMNS.to_string();
    let rows = PICTURE_ROWS.to_string();

    let mut command = Command::new(env!("CARGO_BIN_EXE_ic"));
    command
        .args(["--stdin", "--width", &columns, "--height", &rows])
        .env_clear()
        .env("PATH", table.path())
        .env("TERM", TERM_XTERM_KITTY)
        .env("KITTY_WINDOW_ID", KITTY_WINDOW)
        .env(TRANSPORT_VARIABLE, KITTY_NAME)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    take_the_terminal_away(&mut command);

    if let Some(value) = budgets {
        command.env(BUDGETS_VARIABLE, value);
    }

    let mut child = command.spawn().expect("ic must run");
    let mut stdin = child.stdin.take().expect("ic has no stdin pipe");
    stdin
        .write_all(&grainy_picture())
        .expect("failed to send the picture to ic");
    drop(stdin);

    let done = child.wait_with_output().expect("failed to wait for ic");
    assert!(
        done.status.success(),
        "ic exited with {}: {}",
        done.status,
        String::from_utf8_lossy(&done.stderr)
    );

    done.stdout
}

/// Measure the whole Kitty transmission that a run wrote.
///
/// A cap of mosh counts the command of the protocol together with the payload,
/// and a large payload goes out in one command for each chunk of it. So the
/// stretch that a cap covers runs from the first `ESC _ G` of the stream to the
/// **last** string terminator of it, and every command and every payload
/// between the two belongs to the picture. A measurement that stopped at the
/// first terminator would read one chunk and call it the picture.
///
/// # Arguments
/// * `stdout` - The bytes that `ic` wrote to standard output.
///
/// # Returns
/// The number of characters of the whole transmission.
///
/// # Panics
/// Panics when the stream holds no Kitty graphics command, when it closes none,
/// or when it closes one before it opens one.
fn kitty_transmission_characters(stdout: &[u8]) -> usize {
    let start = find(stdout, KITTY_START).expect("the run must write a Kitty graphics command");
    let end = stdout
        .windows(STRING_TERMINATOR.len())
        .rposition(|window| window == STRING_TERMINATOR)
        .expect("the run must close its Kitty transmission")
        + STRING_TERMINATOR.len();

    assert!(
        end > start,
        "the run must close its Kitty transmission after it opens one"
    );

    end - start
}

/// A cap that the session states in `MOSH_IMAGE_BUDGETS` bounds the whole Kitty
/// transmission, and a session that states none writes a transmission far above
/// that cap.
///
/// The two runs draw the same picture through the same terminal on the same
/// stated process tree. The value of `MOSH_IMAGE_BUDGETS` is the one thing that
/// parts them, so the difference between the two numbers belongs to that
/// variable and to nothing else.
///
/// The behavior already held when this test arrived, so a mutation proved that
/// the test can fail. It was measured on 2026-09-11.
///
/// * **A `BUDGETS_VARIABLE` of `MOSH_IMAGE_BUDGET` in
///   `src/termgfx/src/session.rs`.** One letter short of the name that mosh
///   writes. `MoshImages::detect` then reads no cap at all, every protocol
///   falls back to `PayloadBudget::MOSH` of 1044480 characters, and the
///   picture goes out whole: the run under the stated cap writes 385327
///   characters against a cap of 120000, and the first assertion reports it as
///   `a stated cap of 120000 characters must bound the whole Kitty
///   transmission, and the run wrote 385327 characters`. Every unit test of
///   `termgfx` passes with that mutation in place, because a unit test states
///   the value of the variable and never reads its name.
#[test]
fn a_stated_kitty_cap_bounds_the_whole_transmission() {
    let table = MoshProcessTable::new(TARGET_NAME);

    let stated =
        kitty_transmission_characters(&run_ic(&table, Some(&format!("{KITTY_NAME}={KITTY_CAP}"))));
    assert!(
        stated <= KITTY_CAP,
        "a stated cap of {KITTY_CAP} characters must bound the whole Kitty transmission, and the run wrote {stated} characters"
    );
    assert!(
        stated > KITTY_CAP / SPENT_SHARE_DIVISOR,
        "the picture under that cap must still spend the room it was given, and the run wrote {stated} characters of the {KITTY_CAP} it had"
    );

    let silent = kitty_transmission_characters(&run_ic(&table, None));
    assert!(
        silent > KITTY_CAP,
        "the same picture with no {BUDGETS_VARIABLE} must stand above that cap, or the run under the cap proves nothing about the variable, and it wrote {silent} characters"
    );
}

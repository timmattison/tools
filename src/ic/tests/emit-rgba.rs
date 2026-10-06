//! Black-box tests for `--emit-rgba`, which gives the pixels of a picture to a
//! program that draws the picture itself.
//!
//! Some programs own the whole screen. Claude Code is one: it draws every cell
//! of its window and it draws each cell again when something changes, so an
//! escape sequence that a child writes past it lands at a position that the
//! next frame paints over. Such a program draws a picture with its own
//! elements, and it needs the pixels and nothing else. `ic` decodes every
//! format that the `image` crate reads, and `--emit-rgba` gives that decode to
//! the program: the picture, fitted inside a box of pixels, as JSON on
//! standard output.
//!
//! # The flag draws nothing and asks nothing
//!
//! The program that runs `ic` this way owns the terminal, and it reads the
//! keys of the user from it. A query that `ic` wrote there would put an answer
//! in front of that program, which reads the answer as keys that the user
//! typed. So [`emit_rgba_writes_nothing_to_the_terminal`] gives the run a
//! pseudo-terminal that a display would ask, and it holds the run to an empty
//! terminal. [`emit_rgba_ignores_a_terminal_that_draws_no_picture`] holds the
//! other half: a terminal that draws no picture refuses a display, and it
//! refuses no decode.
//!
//! # What each test builds
//!
//! Every run starts from a cleared environment and a `PATH` that reaches
//! nothing, as the other targets of `ic` do. Each test writes its pictures
//! into a directory of its own, named for the test, the process and a
//! nanosecond stamp, so two concurrent runs of this file never share a file.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use image::{Rgba, RgbaImage};
use serde_json::Value;

mod common;

use common::pty::{run_with_terminal, take_the_terminal_away, Window};
use common::{unreachable_path_dir, TERM_XTERM_256COLOR, TEST_IMAGE};

/// The name that this target puts in the unreachable `PATH` of its children.
const TARGET_NAME: &str = "emit-rgba";

/// The flag under test.
const FLAG: &str = "--emit-rgba";

/// The value name that every refusal of a box must show, so the user sees the
/// shape that the flag takes.
const BOX_SHAPE: &str = "WIDTHxHEIGHT";

/// The number of bytes that one RGBA pixel takes.
const BYTES_PER_PIXEL: usize = 4;

/// The exit status of a run that clap refused before `ic` read anything.
const USAGE_ERROR: Option<i32> = Some(2);

/// The exit status of a run that `ic` itself refused.
const RUN_ERROR: Option<i32> = Some(1);

/// A window that reports no pixel size, which is the window that makes a
/// display of `ic` ask its terminal. A run of the flag must ask nothing of it.
const WINDOW: Window = Window {
    columns: 80,
    rows: 24,
    width_px: 0,
    height_px: 0,
};

/// A directory of pictures that belongs to one test, removed when it drops.
struct Scratch {
    /// The directory itself.
    dir: PathBuf,
}

impl Scratch {
    /// Make an empty directory for the test called `test`.
    ///
    /// # Panics
    /// Panics when the clock is before the epoch or the directory does not
    /// make.
    fn new(test: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock must be after the epoch")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("ic-{TARGET_NAME}-{test}-{}-{nanos}", process::id()));
        fs::create_dir_all(&dir).expect("the scratch directory must make");
        Scratch { dir }
    }

    /// The path of `name` inside the directory.
    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Write `picture` as a PNG called `name`, and give its path.
    ///
    /// # Panics
    /// Panics when the PNG does not write.
    fn png(&self, name: &str, picture: &RgbaImage) -> PathBuf {
        let path = self.path(name);
        picture.save(&path).expect("the PNG must write");
        path
    }

    /// Write `bytes` as a file called `name`, and give its path.
    ///
    /// # Panics
    /// Panics when the file does not write.
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, bytes).expect("the file must write");
        path
    }
}

impl Drop for Scratch {
    /// Remove the directory and everything in it.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// A picture of `width` by `height` pixels in which no two neighbors match,
/// so a resize that moved a pixel shows in the bytes.
///
/// # Panics
/// Panics never: each channel is reduced modulo 256 before it narrows.
fn pattern(width: u32, height: u32) -> RgbaImage {
    let channel = |value: u32| u8::try_from(value % 256).expect("a value modulo 256 fits a byte");
    RgbaImage::from_fn(width, height, |x, y| {
        Rgba([
            channel(x * 37 + 11),
            channel(y * 53 + 7),
            channel(x * y + 101),
            255,
        ])
    })
}

/// Invoke the freshly-built `ic` binary in a known-empty environment.
fn ic(term: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ic"));
    command.env_clear();
    command.env("PATH", unreachable_path_dir(TARGET_NAME));
    command.env("TERM", term);
    command
}

/// What one run with no terminal did.
struct Run {
    /// The exit status of the run, as a number.
    code: Option<i32>,
    /// Every byte that the run wrote to standard output.
    stdout: String,
    /// Every byte that the run wrote to standard error.
    stderr: String,
}

/// Run `command` with no terminal at all, and with `stdin` on its standard
/// input.
///
/// # Panics
/// Panics when `ic` does not start, or when the input does not reach it.
fn run_with_input(command: &mut Command, stdin: &[u8]) -> Run {
    take_the_terminal_away(command);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("ic must run");
    child
        .stdin
        .take()
        .expect("the run must hold a pipe for stdin")
        .write_all(stdin)
        .expect("the input must reach ic");
    let done = child.wait_with_output().expect("ic must exit");

    Run {
        code: done.status.code(),
        stdout: String::from_utf8_lossy(&done.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&done.stderr).into_owned(),
    }
}

/// Run `command` with no terminal and an empty standard input.
fn run(command: &mut Command) -> Run {
    run_with_input(command, &[])
}

/// Run the flag over the picture at `path`, inside `pixel_box`.
fn emit(path: &Path, pixel_box: &str) -> Run {
    run(ic(TERM_XTERM_256COLOR).arg(FLAG).arg(pixel_box).arg(path))
}

/// The picture that one run printed.
#[derive(Debug, PartialEq, Eq)]
struct Emitted {
    /// The width of the printed picture, in pixels.
    width: u64,
    /// The height of the printed picture, in pixels.
    height: u64,
    /// The width of the picture in the file, in pixels.
    source_width: u64,
    /// The height of the picture in the file, in pixels.
    source_height: u64,
    /// The pixels, four bytes each, row by row.
    rgba: Vec<u8>,
}

/// Read the JSON that a successful run printed.
///
/// # Panics
/// Panics when the run failed, when the output is not one JSON object, or when
/// a field is absent or of the wrong type. Each panic names the output.
fn emitted(run: &Run) -> Emitted {
    assert_eq!(run.code, Some(0), "the run must succeed: {}", run.stderr);
    let value: Value = serde_json::from_str(run.stdout.trim_end())
        .unwrap_or_else(|error| panic!("stdout must be one JSON object ({error}): {}", run.stdout));
    let number = |field: &str| {
        value[field]
            .as_u64()
            .unwrap_or_else(|| panic!("`{field}` must be a whole number: {value}"))
    };
    let rgba = value["rgba"]
        .as_str()
        .unwrap_or_else(|| panic!("`rgba` must be a string: {value}"));

    Emitted {
        width: number("width"),
        height: number("height"),
        source_width: number("source_width"),
        source_height: number("source_height"),
        rgba: STANDARD
            .decode(rgba)
            .unwrap_or_else(|error| panic!("`rgba` must be standard base64: {error}")),
    }
}

/// The number of bytes that a picture of `width` by `height` pixels takes.
fn rgba_len(width: u64, height: u64) -> usize {
    usize::try_from(width * height).expect("a test picture is small") * BYTES_PER_PIXEL
}

/// A picture wider than the box takes the width of the box, and its height
/// keeps the shape of the picture.
#[test]
fn emit_rgba_fits_a_wide_picture_inside_the_box() {
    let scratch = Scratch::new("wide");
    let path = scratch.png("wide.png", &pattern(40, 20));

    let picture = emitted(&emit(&path, "10x10"));

    assert_eq!((picture.width, picture.height), (10, 5));
    assert_eq!((picture.source_width, picture.source_height), (40, 20));
    assert_eq!(picture.rgba.len(), rgba_len(10, 5));
}

/// A picture taller than the box takes the height of the box, and its width
/// keeps the shape of the picture.
#[test]
fn emit_rgba_fits_a_tall_picture_inside_the_box() {
    let scratch = Scratch::new("tall");
    let path = scratch.png("tall.png", &pattern(20, 40));

    let picture = emitted(&emit(&path, "10x10"));

    assert_eq!((picture.width, picture.height), (5, 10));
    assert_eq!((picture.source_width, picture.source_height), (20, 40));
    assert_eq!(picture.rgba.len(), rgba_len(5, 10));
}

/// A picture that fits the box comes out pixel for pixel, its transparent
/// pixels included. The flag never makes a picture larger, because a larger
/// copy holds no detail that the file did not hold, and the program that
/// draws it scales it itself.
#[test]
fn emit_rgba_gives_a_picture_that_fits_pixel_for_pixel() {
    let scratch = Scratch::new("fits");
    let mut source = pattern(4, 3);
    source.put_pixel(2, 1, Rgba([9, 8, 7, 0]));
    let path = scratch.png("small.png", &source);

    let picture = emitted(&emit(&path, "100x100"));

    assert_eq!((picture.width, picture.height), (4, 3));
    assert_eq!(picture.rgba, source.into_raw());
}

/// A picture far thinner than the box keeps one column, and not zero, because
/// a picture of no pixels is no picture.
#[test]
fn emit_rgba_keeps_one_pixel_of_a_thin_picture() {
    let scratch = Scratch::new("thin");
    let path = scratch.png("thin.png", &pattern(3, 300));

    let picture = emitted(&emit(&path, "100x10"));

    assert_eq!((picture.width, picture.height), (1, 10));
}

/// `--stdin` gives the picture on standard input, as it does for a display.
#[test]
fn emit_rgba_reads_a_picture_from_stdin() {
    let outcome = run_with_input(
        ic(TERM_XTERM_256COLOR).args([FLAG, "8x8", "--stdin"]),
        TEST_IMAGE,
    );

    let picture = emitted(&outcome);

    assert_eq!((picture.width, picture.height), (1, 1));
    assert_eq!(picture.rgba.len(), BYTES_PER_PIXEL);
}

/// A run of the flag writes nothing to its terminal. The window here reports
/// no pixel size, so a display would ask this terminal for the size of a cell,
/// and the program that runs the flag reads that answer as keys of the user.
#[test]
fn emit_rgba_writes_nothing_to_the_terminal() {
    let scratch = Scratch::new("terminal");
    let path = scratch.png("quiet.png", &pattern(6, 6));

    let outcome = run_with_terminal(
        ic(TERM_XTERM_256COLOR).arg(FLAG).arg("8x8").arg(&path),
        WINDOW,
    );

    assert!(
        outcome.wrote.is_empty(),
        "--emit-rgba must write nothing to the terminal, and it wrote {:?}",
        String::from_utf8_lossy(&outcome.wrote)
    );
    let picture = emitted(&Run {
        code: outcome.code,
        stdout: outcome.stdout,
        stderr: outcome.stderr,
    });
    assert_eq!((picture.width, picture.height), (6, 6));
}

/// A terminal that draws no picture refuses a display. It refuses no decode,
/// because the flag puts nothing on that terminal.
#[test]
fn emit_rgba_ignores_a_terminal_that_draws_no_picture() {
    let scratch = Scratch::new("linux");
    let path = scratch.png("console.png", &pattern(2, 2));

    let outcome = run(ic("linux")
        .env("TMUX", "/private/tmp/tmux-501/default,1,0")
        .arg(FLAG)
        .arg("8x8")
        .arg(&path));

    assert_eq!(emitted(&outcome).width, 2);
}

/// A box that is not two whole numbers from 1 to 4096 joined by `x` is a
/// mistake of usage, and the refusal shows the shape the flag takes.
#[test]
fn emit_rgba_refuses_a_box_it_cannot_read() {
    let scratch = Scratch::new("box");
    let path = scratch.png("box.png", &pattern(2, 2));

    for pixel_box in ["10", "0x5", "5x0", "axb", "5x", "4097x5", "5x4097", "-5x5"] {
        let outcome = emit(&path, pixel_box);

        assert_eq!(
            outcome.code, USAGE_ERROR,
            "the box {pixel_box:?} must be refused as usage: {}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains(BOX_SHAPE),
            "the refusal of {pixel_box:?} must show {BOX_SHAPE}: {}",
            outcome.stderr
        );
    }
}

/// The largest box the flag takes is 4096 pixels on each side.
#[test]
fn emit_rgba_takes_the_largest_box() {
    let scratch = Scratch::new("largest");
    let path = scratch.png("largest.png", &pattern(3, 2));

    let picture = emitted(&emit(&path, "4096x4096"));

    assert_eq!((picture.width, picture.height), (3, 2));
}

/// A video holds frames, and the flag gives one picture. `ic` names the
/// reason, where a decode of the container would name a format it never read.
#[test]
fn emit_rgba_refuses_a_video() {
    let scratch = Scratch::new("video");
    let path = scratch.file("clip.mp4", b"not really a video");

    let outcome = emit(&path, "8x8");

    assert_eq!(outcome.code, RUN_ERROR, "stderr: {}", outcome.stderr);
    assert!(
        outcome.stderr.contains("video"),
        "the refusal must name the video: {}",
        outcome.stderr
    );
}

/// A file that holds no picture fails, and the failure names the file.
#[test]
fn emit_rgba_names_a_file_that_holds_no_picture() {
    let scratch = Scratch::new("text");
    let path = scratch.file("notes.png", b"hello, this is text");

    let outcome = emit(&path, "8x8");

    assert_eq!(outcome.code, RUN_ERROR, "stderr: {}", outcome.stderr);
    assert!(
        outcome.stderr.contains(&path.display().to_string()),
        "the failure must name the file: {}",
        outcome.stderr
    );
    assert_eq!(outcome.stdout, "", "a failure must print no JSON");
}

/// A path that does not exist fails as it does for a display.
#[test]
fn emit_rgba_names_a_file_that_does_not_exist() {
    let scratch = Scratch::new("missing");
    let path = scratch.path("gone.png");

    let outcome = emit(&path, "8x8");

    assert_eq!(outcome.code, RUN_ERROR, "stderr: {}", outcome.stderr);
    assert!(
        outcome.stderr.contains("File not found"),
        "the failure must say the file is not there: {}",
        outcome.stderr
    );
}

/// The flag prints one JSON object, so it takes one picture.
#[test]
fn emit_rgba_takes_one_picture() {
    let scratch = Scratch::new("two");
    let first = scratch.png("first.png", &pattern(2, 2));
    let second = scratch.png("second.png", &pattern(2, 2));

    let outcome = run(ic(TERM_XTERM_256COLOR)
        .arg(FLAG)
        .arg("8x8")
        .arg(&first)
        .arg(&second));

    assert_eq!(outcome.code, RUN_ERROR, "stderr: {}", outcome.stderr);
    assert!(
        outcome.stderr.contains("one picture"),
        "the refusal must say the flag takes one picture: {}",
        outcome.stderr
    );
    assert_eq!(outcome.stdout, "", "a refusal must print no JSON");
}

/// The options of a display state cells, a scale or the cursor, and the flag
/// puts nothing on the terminal. Each of them with the flag is a mistake that
/// `ic` reports, where a run that dropped one would print a picture the user
/// did not ask for.
#[test]
fn emit_rgba_refuses_the_options_of_a_display() {
    let scratch = Scratch::new("display");
    let path = scratch.png("display.png", &pattern(2, 2));

    for display_option in [
        &["--width", "10"][..],
        &["--height", "10"],
        &["--scale", "50"],
        &["--no-newline"],
        &["--will-display"],
    ] {
        let outcome = run(ic(TERM_XTERM_256COLOR)
            .arg(FLAG)
            .arg("8x8")
            .args(display_option)
            .arg(&path));

        assert_eq!(
            outcome.code, USAGE_ERROR,
            "{display_option:?} with the flag must be refused as usage: {}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains(display_option[0]),
            "the refusal must name {}: {}",
            display_option[0],
            outcome.stderr
        );
    }
}

/// `--monitor` shows each new file on the terminal, and the flag shows
/// nothing. The two together are a mistake of usage.
#[test]
fn emit_rgba_refuses_monitor_mode() {
    let scratch = Scratch::new("monitor");

    let outcome = run(ic(TERM_XTERM_256COLOR)
        .arg(FLAG)
        .arg("8x8")
        .arg("--monitor")
        .arg(&scratch.dir));

    assert_eq!(outcome.code, USAGE_ERROR, "stderr: {}", outcome.stderr);
    assert!(
        outcome.stderr.contains("--monitor"),
        "the refusal must name --monitor: {}",
        outcome.stderr
    );
}

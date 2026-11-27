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
//! into a directory of its own, named for the test, the process, a nanosecond
//! stamp and a count of the directories of the process, so two tests or two
//! concurrent runs of this file never share a file.

use std::fs;
use std::path::Path;
use std::process::Command;

use image::Rgba;

mod common;

use common::emit::{
    emitted, pattern, rgba_len, run, run_with_input, Run, Scratch, BYTES_PER_PIXEL,
};
use common::pty::{run_with_terminal, Window};
use common::{unreachable_path_dir, TERM_XTERM_256COLOR, TEST_IMAGE};

/// The name that this target puts in the unreachable `PATH` of its children.
const TARGET_NAME: &str = "emit-rgba";

/// The flag under test.
const FLAG: &str = "--emit-rgba";

/// The value name that every refusal of a box must show, so the user sees the
/// shape that the flag takes.
const BOX_SHAPE: &str = "WIDTHxHEIGHT";

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

/// The fixture picture of the `ic-inline` plugin, from this crate.
const PLUGIN_FIXTURE: &str = "../../plugins/ic-inline/tests/fixture.png";

/// The test file of the `ic-inline` plugin, from this crate. It holds a copy of
/// the output of `ic` for [`PLUGIN_FIXTURE`].
const PLUGIN_TEST: &str = "../../plugins/ic-inline/tests/ic-inline.test.tsx";

/// Invoke the freshly-built `ic` binary in a known-empty environment.
fn ic(term: &str) -> Command {
    let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_ic"));
    command.env_clear();
    command.env("PATH", unreachable_path_dir(TARGET_NAME));
    command.env("TERM", term);
    command
}

/// Run the flag over the picture at `path`, inside `pixel_box`.
fn emit(path: &Path, pixel_box: &str) -> Run {
    run(ic(TERM_XTERM_256COLOR).arg(FLAG).arg(pixel_box).arg(path))
}

/// A picture wider than the box takes the width of the box, and its height
/// keeps the shape of the picture.
#[test]
fn emit_rgba_fits_a_wide_picture_inside_the_box() {
    let scratch = Scratch::new(TARGET_NAME, "wide");
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
    let scratch = Scratch::new(TARGET_NAME, "tall");
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
    let scratch = Scratch::new(TARGET_NAME, "fits");
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
    let scratch = Scratch::new(TARGET_NAME, "thin");
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
    let scratch = Scratch::new(TARGET_NAME, "terminal");
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
    let scratch = Scratch::new(TARGET_NAME, "linux");
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
    let scratch = Scratch::new(TARGET_NAME, "box");
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
    let scratch = Scratch::new(TARGET_NAME, "largest");
    let path = scratch.png("largest.png", &pattern(3, 2));

    let picture = emitted(&emit(&path, "4096x4096"));

    assert_eq!((picture.width, picture.height), (3, 2));
}

/// A video holds frames, and the flag gives one picture. `ic` names the
/// reason, where a decode of the container would name a format it never read.
#[test]
fn emit_rgba_refuses_a_video() {
    let scratch = Scratch::new(TARGET_NAME, "video");
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
    let scratch = Scratch::new(TARGET_NAME, "text");
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
    let scratch = Scratch::new(TARGET_NAME, "missing");
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
    let scratch = Scratch::new(TARGET_NAME, "two");
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

/// The plugin `ic-inline` tests its reading of the JSON against a copy of the
/// output of `ic`. This test holds that copy to the real output, so a change
/// of the JSON fails here and not only in the plugin. The run uses the
/// argument vector of the plugin, with `--` before the path.
#[test]
fn emit_rgba_prints_the_json_that_the_plugin_tests_hold() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = manifest.join(PLUGIN_FIXTURE);
    let plugin_test = manifest.join(PLUGIN_TEST);

    let outcome = run(ic(TERM_XTERM_256COLOR)
        .arg(FLAG)
        .arg("720x720")
        .arg("--")
        .arg(&fixture));

    assert_eq!(
        outcome.code,
        Some(0),
        "the run must succeed: {}",
        outcome.stderr
    );
    let json = outcome.stdout.trim_end_matches('\n');
    assert_eq!(
        outcome.stdout.len(),
        json.len() + 1,
        "the output must end with exactly one newline: {:?}",
        outcome.stdout
    );
    let text = fs::read_to_string(&plugin_test)
        .unwrap_or_else(|error| panic!("{} must be readable: {error}", plugin_test.display()));
    assert!(
        text.contains(json),
        "{PLUGIN_TEST} does not hold the output of `ic {FLAG} 720x720 -- {PLUGIN_FIXTURE}`. \
         Copy the new output into FIXTURE_JSON, and into the `rgba` that the test \
         \"draw as an Image once the terminal takes one\" expects. Then run \
         `claude plugin test plugins/ic-inline`. The output is: {json}"
    );
}

/// The options of a display state cells, a scale, the cursor or the protocol,
/// the options of a video state how it plays, and the flag puts nothing on the
/// terminal. Each of them with the flag is a mistake that `ic` reports, where a
/// run that dropped one would print a picture the user did not ask for.
#[test]
fn emit_rgba_refuses_the_options_of_a_display() {
    let scratch = Scratch::new(TARGET_NAME, "display");
    let path = scratch.png("display.png", &pattern(2, 2));

    for display_option in [
        &["--width", "10"][..],
        &["--height", "10"],
        &["--scale", "50"],
        &["--no-newline"],
        &["--will-display"],
        &["--loop-video"],
        &["--fps", "10"],
        &["--do-not-drop-frames"],
        &["--memory-cleanup-frequency", "10"],
        &["--max-fps", "10"],
        &["--adaptive-fps"],
        &["--protocol", "kitty"],
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

/// `IC_PROTOCOL` holds the protocol for a whole shell session, and the README
/// tells a user to export it. A program that runs the flag in such a shell
/// states no protocol, thus the flag ignores the variable where it refuses
/// `--protocol`.
#[test]
fn emit_rgba_ignores_the_protocol_of_the_session() {
    let scratch = Scratch::new(TARGET_NAME, "session-protocol");
    let path = scratch.png("session.png", &pattern(2, 2));

    let outcome = run(ic(TERM_XTERM_256COLOR)
        .env("IC_PROTOCOL", "kitty")
        .arg(FLAG)
        .arg("8x8")
        .arg(&path));

    assert_eq!(emitted(&outcome).width, 2);
}

/// `--monitor` shows each new file on the terminal, and the flag shows
/// nothing. The two together are a mistake of usage.
#[test]
fn emit_rgba_refuses_monitor_mode() {
    let scratch = Scratch::new(TARGET_NAME, "monitor");

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

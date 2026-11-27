//! Black-box tests for `--diff`, which compares two still images pixel by
//! pixel and shows one picture of the differences.
//!
//! Every run starts from a cleared environment and a `PATH` that reaches
//! nothing, as the other targets of `ic` do, and with no terminal. Most tests
//! read the picture through `--emit-rgba`, which gives the exact pixels. One
//! test forces a protocol and a size, so a display run reads no terminal, and
//! it reads the header that the run prints above the picture.
//!
//! Each test writes its pictures into a directory of its own, named for the
//! test, the process, a nanosecond stamp and a count of the directories of the
//! process, so two tests or two concurrent runs of this file never share a file.
//!
//! The fixtures are `a.png`, a 40 by 40 pattern, and `b.png`, the same picture
//! with the pixel at (20, 20) changed. One changed pixel makes one region. The
//! box around it is padded by 4 pixels, thus it spans 16 to 24 on each axis,
//! and the outline of 2 pixels covers the columns 16, 17, 23 and 24.

use std::path::PathBuf;
use std::process::Command;

use image::{Rgba, RgbaImage};

mod common;

use common::emit::{emitted, pattern, run, Emitted, Run, Scratch, BYTES_PER_PIXEL};
use common::{unreachable_path_dir, TERM_XTERM_256COLOR};

/// The name that this target puts in the unreachable `PATH` of its children.
const TARGET_NAME: &str = "diff";

/// The flag under test.
const FLAG: &str = "--diff";

/// The box that the emit runs take. The pictures are smaller, and the flag
/// never makes a picture larger, so the box changes nothing.
const BOX: &str = "64x64";

/// The side of the fixture pictures, in pixels.
const SIDE: u32 = 40;

/// The pixel that `b.png` changes.
const CHANGED: (u32, u32) = (20, 20);

/// The value of the changed pixel in `b.png`.
const CHANGED_PIXEL: [u8; 4] = [1, 2, 3, 255];

/// The color of an outline and of a differing pixel.
const RED: [u8; 4] = [255, 0, 0, 255];

/// Invoke the freshly-built `ic` binary in a known-empty environment.
fn ic(term: &str) -> Command {
    let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_ic"));
    command.env_clear();
    command.env("PATH", unreachable_path_dir(TARGET_NAME));
    command.env("TERM", term);
    command
}

/// The two fixture pictures, written into `scratch`, and the paths of both.
fn fixtures(scratch: &Scratch) -> (PathBuf, PathBuf) {
    let first = pattern(SIDE, SIDE);
    let mut second = first.clone();
    second.put_pixel(CHANGED.0, CHANGED.1, Rgba(CHANGED_PIXEL));
    (scratch.png("a.png", &first), scratch.png("b.png", &second))
}

/// The pixel at `(x, y)` of a printed picture.
///
/// # Panics
/// Panics when the pixel lies outside the picture.
fn pixel(picture: &Emitted, x: u64, y: u64) -> [u8; 4] {
    let start =
        usize::try_from(y * picture.width + x).expect("a test offset fits") * BYTES_PER_PIXEL;
    let bytes = picture
        .rgba
        .get(start..start + BYTES_PER_PIXEL)
        .unwrap_or_else(|| panic!("the pixel ({x}, {y}) must lie inside the picture"));
    bytes.try_into().expect("a pixel is four bytes")
}

/// The pixel at `(x, y)` of a source picture.
fn source_pixel(source: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    source.get_pixel(x, y).0
}

/// Run the flag over the two fixtures, with `extra` options, for the test
/// called `test`, and read the picture that it printed.
fn emit_diff(test: &str, extra: &[&str]) -> (Emitted, RgbaImage) {
    let scratch = Scratch::new(TARGET_NAME, test);
    let (first, second) = fixtures(&scratch);
    let outcome = run(ic(TERM_XTERM_256COLOR)
        .arg(FLAG)
        .args(["--emit-rgba", BOX])
        .args(extra)
        .arg(&first)
        .arg(&second));
    (emitted(&outcome), pattern(SIDE, SIDE))
}

/// `boxes` shows the second image, with a red outline around the region. The
/// picture keeps the size of the images, and the box limits only the fit.
#[test]
fn diff_boxes_outlines_the_region_on_the_second_image() {
    let (picture, first) = emit_diff("boxes", &["--diff-mode", "boxes"]);

    assert_eq!((picture.width, picture.height), (40, 40));
    assert_eq!(pixel(&picture, 20, 20), CHANGED_PIXEL);
    assert_eq!(pixel(&picture, 16, 20), RED);
    assert_eq!(pixel(&picture, 24, 20), RED);
    assert_eq!(pixel(&picture, 10, 10), source_pixel(&first, 10, 10));
}

/// `highlight` shows the first image, with each differing pixel red and no
/// outline.
#[test]
fn diff_highlight_marks_the_pixel_on_the_first_image() {
    let (picture, first) = emit_diff("highlight", &["--diff-mode", "highlight"]);

    assert_eq!(pixel(&picture, 20, 20), RED);
    assert_eq!(pixel(&picture, 10, 10), source_pixel(&first, 10, 10));
    assert_eq!(pixel(&picture, 16, 20), source_pixel(&first, 16, 20));
}

/// `mask` shows opaque black, with each differing pixel red.
#[test]
fn diff_mask_marks_the_pixel_on_black() {
    let (picture, _first) = emit_diff("mask", &["--diff-mode", "mask"]);

    assert_eq!(pixel(&picture, 20, 20), RED);
    assert_eq!(pixel(&picture, 10, 10), [0, 0, 0, 255]);
}

/// `highlight` is the mode that a run with no `--diff-mode` takes.
#[test]
fn diff_mode_highlight_is_the_default() {
    let (named, _) = emit_diff("default-mode", &["--diff-mode", "highlight"]);
    let (unnamed, _) = emit_diff("default-mode", &[]);

    assert_eq!(named, unnamed);
}

/// An emit run prints exactly one JSON line and no header, because the
/// program that reads it parses the whole of standard output.
#[test]
fn diff_emit_prints_no_header() {
    let scratch = Scratch::new(TARGET_NAME, "no-header");
    let (first, second) = fixtures(&scratch);

    let outcome = run(ic(TERM_XTERM_256COLOR)
        .arg(FLAG)
        .args(["--emit-rgba", BOX])
        .arg(&first)
        .arg(&second));

    assert_eq!(outcome.code, Some(0), "stderr: {}", outcome.stderr);
    assert!(
        outcome.stdout.starts_with('{') && outcome.stdout.ends_with("}\n"),
        "stdout must be one JSON line: {:?}",
        outcome.stdout
    );
    assert_eq!(outcome.stdout.matches('\n').count(), 1);
    assert!(
        !outcome.stdout.contains(" vs "),
        "an emit run must print no header: {:?}",
        outcome.stdout
    );
}

/// The exit status of a run that `ic` itself refused.
const RUN_ERROR: Option<i32> = Some(1);

/// The exit status of a run that clap refused before `ic` read anything.
const USAGE_ERROR: Option<i32> = Some(2);

/// Assert that `outcome` is a refusal of `ic` that names `cause` and prints
/// nothing to standard output.
fn assert_refused(outcome: &Run, cause: &str) {
    assert_eq!(outcome.code, RUN_ERROR, "stderr: {}", outcome.stderr);
    assert_eq!(outcome.stdout, "", "a refusal must print nothing");
    assert!(
        outcome.stderr.contains(cause),
        "the refusal must say {cause:?}: {}",
        outcome.stderr
    );
}

/// The flag compares two images. One file, three files, and no file are each
/// refused, and the message says two files.
#[test]
fn diff_takes_exactly_two_files() {
    let scratch = Scratch::new(TARGET_NAME, "count");
    let (first, second) = fixtures(&scratch);

    assert_refused(
        &run(ic(TERM_XTERM_256COLOR).arg(FLAG).arg(&first)),
        "two files",
    );
    assert_refused(
        &run(ic(TERM_XTERM_256COLOR)
            .arg(FLAG)
            .arg(&first)
            .arg(&second)
            .arg(&first)),
        "two files",
    );
    assert_refused(&run(ic(TERM_XTERM_256COLOR).arg(FLAG)), "two files");
}

/// The words of the refusal of a video. The name of the scratch directory
/// holds `video`, and a decode error names the path, thus the bare word
/// `video` passes also when no check refuses the video.
const VIDEO_CAUSE: &str = "is a video";

/// A video is no still image, and the refusal names the reason.
#[test]
fn diff_refuses_a_video() {
    let scratch = Scratch::new(TARGET_NAME, "video");
    let (first, _second) = fixtures(&scratch);
    let clip = scratch.file("clip.mp4", b"not really a video");

    assert_refused(
        &run(ic(TERM_XTERM_256COLOR).arg(FLAG).arg(&first).arg(&clip)),
        VIDEO_CAUSE,
    );
    assert_refused(
        &run(ic(TERM_XTERM_256COLOR).arg(FLAG).arg(&clip).arg(&first)),
        VIDEO_CAUSE,
    );
}

/// The words of the refusal of a text file. A decode error names the path, and
/// the path ends in `.txt`, thus the words start at `is`.
const TEXT_CAUSE: &str = "is a text file";

/// A text file is no still image, and the refusal names the reason.
#[test]
fn diff_refuses_a_text_file() {
    let scratch = Scratch::new(TARGET_NAME, "text");
    let (first, _second) = fixtures(&scratch);
    let notes = scratch.file("notes.txt", b"hello, this is text");

    assert_refused(
        &run(ic(TERM_XTERM_256COLOR).arg(FLAG).arg(&first).arg(&notes)),
        TEXT_CAUSE,
    );
}

/// A text file whose first bytes match the signature of an image format is
/// still a text file. `BM` starts a BMP file, and `P1` starts a PNM file, but
/// no image header follows either one here.
#[test]
fn diff_refuses_a_text_file_that_starts_like_an_image() {
    let scratch = Scratch::new(TARGET_NAME, "text-signature");
    let (first, _second) = fixtures(&scratch);

    for (name, text) in [
        ("notes.txt", b"BMW service notes".as_slice()),
        ("todo.txt", b"P1 priorities".as_slice()),
    ] {
        let notes = scratch.file(name, text);
        assert_refused(
            &run(ic(TERM_XTERM_256COLOR).arg(FLAG).arg(&first).arg(&notes)),
            TEXT_CAUSE,
        );
    }
}

/// A plain PBM file is text from its first byte to its last, and it is still
/// an image. With no extension, the run reads it as it reads the same bytes
/// under the name `plain.pbm`.
#[test]
fn diff_reads_a_plain_pbm_whose_name_has_no_extension() {
    let scratch = Scratch::new(TARGET_NAME, "plain-pbm");
    let (first, _second) = fixtures(&scratch);
    let pbm = b"P1\n# a 2 by 2 checker\n2 2\n0 1\n1 0\n";
    let named = scratch.file("plain.pbm", pbm);
    let bare = scratch.file("plain", pbm);

    let emit = |path: &std::path::Path| {
        run(ic(TERM_XTERM_256COLOR)
            .arg(FLAG)
            .args(["--emit-rgba", BOX])
            .arg(path)
            .arg(&first))
    };

    assert_eq!(emitted(&emit(&bare)), emitted(&emit(&named)));
}

/// A file whose name gives no format that `ic` lists is still an image when its
/// first bytes name a format, as `--emit-rgba` reads it. The run compares it,
/// and it does not call the file text.
#[test]
fn diff_reads_an_image_whose_name_has_no_extension() {
    let scratch = Scratch::new(TARGET_NAME, "no-extension");
    let (first, second) = fixtures(&scratch);
    let bare = scratch.file(
        "screenshot",
        &std::fs::read(&first).expect("a.png must read"),
    );

    let emit = |path: &std::path::Path| {
        run(ic(TERM_XTERM_256COLOR)
            .arg(FLAG)
            .args(["--emit-rgba", BOX])
            .arg(path)
            .arg(&second))
    };

    assert_eq!(emitted(&emit(&bare)), emitted(&emit(&first)));
}

/// The flag beside another input mode or an option of a video is a mistake of
/// usage, and the refusal of clap names the flag.
#[test]
fn diff_refuses_the_other_modes() {
    let scratch = Scratch::new(TARGET_NAME, "modes");
    let (first, second) = fixtures(&scratch);
    let directory = scratch.dir.display().to_string();

    for option in [
        &["--stdin"][..],
        &["--monitor", directory.as_str()],
        &["--will-display"],
        &["--loop-video"],
        &["--fps", "10"],
        &["--do-not-drop-frames"],
        &["--memory-cleanup-frequency", "10"],
        &["--max-fps", "10"],
        &["--adaptive-fps"],
    ] {
        let outcome = run(ic(TERM_XTERM_256COLOR)
            .arg(FLAG)
            .args(option)
            .arg(&first)
            .arg(&second));

        assert_eq!(
            outcome.code, USAGE_ERROR,
            "{option:?} with the flag must be refused as usage: {}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains(option[0]),
            "the refusal must name {}: {}",
            option[0],
            outcome.stderr
        );
    }
}

/// `--diff-mode` means nothing without `--diff`, and clap says so.
#[test]
fn diff_mode_needs_the_flag() {
    let scratch = Scratch::new(TARGET_NAME, "mode-alone");
    let (first, _second) = fixtures(&scratch);

    let outcome = run(ic(TERM_XTERM_256COLOR)
        .args(["--diff-mode", "mask"])
        .arg(&first));

    assert_eq!(outcome.code, USAGE_ERROR, "stderr: {}", outcome.stderr);
    assert!(
        outcome.stderr.contains(FLAG),
        "the refusal must name {FLAG}: {}",
        outcome.stderr
    );
}

/// A display run prints the two paths and the summary above the picture. The
/// run forces a protocol and a size, so it reads no terminal.
#[test]
fn diff_display_prints_the_header() {
    let scratch = Scratch::new(TARGET_NAME, "header");
    let (first, second) = fixtures(&scratch);

    let display = |second: &std::path::Path| {
        run(ic(TERM_XTERM_256COLOR)
            .arg(FLAG)
            .args(["--protocol", "iterm2", "--width", "10", "--height", "5"])
            .arg(&first)
            .arg(second))
    };

    let differing = display(&second);
    assert_eq!(differing.code, Some(0), "stderr: {}", differing.stderr);
    let expected = format!(
        "{} vs {}\n1 region differs (1 pixel)\n",
        first.display(),
        second.display()
    );
    assert!(
        differing.stdout.starts_with(&expected),
        "stdout must start with {expected:?}: {:?}",
        differing.stdout
    );

    let same = display(&first);
    assert_eq!(same.code, Some(0), "stderr: {}", same.stderr);
    let expected = format!(
        "{} vs {}\nthe images are the same\n",
        first.display(),
        first.display()
    );
    assert!(
        same.stdout.starts_with(&expected),
        "stdout must start with {expected:?}: {:?}",
        same.stdout
    );
}

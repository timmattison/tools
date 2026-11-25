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
//! test, the process and a nanosecond stamp, so two concurrent runs of this
//! file never share a file.
//!
//! The fixtures are `a.png`, a 40 by 40 pattern, and `b.png`, the same picture
//! with the pixel at (20, 20) changed. One changed pixel makes one region. The
//! box around it is padded by 4 pixels, thus it spans 16 to 24 on each axis,
//! and the outline of 2 pixels covers the columns 16, 17, 23 and 24.

use std::path::PathBuf;
use std::process::Command;

use image::{Rgba, RgbaImage};

mod common;

use common::emit::{emitted, pattern, run, Emitted, Scratch, BYTES_PER_PIXEL};
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
    let start = usize::try_from((y * picture.width + x) * 4).expect("a test offset fits");
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

/// Run the flag over the two fixtures, with `extra` options, and read the
/// picture that it printed.
fn emit_diff(extra: &[&str]) -> (Emitted, RgbaImage) {
    let scratch = Scratch::new(TARGET_NAME, "emit");
    let (first, second) = fixtures(&scratch);
    let outcome = run(ic(TERM_XTERM_256COLOR)
        .arg(FLAG)
        .args(["--emit-rgba", BOX])
        .args(extra)
        .arg(&first)
        .arg(&second));
    (emitted(&outcome), pattern(SIDE, SIDE))
}

/// With no mode, the picture is the second image with a red outline around the
/// region. The picture keeps the size of the images, and the box limits only
/// the fit.
#[test]
fn diff_boxes_is_the_default_picture() {
    let (picture, first) = emit_diff(&[]);

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
    let (picture, first) = emit_diff(&["--diff-mode", "highlight"]);

    assert_eq!(pixel(&picture, 20, 20), RED);
    assert_eq!(pixel(&picture, 10, 10), source_pixel(&first, 10, 10));
    assert_eq!(pixel(&picture, 16, 20), source_pixel(&first, 16, 20));
}

/// `mask` shows opaque black, with each differing pixel red.
#[test]
fn diff_mask_marks_the_pixel_on_black() {
    let (picture, _first) = emit_diff(&["--diff-mode", "mask"]);

    assert_eq!(pixel(&picture, 20, 20), RED);
    assert_eq!(pixel(&picture, 10, 10), [0, 0, 0, 255]);
}

/// `boxes` is the mode that a run with no `--diff-mode` takes.
#[test]
fn diff_mode_boxes_is_the_default() {
    let (named, _) = emit_diff(&["--diff-mode", "boxes"]);
    let (unnamed, _) = emit_diff(&[]);

    assert_eq!(named, unnamed);
}

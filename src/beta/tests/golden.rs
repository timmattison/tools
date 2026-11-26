//! Golden tests for `beta export web` and `beta export video`.
//!
//! These tests prove that the output of both exports stays the same, byte for
//! byte, when the terminal emulator moves into the `termstate` crate (issue
//! #768). Each test runs the real `beta` binary on one fixed recording and
//! compares the result with a file in `tests/fixtures/golden/`.
//!
//! To write the golden files again from the current output, run:
//!
//! ```text
//! BETA_GOLDEN_UPDATE=1 cargo test -p beta --test golden
//! ```

#![deny(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error"
)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use gitscratch::testing::executable::write_executable;
use tempfile::TempDir;

/// The variable that turns each golden test into a writer of its golden file.
const UPDATE_VARIABLE: &str = "BETA_GOLDEN_UPDATE";

/// The width of the golden frame: 12 columns of 23 px, plus a 320 px margin.
const FRAME_WIDTH: u32 = 596;

/// The height of the golden frame: 4 rows of 53 px, plus a 320 px margin.
const FRAME_HEIGHT: u32 = 532;

/// The bytes of one raw RGB frame.
const FRAME_BYTES: usize = FRAME_WIDTH as usize * FRAME_HEIGHT as usize * 3;

/// A fake `ffmpeg`: it writes the raw frames from stdin to the file that the
/// last argument names.
const FAKE_FFMPEG: &str = "#!/bin/sh
# A fake ffmpeg: write the raw frames from stdin to the file that the last argument names.
for last in \"$@\"; do :; done
cat > \"$last\"
";

/// The directory of the fixtures, found from the manifest and not from the cwd.
fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn golden_dir() -> PathBuf {
    fixtures_dir().join("golden")
}

fn recording_path() -> PathBuf {
    golden_dir().join("recording.json")
}

/// True when the run must write the golden files and not compare with them.
fn update_mode() -> bool {
    std::env::var(UPDATE_VARIABLE).is_ok_and(|value| value == "1")
}

/// Runs `beta` with `args`, with `path_prefix` (if any) before the inherited
/// `PATH`, and asserts that it succeeds.
fn run_beta(args: &[OsString], path_prefix: Option<&Path>) {
    let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_beta"));
    command.args(args);
    if let Some(prefix) = path_prefix {
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let joined = std::env::join_paths(
            std::iter::once(prefix.to_path_buf()).chain(std::env::split_paths(&inherited)),
        )
        .expect("a path prefix has no separator");
        command.env("PATH", joined);
    }
    let output = command.output().expect("beta starts");
    assert!(
        output.status.success(),
        "beta {args:?} failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn export_web_writes_the_golden_html_for_each_theme() {
    let cases = [
        ("auto", "web-auto.html"),
        ("dracula", "web-dracula.html"),
        ("monokai", "web-monokai.html"),
        ("solarized-dark", "web-solarized-dark.html"),
        ("solarized-light", "web-solarized-light.html"),
        // The web export matches theme names exactly, so `Dracula` gets the
        // black palette. The move must keep that.
        ("Dracula", "web-dracula-capitalized.html"),
    ];
    for (theme, golden_name) in cases {
        let tmp = TempDir::new().expect("temp dir");
        let out = tmp.path().join("out.html");
        run_beta(
            &[
                "export".into(),
                "web".into(),
                recording_path().into(),
                "-o".into(),
                out.clone().into(),
                "--theme".into(),
                theme.into(),
            ],
            None,
        );
        let actual = std::fs::read(&out).expect("beta wrote the HTML");
        let golden = golden_dir().join(golden_name);
        if update_mode() {
            std::fs::write(&golden, &actual).expect("write the golden file");
            continue;
        }
        let expected = std::fs::read(&golden).unwrap_or_else(|error| {
            panic!(
                "cannot read {}: {error}. Run with {UPDATE_VARIABLE}=1 to write it.",
                golden.display()
            )
        });
        assert!(
            actual == expected,
            "theme {theme}: the HTML differs from {} ({} bytes, expected {}). \
             Run with {UPDATE_VARIABLE}=1 to write it again if the change is on purpose.",
            golden.display(),
            actual.len(),
            expected.len(),
        );
    }
}

/// Writes the fake `ffmpeg` into `<dir>/bin` and returns that directory.
fn install_fake_ffmpeg(dir: &Path) -> PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("make the bin directory");
    write_executable(&bin.join("ffmpeg"), FAKE_FFMPEG).expect("write the fake ffmpeg");
    bin
}

/// Counts the bytes at which two buffers of the same length differ.
fn differing_bytes(left: &[u8], right: &[u8]) -> usize {
    left.iter().zip(right).filter(|(a, b)| a != b).count()
}

/// Saves `bytes` as a PNG in a file that the OS keeps after the test, and
/// returns its path.
fn save_actual_frame(bytes: Vec<u8>) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let path = std::env::temp_dir().join(format!(
        "beta-golden-frame-{}-{nanos}.png",
        std::process::id()
    ));
    image::RgbImage::from_raw(FRAME_WIDTH, FRAME_HEIGHT, bytes)
        .expect("the frame has the size of its buffer")
        .save(&path)
        .expect("save the actual frame");
    path
}

#[test]
fn export_video_writes_the_golden_frame() {
    let tmp = TempDir::new().expect("temp dir");
    let bin = install_fake_ffmpeg(tmp.path());
    let out = tmp.path().join("frame.raw");
    run_beta(
        &[
            "export".into(),
            "video".into(),
            recording_path().into(),
            "-o".into(),
            out.clone().into(),
            "--fps".into(),
            "1".into(),
            "--theme".into(),
            "dracula".into(),
            "--font".into(),
            fixtures_dir()
                .join("fonts/SourceCodePro-Regular.ttf")
                .into(),
        ],
        Some(&bin),
    );
    // A duration of 0.5 s at 1 fps is exactly one frame.
    let actual = std::fs::read(&out).expect("the fake ffmpeg wrote the frames");
    assert_eq!(
        actual.len(),
        FRAME_BYTES,
        "the output must hold exactly one {FRAME_WIDTH}x{FRAME_HEIGHT} RGB frame"
    );
    let golden = golden_dir().join("frame.png");
    if update_mode() {
        image::RgbImage::from_raw(FRAME_WIDTH, FRAME_HEIGHT, actual)
            .expect("the frame has the size of its buffer")
            .save(&golden)
            .expect("write the golden frame");
        return;
    }
    let expected = image::open(&golden)
        .unwrap_or_else(|error| {
            panic!(
                "cannot read {}: {error}. Run with {UPDATE_VARIABLE}=1 to write it.",
                golden.display()
            )
        })
        .to_rgb8();
    assert_eq!(
        (expected.width(), expected.height()),
        (FRAME_WIDTH, FRAME_HEIGHT),
        "the golden frame has the wrong size"
    );
    if actual != expected.as_raw().as_slice() {
        let differing = differing_bytes(&actual, expected.as_raw());
        let saved = save_actual_frame(actual);
        panic!(
            "the frame differs from {} in {differing} bytes. The actual frame is in {}. \
             Run with {UPDATE_VARIABLE}=1 to write the golden again if the change is on purpose.",
            golden.display(),
            saved.display(),
        );
    }
}

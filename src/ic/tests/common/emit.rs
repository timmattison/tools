//! The helpers of the tests that run `ic` over pictures that they make and read
//! the JSON of `--emit-rgba`.
//!
//! `emit-rgba` and `diff` both write PNG files into a directory of their own,
//! run `ic` with no terminal, and read the pixels that the run printed. The
//! helpers are here so a change lands one time. Each target still builds its
//! own `ic(term)` command, because each target names its own unreachable `PATH`
//! directory.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use image::{Rgba, RgbaImage};
use serde_json::Value;

use super::pty::take_the_terminal_away;

/// The number of bytes that one RGBA pixel takes.
pub const BYTES_PER_PIXEL: usize = 4;

/// A directory of pictures that belongs to one test, removed when it drops.
pub struct Scratch {
    /// The directory itself.
    pub dir: PathBuf,
}

/// The count of the scratch directories that this process made.
static SCRATCH_COUNT: AtomicU64 = AtomicU64::new(0);

impl Scratch {
    /// Make an empty directory for the test called `test` of the target called
    /// `target`. The name holds both, the process, a nanosecond stamp, and the
    /// count of the directories that this process made before it.
    ///
    /// The count keeps apart two tests of one process that start at the same
    /// moment. The stamp alone does not, because the clock of macOS moves in
    /// steps of one microsecond. The directory must not exist, so a name that
    /// two callers share is a panic and not a shared directory.
    ///
    /// # Panics
    /// Panics when the clock is before the epoch, or when the directory exists
    /// or does not make.
    pub fn new(target: &str, test: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock must be after the epoch")
            .as_nanos();
        let count = SCRATCH_COUNT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ic-{target}-{test}-{}-{nanos}-{count}",
            process::id()
        ));
        fs::create_dir(&dir).expect("the scratch directory must make, and must be new");
        Scratch { dir }
    }

    /// The path of `name` inside the directory.
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Write `picture` as a PNG called `name`, and give its path.
    ///
    /// # Panics
    /// Panics when the PNG does not write.
    pub fn png(&self, name: &str, picture: &RgbaImage) -> PathBuf {
        let path = self.path(name);
        picture.save(&path).expect("the PNG must write");
        path
    }

    /// Write `bytes` as a file called `name`, and give its path.
    ///
    /// # Panics
    /// Panics when the file does not write.
    pub fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
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
pub fn pattern(width: u32, height: u32) -> RgbaImage {
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

/// What one run with no terminal did.
pub struct Run {
    /// The exit status of the run, as a number.
    pub code: Option<i32>,
    /// Every byte that the run wrote to standard output.
    pub stdout: String,
    /// Every byte that the run wrote to standard error.
    pub stderr: String,
}

/// Run `command` with no terminal at all, and with `stdin` on its standard
/// input.
///
/// # Panics
/// Panics when `ic` does not start, or when the input does not reach it.
pub fn run_with_input(command: &mut Command, stdin: &[u8]) -> Run {
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
pub fn run(command: &mut Command) -> Run {
    run_with_input(command, &[])
}

/// The picture that one run printed.
#[derive(Debug, PartialEq, Eq)]
pub struct Emitted {
    /// The width of the printed picture, in pixels.
    pub width: u64,
    /// The height of the printed picture, in pixels.
    pub height: u64,
    /// The width of the picture in the file, in pixels.
    pub source_width: u64,
    /// The height of the picture in the file, in pixels.
    pub source_height: u64,
    /// The pixels, four bytes each, row by row.
    pub rgba: Vec<u8>,
}

/// Read the JSON that a successful run printed.
///
/// # Panics
/// Panics when the run failed, when the output is not one JSON object, or when
/// a field is absent or of the wrong type. Each panic names the output.
pub fn emitted(run: &Run) -> Emitted {
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
pub fn rgba_len(width: u64, height: u64) -> usize {
    usize::try_from(width * height).expect("a test picture is small") * BYTES_PER_PIXEL
}

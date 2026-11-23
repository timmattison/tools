//! Black-box tests of the `hexfind` binary, through its real command line.
//!
//! These tests read what no unit test of `find_pattern` reaches: the exit
//! status and the two output streams of the process.
//!
//! Each fixture file has a name that holds the process id and a nanosecond
//! timestamp, so two runs of this file at the same time do not share a file.
//! A guard removes each file, also when a test fails.

use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// A file of this process that is removed when the guard drops.
struct TempFile(PathBuf);

impl TempFile {
    /// Writes `bytes` to a new file with a name unique to this process and
    /// this moment.
    fn with(stem: &str, bytes: &[u8]) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "hexfind-cli-{stem}-{}-{nanos}.bin",
            std::process::id()
        ));
        let file = Self(path);
        std::fs::write(&file.0, bytes).expect("write the fixture file");
        file
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A command for the built `hexfind`, with no inherited git environment.
fn hexfind(args: &[&str], file: &Path) -> std::process::Command {
    let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_hexfind"));
    command.args(args).arg(file);
    command
}

/// Runs `hexfind` to the end and gives back its status and streams.
fn run(args: &[&str], file: &Path) -> Output {
    hexfind(args, file).output().expect("run hexfind")
}

/// A hex string with no bytes gives no pattern, so the search has no meaning.
/// `0x` alone is the same string after the prefix goes away.
#[test]
fn a_hex_string_with_no_bytes_is_refused() {
    let file = TempFile::with("empty-pattern", b"abc");

    for hex_string in ["", "0x"] {
        let output = run(&[hex_string], file.path());
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            !output.status.success(),
            "{hex_string:?} must fail, got {:?} with stdout {:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            stderr.contains("The hex string is empty"),
            "{hex_string:?} must name the empty hex string, got stderr {stderr:?}"
        );
    }
}

/// A reader that closed the pipe (`hexfind ... | head`) took what it wanted,
/// so a closed standard output is a success, with no panic.
///
/// The Rust runtime ignores `SIGPIPE` at startup, so a write to the closed
/// pipe gives `EPIPE` instead of a stop by the signal. `println!` turns that
/// error into a panic.
#[test]
fn a_closed_standard_output_is_a_success_with_no_panic() {
    let file = TempFile::with("closed-stdout", b"abc");
    let (reader, writer) = std::io::pipe().expect("create a pipe");
    // Before the child exists, so the pipe never has a reader.
    drop(reader);

    let output = hexfind(&["62"], file.path())
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped())
        .output()
        .expect("run hexfind");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !stderr.contains("panicked"),
        "a closed standard output must not panic, got stderr {stderr:?}"
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "a closed standard output must be a success, got stderr {stderr:?}"
    );
}

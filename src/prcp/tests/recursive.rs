//! End-to-end tests of recursive copies and moves in `prcp`. Each test runs the
//! real binary. Every run gives `-y` and copies at least two files, because a
//! run with one file puts the terminal of the developer in raw mode.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![warn(clippy::panic)]
#![deny(clippy::unimplemented)]
#![warn(clippy::cast_possible_truncation)]
#![warn(clippy::cast_sign_loss)]
#![warn(clippy::cast_precision_loss)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "each unwrap and expect here acts on the temporary directory the test just made, or on the spawn of the freshly built binary. A failure of either is a broken harness, not the behavior under test"
)]

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

/// Terminal width that every run states, so the layout does not depend on the real terminal.
const TEST_COLUMNS: &str = "80";

/// Run the real `prcp` binary with the given arguments and collect its output.
fn run_prcp<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_prcp"))
        .env("COLUMNS", TEST_COLUMNS)
        .stdin(Stdio::null())
        .args(args)
        .output()
        .expect("the prcp binary must start")
}

/// Return standard error as text without ANSI codes. The pre-commit hook forces color on.
fn visible_stderr(output: &Output) -> String {
    testcolor::strip_ansi(&String::from_utf8_lossy(&output.stderr))
}

/// Return standard output as text without ANSI codes.
#[allow(dead_code, reason = "later tests in this file read standard output")]
fn visible_stdout(output: &Output) -> String {
    testcolor::strip_ansi(&String::from_utf8_lossy(&output.stdout))
}

/// Write a file and make its parent directories.
fn write_file(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn non_recursive_glob_skips_directory_matches_and_warns() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    write_file(&src.join("a.txt"), "a");
    write_file(&src.join("b.txt"), "b");
    write_file(&src.join("sub").join("c.txt"), "c");
    fs::create_dir(&dest).unwrap();

    let mut glob_arg = OsString::from(src.as_os_str());
    glob_arg.push("/*");
    let output = run_prcp([
        OsString::from("-y"),
        OsString::from("-q"),
        glob_arg,
        dest.clone().into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(dest.join("a.txt").exists());
    assert!(dest.join("b.txt").exists());
    assert!(!dest.join("sub").exists());
    assert!(stderr.contains("Skipping directory"), "stderr: {stderr}");
    assert!(
        stderr.contains(&src.join("sub").display().to_string()),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("--recursive"), "stderr: {stderr}");
}

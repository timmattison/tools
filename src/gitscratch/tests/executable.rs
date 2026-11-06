//! Tests of `gitscratch::testing::executable`, the writer of a script that a
//! test runs.
//!
//! Each test writes into a temporary directory of its own, so the tests share
//! no path and run beside each other. No test starts git.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::process::Command;

use gitscratch::testing::executable::write_executable;

/// The text that the script of these tests prints.
const GREETING: &str = "hello from the stub";

/// A script that prints [`GREETING`].
fn greeting_script() -> String {
    format!("#!/bin/sh\necho '{GREETING}'\n")
}

#[test]
fn the_file_holds_the_contents_and_runs() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("stub");
    let script = greeting_script();

    write_executable(&path, script.as_bytes()).expect("write the stub");

    assert_eq!(
        std::fs::read_to_string(&path).expect("read the stub"),
        script,
        "the file holds the exact contents"
    );
    let mode = std::fs::metadata(&path)
        .expect("the stub exists")
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111, "each class can run the file: {mode:o}");
    let output = Command::new(&path).output().expect("run the stub");
    assert!(output.status.success(), "the stub runs: {output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), GREETING);
}

#[test]
fn a_second_write_replaces_the_contents() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("stub");

    write_executable(&path, b"#!/bin/sh\necho first\n").expect("write the first stub");
    write_executable(&path, greeting_script().as_bytes()).expect("write the second stub");

    let output = Command::new(&path).output().expect("run the stub");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        GREETING,
        "the second write replaces the first"
    );
}

#[test]
fn a_path_in_a_directory_that_does_not_exist_is_an_error_that_names_it() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("no-such-directory").join("stub");

    let error = write_executable(&path, greeting_script().as_bytes())
        .expect_err("a path in a missing directory cannot take a file");

    assert!(
        error.to_string().contains(&path.display().to_string()),
        "the error names the path: {error}"
    );
}

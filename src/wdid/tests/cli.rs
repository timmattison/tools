//! Tests that run the built `wdid` binary.

use std::process::Command;

/// The path of the binary that cargo built for these tests.
const BIN: &str = env!("CARGO_BIN_EXE_wdid");

/// The start of the version line: the tool name, the version, and the opening
/// parenthesis of the commit information.
const VERSION_PREFIX: &str = "wdid 0.1.0 (";

#[test]
fn version_flag_prints_the_name_the_version_and_the_commit() {
    let output = Command::new(BIN).arg("--version").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with(VERSION_PREFIX),
        "unexpected version line: {stdout:?}"
    );
}

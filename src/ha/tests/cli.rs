//! End-to-end tests of the command line of `ha`.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::process::Command;

#[test]
fn version_prints_name_number_and_build() {
    let output = Command::new(env!("CARGO_BIN_EXE_ha"))
        .arg("--version")
        .output()
        .expect("ha starts");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(
        stdout.starts_with("ha 0.1.0 ("),
        "unexpected version text: {stdout:?}"
    );
    assert!(output.status.success());
}

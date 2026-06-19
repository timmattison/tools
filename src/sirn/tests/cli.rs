//! Integration tests for the `sirn` binary's CLI surface.
//!
//! These spawn the real binary via `CARGO_BIN_EXE_sirn`. They are parallel-safe:
//! they exercise only argv parsing and early-exit paths, never starting the
//! blocking server (which would hang the suite). `--version` exits early via
//! clap; the duplicate-basename collision errors out before any bind/serve.

use std::process::Command;

/// `sirn --version` exits successfully and prints the buildinfo version string.
#[test]
fn version_flag_prints_buildinfo_string() {
    let output = Command::new(env!("CARGO_BIN_EXE_sirn"))
        .arg("--version")
        .output()
        .expect("spawning sirn --version should succeed");

    assert!(
        output.status.success(),
        "sirn --version should exit 0, got {:?}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("sirn "),
        "version output should name the binary, got: {stdout}"
    );
    assert!(
        stdout.contains("0.1.0"),
        "version output should include the crate version, got: {stdout}"
    );
}

/// Two files sharing a basename abort startup with a non-zero exit and a clear
/// "duplicate basename" error on stderr — before any port derivation or bind, so
/// the process exits immediately rather than hanging on a server.
#[test]
fn duplicate_basename_aborts_startup() {
    // Paths need not exist: build_routes only inspects basenames.
    let output = Command::new(env!("CARGO_BIN_EXE_sirn"))
        .args(["/some/dir1/dup.txt", "/other/dir2/dup.txt"])
        .output()
        .expect("spawning sirn with colliding basenames should succeed");

    assert!(
        !output.status.success(),
        "duplicate basenames should make sirn exit non-zero, got {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("duplicate basename"),
        "stderr should explain the duplicate basename, got: {stderr}"
    );
}

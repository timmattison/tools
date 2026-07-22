//! End-to-end tests that drive the compiled `install-bin` binary, ported from
//! the CLI behaviors of `install-bin.ts`.
//!
//! Parallel-safety: every test gets its own `tempfile::tempdir()` sandbox, so
//! concurrent runs never share a path.

use std::process::Command;

/// Path to the freshly built `install-bin` binary, provided by Cargo to
/// integration tests.
const BIN: &str = env!("CARGO_BIN_EXE_install-bin");

#[test]
fn reports_a_missing_source_and_exits_nonzero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("was-never-built");

    let out = Command::new(BIN)
        .arg(&missing)
        .arg("--dest")
        .arg(dir.path())
        .output()
        .expect("run install-bin");

    assert!(
        !out.status.success(),
        "a missing source must fail the install; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("install-bin:"),
        "the error must be prefixed with the tool name: {stderr:?}"
    );
    assert!(
        stderr.contains("does not exist"),
        "the error must explain the missing source: {stderr:?}"
    );
}

#[test]
fn prints_version_with_git_metadata() {
    let out = Command::new(BIN)
        .arg("--version")
        .output()
        .expect("run install-bin --version");

    assert!(
        out.status.success(),
        "install-bin --version should exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Expected shape (per CLAUDE.md): "install-bin <ver> (<hash>, clean|dirty)".
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.trim();
    assert!(
        line.starts_with("install-bin "),
        "version line must name the tool: {line:?}"
    );
    assert!(
        line.ends_with(')') && line.contains(" ("),
        "version line must carry the (hash, status) suffix: {line:?}"
    );
    assert!(
        line.contains(", clean)") || line.contains(", dirty)") || line.contains(", unknown)"),
        "version line must report git dirty/clean status: {line:?}"
    );
}

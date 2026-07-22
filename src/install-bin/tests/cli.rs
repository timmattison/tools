//! End-to-end tests that drive the compiled `install-bin` binary, ported from
//! the CLI behaviors of the original TypeScript `install-bin`.
//!
//! Parallel-safety: every test gets its own `tempfile::tempdir()` sandbox, so
//! concurrent runs never share a path.

use std::os::unix::fs::PermissionsExt;
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
fn installs_and_verifies_a_binary_that_execs_cleanly() {
    let dir = tempfile::tempdir().expect("tempdir");
    // install-bin itself answers `--version` cleanly, so install it onto a fresh
    // inode and let the real post-install exec check run (this is exactly the
    // macOS signature-cache path the tool exists to survive).
    let out = Command::new(BIN)
        .arg(BIN)
        .arg("--dest")
        .arg(dir.path())
        .output()
        .expect("run install-bin");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "installing a cleanly-exec'ing binary must succeed; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("installed"),
        "must report the install: {stdout:?}"
    );
    assert!(
        stdout.contains("verified"),
        "must report the post-install exec check: {stdout:?}"
    );
}

#[test]
fn no_verify_skips_the_exec_check() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A source that the exec check would SIGKILL: with --no-verify the install
    // must still succeed precisely because the check is skipped. (Without
    // --no-verify this same binary would fail the install, so a clean exit here
    // proves the check never ran.)
    let src = dir.path().join("suicidal");
    std::fs::write(&src, "#!/bin/sh\nkill -KILL $$\n").expect("write source");
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).expect("chmod source");

    let out = Command::new(BIN)
        .arg(&src)
        .arg("--dest")
        .arg(dir.path().join("bin"))
        .arg("--no-verify")
        .output()
        .expect("run install-bin");

    assert!(
        out.status.success(),
        "--no-verify must skip the exec check and succeed; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("installed"),
        "must still install: {stdout:?}"
    );
    assert!(
        !stdout.contains("verified"),
        "--no-verify must not run the exec check: {stdout:?}"
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

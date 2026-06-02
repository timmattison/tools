//! End-to-end tests driving the real `seescc` binary against a stub `sccache`
//! placed on a per-test PATH, so no live sccache server is needed. Each test
//! uses its own unique tempdir (parallel-safe), and stdout is captured (a
//! pipe), which is the scripting/one-shot path.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

/// The captured sccache 0.15.0 payload (Rust hits 1718, misses 963, etc.).
const FIXTURE: &str = include_str!("fixtures/sccache-0.15.0.json");

/// Write an executable `sccache` stub into `dir` with the given shell body.
fn write_stub(dir: &Path, body: &str) {
    let path = dir.join("sccache");
    fs::write(&path, body).expect("write stub");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

/// Run seescc with `path_value` as its PATH, capturing the output.
fn run_seescc(path_value: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_seescc"))
        .env("PATH", path_value)
        .output()
        .expect("invoke seescc")
}

/// PATH that finds the stub first, then the real system PATH (so the stub's
/// own `/bin/sh` and `cat` resolve).
fn path_with_stub(dir: &Path) -> String {
    let real = std::env::var("PATH").unwrap_or_default();
    format!("{}:{}", dir.display(), real)
}

#[test]
fn happy_path_shows_rust_only_metrics() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("fixture.json"), FIXTURE).expect("write fixture");
    write_stub(
        dir.path(),
        &format!("#!/bin/sh\ncat \"{}/fixture.json\"\n", dir.path().display()),
    );

    let out = run_seescc(&path_with_stub(dir.path()));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "seescc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Header + the five default metrics with formatted Rust values.
    assert!(
        stdout.contains("sccache · Rust"),
        "missing header: {stdout}"
    );
    assert!(stdout.contains("Compile requests"));
    assert!(stdout.contains("4,786"));
    assert!(stdout.contains("Requests executed"));
    assert!(stdout.contains("3,880"));
    assert!(stdout.contains("Cache hits"));
    assert!(stdout.contains("1,718")); // Rust hits only
    assert!(stdout.contains("Cache misses"));
    assert!(stdout.contains("963")); // Rust misses only
    assert!(stdout.contains("Hit rate"));
    assert!(stdout.contains("64.1%"));

    // Rust-only: C/C++ and Assembler numbers must NOT leak in, and we must not
    // be summing across languages.
    assert!(!stdout.contains("516"), "C/C++ hits leaked: {stdout}");
    assert!(
        !stdout.contains("2,430"),
        "summed across all languages: {stdout}"
    );
    assert!(!stdout.contains("C/C++"));
    assert!(!stdout.contains("Assembler"));
}

#[test]
fn garbled_json_exits_nonzero() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_stub(dir.path(), "#!/bin/sh\nprintf 'this is not json'\n");

    let out = run_seescc(&path_with_stub(dir.path()));
    assert!(!out.status.success(), "expected failure on garbled JSON");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("parse"),
        "stderr should mention parse failure: {stderr}"
    );
}

#[test]
fn sccache_nonzero_exit_is_reported() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_stub(dir.path(), "#!/bin/sh\necho boom >&2\nexit 2\n");

    let out = run_seescc(&path_with_stub(dir.path()));
    assert!(
        !out.status.success(),
        "expected failure when sccache exits non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("failed"),
        "stderr should report the failed poll: {stderr}"
    );
}

#[test]
fn missing_sccache_exits_nonzero_with_clear_error() {
    let empty = tempfile::tempdir().expect("tempdir"); // empty dir, no sccache
    let out = run_seescc(&empty.path().display().to_string());
    assert!(
        !out.status.success(),
        "expected failure when sccache absent"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("sccache"),
        "stderr should name sccache: {stderr}"
    );
    assert!(
        stderr.contains("not found"),
        "stderr should say not found: {stderr}"
    );
}

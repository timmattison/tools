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

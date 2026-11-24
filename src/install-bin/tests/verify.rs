//! Behavioral tests for `verify_exec`, ported from the `verifyExec` function in
//! the original TypeScript `install-bin`.
//!
//! `verify_exec` exec's a freshly installed binary once to prove the kernel will
//! actually run it: a signal death (especially `SIGKILL` from a stale macOS
//! signature cache), a hang, or a spawn failure is a verdict against the binary,
//! while any normal exit means exec — and thus the signature check — succeeded.
//!
//! Parallel-safety: every test gets its own `tempfile::tempdir()` sandbox
//! (unique per call), so concurrent runs never share a path.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use install_bin::{verify_exec, ExecVerdict, DEFAULT_VERIFY_TIMEOUT};

/// Write `body` as an executable `#!/bin/sh` script at `<dir>/<name>` (mode
/// `0o755`) and return its path.
fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).expect("write script");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

#[test]
fn verify_exec_reports_a_binary_the_kernel_sigkills_as_not_ok() {
    let dir = tempfile::tempdir().expect("tempdir");
    // The script kills itself with SIGKILL, exactly as the macOS signature-cache
    // rejection does to a booby-trapped binary at exec.
    let bin = write_script(dir.path(), "self-kill", "#!/bin/sh\nkill -KILL $$\n");

    let verdict = verify_exec(&bin, "--version", DEFAULT_VERIFY_TIMEOUT);

    match &verdict {
        ExecVerdict::Signal { signal, hint } => {
            assert_eq!(*signal, 9, "SIGKILL is signal 9");
            assert!(!hint.is_empty(), "a signal verdict must carry a hint");
        }
        other => panic!("expected ExecVerdict::Signal for a SIGKILLed binary, got: {other:?}"),
    }
    assert!(!verdict.is_ok(), "a SIGKILLed binary is not ok");
}

#[test]
fn verify_exec_reports_a_binary_that_execs_normally_as_ok() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_script(
        dir.path(),
        "prints-version",
        "#!/bin/sh\necho v1.2.3\nexit 0\n",
    );

    let verdict = verify_exec(&bin, "--version", DEFAULT_VERIFY_TIMEOUT);

    match &verdict {
        ExecVerdict::Ok { exit_code } => {
            assert_eq!(*exit_code, 0, "a clean `exit 0` is exit code 0");
        }
        other => panic!("expected ExecVerdict::Ok for a normally-exiting binary, got: {other:?}"),
    }
    assert!(verdict.is_ok(), "a normally-exiting binary is ok");
}

#[test]
fn verify_exec_times_out_a_binary_that_hangs() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A binary that never returns must not wedge the installer: verify_exec
    // kills it after the timeout and reports a Timeout verdict.
    let bin = write_script(dir.path(), "hangs", "#!/bin/sh\nsleep 5\n");

    // Use a short timeout so the test returns in well under a second rather than
    // waiting out the 15s default.
    let verdict = verify_exec(&bin, "--version", Duration::from_millis(300));

    match &verdict {
        ExecVerdict::Timeout { hint } => {
            assert!(!hint.is_empty(), "a timeout verdict must carry a hint");
        }
        other => panic!("expected ExecVerdict::Timeout for a hanging binary, got: {other:?}"),
    }
    assert!(!verdict.is_ok(), "a hanging binary is not ok");
}

#[test]
fn verify_exec_reports_a_missing_binary_as_a_spawn_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Nothing was ever created at this path, so the exec can't even start.
    let missing = dir.path().join("was-never-installed");

    let verdict = verify_exec(&missing, "--version", DEFAULT_VERIFY_TIMEOUT);

    match &verdict {
        ExecVerdict::SpawnError { hint } => {
            assert!(!hint.is_empty(), "a spawn-error verdict must carry a hint");
        }
        other => panic!("expected ExecVerdict::SpawnError for a missing binary, got: {other:?}"),
    }
    assert!(!verdict.is_ok(), "a binary that can't be spawned is not ok");
}

//! Behavioral tests for `verify_exec`, ported from the `verifyExec` function in
//! `install-bin.ts`.
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

use install_bin::{DEFAULT_VERIFY_TIMEOUT, ExecVerdict, verify_exec};

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

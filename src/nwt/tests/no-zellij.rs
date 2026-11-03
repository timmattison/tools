//! The regression test of issue #633: `nwt` runs no `zellij` command.
//!
//! `nwt` renamed the current zellij tab each time it made a worktree. The shell
//! that starts `nwt` is not always the person at the keyboard, so the rename
//! changed tabs that nobody asked to change. In issue #283, the tests of `nwt`
//! renamed the real tab of the developer. Issue #633 removed the rename.
//!
//! The test runs the real binary in a simulated zellij session. It sets all
//! three session variables (`ZELLIJ`, `ZELLIJ_PANE_ID`, `ZELLIJ_SESSION_NAME`),
//! so the child gets them in every place the test runs, not only inside zellij.
//! A [`FakeMultiplexer`] is first on the `PATH` of the child. The fake records
//! each call, so an empty record shows that `nwt` ran no `zellij` command.
//!
//! Unix only: [`FakeMultiplexer`] is a POSIX `sh` script.
#![cfg(unix)]

mod support;

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

use support::{init_repo, nanos, nwt_command, FakeMultiplexer};

/// The program that `nwt` must not run.
const MULTIPLEXER: &str = "zellij";

/// The variables that zellij sets in each shell that it starts, with values of
/// the shape that a real session gives them. `nwt_command` does not remove
/// them, so without this table the child gets them only when the suite runs
/// inside zellij.
const SESSION: [(&str, &str); 3] = [
    ("ZELLIJ", "0"),
    ("ZELLIJ_PANE_ID", "0"),
    ("ZELLIJ_SESSION_NAME", "nwt-no-zellij-test"),
];

/// The opt-out variable of the old rename.
const OLD_OPT_OUT: &str = "NWT_NO_TAB_RENAME";

/// The shell that does the `PATH` search for [`resolve_on_path`].
const SHELL: &str = "/bin/sh";

/// Finds the program that `name` resolves to on `path`.
///
/// A real shell does the search, so the answer is the answer of a real `PATH`
/// search, not a copy of one. The shell gets only `PATH`, thus no inherited
/// variable can change the answer. A name that does not resolve gives `None`.
fn resolve_on_path(name: &str, path: &OsStr) -> Option<PathBuf> {
    let output = Command::new(SHELL)
        .env_clear()
        .env("PATH", path)
        .args(["-c", "command -v \"$1\"", SHELL, name])
        .output()
        .unwrap_or_else(|e| panic!("run {SHELL}: {e}"));
    output
        .status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

/// Inside zellij, `nwt -b <branch>` makes the worktree and runs no `zellij`
/// command.
#[test]
fn nwt_runs_no_zellij_command_inside_zellij() {
    let fake = FakeMultiplexer::new();
    let path = fake.path_env();

    // The proof is an empty record, and a fake that the child cannot find also
    // gives an empty record. So first make sure that the child finds the fake.
    assert_eq!(
        resolve_on_path(MULTIPLEXER, &path),
        Some(fake.program().to_path_buf()),
        "`{MULTIPLEXER}` on the PATH of the child must resolve to the fake, \
         or an empty record proves nothing"
    );

    let (_temp, repo) = init_repo();
    let branch = format!("no-zellij-{}-{}", std::process::id(), nanos());

    let output = nwt_command(&repo)
        .args(["-b", &branch])
        .env("PATH", &path)
        .envs(SESSION)
        // The old rename obeyed this opt-out. Remove it, so that an opt-out in
        // the environment of the tester cannot hide a rename that comes back.
        .env_remove(OLD_OPT_OUT)
        .output()
        .expect("run the nwt binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "nwt -b {branch} failed.\nstdout: {stdout}\nstderr: {stderr}"
    );

    let worktree = PathBuf::from(stdout.trim());
    assert!(
        worktree.is_dir(),
        "nwt printed {}, which is no directory.\nstderr: {stderr}",
        worktree.display()
    );

    let recorded = fake.recorded();
    assert!(
        recorded.is_empty(),
        "nwt must run no {MULTIPLEXER} command, but the fake recorded:\n{recorded}"
    );
}

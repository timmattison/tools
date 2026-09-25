//! Integration tests pinning `nwt`'s terminal-multiplexer tab/window renaming
//! against issue #283: tests (and stray scripts) must never hijack the user's
//! real tab, while interactive `nwt` must keep renaming as before.
//!
//! Every test here drives the *real* `nwt` binary with a [`FakeMultiplexer`] on
//! `PATH`, so `nwt`'s `zellij`/`tmux` calls hit a recording fake instead of the
//! tester's live session. That makes the suite safe to run from inside a real
//! zellij/tmux session (the exact scenario that triggers the bug) and lets each
//! test assert precisely whether a rename was attempted.
//!
//! Unix-only: [`FakeMultiplexer`] relies on executable POSIX `sh` shims, and
//! zellij/tmux are Unix-only, so the hijack can only happen there.
#![cfg(unix)]

mod support;

use support::{init_repo, nanos, nwt_command, FakeMultiplexer};

/// Positive control: inside zellij with no opt-out, `nwt` renames the tab.
///
/// This proves the recorder actually captures renames — without it the opt-out
/// test below could pass vacuously (a broken fake that records nothing) — and it
/// locks acceptance criterion "interactive `nwt -b <name>` still renames the
/// tab" (the rename fires here against the fake).
#[test]
fn zellij_rename_fires_without_optout() {
    let fake = FakeMultiplexer::new();
    let (_temp, repo) = init_repo();
    let branch = format!("ztab-{}-{}", std::process::id(), nanos());

    let output = nwt_command(&repo)
        .args(["-b", &branch])
        .env("PATH", fake.path_env())
        .env("ZELLIJ", "fake-session-0")
        // Make sure an opt-out leaking from the tester's own env can't mask the
        // positive control.
        .env_remove("NWT_NO_TAB_RENAME")
        .output()
        .expect("Failed to run nwt binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "nwt should succeed.\nstderr: {stderr}"
    );

    let recorded = fake.recorded();
    assert!(
        recorded.contains("rename-tab"),
        "inside zellij (no opt-out), nwt must rename the tab.\nrecorded: {recorded:?}"
    );
    assert!(
        recorded.contains(&branch),
        "the rename must target the worktree's tab name.\nrecorded: {recorded:?}"
    );
}

/// Setting `NWT_NO_TAB_RENAME` suppresses the zellij rename even though the
/// multiplexer env is present — the belt-and-suspenders safety net of #283.
#[test]
fn zellij_rename_suppressed_by_optout() {
    let fake = FakeMultiplexer::new();
    let (_temp, repo) = init_repo();
    let branch = format!("ztab-optout-{}-{}", std::process::id(), nanos());

    let output = nwt_command(&repo)
        .args(["-b", &branch])
        .env("PATH", fake.path_env())
        .env("ZELLIJ", "fake-session-0")
        .env("NWT_NO_TAB_RENAME", "1")
        .output()
        .expect("Failed to run nwt binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "nwt should succeed.\nstderr: {stderr}"
    );

    let recorded = fake.recorded();
    assert!(
        !recorded.contains("rename-tab"),
        "NWT_NO_TAB_RENAME must suppress the zellij rename.\nrecorded: {recorded:?}"
    );
}

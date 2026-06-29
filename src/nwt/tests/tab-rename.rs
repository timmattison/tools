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

/// True if the recorded `tmux new-window …` invocation carries a `-n` window-name
/// flag. Splitting on whitespace keeps the `-c <worktree-path>` argument (a
/// single token that is never exactly `-n`) from being mistaken for the flag.
fn tmux_invocation_has_name_flag(recorded: &str) -> bool {
    recorded.split_whitespace().any(|token| token == "-n")
}

/// Positive control: inside tmux with no opt-out, `nwt --tmux` opens the new
/// window named after the worktree (`-n <tab>`). Proves the recorder captures
/// the tmux path and locks the interactive behavior.
#[test]
fn tmux_window_named_without_optout() {
    let fake = FakeMultiplexer::new();
    let (_temp, repo) = init_repo();
    let branch = format!("tmuxwin-{}-{}", std::process::id(), nanos());

    let output = nwt_command(&repo)
        .args(["-b", &branch, "--tmux"])
        .env("PATH", fake.path_env())
        .env("TMUX", "/fake/tmux-socket,0,0")
        // Exercise only the tmux path: don't let an inherited ZELLIJ also fire a
        // rename into the recorder.
        .env_remove("ZELLIJ")
        .env_remove("NWT_NO_TAB_RENAME")
        .output()
        .expect("Failed to run nwt binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "nwt --tmux should succeed.\nstderr: {stderr}"
    );

    let recorded = fake.recorded();
    assert!(
        recorded.contains("new-window"),
        "nwt --tmux must open a tmux window.\nrecorded: {recorded:?}"
    );
    assert!(
        tmux_invocation_has_name_flag(&recorded),
        "the new tmux window must be named (`-n`) after the worktree.\nrecorded: {recorded:?}"
    );
    assert!(
        recorded.split_whitespace().any(|token| token == branch),
        "the window name must be the worktree's tab name.\nrecorded: {recorded:?}"
    );
}

/// Setting `NWT_NO_TAB_RENAME` drops the `-n <tab>` window name from the
/// `tmux new-window` invocation (tmux then auto-names the window). The window is
/// still opened — the opt-out only suppresses the rename, not the `--tmux`
/// behavior.
#[test]
fn tmux_window_name_dropped_by_optout() {
    let fake = FakeMultiplexer::new();
    let (_temp, repo) = init_repo();
    let branch = format!("tmuxwin-optout-{}-{}", std::process::id(), nanos());

    let output = nwt_command(&repo)
        .args(["-b", &branch, "--tmux"])
        .env("PATH", fake.path_env())
        .env("TMUX", "/fake/tmux-socket,0,0")
        .env_remove("ZELLIJ")
        .env("NWT_NO_TAB_RENAME", "1")
        .output()
        .expect("Failed to run nwt binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "nwt --tmux should still succeed.\nstderr: {stderr}"
    );

    let recorded = fake.recorded();
    assert!(
        recorded.contains("new-window"),
        "nwt --tmux must still open a tmux window (opt-out only drops the name).\nrecorded: {recorded:?}"
    );
    assert!(
        !tmux_invocation_has_name_flag(&recorded),
        "NWT_NO_TAB_RENAME must drop the `-n` window name from tmux new-window.\nrecorded: {recorded:?}"
    );
}

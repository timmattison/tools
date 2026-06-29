//! Shared test-support for `nwt`'s integration tests.
//!
//! Every integration test that drives the real `nwt` binary goes through this
//! module instead of building a [`Command`] by hand. Centralising the spawn is
//! what closes issue #283: `nwt` renames the *current* terminal-multiplexer tab
//! whenever `ZELLIJ`/`TMUX` is present, so a test that inherits the multiplexer
//! env (because the suite was launched from inside zellij/tmux) would hijack the
//! user's real tab. [`nwt_command`] scrubs that env so the spawned binary never
//! believes it is inside a multiplexer, and [`FakeMultiplexer`] lets the
//! dedicated tab-rename tests *simulate* the multiplexer with a recording fake
//! so they can assert exactly when a rename does and does not fire.
//!
//! Each integration test file is compiled as its own crate that pulls this
//! module in via `mod support;`, so not every binary uses every helper — hence
//! the crate-level dead-code allowance below.
#![allow(
    dead_code,
    reason = "shared across integration-test crates; not every test binary uses every helper"
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::TempDir;

/// Runs a git command in `dir` with stdin/stdout/stderr nulled, returning
/// whether it succeeded. Output is nulled so concurrent test runs (a background
/// `bacon` loop alongside the pre-commit hook's own run) don't interleave noise.
pub fn run_git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Nanosecond timestamp for building process-unique, parallel-safe names.
///
/// Test branch/worktree names are keyed on `std::process::id()` + this value so
/// two concurrent copies of the same test never collide on a shared resource.
pub fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos()
}

/// Creates a throwaway git repo with a single baseline commit and returns the
/// `TempDir` (keep it alive) plus the repo path.
///
/// The repo is a *subdir* of the `TempDir` so that `nwt`'s sibling
/// `<repo-name>-worktrees` output directory also lands inside the `TempDir` and
/// is cleaned up with it. gpg signing is disabled so a globally-configured
/// signer can't break the commit, and the commit is made exactly once (no retry
/// loop, per the repo's git discipline).
pub fn init_repo() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).expect("Failed to create repo subdir");

    assert!(run_git(&repo, &["init"]), "git init failed");
    assert!(
        run_git(&repo, &["config", "user.email", "test@example.com"]),
        "git config user.email failed"
    );
    assert!(
        run_git(&repo, &["config", "user.name", "Test User"]),
        "git config user.name failed"
    );

    std::fs::write(repo.join("README.md"), "baseline\n").expect("Failed to write baseline file");
    assert!(run_git(&repo, &["add", "README.md"]), "git add failed");
    assert!(
        run_git(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "baseline"]
        ),
        "git commit failed"
    );

    (temp, repo)
}

/// Builds a [`Command`] that runs the real `nwt` binary against `repo`.
///
/// This is the single, mandatory entrance every integration test uses to spawn
/// `nwt`. It sets the working directory to `repo` and nulls stdin (so an
/// unexpected prompt can't hang the suite), and — crucially for issue #283 — it
/// scrubs the terminal-multiplexer environment from the child so a suite
/// launched from inside zellij/tmux can never hijack the user's real tab.
///
/// Tests that deliberately *exercise* the multiplexer behaviour (see
/// [`FakeMultiplexer`]) re-add `ZELLIJ`/`TMUX` on the returned command; because
/// those `.env(...)` calls run after the scrub here, they win for that child.
pub fn nwt_command(repo: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nwt"));
    cmd.current_dir(repo)
        .stdin(Stdio::null())
        // Issue #283: strip the terminal-multiplexer env so a suite launched
        // from inside zellij/tmux can't make the spawned nwt rename the user's
        // real tab. Tests that deliberately exercise the multiplexer behavior
        // re-add ZELLIJ/TMUX after calling this (a later `.env(..)` wins).
        .env_remove("ZELLIJ")
        .env_remove("TMUX");
    cmd
}

/// A pair of fake `zellij`/`tmux` executables that record every invocation
/// instead of touching the real multiplexer.
///
/// The dedicated tab-rename tests need to answer "did `nwt` try to rename the
/// tab?" without actually renaming the tester's real tab — which matters
/// because the suite is frequently run *from inside* a live zellij session
/// (that is the very bug). [`FakeMultiplexer`] writes throwaway `zellij` and
/// `tmux` scripts into a temp dir; a test prepends [`path_env`](Self::path_env)
/// to the child's `PATH` so `nwt`'s `Command::new("zellij")` /
/// `Command::new("tmux")` resolve to the fakes. Each fake appends its argv to a
/// recorder file and exits `0`, so the real socket is never contacted and
/// [`recorded`](Self::recorded) reports exactly what `nwt` attempted.
///
/// Unix-only: the fakes are POSIX `sh` scripts marked executable via the Unix
/// permission bits. zellij and tmux are Unix-only anyway, so the tab-hijack
/// this guards against can only occur there.
#[cfg(unix)]
pub struct FakeMultiplexer {
    /// Owns the temp dir; dropping it deletes the fakes and the recorder.
    _dir: TempDir,
    bin_dir: PathBuf,
    recorder: PathBuf,
}

#[cfg(unix)]
impl FakeMultiplexer {
    /// Creates the temp dir, writes executable fake `zellij`/`tmux` scripts into
    /// it, and points them at a shared recorder file.
    pub fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().expect("Failed to create fake-multiplexer temp dir");
        let bin_dir = dir.path().to_path_buf();
        let recorder = bin_dir.join("invocations.log");

        for tool in ["zellij", "tmux"] {
            let script_path = bin_dir.join(tool);
            // POSIX sh that appends `<tool> <args>` to the recorder, then exits 0
            // so the spawning `nwt` believes the multiplexer command succeeded.
            // `"$*"` joins the args with spaces, which is all the substring-based
            // assertions need (tab names never contain spaces).
            let script = format!(
                "#!/bin/sh\nprintf '%s %s\\n' '{tool}' \"$*\" >> '{recorder}'\nexit 0\n",
                recorder = recorder.display()
            );
            std::fs::write(&script_path, script).expect("Failed to write fake multiplexer script");
            let mut perms = std::fs::metadata(&script_path)
                .expect("Failed to stat fake multiplexer script")
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script_path, perms)
                .expect("Failed to mark fake multiplexer script executable");
        }

        Self {
            _dir: dir,
            bin_dir,
            recorder,
        }
    }

    /// A `PATH` value with the fake bin dir prepended to the inherited `PATH`, so
    /// the fakes shadow any real `zellij`/`tmux` while real tools (e.g. `git`,
    /// which `nwt` shells out to) still resolve normally.
    pub fn path_env(&self) -> std::ffi::OsString {
        let mut joined = std::ffi::OsString::from(&self.bin_dir);
        if let Some(existing) = std::env::var_os("PATH") {
            joined.push(":");
            joined.push(existing);
        }
        joined
    }

    /// Every recorded invocation, newline-separated. Empty string if no fake was
    /// ever invoked (the recorder file is only created on first write).
    pub fn recorded(&self) -> String {
        std::fs::read_to_string(&self.recorder).unwrap_or_default()
    }
}

#[cfg(unix)]
impl Default for FakeMultiplexer {
    fn default() -> Self {
        Self::new()
    }
}

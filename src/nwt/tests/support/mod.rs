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
//! The same class of escape exists for git, and
//! [`gitscratch::shed_inherited_git_environment`] is the single answer to it:
//! every `git` child this module spawns — the fixture's own commands and the
//! real `nwt` binary alike — has the entire inherited `GIT_*` family removed, so
//! it can only act on the directory it was handed. The rule is the prefix, never
//! a list of names, and it lives in `gitscratch` because every git-spawning
//! caller in this repository is broken the same way by the same environment.
//! That function explains why a list is the bug.
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

use gitscratch::shed_inherited_git_environment;
use tempfile::TempDir;

/// Runs a git command in `dir` with stdin/stdout/stderr nulled, returning
/// whether it succeeded. Output is nulled so concurrent test runs (a background
/// `bacon` loop alongside the pre-commit hook's own run) don't interleave noise.
///
/// Sheds the whole inherited `GIT_*` family through
/// [`gitscratch::shed_inherited_git_environment`], which is what makes `dir` the
/// repo git operates on. `current_dir(dir)` alone is not enough: when the suite
/// runs from inside a git hook, git exports its own variables into the hook's
/// environment, `cargo test` inherits them, and `GIT_DIR` overrides cwd-based
/// discovery — so a fixture's `git config`/`commit` lands in the *real* repo.
/// That is how `Test <t@example.com>` got written into this repo's own
/// `.git/config` and then authored every later commit until it was noticed. A
/// config write is sticky: one leak outlives the run that caused it.
///
/// The rule is the `GIT_` prefix and never a list of names, and it lives in
/// `gitscratch` so this suite and every other git-spawning caller in the
/// repository share one answer instead of each keeping a copy to drift. See
/// that function for why a list is the bug and which variables walked through
/// the last one. Pinned here by `tests/git-env-isolation.rs` and
/// `tests/builder-guard.rs`, which probe the behaviour at this call site rather
/// than trusting the helper.
///
/// Shedding also drops `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM`, and that costs
/// nothing: removing a variable is not the same as pinning it to `/dev/null`.
/// With them gone git falls back to the host's `~/.gitconfig` and
/// `/etc/gitconfig` exactly as it does in a normal shell, so settings the
/// fixtures rely on — `init.defaultBranch` among them — still apply.
pub fn run_git(dir: &Path, args: &[&str]) -> bool {
    let mut cmd = Command::new("git");
    shed_inherited_git_environment(&mut cmd);
    cmd.args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Runs a git command in `dir` and hands back its standard output.
///
/// [`run_git`] nulls every stream, so a test that has to *read* an answer out of
/// git — `git worktree list --porcelain`, say — needs this one instead. The two
/// share the one rule that matters: the whole inherited `GIT_*` family is shed
/// through [`gitscratch::shed_inherited_git_environment`], so `dir` is the only
/// repository the command can reach. See [`run_git`] for why a list of names is
/// the bug and the prefix is the rule.
///
/// # Panics
///
/// Panics if git cannot be spawned or exits non-zero. The panic message carries
/// the command, its standard output and its standard error.
pub fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    shed_inherited_git_environment(&mut cmd);

    let output = cmd
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn git {args:?}: {e}"));

    assert!(
        output.status.success(),
        "git {args:?} failed in {}:\n{}\n{}",
        dir.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    String::from_utf8_lossy(&output.stdout).into_owned()
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
/// It likewise sheds the whole inherited `GIT_*` family via
/// [`gitscratch::shed_inherited_git_environment`], so the spawned `nwt` — which
/// shells out to `git worktree add` — operates on
/// `repo` and not on whatever repo an inherited `GIT_DIR` names, writes its
/// objects into `repo`'s own store, and reads no config the launching
/// environment injected. Both scrubs guard the same class of bug: a fixture
/// reaching out and acting on the developer's real session.
///
/// `ZELLIJ`/`TMUX` stay named one at a time because they are not a prefix
/// family — there is no `MULTIPLEXER_*` to sweep — while the git variables are,
/// which is why they get the prefix rule instead of a list.
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
    // Same idea for git: a hook exports these, `cargo test` inherits them, and
    // GIT_DIR beats `current_dir`. Left in place, the spawned nwt would add its
    // worktree to the real repo, write objects into it, or run under config the
    // launching shell injected. Shed by prefix, never by name — see `run_git`
    // above, and `gitscratch::shed_inherited_git_environment` for the rule.
    shed_inherited_git_environment(&mut cmd);
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

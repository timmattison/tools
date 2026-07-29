//! Pins that `nwt`'s shared test fixtures stay sandboxed from the ambient git
//! environment.
//!
//! When git invokes a hook it exports `GIT_DIR`/`GIT_WORK_TREE`/
//! `GIT_INDEX_FILE` into the hook's environment, and anything that hook
//! launches — `cargo test` included — inherits them. `GIT_DIR` overrides
//! cwd-based repo discovery, so a fixture helper that only sets
//! `current_dir(tempdir)` still has its `git` child operate on the *real* repo.
//!
//! That is not hypothetical. It is how `user.email = t@example.com` /
//! `user.name = Test` (this suite's sibling fixture values over in `gsw`) came
//! to be written into this repo's own `.git/config`, which then silently
//! authored every subsequent commit as `Test <t@example.com>` until it was
//! noticed. A config write is sticky: one leak outlives the run that caused it.
//!
//! `.husky/pre-commit` already scrubs these vars before `cargo test`, but that
//! is a single caller. This pins the guarantee at the fixture itself — matching
//! what `gsw`'s suite already does — so the sandbox holds for any other runner
//! (`bacon`, an IDE, a future hook or CI job) that forgets the scrub.
//!
//! This file deliberately holds a SINGLE `#[test]`: it mutates the process-wide
//! environment, and cargo runs the tests within one test binary on parallel
//! threads. One test per binary means there is no sibling thread to race with,
//! while every other test file is a separate process with its own environment.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

use support::run_git;

/// The email the probe asks `run_git` to write.
///
/// Deliberately unique to this test and under the reserved `.invalid` TLD
/// (RFC 2606), so finding it in any config file is unambiguous evidence of a
/// leak rather than a pre-existing value someone legitimately configured.
const LEAK_MARKER: &str = "nwt-git-env-isolation-probe@example.invalid";

/// Runs `git init` in `dir` with the git-location environment scrubbed.
///
/// The test's own setup must not be redirected by the very leak it is probing
/// for, so this does not go through [`run_git`] — that is the helper under
/// test, and before the fix it is exactly what fails to scrub.
fn init_sandboxed_repo(dir: &Path) {
    let status = Command::new("git")
        .arg("init")
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .status()
        .expect("invoke git init");
    assert!(status.success(), "git init failed in {}", dir.display());
}

/// Creates `<temp>/<name>` as a freshly initialised git repo, returning its path.
fn sandboxed_repo(temp: &TempDir, name: &str) -> PathBuf {
    let repo = temp.path().join(name);
    fs::create_dir(&repo).expect("create repo dir");
    init_sandboxed_repo(&repo);
    repo
}

/// Reads a repo's local git config file as a string.
fn read_local_config(repo: &Path) -> String {
    fs::read_to_string(repo.join(".git").join("config")).expect("read local git config")
}

#[test]
fn run_git_ignores_an_ambient_git_dir() {
    let temp = TempDir::new().expect("create temp dir");
    // Stands in for the developer's real repo — the one an inherited `GIT_DIR`
    // points at when the suite runs from inside a git hook.
    let sentinel = sandboxed_repo(&temp, "sentinel");
    // The repo `run_git` is explicitly pointed at, and the only one it may touch.
    let fixture = sandboxed_repo(&temp, "fixture");

    // Reproduce the hook environment: `GIT_DIR` naming the sentinel repo, while
    // the helper is handed an entirely different directory.
    std::env::set_var("GIT_DIR", sentinel.join(".git"));
    let ok = run_git(&fixture, &["config", "user.email", LEAK_MARKER]);
    std::env::remove_var("GIT_DIR");

    assert!(ok, "the probe's `git config` invocation should succeed");

    let sentinel_config = read_local_config(&sentinel);
    assert!(
        !sentinel_config.contains(LEAK_MARKER),
        "run_git wrote to the repo named by the ambient GIT_DIR instead of the \
         directory it was given, so a fixture running under a git hook would \
         corrupt the real repo's config.\nsentinel config:\n{sentinel_config}"
    );

    let fixture_config = read_local_config(&fixture);
    assert!(
        fixture_config.contains(LEAK_MARKER),
        "run_git must still write to the directory it was given — otherwise \
         this test could pass merely because the write went nowhere.\nfixture \
         config:\n{fixture_config}"
    );
}

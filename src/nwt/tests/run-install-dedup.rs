//! Integration test pinning the dedup of nwt's own hook-bootstrap install
//! against a synchronous `--run` command that already installs dependencies
//! (issue #275 code-review finding).
//!
//! In a repo whose `package.json` declares a `prepare` script, nwt bootstraps
//! git hooks by running the package manager's install. But a user who also
//! passes `--run "pnpm install"` would, before this fix, install TWICE per
//! worktree — once for the bootstrap and once for the run — roughly doubling
//! creation time on Node repos. The fix: when the synchronous (non-tmux) run
//! command already invokes a known package manager's install, skip nwt's own
//! bootstrap install; the deferred ungated-worktree safety net still catches a
//! run that fails to create the hooks dir.
//!
//! The observable signal is `bootstrap_hooks`'s own stderr line — it prints
//! `Bootstrapping git hooks: <pm> install` exactly when it actually runs an
//! install. (Counting `prepare`-script runs is NOT reliable here: pnpm short-
//! circuits the second install of an already-installed tree as "Already up to
//! date" and skips lifecycle scripts, so two installs can yield one prepare
//! run.) These tests prove BOTH halves of the contract against the real binary:
//!   * `--run "pnpm install"` → bootstrap line ABSENT + skip notice PRESENT,
//!     proving nwt's own bootstrap install was skipped.
//!   * `--run "true"` (a run that does NOT install) → bootstrap line PRESENT,
//!     proving the dedup didn't degrade into "never bootstrap when any --run is
//!     present".

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::TempDir;

/// The `prepare` script appends one line to this file on every install, so the
/// file's presence proves the hooks were bootstrapped at least once.
const PREPARE_MARKER: &str = "prepare-runs.txt";

/// Substring `bootstrap_hooks` prints (non-quiet) when it actually runs an
/// install. Its presence/absence is the load-bearing dedup signal.
const BOOTSTRAP_LINE: &str = "Bootstrapping git hooks:";

/// Substring of the one-line notice printed when bootstrap is skipped because
/// the run command already installs dependencies.
const SKIP_NOTICE: &str = "Skipping hook bootstrap";

/// Runs a git command in `dir` with stdout/stderr nulled, returning success.
/// Stdout/stderr are nulled so concurrent test runs don't interleave noise.
fn run_git(dir: &Path, args: &[&str]) -> bool {
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
fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos()
}

/// Returns true if `pnpm --version` runs, i.e. pnpm is installed.
fn pnpm_available() -> bool {
    Command::new("pnpm")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Creates a git repo (inside a TempDir subdir so the sibling
/// `<name>-worktrees` output directory also lands inside the TempDir), commits
/// a baseline `package.json` whose `prepare` script appends a line to
/// [`PREPARE_MARKER`] on every install. The package.json is COMMITTED so it
/// exists (tracked) in the new worktree. Zero dependencies keeps the install
/// offline-safe. Returns the TempDir (keep it alive) and the repo path.
fn repo_with_prepare_script() -> (TempDir, PathBuf) {
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

    // A prepare script that appends a marker line on every install. pnpm runs
    // lifecycle scripts through a shell, so `sh`-style syntax is fine. Zero
    // dependencies keeps the install offline-safe.
    std::fs::write(
        repo.join("package.json"),
        format!(
            r#"{{"name":"t","private":true,"version":"0.0.0","scripts":{{"prepare":"echo x >> {PREPARE_MARKER}"}}}}"#
        ),
    )
    .expect("Failed to write package.json");

    // Baseline commit. Commit exactly once (no retry loop); disable gpg signing
    // so a globally-configured signer can't break the test. package.json must be
    // tracked so it appears in the new worktree.
    assert!(run_git(&repo, &["add", "package.json"]), "git add failed");
    assert!(
        run_git(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "baseline"]
        ),
        "git commit failed"
    );

    (temp, repo)
}

/// Parses the worktree path from the FIRST line of nwt's stdout (later lines may
/// carry run-command output).
fn worktree_path_from_stdout(stdout: &str) -> PathBuf {
    let first = stdout
        .lines()
        .next()
        .expect("nwt printed no stdout; expected a worktree path");
    PathBuf::from(first.trim())
}

#[test]
fn run_that_installs_skips_bootstrap_install() {
    if !pnpm_available() {
        eprintln!("Skipping test: pnpm not available");
        return;
    }
    let (_temp, repo) = repo_with_prepare_script();

    // Process-unique branch name for parallel safety: a background bacon loop
    // runs these tests concurrently with the pre-commit hook's own run.
    let branch = format!("dedup-installs-{}-{}", std::process::id(), nanos());

    // `--run "pnpm install"` already runs an install. nwt must NOT also run its
    // own bootstrap install. stdin nulled so the install can't block on a prompt.
    let output = Command::new(env!("CARGO_BIN_EXE_nwt"))
        .args(["-b", &branch, "--run", "pnpm install"])
        .current_dir(&repo)
        .stdin(Stdio::null())
        .output()
        .expect("Failed to run nwt binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "nwt should succeed.\nstdout: {stdout}\nstderr: {stderr}"
    );

    // The crux: nwt's own bootstrap install must NOT run when the run command
    // already installs, so its stderr line must be absent and the skip notice
    // present. On current (pre-fix) code the bootstrap line is present → fails.
    assert!(
        !stderr.contains(BOOTSTRAP_LINE),
        "bootstrap install must be SKIPPED (no '{BOOTSTRAP_LINE}' line) when the \
         run command already installs dependencies.\nstderr: {stderr}"
    );
    assert!(
        stderr.contains(SKIP_NOTICE),
        "a skip notice ('{SKIP_NOTICE}') must be printed when bootstrap is \
         skipped.\nstderr: {stderr}"
    );

    // The run's own install still set up the hooks (prepare ran at least once).
    let worktree = worktree_path_from_stdout(&stdout);
    assert!(
        worktree.join(PREPARE_MARKER).exists(),
        "the run's install must still have run the prepare script.\nstdout: \
         {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn run_that_does_not_install_still_bootstraps() {
    if !pnpm_available() {
        eprintln!("Skipping test: pnpm not available");
        return;
    }
    let (_temp, repo) = repo_with_prepare_script();

    let branch = format!("dedup-noinstall-{}-{}", std::process::id(), nanos());

    // `--run "true"` does NOT install, so nwt must still bootstrap. This guards
    // against "fixing" the dedup by never bootstrapping when any --run present.
    let output = Command::new(env!("CARGO_BIN_EXE_nwt"))
        .args(["-b", &branch, "--run", "true"])
        .current_dir(&repo)
        .stdin(Stdio::null())
        .output()
        .expect("Failed to run nwt binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "nwt should succeed.\nstdout: {stdout}\nstderr: {stderr}"
    );

    // The run does not install, so nwt MUST bootstrap: the bootstrap line must
    // be present and no skip notice may appear.
    assert!(
        stderr.contains(BOOTSTRAP_LINE),
        "bootstrap install must RUN (expected '{BOOTSTRAP_LINE}') when the run \
         command does not install dependencies.\nstderr: {stderr}"
    );
    assert!(
        !stderr.contains(SKIP_NOTICE),
        "no skip notice ('{SKIP_NOTICE}') should appear when bootstrap is not \
         skipped.\nstderr: {stderr}"
    );

    // Bootstrap actually ran the prepare script.
    let worktree = worktree_path_from_stdout(&stdout);
    assert!(
        worktree.join(PREPARE_MARKER).exists(),
        "bootstrap install must have run the prepare script.\nstdout: \
         {stdout}\nstderr: {stderr}"
    );
}

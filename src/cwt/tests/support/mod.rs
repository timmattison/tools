//! Throwaway git fixtures for the `cwt` end-to-end tests.
//!
//! Every test builds its own family of repositories under its own temp
//! directory, so two copies of the suite can run at the same time without
//! sharing a path, a branch name, or a git index.

#![allow(dead_code)] // Each test file uses a different part of this module.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// Scrub the git-location env vars that git exports when it invokes a hook.
///
/// In a worktree, git exports absolute `GIT_DIR`/`GIT_WORK_TREE`/
/// `GIT_INDEX_FILE`/`GIT_PREFIX` to the pre-commit hook. Those leak into child
/// `git` and `cwt` processes and pin them to the real repository regardless of
/// `current_dir(tempdir)`, so fixture commits would land in the real repository.
///
/// `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` point at `/dev/null` so the
/// developer's own git config (aliases, `init.defaultBranch`, hooks) cannot
/// change what these tests observe. The identity vars are set explicitly for the
/// same reason: the pre-commit hook exports its own, and a fixture commit must
/// behave the same under a commit as it does under a bare `cargo test`.
pub fn scrub_git_env(cmd: &mut Command) -> &mut Command {
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "cwt test")
        .env("GIT_AUTHOR_EMAIL", "cwt@example.com")
        .env("GIT_COMMITTER_NAME", "cwt test")
        .env("GIT_COMMITTER_EMAIL", "cwt@example.com")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_PREFIX")
}

/// Run `git <args>` in `dir` and assert it succeeded.
pub fn git(dir: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    let output = scrub_git_env(&mut cmd).output().expect("failed to run git");
    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr),
    );
    output
}

/// Run the real `cwt` binary in `dir` and capture its result.
///
/// `NO_COLOR` keeps the output plain so assertions can match on text.
pub fn cwt(dir: &Path, args: &[&str]) -> Output {
    cwt_with_env(dir, args, &[])
}

/// Run the real `cwt` binary in `dir` with extra environment variables.
pub fn cwt_with_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cwt"));
    cmd.args(args).current_dir(dir).env("NO_COLOR", "1");
    for (key, value) in env {
        cmd.env(key, value);
    }
    scrub_git_env(&mut cmd).output().expect("failed to run cwt")
}

/// Standard output of a `cwt` run.
pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// The single path a navigating `cwt` run prints, with the newline removed.
pub fn target_path(output: &Output) -> String {
    stdout(output).trim_end().to_string()
}

/// Combined stdout and stderr of a `cwt` run, for message assertions.
pub fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The exit code of a `cwt` run.
pub fn code(output: &Output) -> i32 {
    output.status.code().expect("cwt was killed by a signal")
}

/// A family of repositories: one parent repository with child repositories
/// checked out one level below it, in the layout that `.gitignore` keeps out of
/// the parent's history.
///
/// ```text
/// root/
///   aaa-worktree                       worktree of child-b, branch aaa
///   family/                            repository, branch main
///   family-worktrees/feature           worktree of family, branch feature
///   family/docs/                       plain directory, not a repository
///   family/inside-wt                   worktree of family, branch inside
///   family/child-a/                    repository, branch main
///   family/child-a-worktrees/shared    worktree of child-a, branch shared
///   family/child-b/                    repository, branch trunk
///   family/child-b-worktrees/beta      worktree of child-b, branch beta
///   family/child-b-worktrees/shared    worktree of child-b, branch shared
/// ```
///
/// The duplicated names are deliberate. `main` exists in both `family` and
/// `child-a`, and `shared` exists in both `child-a` and `child-b`, so the tests
/// can prove which repository wins a tie and when `cwt` refuses to guess.
///
/// Two of the worktrees are placed where they are on purpose:
///
/// - `aaa-worktree` sorts before every other worktree of child-b, so a listing
///   that assumed a repository's main worktree comes first would be wrong.
/// - `inside-wt` is a worktree of the parent that sits one level below it,
///   where the scan for child repositories will find it. It must join the
///   parent's group rather than start a group of its own.
pub struct Family {
    /// Kept alive so the temp directory outlives the test.
    _tmp: TempDir,
    /// The canonical path of the temp directory. Canonical because git prints
    /// resolved paths, and on macOS the temp directory is reached through a
    /// symbolic link.
    root: PathBuf,
}

impl Family {
    /// Build the family described in the type documentation.
    pub fn build() -> Self {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let root = tmp
            .path()
            .canonicalize()
            .expect("failed to canonicalize temp dir");

        make_repo(&root.join("family"), "main");
        add_worktree(
            &root.join("family"),
            "../family-worktrees/feature",
            "feature",
        );
        // A worktree of the parent, sitting where the scan for children looks.
        add_worktree(&root.join("family"), "inside-wt", "inside");

        std::fs::create_dir_all(root.join("family/docs")).expect("failed to create docs dir");
        std::fs::write(root.join("family/docs/README.md"), "not a repository\n")
            .expect("failed to write docs file");

        make_repo(&root.join("family/child-a"), "main");
        add_worktree(
            &root.join("family/child-a"),
            "../child-a-worktrees/shared",
            "shared",
        );

        make_repo(&root.join("family/child-b"), "trunk");
        add_worktree(
            &root.join("family/child-b"),
            "../child-b-worktrees/beta",
            "beta",
        );
        add_worktree(
            &root.join("family/child-b"),
            "../child-b-worktrees/shared",
            "shared",
        );
        // Outside the family, and sorting before every other worktree child-b has.
        add_worktree(&root.join("family/child-b"), "../../aaa-worktree", "aaa");

        Self { _tmp: tmp, root }
    }

    /// Resolve a path inside the family, for example `family/child-a`.
    pub fn at(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// The path of a worktree as a string, for comparison with `cwt` output.
    pub fn path_of(&self, relative: &str) -> String {
        self.at(relative).display().to_string()
    }
}

/// Create a repository at `path` whose first branch is `branch`, with one
/// commit so `git worktree list` reports a real HEAD.
fn make_repo(path: &Path, branch: &str) {
    std::fs::create_dir_all(path).expect("failed to create repo dir");
    git(path, &["init", "--initial-branch", branch]);
    std::fs::write(path.join("README.md"), "fixture\n").expect("failed to write README");
    git(path, &["add", "README.md"]);
    git(path, &["commit", "--no-verify", "-m", "init"]);
}

/// Add a worktree of the repository at `repo`, at `relative` to that repository,
/// on a new branch named `branch`.
fn add_worktree(repo: &Path, relative: &str, branch: &str) {
    git(repo, &["worktree", "add", "-b", branch, relative]);
}

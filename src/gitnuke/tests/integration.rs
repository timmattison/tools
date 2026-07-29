//! End-to-end: drive the real `gitnuke` binary against throwaway git repos.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// Scrub the git-location env vars that git exports when it invokes a hook.
///
/// In a *worktree*, git exports absolute `GIT_DIR`/`GIT_WORK_TREE`/
/// `GIT_INDEX_FILE`/`GIT_PREFIX` to the pre-commit hook. Those leak into child
/// `git` and `gitnuke` processes and pin them to the *real* repo regardless of
/// `current_dir(tempdir)`, so fixture commits would land in the real repo and
/// `gitnuke` would nuke real worktrees. Every git and gitnuke invocation here
/// routes through this so the per-test tempdir is the only thing at risk.
///
/// `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` are pinned to `/dev/null` so the
/// developer's own git config (aliases, `init.defaultBranch`, hooks) cannot
/// change what these tests observe.
fn scrub_git_env(cmd: &mut Command) -> &mut Command {
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_PREFIX")
}

/// Run `git <args>` in `dir` and assert it succeeded.
fn git(dir: &Path, args: &[&str]) -> Output {
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

/// Run `git <args>` in `dir` without asserting success.
fn git_allow_fail(dir: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    scrub_git_env(&mut cmd).output().expect("failed to run git")
}

/// Run the real `gitnuke` binary in `dir` and capture its result.
fn gitnuke(dir: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gitnuke"));
    cmd.args(args).current_dir(dir);
    scrub_git_env(&mut cmd)
        .output()
        .expect("failed to run gitnuke")
}

/// Combined stdout + stderr of a `gitnuke` run, for message assertions.
fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// A throwaway directory holding a `main` repo plus any worktrees a test adds.
///
/// Every path lives under a fresh `TempDir`, so concurrent runs of the same
/// test never collide.
struct Fixture {
    /// Held only to keep the temp directory alive for the test's lifetime.
    _tmp: TempDir,
    /// Canonicalized temp root (macOS `/var` is a symlink to `/private/var`,
    /// and git records canonical worktree paths).
    root: PathBuf,
}

impl Fixture {
    /// Create a temp root containing a `main` repo with one commit on `main`.
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().canonicalize().expect("canonicalize tempdir");
        let fixture = Fixture { _tmp: tmp, root };
        fixture.init_repo("main");
        fixture
    }

    /// `git init` a repo named `name` under the temp root, with one commit.
    fn init_repo(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        std::fs::create_dir_all(&path).expect("create repo dir");
        git(&path, &["init", "-q", "-b", "main"]);
        git(&path, &["config", "user.email", "test@example.com"]);
        git(&path, &["config", "user.name", "Test"]);
        std::fs::write(path.join("README.md"), "hello\n").expect("write README");
        git(&path, &["add", "README.md"]);
        git(&path, &["commit", "-qm", "initial"]);
        path
    }

    fn main_repo(&self) -> PathBuf {
        self.root.join("main")
    }

    /// Add a linked worktree at `<root>/<dir>` checked out on a new `branch`.
    fn add_worktree(&self, dir: &str, branch: &str) -> PathBuf {
        let path = self.root.join(dir);
        git(
            &self.main_repo(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                path.to_str().expect("utf-8 path"),
            ],
        );
        path
    }
}

/// True if `refs/heads/<branch>` still exists in the repo.
fn branch_exists(repo: &Path, branch: &str) -> bool {
    git_allow_fail(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .status
    .success()
}

/// True if git still tracks a worktree at `path`.
fn worktree_registered(repo: &Path, path: &Path) -> bool {
    let output = git(repo, &["worktree", "list", "--porcelain"]);
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.strip_prefix("worktree ") == Some(path.to_str().expect("utf-8 path")))
}

#[test]
fn nukes_a_worktree_given_its_path() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    let main = fixture.main_repo();

    let output = gitnuke(&main, &[worktree.to_str().expect("utf-8 path")]);

    assert!(
        output.status.success(),
        "gitnuke failed: {}",
        combined(&output)
    );
    assert!(!worktree.exists(), "worktree directory should be gone");
    assert!(
        !worktree_registered(&main, &worktree),
        "git should no longer track the worktree"
    );
    assert!(
        !branch_exists(&main, "feature"),
        "branch 'feature' should be deleted"
    );
}

#[test]
fn nukes_a_worktree_given_its_branch_name() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("some-directory", "issue-42");
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["issue-42"]);

    assert!(
        output.status.success(),
        "gitnuke failed: {}",
        combined(&output)
    );
    assert!(!worktree.exists(), "worktree directory should be gone");
    assert!(
        !branch_exists(&main, "issue-42"),
        "branch 'issue-42' should be deleted"
    );
}

#[test]
fn nukes_a_worktree_given_its_directory_name() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("absurd-rock", "feature/login");
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["absurd-rock"]);

    assert!(
        output.status.success(),
        "gitnuke failed: {}",
        combined(&output)
    );
    assert!(!worktree.exists(), "worktree directory should be gone");
    assert!(
        !branch_exists(&main, "feature/login"),
        "branch 'feature/login' should be deleted"
    );
}

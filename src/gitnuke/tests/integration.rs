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

    /// Create a `sub` repo and register it as a submodule at `sub/` in `main`.
    ///
    /// Must be called *before* `add_worktree` so the worktree's HEAD contains
    /// the gitlink. `protocol.file.allow` is needed because git refuses
    /// file-transport submodule clones by default (CVE-2022-39253).
    fn add_submodule(&self) {
        self.init_repo("sub");
        let main = self.main_repo();
        git(
            &main,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                "../sub",
                "sub",
            ],
        );
        git(&main, &["commit", "-qm", "add submodule"]);
    }

    /// Check out the submodule contents inside a linked worktree, which is what
    /// makes git refuse to remove that worktree.
    fn populate_submodule(&self, worktree: &Path) {
        git(
            worktree,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
                "-q",
            ],
        );
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

/// A worktree containing a populated submodule is the case plain
/// `git worktree remove` refuses outright ("working trees containing submodules
/// cannot be moved or removed"). gitnuke must refuse too — but say what is in
/// the way and how to override it, and leave the branch alone.
#[test]
fn refuses_a_submodule_worktree_without_force() {
    let fixture = Fixture::new();
    fixture.add_submodule();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    fixture.populate_submodule(&worktree);
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["feature"]);
    let message = combined(&output);

    assert!(
        !output.status.success(),
        "gitnuke should refuse a submodule worktree without --force: {message}"
    );
    assert!(
        worktree.exists(),
        "the worktree must survive a refusal: {message}"
    );
    assert!(
        worktree_registered(&main, &worktree),
        "git must still track the worktree after a refusal: {message}"
    );
    assert!(
        branch_exists(&main, "feature"),
        "the branch must survive a refused removal: {message}"
    );
    assert!(
        message.contains("submodule"),
        "the message should say submodules are the problem: {message}"
    );
    assert!(
        message.contains("sub"),
        "the message should name the submodule in the way: {message}"
    );
    assert!(
        message.contains("--force"),
        "the message should point at --force: {message}"
    );
}

#[test]
fn nukes_a_submodule_worktree_with_force() {
    let fixture = Fixture::new();
    fixture.add_submodule();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    fixture.populate_submodule(&worktree);
    assert!(
        worktree.join("sub/README.md").exists(),
        "fixture should have a populated submodule checkout"
    );
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["--force", "feature"]);

    assert!(
        output.status.success(),
        "gitnuke --force should nuke a submodule worktree: {}",
        combined(&output)
    );
    assert!(
        !worktree.exists(),
        "worktree directory should be gone, submodule and all"
    );
    assert!(
        !worktree_registered(&main, &worktree),
        "git should no longer track the worktree"
    );
    assert!(
        !branch_exists(&main, "feature"),
        "branch 'feature' should be deleted"
    );
}

/// Uncommitted work is git's other reason to refuse a removal. gitnuke must not
/// delete the branch when that happens — the whole point of the worktree being
/// left standing is that the work inside it is still recoverable.
#[test]
fn keeps_the_branch_when_a_dirty_worktree_is_refused() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    std::fs::write(worktree.join("README.md"), "uncommitted work\n").expect("dirty the worktree");
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["feature"]);
    let message = combined(&output);

    assert!(
        !output.status.success(),
        "gitnuke should refuse a dirty worktree without --force: {message}"
    );
    assert!(worktree.exists(), "the dirty worktree must survive");
    assert!(
        branch_exists(&main, "feature"),
        "the branch must survive a refused removal: {message}"
    );
}

#[test]
fn force_nukes_a_dirty_worktree() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    std::fs::write(worktree.join("README.md"), "uncommitted work\n").expect("dirty the worktree");
    std::fs::write(worktree.join("scratch.txt"), "untracked\n").expect("add untracked file");
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["--force", "feature"]);

    assert!(
        output.status.success(),
        "gitnuke --force should nuke a dirty worktree: {}",
        combined(&output)
    );
    assert!(!worktree.exists(), "worktree directory should be gone");
    assert!(
        !branch_exists(&main, "feature"),
        "branch 'feature' should be deleted"
    );
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

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

/// Nuking the worktree your shell is sitting in leaves that shell in a deleted
/// directory, where every later git command fails confusingly. gitnuke is not a
/// shell function and cannot cd you out, so it must refuse — even with --force,
/// which is about git's refusals, not about breaking your shell.
#[test]
fn refuses_to_nuke_the_worktree_you_are_standing_in() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    let main = fixture.main_repo();

    let output = gitnuke(&worktree, &["--force", "feature"]);
    let message = combined(&output);

    assert!(
        !output.status.success(),
        "gitnuke should refuse to nuke its own cwd: {message}"
    );
    assert!(worktree.exists(), "the worktree must survive: {message}");
    assert!(
        branch_exists(&main, "feature"),
        "the branch must survive: {message}"
    );
    assert!(
        message.contains(main.to_str().expect("utf-8 path")),
        "the message should point at the main worktree to cd to: {message}"
    );
}

/// A subdirectory of the target is just as fatal to the shell as its root.
#[test]
fn refuses_when_cwd_is_below_the_target_worktree() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    let nested = worktree.join("nested/deeper");
    std::fs::create_dir_all(&nested).expect("create nested dir");
    let main = fixture.main_repo();

    let output = gitnuke(&nested, &["--force", "feature"]);

    assert!(
        !output.status.success(),
        "gitnuke should refuse from a subdirectory of the target: {}",
        combined(&output)
    );
    assert!(worktree.exists(), "the worktree must survive");
    assert!(branch_exists(&main, "feature"), "the branch must survive");
}

#[test]
fn dry_run_reports_the_plan_without_touching_anything() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["--dry-run", "feature"]);
    let message = combined(&output);

    assert!(output.status.success(), "dry run should succeed: {message}");
    assert!(worktree.exists(), "dry run must not remove the worktree");
    assert!(
        branch_exists(&main, "feature"),
        "dry run must not delete the branch"
    );
    assert!(
        message.contains("would"),
        "dry run should describe what it would do: {message}"
    );
    assert!(
        message.contains("feature"),
        "dry run should name the branch: {message}"
    );
}

/// A dry run is a preflight: it runs the same gates, so a submodule worktree
/// reports the same refusal it would hit for real.
#[test]
fn dry_run_reports_a_submodule_refusal() {
    let fixture = Fixture::new();
    fixture.add_submodule();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    fixture.populate_submodule(&worktree);
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["--dry-run", "feature"]);
    let message = combined(&output);

    assert!(
        !output.status.success(),
        "a dry run that would be refused should exit non-zero: {message}"
    );
    assert!(
        message.contains("--force"),
        "should point at --force: {message}"
    );
    assert!(worktree.exists(), "dry run must not remove the worktree");
}

/// `--safe` is the gitclean half of the tool: remove the worktree, but refuse to
/// throw away a branch whose commits are not merged anywhere.
#[test]
fn safe_mode_removes_the_worktree_but_keeps_an_unmerged_branch() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    std::fs::write(worktree.join("work.txt"), "unmerged work\n").expect("write work file");
    git(&worktree, &["add", "work.txt"]);
    git(&worktree, &["commit", "-qm", "unmerged commit"]);
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["--safe", "feature"]);
    let message = combined(&output);

    assert!(
        !output.status.success(),
        "keeping an unmerged branch is a failure to report, not a silent skip: {message}"
    );
    assert!(!worktree.exists(), "the worktree should still be removed");
    assert!(
        branch_exists(&main, "feature"),
        "an unmerged branch must survive --safe: {message}"
    );
    assert!(
        message.contains("feature"),
        "the message should name the surviving branch: {message}"
    );
}

#[test]
fn safe_mode_deletes_a_merged_branch() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["--safe", "feature"]);

    assert!(
        output.status.success(),
        "a merged branch should be deleted under --safe: {}",
        combined(&output)
    );
    assert!(!worktree.exists(), "worktree directory should be gone");
    assert!(
        !branch_exists(&main, "feature"),
        "the merged branch should be deleted"
    );
}

/// Default (non-`--safe`) behaviour is `git branch -D`: unmerged is no obstacle.
#[test]
fn deletes_an_unmerged_branch_by_default() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    std::fs::write(worktree.join("work.txt"), "unmerged work\n").expect("write work file");
    git(&worktree, &["add", "work.txt"]);
    git(&worktree, &["commit", "-qm", "unmerged commit"]);
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["feature"]);

    assert!(
        output.status.success(),
        "gitnuke should force-delete an unmerged branch: {}",
        combined(&output)
    );
    assert!(
        !branch_exists(&main, "feature"),
        "branch 'feature' should be force-deleted"
    );
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

#[test]
fn refuses_the_main_worktree() {
    let fixture = Fixture::new();
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["--force", "main"]);
    let message = combined(&output);

    assert!(
        !output.status.success(),
        "gitnuke must never nuke the main worktree: {message}"
    );
    assert!(main.exists(), "the main worktree must survive");
    assert!(
        branch_exists(&main, "main"),
        "branch 'main' must survive: {message}"
    );
}

#[test]
fn removes_a_detached_head_worktree_without_deleting_a_branch() {
    let fixture = Fixture::new();
    let main = fixture.main_repo();
    let worktree = fixture.root.join("detached-wt");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            worktree.to_str().expect("utf-8 path"),
        ],
    );

    let output = gitnuke(&main, &["detached-wt"]);
    let message = combined(&output);

    assert!(output.status.success(), "gitnuke failed: {message}");
    assert!(!worktree.exists(), "worktree directory should be gone");
    assert!(
        branch_exists(&main, "main"),
        "no branch should have been deleted: {message}"
    );
    assert!(
        message.contains("detached"),
        "should explain why no branch was deleted: {message}"
    );
}

#[test]
fn reports_an_unknown_target_and_lists_the_known_worktrees() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["no-such-thing"]);
    let message = combined(&output);

    assert!(
        !output.status.success(),
        "an unknown target should fail: {message}"
    );
    assert!(
        message.contains("no-such-thing"),
        "should quote the target back: {message}"
    );
    assert!(
        message.contains(worktree.to_str().expect("utf-8 path")),
        "should list the worktrees that do exist: {message}"
    );
    assert!(worktree.exists(), "nothing should have been removed");
}

/// A near-miss must be a miss end to end, not just in the resolver unit tests.
#[test]
fn never_nukes_a_worktree_whose_branch_merely_contains_the_target() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("wt-421", "issue-421");
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["issue-42"]);

    assert!(
        !output.status.success(),
        "a substring must not resolve: {}",
        combined(&output)
    );
    assert!(worktree.exists(), "issue-421's worktree must survive");
    assert!(
        branch_exists(&main, "issue-421"),
        "issue-421 must survive a request to nuke issue-42"
    );
}

#[test]
fn nukes_every_target_it_can_and_reports_the_ones_it_cannot() {
    let fixture = Fixture::new();
    let first = fixture.add_worktree("first-wt", "first");
    let second = fixture.add_worktree("second-wt", "second");
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["first", "no-such-thing", "second"]);
    let message = combined(&output);

    assert!(
        !output.status.success(),
        "a failed target should fail the run: {message}"
    );
    assert!(!first.exists(), "'first' should have been nuked: {message}");
    assert!(
        !second.exists(),
        "'second' should have been nuked: {message}"
    );
    assert!(
        !branch_exists(&main, "first") && !branch_exists(&main, "second"),
        "both resolvable branches should be deleted: {message}"
    );
    assert!(
        message.contains("no-such-thing"),
        "the unresolvable target should be reported: {message}"
    );
}

/// Directory names, branch names and submodule paths can all be multi-byte;
/// none of it may panic or mis-slice.
#[test]
fn nukes_a_worktree_with_multibyte_names_and_submodules() {
    let fixture = Fixture::new();
    fixture.add_submodule();
    let worktree = fixture.add_worktree("日本語テスト", "機能/ログイン-🎉");
    fixture.populate_submodule(&worktree);
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["--force", "機能/ログイン-🎉"]);

    assert!(
        output.status.success(),
        "gitnuke should handle multi-byte names: {}",
        combined(&output)
    );
    assert!(!worktree.exists(), "worktree directory should be gone");
    assert!(
        !branch_exists(&main, "機能/ログイン-🎉"),
        "the multi-byte branch should be deleted"
    );
}

#[test]
fn reports_its_version_with_git_metadata() {
    let fixture = Fixture::new();

    let output = gitnuke(&fixture.main_repo(), &["--version"]);
    let version = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "--version should succeed");
    assert!(
        version.starts_with("gitnuke 0.1.0 ("),
        "expected 'gitnuke <version> (<hash>, <status>)', got: {version}"
    );
}

#[test]
fn refuses_to_run_outside_a_git_repository() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let output = gitnuke(tmp.path(), &["anything"]);

    assert!(
        !output.status.success(),
        "gitnuke should fail outside a repo: {}",
        combined(&output)
    );
    assert!(
        combined(&output).contains("git repository"),
        "should say why: {}",
        combined(&output)
    );
}

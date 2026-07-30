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

/// The exit codes `gitnuke --help` and the README publish.
///
/// Spelled out here rather than imported from the binary on purpose: these are a
/// contract callers script against, so the test has to state the numbers
/// independently. A copy that followed `mod exit_codes` around would pin nothing.
mod exit_codes {
    pub const SUCCESS: i32 = 0;
    pub const NOT_IN_REPO: i32 = 1;
    pub const GIT_COMMAND_ERROR: i32 = 2;
    pub const WORKTREE_NOT_FOUND: i32 = 3;
    pub const MULTIPLE_MATCHES: i32 = 4;
    pub const SUBMODULES_PRESENT: i32 = 5;
    pub const INSIDE_TARGET: i32 = 6;
    pub const BRANCH_NOT_DELETED: i32 = 7;
    pub const LOCKED_WORKTREE: i32 = 8;
}

/// Assert the process exited with exactly `code`, showing gitnuke's output on
/// failure.
///
/// `context` says what the run was supposed to be refusing (or doing), so a
/// failure names the behaviour rather than just the number. A run killed by a
/// signal has no exit code at all; that is reported as such instead of being
/// silently compared against `None`.
fn assert_exit_code(output: &Output, code: i32, context: &str) {
    let actual = output.status.code();
    assert_eq!(
        actual,
        Some(code),
        "{context}: expected exit code {code}, got {}\n--- gitnuke output ---\n{}",
        match actual {
            Some(actual) => actual.to_string(),
            None => format!("no exit code ({})", output.status),
        },
        combined(output),
    );
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

    /// Lock `worktree` the way `git worktree lock` does, with or without a
    /// reason.
    ///
    /// A lock is a deliberate "leave this alone" marker, and git treats it as a
    /// refusal of its own: `git worktree remove --force` still declines and asks
    /// for `remove -f -f`.
    fn lock_worktree(&self, worktree: &Path, reason: Option<&str>) {
        let path = worktree.to_str().expect("utf-8 path");
        match reason {
            Some(reason) => git(
                &self.main_repo(),
                &["worktree", "lock", "--reason", reason, path],
            ),
            None => git(&self.main_repo(), &["worktree", "lock", path]),
        };
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

/// The path the cwd-guard hint offers as somewhere to cd to, if it offered one.
///
/// Asserting on the whole message cannot tell a useful hint from a useless one:
/// the target's path legitimately appears earlier in it ("you are inside
/// <target>"), so a plain `contains` is satisfied by a hint that names the very
/// directory being nuked. This pulls out just the parenthesised suggestion, and
/// yields None when the message makes none at all.
fn suggested_destination(message: &str) -> Option<&str> {
    let (_, after_marker) = message.split_once("(for example ")?;
    let (path, _) = after_marker.split_once(')')?;
    Some(path)
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

    assert_exit_code(
        &output,
        exit_codes::INSIDE_TARGET,
        "gitnuke should refuse to nuke its own cwd",
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
    assert_eq!(
        suggested_destination(&message),
        Some(main.to_str().expect("utf-8 path")),
        "the ordinary case should send the caller to the main worktree: {message}"
    );
}

/// The hint has to name somewhere *else*. It is built from git's worktree list,
/// where the main worktree comes first — which is the right answer for every
/// target except the main worktree itself. Standing in it and naming it, taking
/// the first entry regardless produces "cd somewhere else, for example: here",
/// advice that is useless in exactly the case it fires.
#[test]
fn never_suggests_cd_ing_to_the_worktree_being_nuked() {
    let fixture = Fixture::new();
    let main = fixture.main_repo();
    let elsewhere = fixture.add_worktree("elsewhere-wt", "elsewhere");

    let output = gitnuke(&main, &["--force", "main"]);
    let message = combined(&output);

    assert_exit_code(
        &output,
        exit_codes::INSIDE_TARGET,
        "the cwd guard owns the case of standing in the main worktree",
    );
    let Some(suggestion) = suggested_destination(&message) else {
        panic!("a worktree the caller could go to exists, so the hint should name it: {message}");
    };
    assert_ne!(
        suggestion,
        main.to_str().expect("utf-8 path"),
        "the hint must not send the caller to the directory it is refusing over: {message}"
    );
    assert_eq!(
        suggestion,
        elsewhere.to_str().expect("utf-8 path"),
        "the hint should name the worktree that is not the target: {message}"
    );
    assert!(main.exists(), "the main worktree must survive: {message}");
}

/// A repo with nothing but its main worktree has nowhere to send the caller.
/// The refusal still stands — the shell would still be stranded — so the hint
/// offers nothing rather than offering the target back.
#[test]
fn omits_the_suggestion_when_there_is_no_other_worktree_to_offer() {
    let fixture = Fixture::new();
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["--force", "main"]);
    let message = combined(&output);

    assert_exit_code(
        &output,
        exit_codes::INSIDE_TARGET,
        "the cwd guard still refuses when it has no advice to give",
    );
    assert_eq!(
        suggested_destination(&message),
        None,
        "with nowhere else to go the hint must stay silent, not name the target: {message}"
    );
    assert!(
        message.contains("cd somewhere else"),
        "the refusal should still say why it refused: {message}"
    );
    assert!(main.exists(), "the main worktree must survive: {message}");
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

    assert_exit_code(
        &output,
        exit_codes::INSIDE_TARGET,
        "gitnuke should refuse from a subdirectory of the target",
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

/// Uncommitted changes are one of the two refusals git itself raises, and a dry
/// run never invokes git to find out. The preflight promise is worthless if
/// `gitnuke -n x` clears a target that `gitnuke x` turns away.
#[test]
fn dry_run_reports_a_dirty_worktree_refusal() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    std::fs::write(worktree.join("README.md"), "uncommitted work\n").expect("dirty the worktree");
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["--dry-run", "feature"]);
    let message = combined(&output);

    // The same code the real run reports for this refusal, so the two agree.
    assert_exit_code(
        &output,
        exit_codes::GIT_COMMAND_ERROR,
        "a dry run must report the refusal a real run would hit",
    );
    assert!(worktree.exists(), "dry run must not remove the worktree");
    assert!(
        worktree_registered(&main, &worktree),
        "dry run must leave git still tracking the worktree: {message}"
    );
    assert!(
        branch_exists(&main, "feature"),
        "dry run must not delete the branch: {message}"
    );
    assert!(
        message.contains("--force"),
        "should point at --force, like the real refusal does: {message}"
    );
}

/// git refuses on untracked files too, not just modified ones — a preflight that
/// only asked about tracked changes would clear half the targets git rejects.
#[test]
fn dry_run_reports_an_untracked_only_worktree_refusal() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    std::fs::write(worktree.join("scratch.txt"), "untracked\n").expect("add untracked file");
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["--dry-run", "feature"]);
    let message = combined(&output);

    assert_exit_code(
        &output,
        exit_codes::GIT_COMMAND_ERROR,
        "untracked files alone are enough for git to refuse",
    );
    assert!(worktree.exists(), "dry run must not remove the worktree");
    assert!(
        branch_exists(&main, "feature"),
        "dry run must not delete the branch: {message}"
    );
}

/// The other refusal a real run hits: `--safe` keeps an unmerged branch and
/// exits 7. A dry run that reported "would delete branch feature" for it would
/// be advertising a deletion git is going to refuse.
#[test]
fn dry_run_reports_an_unmerged_branch_refusal_under_safe() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    std::fs::write(worktree.join("work.txt"), "unmerged work\n").expect("write work file");
    git(&worktree, &["add", "work.txt"]);
    git(&worktree, &["commit", "-qm", "unmerged commit"]);
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["--safe", "--dry-run", "feature"]);
    let message = combined(&output);

    assert_exit_code(
        &output,
        exit_codes::BRANCH_NOT_DELETED,
        "a dry run must report the branch --safe would refuse to delete",
    );
    assert!(worktree.exists(), "dry run must not remove the worktree");
    assert!(
        worktree_registered(&main, &worktree),
        "dry run must leave git still tracking the worktree: {message}"
    );
    assert!(
        branch_exists(&main, "feature"),
        "dry run must not delete the branch: {message}"
    );
    assert!(
        message.contains("feature"),
        "the message should name the branch that would be kept: {message}"
    );
}

/// The other half of the preflight's contract: it must not refuse what a real
/// run would happily do. Without this, a merge check that always answered "not
/// merged" would still satisfy the refusal tests above.
#[test]
fn dry_run_clears_a_merged_branch_under_safe() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["--safe", "--dry-run", "feature"]);
    let message = combined(&output);

    assert!(
        output.status.success(),
        "a merged branch would be deleted for real, so the dry run must clear \
         it: {message}"
    );
    assert!(worktree.exists(), "dry run must not remove the worktree");
    assert!(
        branch_exists(&main, "feature"),
        "dry run must not delete the branch: {message}"
    );
}

/// `git branch -d` measures merged-ness against a branch's *upstream* when it
/// has one, so a branch whose commits are only on its remote is still
/// deletable. The dry run has to answer the same question against the same ref
/// — and the real run immediately after it proves that is what git does.
#[test]
fn dry_run_clears_a_branch_merged_only_into_its_upstream_under_safe() {
    let fixture = Fixture::new();
    let main = fixture.main_repo();
    let remote = fixture.root.join("remote.git");
    let remote_path = remote.to_str().expect("utf-8 path");
    git(&main, &["init", "-q", "--bare", remote_path]);
    git(&main, &["remote", "add", "origin", remote_path]);

    let worktree = fixture.add_worktree("feature-wt", "feature");
    std::fs::write(worktree.join("work.txt"), "work only on the remote\n").expect("write file");
    git(&worktree, &["add", "work.txt"]);
    git(&worktree, &["commit", "-qm", "unmerged locally"]);
    // Now 'feature' is ahead of HEAD but identical to its upstream.
    git(&worktree, &["push", "-q", "-u", "origin", "feature"]);

    let dry_run = gitnuke(&main, &["--safe", "--dry-run", "feature"]);

    assert!(
        dry_run.status.success(),
        "a branch merged into its upstream is deletable, so the dry run must \
         not refuse it: {}",
        combined(&dry_run)
    );

    let real_run = gitnuke(&main, &["--safe", "feature"]);

    assert!(
        real_run.status.success(),
        "the real run must reach the same verdict the dry run gave: {}",
        combined(&real_run)
    );
    assert!(
        !branch_exists(&main, "feature"),
        "the branch should have been deleted for real"
    );
}

/// A detached worktree has no branch, so `--safe` has nothing to ask about
/// merged-ness. The preflight must clear it rather than invent a refusal.
#[test]
fn dry_run_of_a_detached_worktree_under_safe_still_succeeds() {
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

    let output = gitnuke(&main, &["--safe", "--dry-run", "detached-wt"]);
    let message = combined(&output);

    assert!(
        output.status.success(),
        "a clean detached worktree would be removed for real, so the dry run \
         must succeed: {message}"
    );
    assert!(worktree.exists(), "dry run must not remove the worktree");
    assert!(
        message.contains("detached"),
        "should explain why no branch would be deleted: {message}"
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

    assert_exit_code(
        &output,
        exit_codes::SUBMODULES_PRESENT,
        "a dry run that would be refused should report the refusal's own code",
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

    assert_exit_code(
        &output,
        exit_codes::BRANCH_NOT_DELETED,
        "keeping an unmerged branch is a failure to report, not a silent skip",
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

    assert_exit_code(
        &output,
        exit_codes::SUBMODULES_PRESENT,
        "gitnuke should refuse a submodule worktree without --force",
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

/// A lock is git's *third* refusal, and the one `--force` deliberately does not
/// buy through: `git worktree remove --force` on a locked worktree still fails,
/// asking for `remove -f -f`. A lock is a deliberate "leave this alone" marker
/// set by hand, so gitnuke honours it rather than escalating — and diagnoses it
/// itself instead of letting git's raw `fatal:` leak out.
#[test]
fn refuses_a_locked_worktree_even_with_force() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    let main = fixture.main_repo();
    fixture.lock_worktree(&worktree, None);

    let output = gitnuke(&main, &["--force", "feature"]);
    let message = combined(&output);

    assert_exit_code(
        &output,
        exit_codes::LOCKED_WORKTREE,
        "a locked worktree must get its own refusal, not git's raw failure",
    );
    assert!(
        worktree.exists(),
        "the locked worktree must survive: {message}"
    );
    assert!(
        worktree_registered(&main, &worktree),
        "git must still track the locked worktree: {message}"
    );
    assert!(
        branch_exists(&main, "feature"),
        "the branch must survive a refused removal: {message}"
    );
    assert!(
        message.contains("locked"),
        "the message should say the lock is what is in the way: {message}"
    );
    assert!(
        message.contains(&format!(
            "git worktree unlock {}",
            worktree.to_str().expect("utf-8 path")
        )),
        "the message should hand back the exact unlock command: {message}"
    );
}

/// `git worktree lock --reason` records *why* the worktree was locked. That
/// reason is the whole reason the lock is respectable, so the refusal has to
/// quote it back rather than making the caller go look it up.
#[test]
fn locked_worktree_refusal_quotes_the_lock_reason() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    let main = fixture.main_repo();
    fixture.lock_worktree(&worktree, Some("mid-bisect, do not touch"));

    let output = gitnuke(&main, &["--force", "feature"]);
    let message = combined(&output);

    assert_exit_code(
        &output,
        exit_codes::LOCKED_WORKTREE,
        "a reason does not change the refusal, only what it says",
    );
    assert!(
        message.contains("mid-bisect, do not touch"),
        "the message should quote git's recorded lock reason: {message}"
    );
    assert!(worktree.exists(), "the locked worktree must survive");
    assert!(branch_exists(&main, "feature"), "the branch must survive");
}

/// git prints a non-ASCII lock reason C-quoted and octal-escaped in its
/// porcelain output (`locked "\343\203\254..."`). Echoing that back at the
/// person who typed the reason is not surfacing it.
#[test]
fn locked_worktree_refusal_quotes_a_multibyte_lock_reason() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    let main = fixture.main_repo();
    fixture.lock_worktree(&worktree, Some("レビュー待ち 🎉"));

    let output = gitnuke(&main, &["--force", "feature"]);
    let message = combined(&output);

    assert_exit_code(
        &output,
        exit_codes::LOCKED_WORKTREE,
        "a multi-byte reason is still just a locked worktree",
    );
    assert!(
        message.contains("レビュー待ち 🎉"),
        "the reason should be readable, not octal-escaped: {message}"
    );
}

/// A dry run is a preflight: `gitnuke -n x` failing has to mean `gitnuke x`
/// fails the same way, so the lock must be reported with the same code there.
#[test]
fn dry_run_reports_a_locked_worktree_refusal() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    let main = fixture.main_repo();
    fixture.lock_worktree(&worktree, Some("held for CI"));

    let output = gitnuke(&main, &["--force", "--dry-run", "feature"]);
    let message = combined(&output);

    assert_exit_code(
        &output,
        exit_codes::LOCKED_WORKTREE,
        "a dry run must report the refusal a real run would hit",
    );
    assert!(worktree.exists(), "dry run must not remove the worktree");
    assert!(
        worktree_registered(&main, &worktree),
        "dry run must leave git still tracking the worktree: {message}"
    );
    assert!(
        branch_exists(&main, "feature"),
        "dry run must not delete the branch: {message}"
    );
    assert!(
        message.contains("held for CI"),
        "the dry run should quote the lock reason too: {message}"
    );
}

/// The other half of the contract: the check must key on the lock being *there*,
/// not on the worktree having once been locked. Unlocking puts it straight back
/// in reach of `--force`.
#[test]
fn force_still_nukes_a_worktree_that_was_unlocked_again() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature-wt", "feature");
    let main = fixture.main_repo();
    let path = worktree.to_str().expect("utf-8 path");
    fixture.lock_worktree(&worktree, Some("briefly"));
    git(&main, &["worktree", "unlock", path]);

    let output = gitnuke(&main, &["--force", "feature"]);

    assert!(
        output.status.success(),
        "an unlocked worktree must still nuke: {}",
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

    // git's own refusal, surfaced verbatim: gitnuke did not gate this itself, so
    // it reports the failed git command rather than a gitnuke-specific code.
    assert_exit_code(
        &output,
        exit_codes::GIT_COMMAND_ERROR,
        "gitnuke should refuse a dirty worktree without --force",
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

/// git refuses to remove the main worktree at all — `fatal: '…' is a main
/// working tree`, with or without `--force` — so gitnuke must fail too.
///
/// The run is deliberately made from a *second* worktree. From inside the main
/// worktree the cwd guard answers first (see
/// `cwd_guard_answers_before_the_main_worktree_refusal`), so a test standing
/// there passes without this rule ever being reached.
#[test]
fn refuses_the_main_worktree() {
    let fixture = Fixture::new();
    let main = fixture.main_repo();
    let elsewhere = fixture.add_worktree("elsewhere-wt", "elsewhere");

    let output = gitnuke(&elsewhere, &["--force", "main"]);
    let message = combined(&output);

    assert_exit_code(
        &output,
        exit_codes::GIT_COMMAND_ERROR,
        "gitnuke must never nuke the main worktree",
    );
    assert!(main.exists(), "the main worktree must survive: {message}");
    assert!(
        worktree_registered(&main, &main),
        "git must still track the main worktree: {message}"
    );
    assert!(
        branch_exists(&main, "main"),
        "branch 'main' must survive: {message}"
    );
    assert!(
        message.contains(main.to_str().expect("utf-8 path")),
        "the refusal should name the worktree it is about: {message}"
    );
}

/// The dry run's whole promise is that its verdict is the real run's verdict.
/// The main worktree is the one target no check gitnuke performs can catch —
/// it is neither locked, nor dirty, nor holding submodules — so a preflight
/// that never consults the worktree's *position* in git's listing reports
/// "would remove" for the one worktree that can never be removed.
#[test]
fn dry_run_refuses_the_main_worktree() {
    let fixture = Fixture::new();
    let main = fixture.main_repo();
    let elsewhere = fixture.add_worktree("elsewhere-wt", "elsewhere");

    let output = gitnuke(&elsewhere, &["--dry-run", "main"]);
    let message = combined(&output);

    assert_exit_code(
        &output,
        exit_codes::GIT_COMMAND_ERROR,
        "a dry run must report the refusal a real run would hit",
    );
    assert!(
        !message.contains("would remove"),
        "a dry run must not advertise a removal git will never perform: {message}"
    );
    assert!(main.exists(), "dry run must not remove the main worktree");
    assert!(
        worktree_registered(&main, &main),
        "dry run must leave git still tracking the main worktree: {message}"
    );
    assert!(
        branch_exists(&main, "main"),
        "dry run must not delete branch 'main': {message}"
    );
}

/// Standing in the main worktree and naming it trips two rules at once, and the
/// cwd guard owns the case. Whichever way the removal would have ended, a shell
/// left in a deleted directory is the caller's more immediate problem, and exit
/// 6 — not the main-worktree refusal's exit 2 — is what says so.
#[test]
fn cwd_guard_answers_before_the_main_worktree_refusal() {
    let fixture = Fixture::new();
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["--force", "main"]);
    let message = combined(&output);

    assert_exit_code(
        &output,
        exit_codes::INSIDE_TARGET,
        "the cwd guard, not the main-worktree rule, answers this case",
    );
    assert!(main.exists(), "the main worktree must survive: {message}");
    assert!(
        worktree_registered(&main, &main),
        "git must still track the main worktree: {message}"
    );
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

    assert_exit_code(
        &output,
        exit_codes::WORKTREE_NOT_FOUND,
        "an unknown target should fail",
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

    assert_exit_code(
        &output,
        exit_codes::WORKTREE_NOT_FOUND,
        "a substring must not resolve",
    );
    assert!(worktree.exists(), "issue-421's worktree must survive");
    assert!(
        branch_exists(&main, "issue-421"),
        "issue-421 must survive a request to nuke issue-42"
    );
}

/// A name that is one worktree's *directory* and another worktree's *branch* is
/// genuinely ambiguous. gitnuke destroys whatever it resolves, so an ambiguous
/// target has to destroy nothing at all and hand back both candidates.
#[test]
fn refuses_an_ambiguous_target_and_destroys_nothing() {
    let fixture = Fixture::new();
    // "shared" is the directory name of one worktree and the branch of another.
    let by_directory = fixture.add_worktree("shared", "branch-a");
    let by_branch = fixture.add_worktree("other", "shared");
    let main = fixture.main_repo();

    // Run from the main repo so "shared" cannot resolve as a relative path:
    // `<main>/shared` does not exist, only `<root>/shared` does.
    let output = gitnuke(&main, &["shared"]);
    let message = combined(&output);

    assert_exit_code(
        &output,
        exit_codes::MULTIPLE_MATCHES,
        "an ambiguous target must be refused",
    );
    assert!(
        by_directory.exists() && worktree_registered(&main, &by_directory),
        "the directory-name match must survive: {message}"
    );
    assert!(
        by_branch.exists() && worktree_registered(&main, &by_branch),
        "the branch-name match must survive: {message}"
    );
    assert!(
        branch_exists(&main, "branch-a") && branch_exists(&main, "shared"),
        "neither branch may be deleted for an ambiguous target: {message}"
    );
    assert!(
        message.contains(by_directory.to_str().expect("utf-8 path"))
            && message.contains(by_branch.to_str().expect("utf-8 path")),
        "both candidates should be listed so the caller can disambiguate: {message}"
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

    // The run exits with the *first* failure's code; the only failing target here
    // is the unresolvable one.
    assert_exit_code(
        &output,
        exit_codes::WORKTREE_NOT_FOUND,
        "a failed target should fail the run",
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

/// Naming the same target twice is one instruction, not two. The first pass
/// nukes the worktree and the second finds nothing left to match, so a run that
/// took the list verbatim would report "no worktree matches 'dup'" and exit 3 —
/// an error about its own success, on a tool people script around exit codes.
#[test]
fn nukes_a_repeated_target_once_and_still_succeeds() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("dup-wt", "dup");
    let main = fixture.main_repo();

    let output = gitnuke(&main, &["dup", "dup"]);
    let message = combined(&output);

    assert_exit_code(
        &output,
        exit_codes::SUCCESS,
        "a repeated target is one nuke, not a nuke followed by a failure",
    );
    assert!(!worktree.exists(), "worktree directory should be gone");
    assert!(
        !worktree_registered(&main, &worktree),
        "git should no longer track the worktree: {message}"
    );
    assert!(
        !branch_exists(&main, "dup"),
        "branch 'dup' should be deleted: {message}"
    );
    assert!(
        !message.contains("no worktree matches"),
        "the second mention must not be reported as a miss: {message}"
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

    assert_exit_code(
        &output,
        exit_codes::NOT_IN_REPO,
        "gitnuke should fail outside a repo",
    );
    assert!(
        combined(&output).contains("git repository"),
        "should say why: {}",
        combined(&output)
    );
}

/// The exit-code list `gitnuke --help` publishes, one entry per rendered line.
///
/// Stated here rather than derived from the binary for the same reason as
/// `mod exit_codes` above: this is the contract users read, so the test has to
/// spell it out independently.
const HELP_EXIT_CODE_LINES: [&str; 9] = [
    "- 0: Success",
    "- 1: Not in a git repository",
    "- 2: A git command failed",
    "- 3: No worktree matched the target",
    "- 4: The target matched more than one worktree",
    "- 5: The worktree contains submodules and `--force` was not given",
    "- 6: The shell is standing inside the target worktree",
    "- 7: The worktree was removed but its branch could not be deleted",
    "- 8: The worktree is locked, which `--force` does not override",
];

/// The usage examples `gitnuke --help` publishes, as (command, trailing note).
///
/// Each pair has to land on a single rendered line of its own: the command at
/// the start, its explanatory comment at the end.
const HELP_USAGE_EXAMPLES: [(&str, &str); 3] = [
    ("gitnuke ../feature-wt", "# by path"),
    ("gitnuke feature-wt", "# by directory name"),
    ("gitnuke issue-42", "# by branch name"),
];

/// `--help` has to render its long description with the newlines intact.
///
/// clap only keeps the line breaks of a doc comment when the *command* itself
/// carries `verbatim_doc_comment`; the per-flag attributes do not cover the
/// struct's own doc comment. Without it every usage example is jammed onto one
/// line and all eight exit codes run together as a single paragraph.
#[test]
fn long_help_renders_one_line_per_entry() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let output = gitnuke(tmp.path(), &["--help"]);

    assert!(
        output.status.success(),
        "--help should succeed: {}",
        combined(&output)
    );
    let help = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = help.lines().map(str::trim).collect();

    for entry in HELP_EXIT_CODE_LINES {
        assert!(
            lines.contains(&entry),
            "exit code entry {entry:?} should be a line of its own, got:\n{help}"
        );
    }

    for (command, note) in HELP_USAGE_EXAMPLES {
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with(command) && line.ends_with(note)),
            "usage example {command:?} should be a line of its own ending in {note:?}, got:\n{help}"
        );
    }
}

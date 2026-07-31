//! `swt merge` end to end: the order the guards fire in, what each refusal
//! leaves standing, and what a successful merge destroys.
//!
//! Every case drives the real binary, because every guarantee `merge` makes is
//! about the world outside the process — a worktree that survives or does not, a
//! branch that survives or does not, a parent whose HEAD did or did not move, and
//! a lock file that must never outlive the run that took it.
//!
//! Two properties are worth stating up front, because most of the cases below
//! exist to pin one of them:
//!
//! - **Cleanliness is scoped per side.** The parent is judged on tracked changes
//!   only — the documented `.swt-check` escape hatch is by definition an
//!   untracked file at the parent root, so an untracked-sensitive parent guard
//!   would hard-block every merge for anyone following the documented workflow.
//!   The subagent is judged including untracked files, because `git worktree
//!   remove` deletes the whole directory.
//! - **Nothing inside the locked region may exit the process.** The rebase
//!   conflict case is the one the region exists for, and it is also the one that
//!   would leak the lock — so it asserts on the lock file directly.
//!
//! Unix only: the fixtures are `sh` scripts dropped as executable `.swt-check`
//! overrides, which is precisely how the escape hatch is documented.
#![cfg(unix)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use support::{
    exiting_check, git, run_swt, unique, write_swt_check, LinkedWorktree, TestRepo, SWT_CHECK,
    TRACKED_FILE,
};

/// Basename of the lock file inside the repository's shared git directory. Built
/// here from the fixture's own layout rather than asked of `swt`, so the
/// assertions pin the location instead of echoing it back.
const LOCK_FILE: &str = "swt.lock";

/// A file the parent commits, so the parent can advance without conflicting.
const PARENT_FILE: &str = "parent-only.txt";

/// A file the subagent commits, so the subagent can advance without conflicting.
const SUBAGENT_FILE: &str = "subagent-only.txt";

/// A check that is green wherever only one side's file is present and red once
/// both are — that is, green in the parent, green in the subagent before a
/// rebase, and red in the subagent after one.
const RED_ONLY_AFTER_REBASE_CHECK: &str = concat!(
    "#!/bin/sh\n",
    "if [ -f parent-only.txt ] && [ -f subagent-only.txt ]; then exit 1; fi\n",
    "exit 0\n"
);

/// A check that is red exactly where a committed `red` marker sits, so the
/// parent can be green while the subagent is not.
const RED_WHERE_MARKED_CHECK: &str = "#!/bin/sh\ntest ! -f red\n";

/// The marker [`RED_WHERE_MARKED_CHECK`] fails on.
const RED_MARKER: &str = "red";

/// A check that appends the directory it ran in to a file beside *itself* — that
/// is, at the config root it was found in, never in the worktree it ran in, so
/// recording leaves no untracked file where untracked files are dirt. `pwd -P`
/// asks the kernel rather than trusting an inherited `PWD`.
const RECORD_CWD_CHECK: &str = "#!/bin/sh\npwd -P >> \"$(dirname \"$0\")/ran-in\"\n";

/// The file [`RECORD_CWD_CHECK`] appends to, at the config root.
const CWD_LOG: &str = "ran-in";

/// An untracked file in the parent that is not the escape hatch, so the parent's
/// tracked-only scope is pinned as a rule rather than as a special case for
/// `.swt-check`.
const PARENT_SCRATCH: &str = "scratch.txt";

/// Decodes a finished run's stdout.
fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Decodes a finished run's stderr.
fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Runs `swt merge <worktree>` from `cwd`.
fn merge_from(cwd: &Path, worktree: &Path) -> Output {
    run_swt(
        cwd,
        &["merge", worktree.to_str().expect("utf-8 fixture path")],
    )
}

/// Where the merge lock must land for a fixture repository: `swt.lock` in the
/// git directory shared by every worktree of it.
fn lock_path(repo: &TestRepo) -> PathBuf {
    repo.path().join(".git").join(LOCK_FILE)
}

/// Writes, stages and commits a file inside any worktree, and returns its path.
fn commit_in(dir: &Path, rel_path: &str, contents: &str) -> PathBuf {
    let full = dir.join(rel_path);
    fs::write(&full, contents).expect("fixture file");
    git(dir, &["add", "--", rel_path]);
    git(
        dir,
        &["commit", "--quiet", "-m", &format!("add {rel_path}")],
    );
    full
}

/// The commit a worktree's HEAD points at.
fn head_of(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"])
}

/// Whether a branch still exists in the repository.
fn branch_exists(repo: &TestRepo, branch: &str) -> bool {
    repo.branches(branch).iter().any(|found| found == branch)
}

/// A repository whose parent root carries `check` as its green-check override,
/// plus a linked subagent worktree that has committed [`SUBAGENT_FILE`].
///
/// This is the shape every merge starts from: a subagent with work to bring
/// back, and a parent configured to be able to judge it.
fn repo_with_subagent(check: &str) -> (TestRepo, LinkedWorktree) {
    let repo = TestRepo::new();
    write_swt_check(repo.path(), check);
    let worktree = repo.add_worktree("sub");
    commit_in(&worktree.path, SUBAGENT_FILE, "subagent work\n");
    (repo, worktree)
}

/// Asserts that a run left no merge lock behind. A leaked lock blocks every
/// later merge in the repository until the one-hour staleness reap.
fn assert_no_lock(repo: &TestRepo, context: &str) {
    let lock = lock_path(repo);
    assert!(
        !lock.exists(),
        "the merge lock at {} outlived the run ({context})",
        lock.display()
    );
}

// The parent is the thing being merged *into*. Merging it into itself would at
// best remove the worktree the user is standing in.
#[test]
fn merging_the_parent_worktree_into_itself_is_refused() {
    let repo = TestRepo::new();

    let output = merge_from(repo.path(), repo.path());

    assert_eq!(
        output.status.code(),
        Some(1),
        "merging the parent into itself must fail: {}",
        stderr_of(&output)
    );
    assert_eq!(
        stderr_of(&output),
        "Refusing: that's the parent worktree.\n",
        "the refusal must say which worktree it is refusing"
    );
    assert_eq!(stdout_of(&output), "", "a refused merge reports nothing");
}

// The path is the only thing the user typed, so a path that names nothing has to
// come back quoted rather than as a git error about some other directory.
#[test]
fn a_worktree_path_that_does_not_exist_is_named_in_the_refusal() {
    let repo = TestRepo::new();
    let ghost = repo.sibling("ghost");

    let output = merge_from(repo.path(), &ghost);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a missing worktree must fail the command: {}",
        stderr_of(&output)
    );
    assert_eq!(
        stderr_of(&output),
        format!("No such worktree: {}\n", ghost.display()),
        "the path the user typed must be named back"
    );
}

// In-progress work in the parent is exactly what a fast-forward would silently
// advance past, so a tracked modification stops the merge before anything moves.
#[test]
fn a_tracked_modification_in_the_parent_blocks_the_merge() {
    let (repo, worktree) = repo_with_subagent(&exiting_check(0));
    fs::write(repo.path().join(TRACKED_FILE), "half-finished\n").expect("parent edit");
    let parent_head = head_of(repo.path());

    let output = merge_from(repo.path(), &worktree.path);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a dirty parent must refuse the merge: {stderr}"
    );
    assert!(
        stderr.contains("Parent worktree has uncommitted changes:"),
        "the refusal must name the side and its scope: {stderr}"
    );
    assert!(
        !stderr.contains("uncommitted/untracked"),
        "untracked files are not the parent's scope, so they must not be claimed: {stderr}"
    );
    assert!(
        stderr.contains(TRACKED_FILE),
        "git's own listing of what is dirty must reach the user: {stderr}"
    );
    assert!(
        stderr.contains("Commit or stash before merging.\n"),
        "the user needs to be told what to do about it: {stderr}"
    );
    assert!(
        worktree.path.is_dir(),
        "a refused merge must leave the subagent worktree alone"
    );
    assert_eq!(
        head_of(repo.path()),
        parent_head,
        "nothing may be merged when the merge was refused"
    );
}

// The regression this scope exists for. The documented escape hatch is an
// uncommitted file at the parent root, so counting untracked files as parent dirt
// would hard-block every merge for anyone following the documented workflow — and
// it is a rule about untracked files generally, not a carve-out for `.swt-check`.
#[test]
fn untracked_files_in_the_parent_do_not_block_the_merge() {
    let (repo, worktree) = repo_with_subagent(&exiting_check(0));
    fs::write(repo.path().join(PARENT_SCRATCH), "scratch\n").expect("untracked parent file");

    let output = merge_from(repo.path(), &worktree.path);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "an untracked file in the parent must not block a merge: {stderr}"
    );
    assert!(
        !stderr.contains("Parent worktree has"),
        "the parent was called dirty over untracked files: {stderr}"
    );
    assert!(
        repo.path().join(SWT_CHECK).exists() && repo.path().join(PARENT_SCRATCH).exists(),
        "fixture precondition: both untracked files were present for the whole run"
    );
    assert!(
        !worktree.path.exists(),
        "the merge should have completed and cleaned the worktree up"
    );
}

// The other half of the asymmetry: `git worktree remove` deletes the whole
// subagent directory, so untracked work there is genuinely destroyed.
#[test]
fn an_untracked_file_in_the_subagent_blocks_the_merge() {
    let (repo, worktree) = repo_with_subagent(&exiting_check(0));
    fs::write(worktree.path.join("notes.txt"), "unsaved thinking\n").expect("untracked subagent");

    let output = merge_from(repo.path(), &worktree.path);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a dirty subagent must refuse the merge: {stderr}"
    );
    assert!(
        stderr.contains("Subagent worktree has uncommitted/untracked changes:"),
        "the refusal must name the side and its wider scope: {stderr}"
    );
    assert!(
        stderr.contains("notes.txt"),
        "git's own listing of what is dirty must reach the user: {stderr}"
    );
    assert!(
        stderr.contains("Commit or stash before merging.\n"),
        "the user needs to be told what to do about it: {stderr}"
    );
    assert!(
        worktree.path.is_dir(),
        "the work that would have been destroyed must still be there"
    );
}

// A red parent is an in-progress red→green cycle. Advancing it with somebody
// else's work would bury the very failure the user is looking at.
#[test]
fn a_red_parent_refuses_the_merge_and_leaves_the_subagent_standing() {
    let (repo, worktree) = repo_with_subagent(&exiting_check(1));
    let parent_head = head_of(repo.path());

    let output = merge_from(repo.path(), &worktree.path);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a red parent must refuse the merge: {stderr}"
    );
    assert!(
        stderr.contains("Parent worktree not green:"),
        "the verdict must say which side was red: {stderr}"
    );
    assert!(
        stderr.contains("Refusing to merge — finish your red→green cycle in the parent first.\n"),
        "the user must be told what the refusal is protecting: {stderr}"
    );
    assert!(
        worktree.path.is_dir(),
        "a refusal must not destroy the subagent's work"
    );
    assert!(
        branch_exists(&repo, &worktree.branch),
        "a refusal must not delete the subagent's branch"
    );
    assert_eq!(
        head_of(repo.path()),
        parent_head,
        "nothing may be merged into a red parent"
    );
}

// The check the tool is named for. A subagent that is not green does not come
// back, and the parent is left exactly as it was.
#[test]
fn a_red_subagent_refuses_the_merge_and_nothing_is_merged() {
    let (repo, worktree) = repo_with_subagent(RED_WHERE_MARKED_CHECK);
    // Committed rather than merely written: an untracked marker would be caught
    // by the dirt guard, and this test is about the green guard.
    commit_in(&worktree.path, RED_MARKER, "red\n");
    let parent_head = head_of(repo.path());

    let output = merge_from(repo.path(), &worktree.path);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a red subagent must refuse the merge: {stderr}"
    );
    assert!(
        stderr.contains("Subagent worktree not green:"),
        "the verdict must say which side was red: {stderr}"
    );
    assert!(
        !stderr.contains("Parent worktree not green:"),
        "the parent was green; blaming it would send the user to the wrong tree: {stderr}"
    );
    assert_eq!(
        head_of(repo.path()),
        parent_head,
        "a red subagent must not reach the parent"
    );
    assert!(
        worktree.path.is_dir(),
        "a refusal must not destroy the subagent's work"
    );
    assert!(
        branch_exists(&repo, &worktree.branch),
        "a refusal must not delete the subagent's branch"
    );
    assert_no_lock(&repo, "red subagent");
}

// The whole point of the command: clean and green on both sides, so the work
// fast-forwards in and the worktree and branch that carried it are gone.
#[test]
fn a_clean_green_subagent_fast_forwards_in_and_is_cleaned_up() {
    let (repo, worktree) = repo_with_subagent(&exiting_check(0));
    let subagent_head = head_of(&worktree.path);

    let output = merge_from(repo.path(), &worktree.path);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "a clean green merge must succeed: {stderr}"
    );
    assert_eq!(
        stdout_of(&output),
        format!(
            "merged {}, removed {}\n",
            worktree.branch,
            worktree.path.display()
        ),
        "stdout reports what was merged and what was removed, and nothing else"
    );
    assert_eq!(
        head_of(repo.path()),
        subagent_head,
        "the parent must now be at the subagent's commit"
    );
    assert!(
        repo.path().join(SUBAGENT_FILE).exists(),
        "the subagent's work must actually be in the parent's tree"
    );
    assert!(
        !worktree.path.exists(),
        "the merged worktree must be gone from {}",
        worktree.path.display()
    );
    assert!(
        !branch_exists(&repo, &worktree.branch),
        "the merged branch must be gone too"
    );
    assert_no_lock(&repo, "successful merge");
}

// The parent moving on during a subagent's work is the normal case, not an
// error: the subagent is rebased onto it, re-verified, and only then fast-
// forwarded — so what lands is green *as merged*.
#[test]
fn a_parent_that_advanced_rebases_the_subagent_before_fast_forwarding() {
    let (repo, worktree) = repo_with_subagent(&exiting_check(0));
    commit_in(repo.path(), PARENT_FILE, "parent work\n");

    let output = merge_from(repo.path(), &worktree.path);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "a diverged parent must be handled by rebasing, not by failing: {stderr}"
    );
    assert!(
        stderr.contains("Parent advanced; rebasing subagent onto parent…\n"),
        "the user must be told why this run did more than a fast-forward: {stderr}"
    );
    assert!(
        repo.path().join(SUBAGENT_FILE).exists() && repo.path().join(PARENT_FILE).exists(),
        "both sides' work must survive the rebase"
    );
    assert!(
        !worktree.path.exists(),
        "the merged worktree must be gone from {}",
        worktree.path.display()
    );
    assert!(
        !branch_exists(&repo, &worktree.branch),
        "the merged branch must be gone too"
    );
    assert_no_lock(&repo, "merge after rebase");
}

// The case the locked region exists to handle, and the case that would leak the
// lock if it exited the process instead of returning. Nothing is destroyed: the
// conflicted rebase is left in the worktree for the user to finish.
#[test]
fn a_rebase_conflict_preserves_everything_and_leaves_no_lock_behind() {
    let (repo, worktree) = repo_with_subagent(&exiting_check(0));
    // Both sides rewrite the same line of the same tracked file.
    commit_in(&worktree.path, TRACKED_FILE, "subagent version\n");
    commit_in(repo.path(), TRACKED_FILE, "parent version\n");
    let parent_head = head_of(repo.path());

    let output = merge_from(repo.path(), &worktree.path);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "an unresolvable rebase must fail the merge: {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "Resolve conflicts in {}, then re-run: swt merge {}\n",
            worktree.path.display(),
            worktree.path.display()
        )),
        "the user needs the directory to fix and the command to re-run: {stderr}"
    );
    assert!(
        worktree.path.is_dir(),
        "a conflict must not destroy the work that conflicted"
    );
    assert!(
        branch_exists(&repo, &worktree.branch),
        "a conflict must not delete the subagent's branch"
    );
    assert_eq!(
        head_of(repo.path()),
        parent_head,
        "the parent must not move while the conflict is unresolved"
    );
    // The load-bearing one: a `process::exit` from inside the locked region
    // would skip the release, and this is the very failure that path exists for.
    assert_no_lock(&repo, "rebase conflict");
}

// Green in isolation is not enough — the merged result is what has to be green.
// A check that only fails once both sides' work is in one tree catches exactly
// the case a plain fast-forward would have waved through.
#[test]
fn a_check_that_fails_only_after_the_rebase_refuses_and_preserves_the_worktree() {
    let (repo, worktree) = repo_with_subagent(RED_ONLY_AFTER_REBASE_CHECK);
    commit_in(repo.path(), PARENT_FILE, "parent work\n");
    let parent_head = head_of(repo.path());

    let output = merge_from(repo.path(), &worktree.path);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a combination that is not green must not land: {stderr}"
    );
    assert!(
        stderr.contains("Not green after rebase:"),
        "the verdict must say the rebase is what turned it red: {stderr}"
    );
    assert!(
        worktree.path.is_dir(),
        "the rebased work must be left for the user to fix"
    );
    assert!(
        branch_exists(&repo, &worktree.branch),
        "the rebased branch must be left alone too"
    );
    assert_eq!(
        head_of(repo.path()),
        parent_head,
        "a result that is not green must never reach the parent"
    );
    assert_no_lock(&repo, "not green after rebase");
}

// The normal way this tool is used: the "parent" is itself a linked worktree, not
// the main repo. Its `.git` is a regular file, which is why the lock is resolved
// through `--git-common-dir` rather than by joining onto it.
#[test]
fn a_merge_launched_from_a_linked_worktree_works() {
    let repo = TestRepo::new();
    let parent = repo.add_worktree("parent");
    assert!(
        fs::metadata(parent.path.join(".git"))
            .expect("linked worktree .git")
            .is_file(),
        "fixture precondition: a linked worktree's .git must be a regular file"
    );
    write_swt_check(&parent.path, &exiting_check(0));

    // Branched from the linked worktree, exactly as `swt create` would have.
    let branch = unique("swt/nested");
    let subagent = repo.siblings().join(unique("nested"));
    git(
        &parent.path,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            &branch,
            subagent.to_str().expect("utf-8 fixture path"),
            "HEAD",
        ],
    );
    commit_in(&subagent, SUBAGENT_FILE, "subagent work\n");
    let subagent_head = head_of(&subagent);

    let output = merge_from(&parent.path, &subagent);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "a merge from a linked worktree must work: {stderr}"
    );
    assert_eq!(
        head_of(&parent.path),
        subagent_head,
        "the linked parent must be at the subagent's commit"
    );
    assert!(
        !subagent.exists(),
        "the merged worktree must be gone from {}",
        subagent.display()
    );
    assert!(
        !branch_exists(&repo, &branch),
        "the merged branch must be gone too"
    );
    assert_no_lock(&repo, "merge from a linked worktree");
}

// The same asymmetry `create` relies on: the override is looked up in the parent,
// because it is an uncommitted per-developer file a subagent's checkout does not
// have, and it is *run* in the worktree being judged.
#[test]
fn the_subagents_check_comes_from_the_parent_and_runs_in_the_subagent() {
    let (repo, worktree) = repo_with_subagent(RECORD_CWD_CHECK);
    assert!(
        !worktree.path.join(SWT_CHECK).exists(),
        "fixture precondition: the subagent has no override of its own"
    );
    let subagent_path = worktree.path.clone();

    let output = merge_from(repo.path(), &worktree.path);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "the parent's override must be found for both sides: {stderr}"
    );
    let log = fs::read_to_string(repo.path().join(CWD_LOG))
        .expect("the check should have recorded the directories it ran in");
    let ran_in: Vec<&str> = log.lines().collect();
    assert_eq!(
        ran_in,
        vec![
            repo.path().to_string_lossy().as_ref(),
            subagent_path.to_string_lossy().as_ref()
        ],
        "the parent is checked in the parent and the subagent in the subagent, in that order"
    );
}

//! Integration tests pinning what `nwt` says when the branch already exists.
//!
//! `nwt -b <branch>` asks git for a new branch. When that branch is already
//! there, git refuses, and the user needs to hear which of two different
//! problems they have: a branch that exists, which `--checkout` solves, or a
//! directory that exists, which `--random-directory` solves.
//!
//! `nwt` read git's message to tell the two apart, and it read the message for
//! a capital letter git does not write. Git says `fatal: a branch named 'x'
//! already exists`, and the check looked for `A branch named`. The check missed
//! every time, the next check caught the words `already exists`, and every
//! branch collision was reported as a directory collision. The named directory
//! did not exist, and the advice was for the wrong problem.
//!
//! With a random directory name the same miss cost more. A directory collision
//! is a reason to try another name, so `nwt` tried ten of them against the one
//! branch that could never work, and then reported a name collision that never
//! happened.
//!
//! So these tests read the message a user reads. They also drive the case that
//! retries, because a wrong answer there is the expensive one.

mod support;

use support::{init_repo, nanos, nwt_command, run_git};

/// Exit code `nwt` returns when git refuses to make the worktree.
const WORKTREE_FAILED: i32 = 7;

/// A branch name no other copy of this suite can be using.
fn unique_branch(label: &str) -> String {
    format!("{label}-{}-{}", std::process::id(), nanos())
}

/// A repository with one commit and `branch` already on it.
fn repo_with_branch(branch: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let (temp, repo) = init_repo();
    assert!(run_git(&repo, &["branch", branch]), "git branch failed");
    (temp, repo)
}

#[test]
fn a_branch_that_exists_is_named_as_a_branch() {
    let branch = unique_branch("taken");
    let (_temp, repo) = repo_with_branch(&branch);

    let output = nwt_command(&repo)
        .args(["-b", &branch, "--no-bootstrap-hooks"])
        .output()
        .expect("failed to run nwt");

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(WORKTREE_FAILED),
        "a branch collision is a worktree failure, not a name collision:\n{stderr}"
    );
    assert!(
        stderr.contains(&format!("Branch '{branch}' already exists.")),
        "the message must name the branch:\n{stderr}"
    );
    assert!(
        stderr.contains("Use --checkout"),
        "the advice must be the one that solves a branch collision:\n{stderr}"
    );
}

#[test]
fn a_branch_that_exists_is_never_reported_as_a_directory() {
    let branch = unique_branch("taken");
    let (temp, repo) = repo_with_branch(&branch);

    let output = nwt_command(&repo)
        .args(["-b", &branch, "--no-bootstrap-hooks"])
        .output()
        .expect("failed to run nwt");

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !stderr.contains("Directory"),
        "the directory does not exist, so no message may name one:\n{stderr}"
    );
    assert!(
        !stderr.contains("--random-directory"),
        "a new directory name cannot solve a branch collision:\n{stderr}"
    );
    assert!(
        !temp.path().join("repo-worktrees").exists(),
        "the run failed, so it must leave no worktrees directory"
    );
}

#[test]
fn a_branch_that_exists_stops_a_random_directory_run_at_once() {
    let branch = unique_branch("taken");
    let (_temp, repo) = repo_with_branch(&branch);

    // `--random-directory` keeps the branch fixed and asks for a new directory
    // name on each attempt. A branch collision cannot be solved that way, so the
    // run must stop at the first refusal rather than spend every attempt.
    let output = nwt_command(&repo)
        .args(["-b", &branch, "--random-directory", "--no-bootstrap-hooks"])
        .output()
        .expect("failed to run nwt");

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(WORKTREE_FAILED),
        "the run must fail as a worktree failure:\n{stderr}"
    );
    assert!(
        stderr.contains(&format!("Branch '{branch}' already exists.")),
        "the message must name the branch:\n{stderr}"
    );
    assert!(
        !stderr.contains("after 10 attempts"),
        "no attempt after the first can succeed, so none may be spent:\n{stderr}"
    );

    // How many attempts the run spent. `--quiet` silences nwt, so every line of
    // standard error comes from git, and git writes one `fatal:` for each
    // command that fails. The count is thus the number of attempts, and it was
    // ten before this fix.
    //
    // The count reads `fatal:` and not the sentence after it. That prefix is
    // how git marks an error in every message it writes, so it survives the
    // rewording that defeated the check this test exists for.
    let quiet = nwt_command(&repo)
        .args([
            "-b",
            &branch,
            "--random-directory",
            "--no-bootstrap-hooks",
            "-q",
        ])
        .output()
        .expect("failed to run nwt");

    let quiet_stderr = String::from_utf8_lossy(&quiet.stderr);
    assert_eq!(
        quiet_stderr.matches("fatal:").count(),
        1,
        "git must be asked for the branch once, not once for each attempt:\n{quiet_stderr}"
    );
}

#[test]
fn a_ref_checked_out_elsewhere_is_still_named_as_a_ref() {
    // The branch check must not capture the other failures git reports. This ref
    // exists and is checked out in a worktree, so the answer is the ref, not the
    // branch.
    let branch = unique_branch("busy");
    let (_temp, repo) = repo_with_branch(&branch);

    let first = nwt_command(&repo)
        .args(["-c", &branch, "--no-bootstrap-hooks"])
        .output()
        .expect("failed to run nwt");
    assert!(
        first.status.success(),
        "the first checkout must succeed:\n{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let output = nwt_command(&repo)
        .args(["-c", &branch, "--no-bootstrap-hooks"])
        .output()
        .expect("failed to run nwt");

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(WORKTREE_FAILED),
        "a ref in use is a worktree failure:\n{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "The ref '{branch}' is already checked out in another worktree."
        )),
        "the message must name the ref:\n{stderr}"
    );
}

#[test]
fn a_branch_that_does_not_exist_still_gets_its_worktree() {
    // The guard on the fix: asking git about the branch must not refuse a run
    // that has no branch collision at all.
    let branch = unique_branch("fresh");
    let (_temp, repo) = init_repo();

    let output = nwt_command(&repo)
        .args(["-b", &branch, "--no-bootstrap-hooks"])
        .output()
        .expect("failed to run nwt");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "a branch that does not exist must be created:\n{stderr}"
    );

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(
        std::path::Path::new(&path).is_dir(),
        "nwt must print the path of the worktree it made, got {path:?}"
    );
}

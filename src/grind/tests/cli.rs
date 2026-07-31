//! End-to-end coverage of the binary itself, so the wiring between argument
//! parsing, the replay and the exit code is exercised the way a user runs it.
//!
//! The exit code is the load-bearing half of every assertion here. `grind`'s
//! whole reason to exist is that a scripted caller can tell "conflicts" from
//! "something went wrong", so a test that only checked the words on stdout
//! would pass for a binary that answers every question with the same number.

use std::path::Path;
use std::process::{Command, Output};

use gitscratch::testing::{
    contested_region_repo, equal_hunks_unequal_stops_repo, independent_branches_repo, TestRepo,
};

/// Exit code for a replay that hit no conflicts.
const CLEAN: i32 = 0;

/// Exit code for a replay that hit conflicts.
const CONFLICTS: i32 = 1;

fn grind(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grind"))
        .args(args)
        .current_dir(repo)
        .output()
        .expect("failed to run grind")
}

/// Everything a test wants to look at, gathered once so an assertion failure
/// can print the whole picture rather than the one stream it happened to check.
fn run(repo: &TestRepo, head: &str, onto: &str) -> (Option<i32>, String, String) {
    repo.checkout(head);
    let output = grind(repo.path(), &[onto]);

    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string(),
        String::from_utf8_lossy(&output.stderr)
            .trim_end()
            .to_string(),
    )
}

/// Two branches that each add a file of their own rebase onto each other
/// without a single collision, and the only useful thing to say about that is
/// so — in one line, with exit 0 so a script can act on it without parsing
/// anything.
#[test]
fn a_rebase_that_collides_with_nothing_exits_clean_and_says_so_in_one_line() {
    let repo = independent_branches_repo();

    let (code, stdout, stderr) = run(&repo, "alpha", "beta");

    assert_eq!(
        code,
        Some(CLEAN),
        "a clean rebase must exit {CLEAN}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout, "grind: clean - replaying HEAD onto beta hit no conflicts",
        "stderr:\n{stderr}"
    );
}

/// `one` rewrites the same line of `x.txt` and `y.txt` that `two` already
/// rewrote, so replaying it collides in both files at once.
///
/// Asserted as one block rather than line by line because the shape *is* the
/// contract - the header, the summary indented under it, the blank line, and
/// the breakdown that says where the work lands - and a developer comparing
/// this against `grime` reads all of it together.
#[test]
fn a_rebase_that_collides_exits_conflicts_and_says_how_much_work_lands_where() {
    let repo = equal_hunks_unequal_stops_repo();

    let (code, stdout, stderr) = run(&repo, "one", "two");

    assert_eq!(
        code,
        Some(CONFLICTS),
        "a conflicting rebase must exit {CONFLICTS}, not be lumped in with clean\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout,
        r"grind: conflicts - replaying HEAD onto two
       2 hunks across 2 files, 1 stop

  x.txt    1 hunk
  y.txt    1 hunk",
        "stderr:\n{stderr}"
    );
}

/// How many stops the summary line reports.
///
/// Read back out of the rendered text rather than asserted as a whole block,
/// because what this test cares about is the *number* - the hunk count that
/// travels with it is an artefact of how conflict markers accumulate across
/// three collisions, and pinning it here would make the test fail for a reason
/// it is not about.
fn stop_count(stdout: &str) -> usize {
    let summary = stdout
        .lines()
        .find(|line| line.contains(" across "))
        .unwrap_or_else(|| panic!("no summary line in:\n{stdout}"));

    let clause = summary
        .rsplit(", ")
        .next()
        .expect("rsplit always yields at least one piece");
    let (count, unit) = clause
        .split_once(' ')
        .unwrap_or_else(|| panic!("summary does not end in a counted clause:\n{stdout}"));

    assert!(
        unit.starts_with("stop"),
        "the summary should end with the stop count, got {clause:?} in:\n{stdout}"
    );
    count
        .parse()
        .unwrap_or_else(|_| panic!("stop count {count:?} is not a number in:\n{stdout}"))
}

/// The asymmetry that makes a stop count worth printing at all: `iterated`
/// rewrote one line across three commits, so replaying it onto a branch that
/// already changed that line halts the rebase once per commit.
///
/// A tool that reported this as a single collision - the way a merge would, and
/// the way a rebase measured only at its first stop does - would tell a
/// developer the cheap and the expensive branch cost the same.
#[test]
fn a_branch_that_rewrote_one_region_across_three_commits_stops_more_than_once() {
    let repo = contested_region_repo();

    let (code, stdout, stderr) = run(&repo, "iterated", "single");

    assert_eq!(
        code,
        Some(CONFLICTS),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stop_count(&stdout) > 1,
        "three commits over one contested line must halt the rebase more than once, got:\n{stdout}"
    );
}

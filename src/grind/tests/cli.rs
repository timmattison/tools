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
    contested_region_repo, equal_hunks_unequal_stops_repo, independent_branches_repo,
    not_a_repository, TestRepo,
};

/// Exit code for a replay that hit no conflicts.
const CLEAN: i32 = 0;

/// Exit code for a replay that hit conflicts.
const CONFLICTS: i32 = 1;

/// Exit code for a run that could not answer the question at all.
///
/// Deliberately not [`CONFLICTS`]: "the rebase would collide" and "I could not
/// tell you" are different answers, and conflating them is the defect `grind`
/// exists to fix.
const ERROR: i32 = 2;

fn grind(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grind"))
        .args(args)
        .current_dir(repo)
        .output()
        .expect("failed to run grind")
}

/// Everything a test wants to look at, gathered once so an assertion failure
/// can print the whole picture rather than the one stream it happened to check.
fn streams(output: &Output) -> (Option<i32>, String, String) {
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

fn run(repo: &TestRepo, head: &str, onto: &str) -> (Option<i32>, String, String) {
    repo.checkout(head);

    streams(&grind(repo.path(), &[onto]))
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

/// Run `grind` with nowhere to put a temporary directory, so creating a scratch
/// worktree is guaranteed to fail and everything before it is not.
///
/// `TMPDIR` is set on the child process only. `std::env::set_var` is
/// process-global and Rust runs the tests in this binary as threads of one
/// process, so poisoning it there would sabotage every other test in the file.
fn grind_with_nowhere_to_put_a_scratch(
    repo: &TestRepo,
    branch: &str,
) -> (Option<i32>, String, String) {
    // Under the fixture's own `TempDir`, so two concurrent copies of this test
    // cannot name the same path - and never created, so it stays missing.
    let missing = repo.path().join("tmpdir-that-does-not-exist");

    let output = Command::new(env!("CARGO_BIN_EXE_grind"))
        .arg(branch)
        .current_dir(repo.path())
        .env("TMPDIR", missing)
        .output()
        .expect("failed to run grind");

    streams(&output)
}

/// A branch name that does not resolve is a bad argument, not a conflict, and
/// answering it must not cost a scratch worktree.
///
/// Proving *no scratch worktree was created* needs a discriminator that
/// survives the tool's own cleanup. `git worktree list` is not one: a `Scratch`
/// removes itself on drop, so the list comes back empty whether one was built
/// or not, and the assertion would pass for exactly the binary it is supposed
/// to catch.
///
/// `TMPDIR` is that discriminator. `Scratch::create` calls `TempDir::new`,
/// which resolves `TMPDIR`; `Repo` deliberately creates no temporary directory
/// at all, which is what makes the pre-flight unconditionally cheap. Pointing
/// `TMPDIR` at a path that does not exist therefore breaks exactly one of the
/// two - so if resolution still gets its word in, it demonstrably ran first.
///
/// The control half is what makes the first half mean anything: it proves the
/// poisoned `TMPDIR` really does reach `Scratch::create` rather than being
/// quietly ignored, which would make "no scratch error" vacuously true.
#[test]
fn a_branch_that_does_not_resolve_is_refused_before_any_scratch_worktree_exists() {
    let repo = independent_branches_repo();

    let (code, stdout, stderr) = grind_with_nowhere_to_put_a_scratch(&repo, "nonexistent-branch");

    assert_eq!(
        code,
        Some(ERROR),
        "an unresolvable branch must exit {ERROR}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("could not resolve 'nonexistent-branch'"),
        "the message must name the ref that did not resolve, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("scratch directory"),
        "resolution must happen before a scratch worktree is built, but the run \
         got as far as needing one:\n{stderr}"
    );
    // The live defect this tool was written to kill: the shell function it
    // replaces ran a bare `git rebase` and announced a typo as a conflict.
    assert!(
        !stdout.contains("conflicts") && !stderr.contains("conflicts"),
        "a typo'd branch name must never be reported as a conflict\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let (control_code, control_stdout, control_stderr) =
        grind_with_nowhere_to_put_a_scratch(&repo, "beta");

    assert_eq!(
        control_code,
        Some(ERROR),
        "stdout:\n{control_stdout}\nstderr:\n{control_stderr}"
    );
    assert!(
        control_stderr.contains("could not create a scratch directory"),
        "a resolvable branch with the same poisoned TMPDIR must fail at the \
         scratch, or the assertion above proves nothing:\n{control_stderr}"
    );
}

/// Somewhere outside every repository there is no question to answer, and
/// saying so has to be distinguishable from answering it.
///
/// The exit code is the whole point. A tool that reported this as `1` would be
/// telling a script "the rebase would conflict" about a directory it never
/// found a rebase in.
#[test]
fn a_directory_that_is_not_a_repository_is_an_error_not_a_conflict() {
    let elsewhere = not_a_repository();

    let (code, stdout, stderr) = streams(&grind(elsewhere.path(), &["main"]));

    assert_eq!(
        code,
        Some(ERROR),
        "running outside a repository must exit {ERROR}, never {CONFLICTS}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("is not inside a git repository"),
        "the message must say what was wrong with the directory, got:\n{stderr}"
    );
    assert!(
        !stdout.contains("conflicts") && !stderr.contains("conflicts"),
        "there was no rebase to conflict\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// A rebase can fail outright, leaving no halted rebase and no unmerged paths
/// to measure - and a replay that measures nothing must not therefore announce
/// that nothing went wrong.
///
/// `@{-1}` looks like an arbitrary choice and is not. It is the trigger because
/// it is *per-worktree*: it means "the branch checked out before this one", and
/// git answers it from the HEAD reflog of whichever worktree is asking.
///
/// That splits the two places `grind` resolves things. Standing in the
/// developer's repository - which has switched branches at least once, because
/// the harness just checked one out - `@{-1}` resolves, so the pre-flight
/// `Repo::resolve` accepts the argument and the run proceeds. The scratch
/// worktree, however, was created seconds ago and detached, so its HEAD reflog
/// holds no previous *branch* at all and `git rebase '@{-1}'` dies with
/// `fatal: invalid upstream '@{-1}'`, exit 128, having entered no rebase.
///
/// That is exactly the shape being pinned - git failed, there is no rebase in
/// progress, and `git diff --diff-filter=U` is empty - reached without
/// corrupting a repository or racing a background process to produce it.
#[test]
fn a_rebase_that_fails_with_nothing_to_measure_is_neither_clean_nor_conflicts() {
    let repo = independent_branches_repo();

    let (code, stdout, stderr) = run(&repo, "alpha", "@{-1}");

    assert_eq!(
        code,
        Some(ERROR),
        "a rebase that failed outright must exit {ERROR}, not {CLEAN} for \
         having counted no conflicts\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("invalid upstream"),
        "git's own explanation is the only part that says what went wrong, so \
         it has to survive to the user:\n{stderr}"
    );
    assert!(
        !stdout.contains("clean") && !stdout.contains("conflicts"),
        "a run that could not measure anything must claim neither verdict:\n{stdout}"
    );
}

/// `grind` simulates from HEAD, which is the only thing it *can* simulate from,
/// so uncommitted work is not an error - but a `clean` verdict must never be
/// read as covering it.
///
/// The clean run is taken first and used as the baseline, which is what makes
/// this one test rather than three. It pins that a tree with nothing
/// uncommitted says nothing at all - a note printed unconditionally would be
/// noise people learn to ignore - and then that dirtying the tree adds the
/// note and changes literally nothing else, neither the verdict a human reads
/// nor the number a script acts on.
///
/// The note goes to stderr precisely so that last part is true: a caller
/// piping stdout somewhere gets the same bytes either way.
#[test]
fn uncommitted_work_gets_a_note_on_stderr_and_leaves_the_answer_alone() {
    let repo = independent_branches_repo();

    let (clean_code, clean_stdout, clean_stderr) = run(&repo, "alpha", "beta");

    assert_eq!(
        clean_stderr, "",
        "a tree with nothing uncommitted has nothing to warn about"
    );

    // One tracked file modified and one file never added, because
    // `uncommitted_files` counts both and a note that missed either would be
    // undercounting exactly the work it exists to mention.
    repo.write_file("shared.txt", "locally edited, never committed\n");
    repo.write_file("scratch-notes.txt", "untracked work in progress\n");

    let (dirty_code, dirty_stdout, dirty_stderr) = run(&repo, "alpha", "beta");

    assert_eq!(
        dirty_stderr, "grind: note: 2 uncommitted files are not included; simulating from HEAD",
        "stdout:\n{dirty_stdout}"
    );
    assert_eq!(
        dirty_code, clean_code,
        "a dirty tree is not an error, so the exit code must not move\nstderr:\n{dirty_stderr}"
    );
    assert_eq!(
        dirty_stdout, clean_stdout,
        "the note belongs on stderr; stdout must be byte-for-byte what the \
         clean run produced"
    );
}

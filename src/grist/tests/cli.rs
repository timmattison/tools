//! End-to-end coverage of the binary itself, so the wiring between argument
//! parsing, simulation and output is exercised the way a user runs it.

use std::process::Command;

use gitscratch::testing::{
    contested_region_repo, equal_hunks_unequal_stops_repo, independent_branches_repo,
    not_a_repository,
};
use gitscratch::NoInheritedRepository;

const TIE_ADVICE: &str = "Every order costs the same";

/// Run `grist` in `repo`, with the ambient git environment taken back off.
///
/// A `cargo test` run from `.husky/pre-commit` inherits the hook's `GIT_DIR`
/// and `GIT_INDEX_FILE`, which name the developer's real repository. `grist`
/// reaches git only through `gitscratch`, which scrubs at the single place it
/// spawns one, so this is belt to the binary's braces — but it costs one call
/// and it means what these tests assert does not depend on how the suite was
/// started.
fn grist(repo: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_grist"))
        .args(args)
        .current_dir(repo)
        .without_inherited_repository()
        .output()
        .expect("failed to run grist")
}

/// `-q` exists so the answer can be piped straight into the next command.
#[test]
fn quiet_mode_prints_only_the_winning_order() {
    let repo = contested_region_repo();

    let output = grist(repo.path(), &["--onto", "main", "-q", "single", "iterated"]);

    assert!(
        output.status.success(),
        "grist failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "iterated single"
    );
}

/// The table names both orderings and marks the winner.
#[test]
fn default_output_ranks_both_orderings_with_their_costs() {
    let repo = contested_region_repo();

    let output = grist(repo.path(), &["--onto", "main", "single", "iterated"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert!(
        stdout.contains("iterated \u{2192} single"),
        "got:\n{stdout}"
    );
    assert!(
        stdout.contains("single \u{2192} iterated"),
        "got:\n{stdout}"
    );
    assert!(
        stdout.contains("Land them in this order: iterated single"),
        "got:\n{stdout}"
    );
}

/// Cost is ranked lexicographically on hunks, then stops, then files, so
/// matching hunk counts alone are not a tie. Here `two \u{2192} one` stops once and
/// `one \u{2192} two` stops twice on identical hunk and file counts: grist names a
/// winner, and must not then tell the user that winner does not matter.
#[test]
fn does_not_call_it_a_tie_when_only_the_hunk_counts_match() {
    let repo = equal_hunks_unequal_stops_repo();

    let output = grist(repo.path(), &["--onto", "main", "one", "two"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "grist failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let winner = stdout
        .find("two \u{2192} one")
        .unwrap_or_else(|| panic!("no row for the cheaper ordering, got:\n{stdout}"));
    let runner_up = stdout
        .find("one \u{2192} two")
        .unwrap_or_else(|| panic!("no row for the costlier ordering, got:\n{stdout}"));
    assert!(
        winner < runner_up,
        "fewer stops must rank first, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Land them in this order: two one"),
        "got:\n{stdout}"
    );

    assert!(
        !stdout.contains(TIE_ADVICE),
        "the orderings differ in stops, so the order does matter, got:\n{stdout}"
    );
}

/// The advice itself is not the bug, so it has to survive: branches that touch
/// nothing in common cost zero of everything in either order, and there grist
/// should say so.
#[test]
fn still_calls_it_a_tie_when_the_whole_cost_key_matches() {
    let repo = independent_branches_repo();

    let output = grist(repo.path(), &["--onto", "main", "alpha", "beta"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "grist failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains(TIE_ADVICE), "got:\n{stdout}");
}

/// The same rule as the library keeps, stated where the user actually meets it:
/// a replay grist could not carry out may reach them as an explanation, never as
/// a number.
///
/// The branches here touch nothing in common, which is why this is the input
/// worth testing. Unsealed, this repository is
/// [`still_calls_it_a_tie_when_the_whole_cost_key_matches`] — every ordering
/// costs zero and grist rightly says the order does not matter. Sealed, a replay
/// whose failure got swallowed would collapse to those same zeroes and print
/// that same shrug, and the user would take "pick whichever you prefer" as an
/// answer to a question grist never asked git.
///
/// So the whole of stdout is the assertion: no table, no winner, no tie advice.
/// The reason has to go to stderr, where a non-zero exit says the run is not an
/// answer.
#[cfg(unix)]
#[test]
fn a_failed_replay_never_reaches_the_user_as_a_cost_or_a_shrug() {
    use gitscratch::testing::branches_behind_main_repo;

    let repo = branches_behind_main_repo();

    // Sealed for the run only. `main` has moved ahead of both branches, so
    // replaying either has to write a commit and a read-only object database
    // cannot - while `git worktree add` writes no objects, so what fails is the
    // replay rather than grist's setup.
    let sealed = repo.seal_object_store();
    let output = grist(repo.path(), &["--onto", "main", "alpha", "beta"]);
    // Released before a single assertion runs, so a failing one cannot leave a
    // read-only directory behind for the temporary directory to trip over.
    drop(sealed);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "grist replayed nothing, so exiting zero tells the shell the ranking is good. \
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("Hunks"),
        "no replay finished, so there are no costs to tabulate, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("Land them in this order"),
        "grist cannot recommend an order it failed to price, got:\n{stdout}"
    );
    assert!(
        !stdout.contains(TIE_ADVICE),
        "costs that are all zero because nothing was replayed are not a tie, got:\n{stdout}"
    );
    assert!(
        !stderr.trim().is_empty(),
        "a non-zero exit with nothing said leaves the user guessing what went wrong"
    );
    // Named, not merely present. A swallowed replay still fails this run later
    // on - the squash cannot write against a sealed store either - so a bare
    // non-zero exit is satisfied by the wrong failure entirely, and only the
    // replay's own attribution tells the two apart.
    assert!(
        stderr.contains("could not replay 'alpha'"),
        "stderr should name the replay that failed, got:\n{stderr}"
    );
}

/// A branch list grist will not simulate has to be turned away cleanly. The
/// ordering count for 25 branches does not fit in a `usize`, so announcing the
/// run before the list is validated either panics or prints a wrapped, nonsense
/// count - and either way the user sees a run start that never could.
#[test]
fn rejects_an_over_limit_branch_count_without_panicking_or_announcing_a_run() {
    let repo = contested_region_repo();
    let names: Vec<String> = (1..=25).map(|n| format!("br{n}")).collect();

    let mut args = vec!["--onto", "main"];
    args.extend(names.iter().map(String::as_str));

    let output = grist(repo.path(), &args);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "grist should refuse 25 branches, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("panicked") && !stderr.contains("overflow"),
        "refusing an over-limit branch count must not panic, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("Simulating"),
        "grist must not announce a run it is about to refuse, got:\n{stderr}"
    );
}

/// Somewhere outside every repository there is no ordering to rank, and grist
/// has to say so in those words before it starts anything.
///
/// `gitscratch::Repo` is the pre-flight that produces that answer, and `grist`
/// is the consumer that skipped it: it went straight to building a scratch
/// worktree, so the refusal arrived as git's own `not a git repository`
/// complaint from inside `worktree add` — after the run had already been
/// announced, and naming `.git` rather than the directory the user pointed at.
///
/// All three halves of that matter to somebody reading a terminal. The message
/// has to be the pre-flight's, so it reads as a bad argument rather than a
/// simulation that fell over; it has to name the directory, because that is the
/// thing that was wrong; and nothing may announce a run that cannot happen,
/// which is the same rule an over-limit branch list is already held to above.
#[test]
fn refuses_a_directory_that_is_not_a_repository_before_announcing_a_run() {
    let elsewhere = not_a_repository();

    let output = grist(elsewhere.path(), &["--onto", "main", "left", "right"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "there is no repository to simulate against, got:\n{stderr}"
    );
    assert!(
        stderr.contains("is not inside a git repository"),
        "the refusal has to be the pre-flight's, not a failed simulation's, got:\n{stderr}"
    );
    assert!(
        stderr.contains(&elsewhere.path().display().to_string()),
        "the refusal has to name the directory it was pointed at, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("Simulating"),
        "grist must not announce a run it cannot start, got:\n{stderr}"
    );
}

/// Listing the same branch twice is a mistake, not a plan.
#[test]
fn rejects_a_repeated_branch() {
    let repo = contested_region_repo();

    let output = grist(repo.path(), &["--onto", "main", "single", "single"]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("only be listed once"),
        "got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

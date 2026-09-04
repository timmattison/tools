//! End-to-end coverage of the binary itself, so the wiring between argument
//! parsing, the replay and the exit code is exercised the way a user runs it.
//!
//! The exit code is the load-bearing half of every assertion here. `grime`'s
//! whole reason to exist is that a scripted caller can tell "conflicts" from
//! "something went wrong", so a test that only checked the words on stdout
//! would pass for a binary that answers every question with the same number.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output};

use gitscratch::testing::{equal_hunks_unequal_stops_repo, independent_branches_repo, TestRepo};
use gitscratch::NoInheritedGitEnvironment;

/// Exit code for a replay that hit no conflicts.
const CLEAN: i32 = 0;

/// Exit code for a replay that hit conflicts.
///
/// Deliberately not the code a failed run leaves behind: "the merge would
/// collide" and "I could not tell you" are different answers, and conflating
/// them is the defect `grime` exists to fix.
const CONFLICTS: i32 = 1;

/// The whole verdict for merging `two` into `one` in
/// [`equal_hunks_unequal_stops_repo`].
///
/// A constant because it is the shape a reader compares against `grind`'s own
/// verdict for the same fixture: the same header, the same summary indented
/// under it, the same blank line, and the same breakdown - with the verb
/// changed and the stop count gone.
const EQUAL_HUNKS_VERDICT: &str = r"grime: conflicts - merging two into HEAD
       2 hunks across 2 files

  x.txt    1 hunk
  y.txt    1 hunk";

/// The locale every `grime` this file starts is pinned to, and the two
/// variables that pin it.
///
/// Git wraps its own words in gettext, so a git built with the translations
/// answers a developer whose shell carries `LC_ALL=de_DE` in German. Any
/// assertion that reads git's words back would then pass or fail for a reason
/// it is not about. `LC_ALL` is the variable that decides. `LANG` stands beside
/// it because it costs nothing and spares the next reader the precedence rule.
///
/// A constant rather than two literals inside the builder, because the test
/// below reads it back off the command: a pin nothing asserts is a pin the next
/// person deletes.
const PINNED_LOCALE: [(&str, &str); 2] = [("LC_ALL", "C"), ("LANG", "C")];

/// The variable that states how many columns wide the terminal is.
///
/// `grime` lays its breakdown out for the width it is given, and every run this
/// file starts states that width rather than measuring one. The measurement is
/// of the *controlling terminal*, which is the window of the developer who
/// typed `cargo test`. A golden that depends on it holds in an ordinary window
/// and breaks in a narrow one, and it breaks on the machine of whoever has the
/// narrow window rather than on the machine of whoever wrote it.
const WIDTH_VARIABLE: &str = "COLUMNS";

/// The width every run in this file states.
///
/// Wide enough that no breakdown in this file is clamped, so each golden below
/// is the layout with no right-hand edge in it.
const STATED_WIDTH: &str = "200";

/// A `grime` run in `repo`, built and not yet started.
///
/// Handed back rather than run, so a call site that wants one more thing on the
/// command - a `TMPDIR` of its own, a stream pointed at a pipe that nobody
/// reads - still gets the environment of an ordinary run.
///
/// The scrub is belt to the binary's braces. `grime` reaches git only through
/// `gitscratch`, which scrubs at the single place it spawns one, so a leak
/// cannot reach the tool - but a test suite that let one through would be
/// asserting against a run nobody could reproduce.
fn grime_command(repo: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grime"));
    command
        .args(args)
        .current_dir(repo)
        .without_inherited_git_environment();

    // After the scrub, and for the reason `TestRepo::try_git` applies a
    // caller's own variables after its own: the rule the scrub applies is the
    // `GIT_` prefix, so anything set ahead of it that wears that prefix comes
    // straight back off. No name below wears it, and stating the order here is
    // what spares the next reader who adds one that does.
    command.envs(PINNED_LOCALE);
    command.env(WIDTH_VARIABLE, STATED_WIDTH);

    command
}

/// Run `grime` in `repo` and hand back everything it left behind.
fn grime(repo: &Path, args: &[&str]) -> Output {
    grime_command(repo, args)
        .output()
        .expect("failed to run grime")
}

/// What `command` gives the child for `name`, or `None` when it gives it
/// nothing of its own.
///
/// [`Command::get_envs`] reports a variable the caller removed as a `None`
/// value against its name, so the two answers this flattens together - never
/// mentioned, and mentioned to be taken away - are the same answer to the
/// question asked here.
fn environment_value(command: &Command, name: &str) -> Option<String> {
    command
        .get_envs()
        .find(|(key, _)| *key == OsStr::new(name))
        .and_then(|(_, value)| value)
        .map(|value| value.to_string_lossy().into_owned())
}

/// Every run in this file is pinned to one locale, and the pin is asserted here
/// rather than left to whichever test comes to depend on it.
///
/// Read off the built command rather than out of a run, because the machine
/// this suite is written on cannot show the failure: its git ships no
/// `share/git-core/locale`, so it answers in English whatever the environment
/// says. The risk is latent here and live on any machine or CI image that
/// carries git's translations, which is exactly the shape a test has to pin
/// rather than reproduce.
#[test]
fn every_run_is_pinned_to_one_locale_so_gits_own_words_arrive_untranslated() {
    let command = grime_command(Path::new("."), &["main"]);

    for (name, value) in PINNED_LOCALE {
        assert_eq!(
            environment_value(&command, name).as_deref(),
            Some(value),
            "a run that takes {name} from the developer's shell reads git's \
             words in the developer's language, and any assertion that matches \
             them fails for a reason it is not about"
        );
    }
}

/// Every run in this file states the width of its terminal, and the statement
/// is asserted here rather than left to the goldens that depend on it.
///
/// Read off the built command for the reason the locale pin is: the machine
/// this suite is written on cannot show the failure. Its window is wide, so
/// every golden below holds whether or not the width is stated. The failure is
/// live on a narrow window, which is exactly the shape a test has to pin rather
/// than reproduce.
#[test]
fn every_run_states_the_width_of_its_terminal_so_no_golden_reads_the_window() {
    let command = grime_command(Path::new("."), &["main"]);

    assert_eq!(
        environment_value(&command, WIDTH_VARIABLE).as_deref(),
        Some(STATED_WIDTH),
        "a run that measures the developer's window lays its breakdown out for \
         a width the test never chose, and every golden here then holds in a \
         wide window and breaks in a narrow one"
    );
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

/// Stand on `head` and ask about `branch`, the way a developer would.
fn run(repo: &TestRepo, head: &str, branch: &str) -> (Option<i32>, String, String) {
    repo.checkout(head);

    streams(&grime(repo.path(), &[branch]))
}

/// Two branches that each add a file of their own merge into each other without
/// a single collision, and the only useful thing to say about that is so - in
/// one line, with exit 0 so a script can act on it without parsing anything.
///
/// `alpha` and `beta` are siblings off `main`, so this is a genuine three-way
/// merge rather than a fast-forward: neither branch holds the other's commit.
#[test]
fn a_merge_that_collides_with_nothing_exits_clean_and_says_so_in_one_line() {
    let repo = independent_branches_repo();

    let (code, stdout, stderr) = run(&repo, "alpha", "beta");

    assert_eq!(
        code,
        Some(CLEAN),
        "a clean merge must exit {CLEAN}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout, "grime: clean - merging beta into HEAD hit no conflicts",
        "stderr:\n{stderr}"
    );
}

/// `two` rewrites the same line of `x.txt` and `y.txt` that `one` already
/// rewrote, so merging the two collides in both files at once.
///
/// Asserted as one block rather than line by line because the shape *is* the
/// contract - the header, the summary indented under it, the blank line, and
/// the breakdown that says where the work lands - and a developer comparing
/// this against `grind` reads all of it together.
#[test]
fn a_merge_that_collides_exits_conflicts_and_says_how_much_work_lands_where() {
    let repo = equal_hunks_unequal_stops_repo();

    let (code, stdout, stderr) = run(&repo, "one", "two");

    assert_eq!(
        code,
        Some(CONFLICTS),
        "a conflicting merge must exit {CONFLICTS}, not be lumped in with clean\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, EQUAL_HUNKS_VERDICT, "stderr:\n{stderr}");
}

/// A merge halts exactly once, so the number of halts is a constant dressed up
/// as a measurement, and `grime` says nothing about it.
///
/// Its own test rather than a clause of the golden above, because the golden
/// would hide it. A binary that started printing the stop count again would
/// fail the golden for the same reason it would fail a change to any other
/// character on that line, and nobody reading the failure would learn which
/// claim broke. This one can only break for one reason.
///
/// The whole of stdout is searched rather than the summary line alone, because
/// the claim is that the word reaches the reader nowhere - a stop count moved
/// onto the header or into the breakdown is the same constant in a new place.
///
/// `Conflicts` still counts the halt. The count is what `grind` reports and
/// what a fold over several replays adds up; only this rendering leaves it out.
#[test]
fn the_summary_of_a_conflicting_merge_carries_no_stop_count() {
    let repo = equal_hunks_unequal_stops_repo();

    let (code, stdout, stderr) = run(&repo, "one", "two");

    assert_eq!(
        code,
        Some(CONFLICTS),
        "the run has to reach a conflict verdict, or there is no summary to \
         read\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("stop"),
        "a merge halts once, so the number says nothing and belongs nowhere in \
         the verdict:\n{stdout}"
    );
}

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

use gitscratch::testing::{
    equal_hunks_unequal_stops_repo, independent_branches_repo, multi_byte_names_repo,
    nested_conflict_repo, not_a_repository, unrelated_histories_repo, TestRepo,
};
use gitscratch::NoInheritedGitEnvironment;
use unicode_width::UnicodeWidthStr;

/// Exit code for a replay that hit no conflicts.
const CLEAN: i32 = 0;

/// Exit code for a replay that hit conflicts.
///
/// Deliberately not the code a failed run leaves behind: "the merge would
/// collide" and "I could not tell you" are different answers, and conflating
/// them is the defect `grime` exists to fix.
const CONFLICTS: i32 = 1;

/// Exit code for a run that could not answer the question at all.
///
/// Deliberately not [`CONFLICTS`]: "the merge would collide" and "I could not
/// tell you" are different answers, and conflating them is the defect `grime`
/// exists to fix.
const ERROR: i32 = 2;

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
    streams(&run_raw(repo, head, &[branch]))
}

/// The raw output, for the assertions that care about the difference between
/// "printed nothing" and "printed only whitespace" - which [`streams`] trims
/// away and `-q` is judged on.
fn run_raw(repo: &TestRepo, head: &str, args: &[&str]) -> Output {
    repo.checkout(head);

    grime(repo.path(), args)
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

/// Run `grime` with nowhere to put a temporary directory, so creating a scratch
/// worktree is guaranteed to fail and everything before it is not.
///
/// `TMPDIR` is set on the child process only. `std::env::set_var` is
/// process-global and Rust runs the tests in this binary as threads of one
/// process, so poisoning it there would sabotage every other test in the file.
fn grime_with_nowhere_to_put_a_scratch(
    repo: &TestRepo,
    branch: &str,
) -> (Option<i32>, String, String) {
    // Under the fixture's own `TempDir`, so two concurrent copies of this test
    // cannot name the same path - and never created, so it stays missing.
    let missing = repo.path().join("tmpdir-that-does-not-exist");

    let output = grime_command(repo.path(), &[branch])
        .env("TMPDIR", missing)
        .output()
        .expect("failed to run grime");

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
/// `TMPDIR` is that discriminator. Building a scratch worktree - `Repo::scratch`
/// and, behind it, `Scratch`'s own constructor - calls `TempDir::new`, which
/// resolves `TMPDIR`. The pre-flight queries beside it (`Repo::open`,
/// `Repo::resolve`, `Repo::uncommitted_files`) deliberately create no temporary
/// directory at all, which is what makes them unconditionally cheap. Pointing
/// `TMPDIR` at a path that does not exist therefore breaks exactly one of the
/// two - so if resolution still gets its word in, it demonstrably ran first.
///
/// The control half is what makes the first half mean anything: it proves the
/// poisoned `TMPDIR` really does reach the worktree half rather than being
/// quietly ignored, which would make "no scratch error" vacuously true.
///
/// The tree is dirty on purpose, which is what gives the two runs a caveat to
/// hold back. The uncommitted-work note qualifies a verdict, so neither run
/// prints one: the first has no verdict because the branch does not resolve,
/// and the control has none because the scratch worktree cannot be built.
#[test]
fn a_branch_that_does_not_resolve_is_refused_before_any_scratch_worktree_exists() {
    let repo = independent_branches_repo();
    repo.write_file("scratch-notes.txt", "untracked work in progress\n");

    let (code, stdout, stderr) = grime_with_nowhere_to_put_a_scratch(&repo, "nonexistent-branch");

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
    // replaces ran a bare `git merge` and announced a typo as a conflict.
    assert!(
        !stdout.contains("conflicts") && !stderr.contains("conflicts"),
        "a typo'd branch name must never be reported as a conflict\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("note:"),
        "the tree is dirty, but a caveat qualifies a verdict and this run has \
         no verdict to qualify:\n{stderr}"
    );

    let (control_code, control_stdout, control_stderr) =
        grime_with_nowhere_to_put_a_scratch(&repo, "beta");

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
    assert!(
        !control_stderr.contains("note:"),
        "the caveat about uncommitted work qualifies the verdict, so a run \
         that dies before the verdict must not print it:\n{control_stderr}"
    );
}

/// Somewhere outside every repository there is no question to answer, and
/// saying so has to be distinguishable from answering it.
///
/// The exit code is the whole point. A tool that reported this as [`CONFLICTS`]
/// would be telling a script "the merge would conflict" about a directory it
/// never found a repository in.
#[test]
fn a_directory_that_is_not_a_repository_is_an_error_not_a_conflict() {
    let elsewhere = not_a_repository();

    let (code, stdout, stderr) = streams(&grime(elsewhere.path(), &["main"]));

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
        "there was no merge to conflict\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// Git refuses to merge two histories with no commit in common, and a refusal
/// leaves no unmerged path to measure - so a replay that measured nothing must
/// not therefore announce that nothing went wrong.
///
/// This is the shape the merge replay guards against, reached end to end: git
/// failed, the merge never started, and `git diff --diff-filter=U` is empty.
/// A binary that read "no unmerged paths" as "no conflicts" would tell a
/// developer that a merge git will not perform at all costs nothing.
///
/// The exit code is the load-bearing half. [`CLEAN`] would say the merge is
/// free and [`CONFLICTS`] would say it costs work a person can sit down and
/// resolve. Neither is true, and only [`ERROR`] says so.
///
/// This is the one assertion in the file that matches git's own words rather
/// than `grime`'s, so it is the one that [`PINNED_LOCALE`] exists for: git
/// wraps `refusing to merge unrelated histories` in gettext, and a git built
/// with the translations answers in the language of whoever runs the suite.
///
/// The tree is dirty on purpose. The scratch worktree gets built here and the
/// replay is what fails, which is the other half of the rule the poisoned
/// `TMPDIR` test above pins: a caveat qualifies a verdict, and this run has no
/// verdict to qualify.
#[test]
fn a_merge_that_fails_with_nothing_to_measure_is_neither_clean_nor_conflicts() {
    let repo = unrelated_histories_repo();
    repo.write_file("scratch-notes.txt", "untracked work in progress\n");

    let (code, stdout, stderr) = run(&repo, "main", "unrelated");

    assert_eq!(
        code,
        Some(ERROR),
        "a merge git would not perform must exit {ERROR}, not {CLEAN} for \
         having counted no conflicts\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("refusing to merge unrelated histories"),
        "git's own explanation is the only part that says what went wrong, so \
         it has to survive to the user:\n{stderr}"
    );
    assert!(
        !stdout.contains("clean")
            && !stdout.contains("conflicts")
            && !stderr.contains("clean")
            && !stderr.contains("conflicts"),
        "a run that could not measure anything must claim neither \
         verdict\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("note:"),
        "a merge that failed leaves no verdict, and a caveat with nothing to \
         qualify is a wrong sentence:\n{stderr}"
    );
}

/// `grime` simulates from HEAD, which is the only thing it *can* simulate from,
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
        dirty_stderr, "grime: note: 2 uncommitted files are not included; simulating from HEAD",
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

/// One of the three build-state words `CLAUDE.md` allows after the hash.
const BUILD_STATES: [&str; 3] = ["clean", "dirty", "unknown"];

/// How many characters of the commit hash the repository's version format
/// carries.
const HASH_LENGTH: usize = 7;

/// Assert `line` has every part of the version format this repository requires:
/// `grime 0.1.0 (abc1234, clean)`.
///
/// Checked as a *shape* rather than against a literal, because two of the four
/// parts move on their own: the hash changes with every commit and the build
/// state with every unstaged edit, so a golden string would fail on the next
/// commit for a reason that has nothing to do with the format.
///
/// All four parts are checked, because the mistake the repository rule exists
/// to prevent is dropping one of them - a tool wired up with clap's bare
/// `version` prints `grime 0.1.0` and nothing else, which tells a developer
/// holding a binary the release it claims to be but not which build it actually
/// is. A substring assertion would pass for exactly that binary.
fn assert_version_line(line: &str) {
    let (name, rest) = line
        .split_once(' ')
        .unwrap_or_else(|| panic!("the version line must name the tool, got {line:?}"));
    assert_eq!(
        name, "grime",
        "the version line must start with the tool's own name, got {line:?}"
    );

    let (release, build) = rest.split_once(' ').unwrap_or_else(|| {
        panic!("the version line must carry the build alongside the release, got {line:?}")
    });

    let components: Vec<&str> = release.split('.').collect();
    assert_eq!(
        components.len(),
        3,
        "the release must be a semver, got {release:?} in {line:?}"
    );
    assert!(
        components
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())),
        "every semver component must be a number, got {release:?} in {line:?}"
    );

    let inner = build
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_else(|| panic!("the build must be parenthesised, got {build:?} in {line:?}"));
    let (hash, state) = inner
        .split_once(", ")
        .unwrap_or_else(|| panic!("the build must be a hash and a state, got {inner:?}"));

    // `unknown` is the documented stand-in for a build made where git could not
    // be consulted, so it is a legal hash - but nothing else short of a real
    // abbreviated commit id is.
    assert!(
        hash == "unknown"
            || (hash.chars().count() == HASH_LENGTH && hash.chars().all(|c| c.is_ascii_hexdigit())),
        "the build must name the commit it came from as a {HASH_LENGTH}-character \
         hash, got {hash:?} in {line:?}"
    );
    assert!(
        BUILD_STATES.contains(&state),
        "the build state must be one of {BUILD_STATES:?}, got {state:?} in {line:?}"
    );
}

/// A binary that cannot say which build it is cannot be debugged from a bug
/// report, which is why every tool in this repository owes the same four-part
/// version line - name, release, commit, and whether the tree was dirty when it
/// was built.
///
/// Run outside every repository on purpose: the version is a fact baked in at
/// compile time, so asking for it must not depend on where the binary is
/// standing when you ask.
///
/// `-V` is asserted to be byte-identical rather than merely present, because
/// the documented rule names both spellings and two renderings of the same
/// fact is precisely the drift a shared format exists to prevent.
#[test]
fn version_names_the_tool_the_release_and_the_build_it_came_from() {
    let elsewhere = not_a_repository();

    let long = streams(&grime(elsewhere.path(), &["--version"]));
    let short = streams(&grime(elsewhere.path(), &["-V"]));

    assert_eq!(
        long.0,
        Some(0),
        "asking for the version is not a question about conflicts, so it \
         succeeds\nstdout:\n{}\nstderr:\n{}",
        long.1,
        long.2
    );
    assert_version_line(&long.1);
    assert_eq!(
        short, long,
        "-V and --version are two spellings of one switch, not two renderings"
    );
}

/// The sentence `--help` opens with, which is the doc comment on `Args`.
///
/// clap's derive takes the doc comment on the struct as the help text, unless
/// the attribute names `about`. A bare `about` takes `CARGO_PKG_DESCRIPTION`
/// instead, and the doc comment then says nothing to anybody who runs the tool.
/// Two sentences describe `grime`, one of them is dead, and the dead one is the
/// one sitting where a developer edits the help.
///
/// `grime` names no `about`, so the doc comment is the help. The manifest keeps
/// a sentence of its own for crates.io and `cargo search`, where the reader has
/// run nothing and `BRANCH` names nothing.
const HELP_SUMMARY: &str = "Report whether merging BRANCH into HEAD would conflict, and by how much";

/// The help a user reads has to be the help a developer edits.
///
/// The first line only, because the lines under it are clap's own layout, and
/// pinning those makes an assertion about the version of clap.
///
/// `-h` is asserted byte for byte against `--help` for the reason the version
/// test gives about `-V`: two spellings are one switch, and two renderings of
/// one fact are the drift a shared format exists to stop.
///
/// Run outside every repository, because the help is a fact baked in at compile
/// time and asking for it must not depend on where the binary stands.
#[test]
fn help_opens_with_the_summary_the_source_carries_rather_than_the_manifest_one() {
    assert_ne!(
        HELP_SUMMARY,
        env!("CARGO_PKG_DESCRIPTION"),
        "the two sentences have to differ, or this test cannot tell the help \
         apart from the manifest wording it exists to keep out of it"
    );

    let elsewhere = not_a_repository();

    let long = streams(&grime(elsewhere.path(), &["--help"]));
    let short = streams(&grime(elsewhere.path(), &["-h"]));

    assert_eq!(
        long.0,
        Some(0),
        "asking for the help is not a question about conflicts, so it \
         succeeds\nstdout:\n{}\nstderr:\n{}",
        long.1,
        long.2
    );
    assert_eq!(
        long.1.lines().next(),
        Some(HELP_SUMMARY),
        "the help must open with the sentence the source carries\nstdout:\n{}",
        long.1
    );
    assert_eq!(
        short, long,
        "-h and --help are two spellings of one switch, not two renderings"
    );
}

/// Assert `-q` printed nothing whatsoever and still answered with `expected`.
///
/// Both streams, because the answer being silent is only useful if the *whole*
/// run is: a caller redirecting stdout to `/dev/null` and getting a note or an
/// error message on the terminal anyway has not been given a quiet tool.
fn assert_silent(output: &Output, expected: i32, path: &str) {
    let (code, stdout, stderr) = streams(output);

    assert_eq!(
        code,
        Some(expected),
        "-q must not change the answer on the {path} path\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "-q printed to stdout on the {path} path:\n{stdout}"
    );
    assert!(
        output.stderr.is_empty(),
        "-q printed to stderr on the {path} path:\n{stderr}"
    );
}

/// Unlike a tool that prints a value, `grime` has no answer to pipe - the
/// answer *is* the exit code. So a scripted caller asking for quiet wants
/// silence, not a terser rendering, and gets it on the happy path first.
#[test]
fn quiet_prints_nothing_when_the_merge_is_clean() {
    let repo = independent_branches_repo();

    let output = run_raw(&repo, "alpha", &["-q", "beta"]);

    assert_silent(&output, CLEAN, "clean");
}

/// Deliberately measured over a *dirty* tree, because the verdict is not the
/// only thing `-q` has to swallow. A quiet mode that silences the report and
/// leaves the uncommitted-work note on stderr would pass a clean-tree test and
/// still spray output into a script's terminal.
#[test]
fn quiet_prints_nothing_when_the_merge_conflicts_over_a_dirty_tree() {
    let repo = equal_hunks_unequal_stops_repo();
    repo.write_file("scratch-notes.txt", "untracked work in progress\n");

    let output = run_raw(&repo, "one", &["-q", "two"]);

    assert_silent(&output, CONFLICTS, "conflicts");
}

/// The error message is the last thing that could leak, and the one most
/// easily forgotten, because it is printed from `main` rather than from the
/// code that does the work.
#[test]
fn quiet_prints_nothing_when_the_run_cannot_answer_at_all() {
    let repo = independent_branches_repo();

    let output = run_raw(&repo, "alpha", &["-q", "nonexistent-branch"]);

    assert_silent(&output, ERROR, "error");
}

/// `-q` and `--quiet` are two spellings of one switch, and the file already
/// holds that standard: the version test asserts `-V` and `--version` are
/// byte-identical rather than merely both present.
///
/// The three tests above all reach for the short spelling, so the long one runs
/// nowhere. Asserted byte for byte against the short run rather than against
/// silence alone, because "both spellings print nothing" is also true of a
/// binary that refuses the long one - clap prints its refusal to stderr, which
/// [`assert_silent`] catches, and the comparison catches an exit code that
/// moved.
///
/// The conflict path carries it, because that is the path with the most to
/// swallow: a verdict on stdout, and an uncommitted-work note on stderr.
#[test]
fn the_long_spelling_of_quiet_answers_exactly_as_the_short_one_does() {
    let repo = equal_hunks_unequal_stops_repo();
    repo.write_file("scratch-notes.txt", "untracked work in progress\n");

    let short = run_raw(&repo, "one", &["-q", "two"]);
    let long = run_raw(&repo, "one", &["--quiet", "two"]);

    assert_silent(&long, CONFLICTS, "conflicts");
    assert_eq!(
        (long.status.code(), &long.stdout, &long.stderr),
        (short.status.code(), &short.stdout, &short.stderr),
        "one switch has one behaviour, whichever way a caller spells it"
    );
}

/// `-q` reaches every write `grime` owns, and stops at the argument parser,
/// which answers before `grime` starts.
///
/// `Args::parse` runs ahead of the `Console` that carries the switch, so a
/// command line clap refuses is answered by clap. That is the right place for
/// it: silencing the refusal leaves a caller with a bare number and no way to
/// learn which argument is missing, and the missing `BRANCH` is the likely one
/// in the very script the README prints.
///
/// The code is [`ERROR`], which already means "I could not tell you". A command
/// line `grime` cannot read is one more way to get no answer.
///
/// Run outside every repository, which is what shows the parser answered first:
/// there is no repository here to answer from.
#[test]
fn quiet_leaves_the_usage_error_to_the_parser_that_answers_before_grime_starts() {
    let elsewhere = not_a_repository();

    let (code, stdout, stderr) = streams(&grime(elsewhere.path(), &["-q"]));

    assert_eq!(
        code,
        Some(ERROR),
        "a command line grime cannot read is a run that cannot answer, so \
         {ERROR}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("<BRANCH>"),
        "the refusal has to name the argument that is missing:\n{stderr}"
    );
    assert_eq!(
        stdout, "",
        "the refusal belongs on stderr, where it cannot contaminate a pipeline"
    );
}

/// `--version` answers about the binary rather than about a merge, so `-q`
/// leaves it alone.
///
/// Asserted byte for byte against the same run without `-q`, because the claim
/// is that the switch does not reach this path at all.
///
/// The exit code is `0` without a merge behind it, and that is the better
/// trade. Every script that asks a tool which build it is reads that number, so
/// moving the version off `0` to keep one table simple costs more than it buys.
#[test]
fn quiet_leaves_the_version_alone_because_it_answers_about_the_tool() {
    let elsewhere = not_a_repository();

    let quiet = streams(&grime(elsewhere.path(), &["-q", "--version"]));
    let loud = streams(&grime(elsewhere.path(), &["--version"]));

    assert_eq!(
        quiet.0,
        Some(0),
        "asking for the version is not a question about conflicts, so it \
         succeeds\nstdout:\n{}\nstderr:\n{}",
        quiet.1,
        quiet.2
    );
    assert_version_line(&quiet.1);
    assert_eq!(
        quiet, loud,
        "-q silences what grime says about a merge, and the version is not that"
    );
}

/// A file name and a branch name that are both outside ASCII, end to end
/// through the binary.
///
/// Three things a developer would notice go wrong here, and the verdict is
/// asserted as one block because it pins all three together: the header echoes
/// the branch name back, so `right-右` has to arrive unmangled; the breakdown
/// names the file, so `日本語.txt` must not come back as the C-quoted octal
/// escape git hands out by default; and the counts have to be the real ones,
/// because an escaped name resolves to no file on disk and a conflicted file
/// that cannot be read is silently floored at one hunk.
///
/// The panic check is separate from all of that and cheap to keep: byte-slicing
/// a path is the classic way this code goes wrong, and a binary that died on a
/// multi-byte name would otherwise be reported here as a plain assertion
/// mismatch rather than as the crash it is.
#[test]
fn a_conflict_in_a_multi_byte_named_file_on_a_multi_byte_branch_survives_intact() {
    let repo = multi_byte_names_repo();

    let (code, stdout, stderr) = run(&repo, "left-左", "right-右");

    assert!(
        !stderr.contains("panicked"),
        "a multi-byte name must not crash the binary:\n{stderr}"
    );
    assert_eq!(
        code,
        Some(CONFLICTS),
        "a conflicting merge must exit {CONFLICTS}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout,
        r"grime: conflicts - merging right-右 into HEAD
       3 hunks across 2 files

  readme.md     1 hunk
  日本語.txt    2 hunks",
        "stderr:\n{stderr}"
    );
}

/// Which terminal column `count` starts in on `line`.
///
/// Measured in display width, because that is the only one of the three lengths
/// Rust will hand you for a path that corresponds to what a reader sees. A
/// helper that used `len` or `chars().count()` would agree with a renderer that
/// made the same mistake and call a ragged column aligned.
///
/// Read from the *last* place `count` appears, because the count is the last
/// thing on the row. A name can spell the count itself - `11 hunks.txt` is a
/// legal file name - and the first occurrence is then the name rather than the
/// count.
///
/// # Panics
///
/// Panics when `count` is nowhere on `line`, because a count the renderer never
/// printed is a failure of the renderer rather than of the search.
fn count_column(line: &str, count: &str) -> usize {
    let start = line
        .rfind(count)
        .unwrap_or_else(|| panic!("no {count:?} on the row {line:?}"));
    let (prefix, _) = line.split_at(start);

    prefix.width()
}

/// The count is the last thing on a breakdown row, so [`count_column`] has to
/// read the last place the count appears and not the first.
///
/// A file name holds every byte but NUL, and `11 hunks.txt` is a name a
/// repository can carry. A helper that stops at the first occurrence answers 2
/// for the first row below - the column the *name* starts in - and every
/// alignment assertion built on it then passes or fails for a reason that has
/// nothing to do with alignment.
///
/// The second row is the ordinary one, and it is here as the control: a helper
/// that answered the last occurrence of something else would satisfy the first
/// assertion and break every real caller.
#[test]
fn the_count_column_is_read_from_the_last_place_the_count_appears() {
    assert_eq!(
        count_column("  11 hunks.txt    11 hunks", "11 hunks"),
        18,
        "a name that spells the count is not the count"
    );
    assert_eq!(
        count_column("  readme.md    1 hunk", "1 hunk"),
        15,
        "an ordinary row still reads as the column its count starts in"
    );
}

/// The breakdown lines, which are everything after the blank line that
/// separates them from the summary.
fn breakdown(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .skip_while(|line| !line.is_empty())
        .skip(1)
        .collect()
}

/// The counts have to start in the same terminal column whether or not a name
/// is multi-byte, which is a different claim from the block above rather than a
/// restatement of it.
///
/// The block is a golden string: someone who broke the padding and re-ran with
/// the output pasted back in would leave it green. This measures the property
/// instead, against a name git itself produced - `日本語.txt` is 13 bytes, 7
/// characters and 10 columns, so padding by either of the two measures Rust
/// offers for free lands the two counts in different columns, in opposite
/// directions.
#[test]
fn the_per_file_counts_line_up_by_display_width_when_a_name_is_multi_byte() {
    let repo = multi_byte_names_repo();

    let (_, stdout, stderr) = run(&repo, "left-左", "right-右");

    let lines = breakdown(&stdout);
    let ascii = lines
        .iter()
        .find(|line| line.contains("readme.md"))
        .unwrap_or_else(|| panic!("no breakdown line for readme.md in:\n{stdout}\n{stderr}"));
    let wide = lines
        .iter()
        .find(|line| line.contains("日本語.txt"))
        .unwrap_or_else(|| panic!("no breakdown line for 日本語.txt in:\n{stdout}\n{stderr}"));

    assert_eq!(
        count_column(ascii, "1 hunk"),
        count_column(wide, "2 hunks"),
        "the counts must start in the same terminal column:\n{stdout}"
    );
}

/// A bare repository is where the cheap pre-flight query turns out to be
/// *stricter* than the expensive replay it exists to spare you: `git worktree
/// add --detach HEAD` works against one, so the merge can be replayed and
/// measured exactly as usual, while `git status --porcelain` cannot run at all
/// - there is no working tree to take a status of.
///
/// The dirty-tree note is documented as a caveat that qualifies the verdict
/// without changing it, so a caveat that cannot be computed must cost the
/// caveat. Failing the run instead trades a right answer - these two branches
/// genuinely collide - for git's own complaint about a query the user never
/// asked for, which does not even say what is unsupported.
///
/// Asserted against [`EQUAL_HUNKS_VERDICT`], so the claim is the strong one:
/// the same fixture answers a bare repository with the byte-identical verdict
/// it gives through its working tree, since a replay never looks at the working
/// tree in the first place.
#[test]
fn a_repository_with_no_working_tree_is_answered_rather_than_refused() {
    let repo = equal_hunks_unequal_stops_repo();
    let bare = repo.bare_clone("one");

    let (code, stdout, stderr) = streams(&grime(bare.path(), &["two"]));

    assert_eq!(
        code,
        Some(CONFLICTS),
        "the replay can run in a bare repository, so the answer is {CONFLICTS} \
         rather than {ERROR}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, EQUAL_HUNKS_VERDICT, "stderr:\n{stderr}");
    assert_eq!(
        stderr, "",
        "a note that cannot be computed is a note nobody gets, not an error \
         message about git internals"
    );
}

/// A run depends on *two* revisions - the branch the user named and the HEAD
/// being merged into - and only one of them arrives as an argument, so the
/// other is the one a pre-flight is liable to forget.
///
/// An orphan branch is the shape where they disagree: `git checkout --orphan`
/// leaves HEAD naming a branch that has no commit on it, while every other
/// branch in the repository resolves as usual. Built inline rather than added
/// to `gitscratch::testing` because it is one git call on top of an existing
/// fixture, and *which* revision is unborn is the whole subject of this test
/// rather than a shape other suites would share.
///
/// The exit code was never wrong here, so this is about the message. Without
/// HEAD resolved up front, a repository with nothing committed pays for a
/// `TempDir` and a real `git worktree add` on the way to being told off, and is
/// then told off in git's words rather than `grime`'s - `fatal: invalid
/// reference: HEAD`, wrapped around an absolute path inside a temporary
/// directory that no longer exists by the time anybody reads it. A bad argument
/// has to arrive looking like a bad argument.
///
/// `TMPDIR` is pointed somewhere this test knows the name of, which is what
/// makes the leak assertable rather than merely unlikely: the scratch path is
/// otherwise a name only the child process ever learns.
#[test]
fn a_head_with_no_commit_on_it_is_refused_in_grimes_own_words_and_costs_no_worktree() {
    let repo = independent_branches_repo();
    repo.git(&["checkout", "-q", "--orphan", "unborn"]);

    // Under the fixture's own `TempDir`, so two concurrent copies of this test
    // cannot name the same path, and created rather than missing, so a run that
    // gets as far as building a scratch worktree succeeds at it and leaks the
    // path instead of failing earlier for an unrelated reason.
    let scratch_tmp = repo.path().join("scratch-tmp");
    std::fs::create_dir(&scratch_tmp).expect("create the scratch TMPDIR");

    let output = grime_command(repo.path(), &["beta"])
        .env("TMPDIR", &scratch_tmp)
        .output()
        .expect("failed to run grime");
    let (code, stdout, stderr) = streams(&output);

    assert_eq!(
        code,
        Some(ERROR),
        "a HEAD with nothing on it is a state grime cannot answer from, so \
         {ERROR}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("a merge starts from HEAD") && stderr.contains("no commit at HEAD"),
        "the message must say in grime's own words that HEAD is what has \
         nothing on it, since the user named a branch and never mentioned \
         HEAD:\n{stderr}"
    );
    assert!(
        !stderr.contains(&scratch_tmp.display().to_string()),
        "an internal temporary path is no part of a bad-argument message, and \
         the directory it names is gone before anyone reads it:\n{stderr}"
    );
    assert!(
        !stderr.contains("worktree add"),
        "resolving HEAD costs nothing, so it has to happen before a scratch \
         worktree is built rather than inside one:\n{stderr}"
    );
    assert!(
        !stderr.contains("note:"),
        "a repository with nothing to merge into is refused before there is \
         anything to qualify with a caveat:\n{stderr}"
    );
}

/// A branch name that starts with a dash is a branch name, and the pre-flight
/// has to read it as one.
///
/// `git rev-parse --root^{commit}` prints its argument back and exits 0,
/// because rev-parse passes an option it does not know through to rev-list
/// rather than refusing it. A pre-flight built on that exit code reads "the
/// revision names a commit" for a name that names nothing, and `grind -- --root`
/// answered `clean` for a branch that does not exist - the one answer this
/// family of tools exists never to give.
///
/// `--verify` is what refuses it, so the refusal arrives from the pre-flight in
/// `grime`'s own words and before any scratch worktree is built. `git merge`
/// would refuse `--root` too, having no such option, but only after a temporary
/// directory and a real `git worktree add` have been paid for, and then in
/// git's words about a command nobody typed. Both halves are asserted: the
/// exit code, and whose sentence says why.
///
/// The `--` in the argument list is how a user says "this is the positional".
/// Without it clap reads `--root` as an option of grime's own and refuses it
/// before any of the code under test runs, so the test would pass on clap's
/// refusal and say nothing about the pre-flight.
///
/// The control at the end asks the same binary about a branch that does exist.
/// Without it a `grime` that refused every branch would pass here.
#[test]
fn a_branch_name_that_starts_with_a_dash_is_refused_rather_than_reported_clean() {
    let repo = independent_branches_repo();

    let (code, stdout, stderr) = streams(&run_raw(&repo, "alpha", &["--", "--root"]));

    assert_eq!(
        code,
        Some(ERROR),
        "a branch name that names no commit must exit {ERROR}, never {CLEAN}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("could not resolve '--root'"),
        "the message must name the branch that did not resolve, in grime's own words:\n{stderr}"
    );
    assert!(
        !stdout.contains("clean") && !stderr.contains("clean"),
        "a branch that does not exist must never be reported as a clean merge\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let (control_code, control_stdout, control_stderr) = run(&repo, "alpha", "beta");

    assert_eq!(
        control_code,
        Some(CLEAN),
        "a branch that does exist must still be answered, or the refusal above proves only that \
         this binary refuses everything\nstdout:\n{control_stdout}\nstderr:\n{control_stderr}"
    );
}

/// `grime` takes its repository from the directory it was run in, and that
/// directory is hardly ever the repository root - so the run this test performs
/// is the ordinary one and every other test in this file is the special case.
///
/// What a subdirectory can quietly change is the *names*. Git will happily
/// report a path relative to the directory it was asked from, so a breakdown
/// naming `shared.txt` for a file that is really `sub/nested/shared.txt` reads
/// as perfectly good output while pointing at the wrong file - and a reader
/// scoped to the cwd instead drops the root `shared.txt` altogether and reports
/// less work than there is. Both are asserted here, and [`nested_conflict_repo`]
/// exists to make the two distinguishable: one conflicted file inside the
/// subdirectory the run starts in, one outside it.
///
/// The final assertion is the strongest form of the claim: the whole answer is
/// byte-identical to the one the same fixture gives from its root, which is the
/// only place a developer's mental model of the tool comes from.
#[test]
fn a_run_from_a_subdirectory_names_conflicts_from_the_repository_root() {
    let repo = nested_conflict_repo();
    repo.checkout("left");
    let nested = repo.path().join("sub").join("nested");

    let (code, stdout, stderr) = streams(&grime(&nested, &["right"]));

    assert_eq!(
        code,
        Some(CONFLICTS),
        "a subdirectory of a repository is inside one, so the question is \
         answerable from it\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        breakdown(&stdout),
        vec![
            "  shared.txt               1 hunk",
            "  sub/nested/shared.txt    1 hunk",
        ],
        "the breakdown has to name both files by their whole path from the \
         repository root, including the one that conflicted outside the \
         directory the run started in\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("../"),
        "a path climbing out of the directory the run started in is a name \
         relative to the cwd, not to the repository:\n{stdout}"
    );
    assert!(
        !stdout.contains(&repo.path().display().to_string()),
        "an absolute path inside a temporary fixture is no name for a \
         conflicted file:\n{stdout}"
    );

    let (root_code, root_stdout, root_stderr) = streams(&grime(repo.path(), &["right"]));

    assert_eq!(
        (code, stdout),
        (root_code, root_stdout),
        "the same question about the same repository has one answer, whichever \
         of its directories it was asked from\nstderr:\n{stderr}\nroot stderr:\n{root_stderr}"
    );
}

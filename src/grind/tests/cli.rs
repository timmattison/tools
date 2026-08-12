//! End-to-end coverage of the binary itself, so the wiring between argument
//! parsing, the replay and the exit code is exercised the way a user runs it.
//!
//! The exit code is the load-bearing half of every assertion here. `grind`'s
//! whole reason to exist is that a scripted caller can tell "conflicts" from
//! "something went wrong", so a test that only checked the words on stdout
//! would pass for a binary that answers every question with the same number.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use gitscratch::testing::{
    contested_region_repo, equal_hunks_unequal_stops_repo, independent_branches_repo,
    multi_byte_names_repo, nested_conflict_repo, not_a_repository, TestRepo,
};
use gitscratch::NoInheritedRepository;
use unicode_width::UnicodeWidthStr;

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

/// The whole verdict for replaying `one` onto `two` in
/// [`equal_hunks_unequal_stops_repo`].
///
/// A constant because three tests assert it: the verdict itself, and the two
/// leaked-environment tests, which are only worth anything if the answer they
/// check is the *same* answer the undisturbed run produces. Two copies of it
/// could drift into checking two different claims.
const EQUAL_HUNKS_VERDICT: &str = r"grind: conflicts - replaying HEAD onto two
       2 hunks across 2 files, 1 stop

  x.txt    1 hunk
  y.txt    1 hunk";

/// Run `grind` in `repo`, with the ambient git environment taken back off.
///
/// The scrub is belt to the binary's braces. `grind` reaches git only through
/// `gitscratch`, which scrubs at the single place it spawns one, so a leak
/// cannot reach the tool - but a test suite that let one through would be
/// asserting against a run nobody could reproduce, and the two tests at the
/// bottom of this file set these variables *deliberately*. Leaving the ambient
/// environment in play everywhere else is what keeps those two the only place
/// the answer depends on it.
fn grind(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grind"))
        .args(args)
        .current_dir(repo)
        .without_inherited_repository()
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

/// Stand on `head` and ask about `onto`, the way a developer would.
fn run(repo: &TestRepo, head: &str, onto: &str) -> (Option<i32>, String, String) {
    streams(&run_raw(repo, head, &[onto]))
}

/// The raw output, for the assertions that care about the difference between
/// "printed nothing" and "printed only whitespace" - which [`streams`] trims
/// away and `-q` is judged on.
fn run_raw(repo: &TestRepo, head: &str, args: &[&str]) -> Output {
    repo.checkout(head);

    grind(repo.path(), args)
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
    assert_eq!(stdout, EQUAL_HUNKS_VERDICT, "stderr:\n{stderr}");
}

/// A file name and a branch name that are both outside ASCII, end to end
/// through the binary.
///
/// Three things a developer would notice go wrong here, and the verdict is
/// asserted as one block because it pins all three together: the header echoes
/// the branch name back, so `left-左` has to arrive unmangled; the breakdown
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

    let (code, stdout, stderr) = run(&repo, "right-右", "left-左");

    assert!(
        !stderr.contains("panicked"),
        "a multi-byte name must not crash the binary:\n{stderr}"
    );
    assert_eq!(
        code,
        Some(CONFLICTS),
        "a conflicting rebase must exit {CONFLICTS}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout,
        r"grind: conflicts - replaying HEAD onto left-左
       3 hunks across 2 files, 1 stop

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
fn count_column(line: &str, count: &str) -> usize {
    line.split(count)
        .next()
        .expect("splitting always yields at least one piece")
        .width()
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

    let (_, stdout, stderr) = run(&repo, "right-右", "left-左");

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

/// `grind` takes its repository from the directory it was run in, and that
/// directory is hardly ever the repository root - so the run this test performs
/// is the ordinary one and every other test in this file is the special case.
///
/// What a subdirectory can quietly change is the *names*. Git will happily report
/// a path relative to the directory it was asked from, so a breakdown naming
/// `shared.txt` for a file that is really `sub/nested/shared.txt` reads as
/// perfectly good output while pointing at the wrong file - and a reader scoped
/// to the cwd instead drops the root `shared.txt` altogether and reports less
/// work than there is. Both are asserted here, and
/// [`nested_conflict_repo`] exists to make the two distinguishable: one
/// conflicted file inside the subdirectory the run starts in, one outside it.
///
/// The final assertion is the strongest form of the claim: the whole answer is
/// byte-identical to the one the same fixture gives from its root, which is the
/// only place a developer's mental model of the tool comes from.
///
/// Verified by mutation: replacing the conflicted-path reader's names with their
/// last component - a path that is no longer repository-root-relative - fails
/// this test and leaves the rest of the suite green, every other fixture's
/// conflicts living at the repository root where the two spellings coincide.
#[test]
fn a_run_from_a_subdirectory_names_conflicts_from_the_repository_root() {
    let repo = nested_conflict_repo();
    repo.checkout("left");
    let nested = repo.path().join("sub").join("nested");

    let (code, stdout, stderr) = streams(&grind(&nested, &["right"]));

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

    let (root_code, root_stdout, root_stderr) = streams(&grind(repo.path(), &["right"]));

    assert_eq!(
        (code, stdout),
        (root_code, root_stdout),
        "the same question about the same repository has one answer, whichever \
         of its directories it was asked from\nstderr:\n{stderr}\nroot stderr:\n{root_stderr}"
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
        .without_inherited_repository()
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

/// A run depends on *two* revisions - the branch the user named and the HEAD
/// being replayed - and only one of them arrives as an argument, so the other is
/// the one a pre-flight is liable to forget.
///
/// An orphan branch is the shape where they disagree: `git checkout --orphan`
/// leaves HEAD naming a branch that has no commit on it, while every other
/// branch in the repository resolves as usual. Built inline rather than added to
/// `gitscratch::testing` because it is one git call on top of an existing
/// fixture, and *which* revision is unborn is the whole subject of this test
/// rather than a shape other suites would share.
///
/// The exit code was never wrong here, so this is about the message. Before HEAD
/// was resolved up front, a repository with nothing committed paid for a
/// `TempDir` and a real `git worktree add` on the way to being told off, and was
/// then told off in git's words rather than grind's - `fatal: invalid reference:
/// HEAD`, wrapped around an absolute path inside a temporary directory that no
/// longer exists by the time anybody reads it. A bad argument has to arrive
/// looking like a bad argument, which is `repo.rs`'s own stated reason for the
/// pre-flight existing at all.
///
/// `TMPDIR` is pointed somewhere this test knows the name of, which is what makes
/// the leak assertable rather than merely unlikely: the scratch path is otherwise
/// a name only the child process ever learns.
#[test]
fn a_head_with_no_commit_on_it_is_refused_in_grinds_own_words_and_costs_no_worktree() {
    let repo = independent_branches_repo();
    repo.git(&["checkout", "-q", "--orphan", "unborn"]);

    // Under the fixture's own `TempDir`, so two concurrent copies of this test
    // cannot name the same path, and created rather than missing, so a run that
    // gets as far as building a scratch worktree succeeds at it and leaks the
    // path instead of failing earlier for an unrelated reason.
    let scratch_tmp = repo.path().join("scratch-tmp");
    std::fs::create_dir(&scratch_tmp).expect("create the scratch TMPDIR");

    let output = Command::new(env!("CARGO_BIN_EXE_grind"))
        .arg("beta")
        .current_dir(repo.path())
        .without_inherited_repository()
        .env("TMPDIR", &scratch_tmp)
        .output()
        .expect("failed to run grind");
    let (code, stdout, stderr) = streams(&output);

    assert_eq!(
        code,
        Some(ERROR),
        "a HEAD with nothing on it is a state grind cannot answer from, so \
         {ERROR}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("a replay starts from HEAD") && stderr.contains("no commit at HEAD"),
        "the message must say in grind's own words that HEAD is what has \
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
        "a repository with nothing to replay from is refused before there is \
         anything to qualify with a caveat:\n{stderr}"
    );
}

/// One of the three build-state words `CLAUDE.md` allows after the hash.
const BUILD_STATES: [&str; 3] = ["clean", "dirty", "unknown"];

/// How many characters of the commit hash the repository's version format
/// carries.
const HASH_LENGTH: usize = 7;

/// Assert `line` has every part of the version format this repository requires:
/// `grind 0.1.0 (abc1234, clean)`.
///
/// Checked as a *shape* rather than against a literal, because two of the four
/// parts move on their own: the hash changes with every commit and the build
/// state with every unstaged edit, so a golden string would fail on the next
/// commit for a reason that has nothing to do with the format.
///
/// All four parts are checked, because the mistake the repository rule exists
/// to prevent is dropping one of them — a tool wired up with clap's bare
/// `version` prints `grind 0.1.0` and nothing else, which tells a developer
/// holding a binary the release it claims to be but not which build it actually
/// is. A substring assertion would pass for exactly that binary.
fn assert_version_line(line: &str) {
    let (name, rest) = line
        .split_once(' ')
        .unwrap_or_else(|| panic!("the version line must name the tool, got {line:?}"));
    assert_eq!(
        name, "grind",
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

    let long = streams(&grind(elsewhere.path(), &["--version"]));
    let short = streams(&grind(elsewhere.path(), &["-V"]));

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

/// A bare repository is where the cheap pre-flight query turns out to be
/// *stricter* than the expensive replay it exists to spare you: `git worktree
/// add --detach HEAD` works against one, so the rebase can be replayed and
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
/// the same fixture answers a bare repository with the byte-identical verdict it
/// gives through its working tree, since a replay never looks at the working
/// tree in the first place.
#[test]
fn a_repository_with_no_working_tree_is_answered_rather_than_refused() {
    let repo = equal_hunks_unequal_stops_repo();
    let bare = repo.bare_clone("one");

    let (code, stdout, stderr) = streams(&grind(bare.path(), &["two"]));

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

/// Unlike a tool that prints a value, `grind` has no answer to pipe - the
/// answer *is* the exit code. So a scripted caller asking for quiet wants
/// silence, not a terser rendering, and gets it on the happy path first.
#[test]
fn quiet_prints_nothing_when_the_replay_is_clean() {
    let repo = independent_branches_repo();

    let output = run_raw(&repo, "alpha", &["-q", "beta"]);

    assert_silent(&output, CLEAN, "clean");
}

/// Deliberately measured over a *dirty* tree, because the verdict is not the
/// only thing `-q` has to swallow. A quiet mode that silences the report and
/// leaves the uncommitted-work note on stderr would pass a clean-tree test and
/// still spray output into a script's terminal.
#[test]
fn quiet_prints_nothing_when_the_replay_conflicts_over_a_dirty_tree() {
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

/// Which of `grind`'s streams is handed a pipe nobody is reading.
#[derive(Debug, Clone, Copy)]
enum Unread {
    Stdout,
    Stderr,
}

/// Run `grind` with one stream pointed at a pipe whose read end is already
/// closed, and hand back the exit code plus whatever the *other* stream said.
///
/// Ordinary rather than contrived: `grind main | head -1`, a pipeline whose
/// reader exits first, a terminal closed mid-run. Every one of them leaves
/// `grind` writing into a pipe with nobody on the far end.
///
/// Deterministic by construction rather than by timing. The read end is dropped
/// *before* the child is spawned, so there is no window in which a write could
/// succeed and nothing to race: the first byte `grind` puts on that stream fails
/// with `EPIPE`, whichever of its three writes gets there first.
///
/// Closing the descriptor outright would look simpler and would test something
/// else entirely. With fd 1 closed it becomes the lowest free descriptor, so the
/// first file the run opens - the scratch `TempDir`, a git pipe - lands on it and
/// the verdict is silently written *into that file* rather than failing.
///
/// SIGPIPE is why the failure surfaces as a panic rather than as a signal: Rust's
/// runtime ignores it at startup, so the write returns `EPIPE` instead of killing
/// the process, and `println!` turns that into a panic and exit `PANICKED`.
fn grind_into_an_unread_pipe(repo: &Path, args: &[&str], unread: Unread) -> (Option<i32>, String) {
    let (reader, writer) = std::io::pipe().expect("create a pipe");
    // Before the child exists, so the stream it is handed has never had a
    // reader and cannot acquire one.
    drop(reader);

    let mut command = Command::new(env!("CARGO_BIN_EXE_grind"));
    command
        .args(args)
        .current_dir(repo)
        .without_inherited_repository();
    match unread {
        Unread::Stdout => {
            command.stdout(Stdio::from(writer)).stderr(Stdio::piped());
        }
        Unread::Stderr => {
            command.stderr(Stdio::from(writer)).stdout(Stdio::piped());
        }
    }

    let output = command.output().expect("failed to run grind");
    let surviving = match unread {
        Unread::Stdout => &output.stderr,
        Unread::Stderr => &output.stdout,
    };

    (
        output.status.code(),
        String::from_utf8_lossy(surviving).trim_end().to_string(),
    )
}

/// The exit code a Rust process leaves behind when it unwound out of a panic -
/// the fourth, undocumented code `grind` must never produce.
const PANICKED: i32 = 101;

/// The verdict is the one thing `grind` puts on stdout, and stdout is the stream
/// a pipeline takes away first.
///
/// The exit code *is* the answer here, so a reader that closed early must cost
/// the verdict and nothing else. Losing the answer to a panic would publish a
/// fourth exit code nothing documents, on the one path a script is most likely
/// to be reading.
#[test]
fn a_verdict_nobody_is_reading_costs_the_words_and_not_the_answer() {
    let repo = equal_hunks_unequal_stops_repo();
    repo.checkout("one");

    let (code, stderr) = grind_into_an_unread_pipe(repo.path(), &["two"], Unread::Stdout);

    assert_ne!(
        code,
        Some(PANICKED),
        "a broken pipe must not turn the answer into a panic\nstderr:\n{stderr}"
    );
    assert_eq!(
        code,
        Some(CONFLICTS),
        "the replay conflicted, so the answer is {CONFLICTS} whether or not \
         anyone read it\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "a broken pipe is not a bug in grind and must not be reported as \
         one:\n{stderr}"
    );
}

/// Both of stderr's writers, on the two paths that reach them: the note, which
/// is printed before the verdict, and the failure, which is printed from `main`
/// after `run` has given up.
///
/// Neither is the answer, so neither may be able to take it away. `2>/dev/null`
/// on a pipeline whose reader has gone, a hook whose log is closed - the caller
/// has said they do not want the caveat, not that they do not want the exit
/// code.
#[test]
fn a_note_or_a_failure_nobody_is_reading_costs_the_words_and_not_the_answer() {
    let repo = independent_branches_repo();
    repo.checkout("alpha");
    // Untracked rather than modified, so the note has something to report
    // without the checkout above having anything to refuse.
    repo.write_file("scratch-notes.txt", "untracked work in progress\n");

    let (clean_code, stdout) = grind_into_an_unread_pipe(repo.path(), &["beta"], Unread::Stderr);

    assert_eq!(
        clean_code,
        Some(CLEAN),
        "a note nobody read must not move the verdict off {CLEAN}\nstdout:\n{stdout}"
    );
    assert_eq!(
        stdout, "grind: clean - replaying HEAD onto beta hit no conflicts",
        "the run has to carry on past the note it could not print"
    );

    let (error_code, error_stdout) =
        grind_into_an_unread_pipe(repo.path(), &["nonexistent-branch"], Unread::Stderr);

    assert_eq!(
        error_code,
        Some(ERROR),
        "a failure it could not print is still a failure, and still \
         {ERROR}\nstdout:\n{error_stdout}"
    );
}

/// Read a file that must come back byte-identical after `grind` has run.
///
/// # Panics
///
/// Panics if the file cannot be read.
fn snapshot(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// `grind` answering a question about the repository it is standing in, while
/// its environment insists the repository is somewhere else entirely.
///
/// Not a contrived situation: git exports `GIT_DIR`, `GIT_WORK_TREE`,
/// `GIT_INDEX_FILE` and `GIT_PREFIX` into every hook it runs, and every child
/// of that hook inherits them. A `pre-push` hook that asks `grind` whether the
/// branch about to be pushed would rebase cleanly is exactly this shape - and
/// so is `git bisect run`, `rebase --exec`, and a `cargo test` run from
/// `.husky/pre-commit`.
///
/// The environment is set on the *child* process. `std::env::set_var` is
/// process-global and Rust runs the tests in this binary as threads of one
/// process, so poisoning it there would sabotage every other test in the file -
/// and a whole process whose environment names another repository is the leak
/// verbatim anyway.
///
/// The two branches are named so they cannot exist in the repository the
/// environment points at, which is what makes the failure unambiguous: a
/// `grind` that took the environment's word for where it was would be looking
/// for `two` in a repository that has only `alpha`, `beta` and `main`.
#[test]
fn a_leaked_repository_location_does_not_redirect_the_replay() {
    let repo = equal_hunks_unequal_stops_repo();
    let hooks_repo = independent_branches_repo();
    repo.checkout("one");

    let git_dir = hooks_repo.path().join(".git");
    let config = snapshot(&git_dir.join("config"));

    let output = Command::new(env!("CARGO_BIN_EXE_grind"))
        .arg("two")
        .current_dir(repo.path())
        .env("GIT_DIR", &git_dir)
        .env("GIT_WORK_TREE", hooks_repo.path())
        .env("GIT_PREFIX", "")
        .output()
        .expect("failed to run grind");
    let (code, stdout, stderr) = streams(&output);

    assert_eq!(
        code,
        Some(CONFLICTS),
        "the answer is about the directory grind is standing in, not the one \
         the environment names\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, EQUAL_HUNKS_VERDICT, "stderr:\n{stderr}");
    assert_eq!(
        snapshot(&git_dir.join("config")),
        config,
        "grind wrote into the config of the repository the environment named"
    );
    assert!(
        !git_dir.join("worktrees").exists(),
        "grind built its scratch worktree in the repository the environment named"
    );
}

/// The half of the leak that a hook produces on its own: `.husky/pre-commit`
/// runs `cargo test`, and `GIT_INDEX_FILE` is what that inherits. Repository
/// discovery still finds the right repository, so the run looks fine - but
/// every index read and write goes to the hook's repository instead.
///
/// Two things give it away. `git status` against a foreign index sees the whole
/// working tree as uncommitted, so the note appears over a tree with nothing
/// uncommitted in it; and the index it read is left rewritten, carrying entries
/// for files that do not exist in that repository at all. The index is
/// snapshotted rather than re-read through git, because once a phantom entry
/// points at an object the victim does not have, git's own answers about it
/// stop being trustworthy.
#[test]
fn a_leaked_index_file_is_neither_read_from_nor_written_to() {
    let repo = equal_hunks_unequal_stops_repo();
    let hooks_repo = independent_branches_repo();
    repo.checkout("one");

    let index = hooks_repo.path().join(".git").join("index");
    let before = snapshot(&index);

    let output = Command::new(env!("CARGO_BIN_EXE_grind"))
        .arg("two")
        .current_dir(repo.path())
        .env("GIT_INDEX_FILE", &index)
        .output()
        .expect("failed to run grind");
    let (code, stdout, stderr) = streams(&output);

    assert_eq!(
        stderr, "",
        "the tree has nothing uncommitted in it; a note here means the status \
         was taken against the index the environment named\nstdout:\n{stdout}"
    );
    assert_eq!(
        code,
        Some(CONFLICTS),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, EQUAL_HUNKS_VERDICT, "stderr:\n{stderr}");
    assert_eq!(
        snapshot(&index),
        before,
        "grind wrote into the index the environment named"
    );
}

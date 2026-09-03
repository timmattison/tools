//! Black-box tests for the `wn` binary, driving the real command line against
//! a `gh` this file writes.
//!
//! The tool asks GitHub through `gh`, so the seam between the two is a program
//! on `PATH`. Each test writes its own `gh` into its own temporary directory
//! and puts that directory first, which is what lets the whole path — the
//! chain, the query, the answer, the rows, the exit status — run without a
//! network and without a credential.
//!
//! The child gets an environment this file builds from nothing. A test that
//! passed its own environment down would hand the tool whatever the terminal
//! that started `cargo test` happened to export, and `COLUMNS` and `NO_COLOR`
//! both change what the tool prints.
//!
//! That environment names [`NO_CLIPBOARD_ENV`] in every test but one. A child
//! of a test reads `/dev/null` on standard input, which is not a terminal and
//! holds no text, so a run with no chain argument walks on to the clipboard.
//! The clipboard is one shared resource of the whole machine: a test that reads
//! it races the person at the keyboard. The variable turns that last step off
//! for the other children of this file, which is a mechanism rather than a rule
//! each test must remember.
//!
//! The one exception is `the_run_with_no_argument_reads_the_clipboard`, which
//! leaves the variable out because it is the test that holds the clipboard in
//! the run at all. It reads the clipboard, it writes nothing to it, and it
//! asserts only which path the run took, so it takes nothing away from the
//! person at the keyboard.
//!
//! A plan of parallel work is many lines, and `Command::output` hands the
//! child a standard input that holds nothing. So the tests that walk a plan go
//! through `run_with_stdin`, which opens a pipe and builds the same
//! environment every other child of this file gets.
//!
//! Each test builds its own temporary directory, so concurrent test runs stay
//! isolated (see the parallel-safety note in the project guidelines).

#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error: on the temporary directory and the fixture files the test just created, on spawning the freshly built binary (a spawn failure is a broken harness, not behavior under test), and on reading back a file the fake gh wrote. The error paths of the tool itself are never unwrapped — they are asserted through the exit status and the text on standard error"
)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use unicode_width::UnicodeWidthStr;

/// The repository the fake `gh` answers for.
const REPO: &str = "timmattison/tools";

/// The variable that names the command the answer prints.
const START_COMMAND_ENV: &str = "WN_START_COMMAND";

/// The variable that turns the clipboard fallback off.
///
/// Every child of this file gets it, because no test of a command line may
/// touch the clipboard of the machine that runs it.
const NO_CLIPBOARD_ENV: &str = "WN_NO_CLIPBOARD";

/// The value [`NO_CLIPBOARD_ENV`] carries. Any value with a character in it
/// turns the fallback off; this one says why it is there.
const NO_CLIPBOARD: &str = "1";

/// The error of a run that no input gave a chain, and that had no clipboard to
/// fall back on.
///
/// It is the one message a run reaches only when the clipboard was not one of
/// the inputs, so its absence is what holds the clipboard in the run.
const NO_CHAIN: &str = "no chain given";

/// A chain of three issues: one closed, two open.
const THREE_ISSUES: &str = r#"{"data":{"repository":{
"i277":{"__typename":"Issue","number":277,"title":"First thing","state":"CLOSED","stateReason":"COMPLETED"},
"i278":{"__typename":"Issue","number":278,"title":"Second thing","state":"OPEN","stateReason":null},
"i279":{"__typename":"Issue","number":279,"title":"Third thing","state":"OPEN","stateReason":null}
}}}"#;

/// The chain the start-command tests walk. Two of the three issues, so the
/// answer names #278 whichever command it prints.
const ONE_OPEN_CHAIN: &str = "#277 → #278";

/// A plan of three streams, as a record for each stream.
///
/// The notes carry real prose, because real prose is the trap the plan reader
/// exists for: they name 265, 5113, and 1566-1650, and none of those is an
/// issue of the repository. A run that read them would print a row for each
/// and would exit `1`.
const PLAN: &str = "\
Stream: S1 gitscratch
Order: #344 → #330
Zone: src/gitscratch
Notes: The two hunks sit 265 lines apart in a 5113-line file, so the rebase is cheap.

Stream: S2 ic
Order: #350 → #187
Zone: src/ic
Notes: Both land inside display_image (main.rs:1566-1650).

Stream: S3 wn
Order: #411
Zone: src/wn
Notes: One issue, no neighbors.
";

/// The same three streams, as one Markdown table.
const PLAN_TABLE: &str = "\
| Stream | Order | Zone | Notes |
| --- | --- | --- | --- |
| S1 gitscratch | #344 → #330 | src/gitscratch | The two hunks sit 265 lines apart in a 5113-line file. |
| S2 ic | #350 → #187 | src/ic | Both land inside display_image (main.rs:1566-1650). |
| S3 wn | #411 | src/wn | One issue, no neighbors. |
";

/// What GitHub says about every number of [`PLAN`]: one closed issue and four
/// open ones.
const PLAN_ISSUES: &str = r#"{"data":{"repository":{
"i344":{"__typename":"Issue","number":344,"title":"First thing","state":"CLOSED","stateReason":"COMPLETED"},
"i330":{"__typename":"Issue","number":330,"title":"Second thing","state":"OPEN","stateReason":null},
"i350":{"__typename":"Issue","number":350,"title":"Third thing","state":"OPEN","stateReason":null},
"i187":{"__typename":"Issue","number":187,"title":"Fourth thing","state":"OPEN","stateReason":null},
"i411":{"__typename":"Issue","number":411,"title":"Fifth thing","state":"OPEN","stateReason":null}
}}}"#;

/// The answer [`PLAN`] earns: one block for each stream, and one summary that
/// carries all three.
const PLAN_ANSWER: &str = concat!(
    "S1 gitscratch\n",
    "  ✓ #344  First thing\n",
    "  → #330  Second thing\n",
    "\n",
    "S2 ic\n",
    "  → #350  Third thing\n",
    "  · #187  Fourth thing\n",
    "\n",
    "S3 wn\n",
    "  → #411  Fifth thing\n",
    "\n",
    "Take one from each stream:\n",
    "  S1 gitscratch  → #330  si 330\n",
    "  S2 ic          → #350  si 350\n",
    "  S3 wn          → #411  si 411\n",
);

/// The report of the `plan-parallel-work` skill, as it arrives on the
/// clipboard.
///
/// The same file the reader of the plan reads in its own tests, so the paste
/// this test drives the binary with is the paste that reader was written for.
const BOX_TABLE: &str = include_str!("../fixtures/plan-parallel-work.txt");

/// What GitHub says about every number of [`BOX_TABLE`].
///
/// Three of the ten are done, so each of the four streams has one issue to
/// start and none of them is the first step of its stream in every case.
const BOX_ISSUES: &str = r#"{"data":{"repository":{
"i15":{"__typename":"PullRequest","number":15,"title":"The visualizer branch","state":"OPEN"},
"i4":{"__typename":"Issue","number":4,"title":"The visualizers","state":"OPEN","stateReason":null},
"i7":{"__typename":"Issue","number":7,"title":"Keep or delete","state":"OPEN","stateReason":null},
"i11":{"__typename":"Issue","number":11,"title":"The oscillator","state":"CLOSED","stateReason":"COMPLETED"},
"i5":{"__typename":"Issue","number":5,"title":"The Scaled mapping","state":"OPEN","stateReason":null},
"i13":{"__typename":"Issue","number":13,"title":"The sort tone","state":"OPEN","stateReason":null},
"i9":{"__typename":"Issue","number":9,"title":"The MIDI array","state":"CLOSED","stateReason":"COMPLETED"},
"i10":{"__typename":"Issue","number":10,"title":"The MIDI flags","state":"CLOSED","stateReason":"COMPLETED"},
"i12":{"__typename":"Issue","number":12,"title":"Listen to it","state":"OPEN","stateReason":null},
"i6":{"__typename":"Issue","number":6,"title":"The manifest","state":"OPEN","stateReason":null}
}}}"#;

/// The answer [`BOX_TABLE`] earns: one block for each of the four streams,
/// and one summary that names an issue to start in each of them.
const BOX_ANSWER: &str = concat!(
    "A — visualizers\n",
    "  → #15 (#4)  The visualizer branch\n",
    "  · #7        Keep or delete\n",
    "\n",
    "B — audio engine\n",
    "  ✓ #11  The oscillator\n",
    "  → #5   The Scaled mapping\n",
    "  · #13  The sort tone\n",
    "\n",
    "C — MIDI array\n",
    "  ✓ #9   The MIDI array\n",
    "  ✓ #10  The MIDI flags\n",
    "  → #12  Listen to it\n",
    "\n",
    "D — manifest\n",
    "  → #6  The manifest\n",
    "\n",
    "Take one from each stream:\n",
    "  A — visualizers   → #15  si 15\n",
    "  B — audio engine  → #5   si 5\n",
    "  C — MIDI array    → #12  si 12\n",
    "  D — manifest      → #6   si 6\n",
);

/// A plan drawn as a picture: two streams that join.
///
/// The paste of issue #418. A picture says the one thing no chain and no table
/// of a plan says: two streams that run at the same time, and the step they
/// both reach.
const PICTURE: &str = "\
#242 ──→ #247 ──┐
                ├──→ #249  (gallery)
#246 ──→ #248 ──┘
";

/// What GitHub says about every number of [`PICTURE`] when each of them is
/// open.
const PICTURE_ISSUES: &str = r#"{"data":{"repository":{
"i242":{"__typename":"Issue","number":242,"title":"Read the picture","state":"OPEN","stateReason":null},
"i247":{"__typename":"Issue","number":247,"title":"Answer the picture","state":"OPEN","stateReason":null},
"i246":{"__typename":"Issue","number":246,"title":"Read the table","state":"OPEN","stateReason":null},
"i248":{"__typename":"Issue","number":248,"title":"Answer the table","state":"OPEN","stateReason":null},
"i249":{"__typename":"Issue","number":249,"title":"Paint the gallery","state":"OPEN","stateReason":null}
}}}"#;

/// The answer [`PICTURE`] earns while every issue of it is open.
///
/// One row for each step, the work each blocked step waits for, and one start
/// line for each stream that is ready.
const PICTURE_ANSWER: &str = concat!(
    "→ #242  Read the picture\n",
    "· #247  Answer the picture  waits for #242\n",
    "→ #246  Read the table\n",
    "· #248  Answer the table    waits for #246\n",
    "· #249  Paint the gallery   waits for #247, #248\n",
    "\n",
    "Start #242 next with 'si 242'\n",
    "Start #246 next with 'si 246'\n",
);

/// A fake `gh` in a temporary directory of its own.
struct FakeGh {
    dir: tempfile::TempDir,
}

impl FakeGh {
    /// Write a `gh` that answers `repo view` with [`REPO`] and answers every
    /// GraphQL query with `body`, and that records the arguments it was given.
    fn new(body: &str) -> Self {
        Self::with_status(body, 0)
    }

    /// The same, with an exit status of its own. `gh` exits non-zero for a
    /// query whose answer carries a top-level `errors` list, which is what a
    /// number the repository does not have produces.
    fn with_status(body: &str, status: i32) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let args_file = dir.path().join("args");
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = "repo" ]; then
    printf '%s\n' '{REPO}'
    exit 0
fi
for arg in "$@"; do
    printf '%s\n' "$arg" >> '{args}'
done
cat <<'WN_FAKE_GH_BODY'
{body}
WN_FAKE_GH_BODY
exit {status}
"#,
            args = args_file.display(),
        );
        let gh = dir.path().join("gh");
        std::fs::write(&gh, script).unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The arguments of the last GraphQL call, one to a line.
    fn recorded_args(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("args")).unwrap()
    }
}

/// Run `wn` with an environment built from nothing, and with no start command
/// named.
///
/// `columns` and `color` are the two inputs that change what the tool prints,
/// so each test states both.
fn run(gh: &FakeGh, args: &[&str], columns: &str, color: bool) -> Output {
    run_with_start(gh, args, columns, color, None)
}

/// The same, with [`START_COMMAND_ENV`] set to `start`.
///
/// `None` leaves the variable out of the environment, which is the state of a
/// machine that never set it.
fn run_with_start(
    gh: &FakeGh,
    args: &[&str],
    columns: &str,
    color: bool,
    start: Option<&str>,
) -> Output {
    wn(gh, args, columns, color, start).output().unwrap()
}

/// Run `wn` with `text` on standard input.
///
/// `Command::output` hands the child a standard input that holds nothing,
/// which is what every other test of this file wants. A plan is many lines,
/// and a plan reaches the tool through a pipe as readily as through the
/// command line, so this helper opens a pipe, writes the text, closes it, and
/// waits.
///
/// The environment is the environment [`wn`] builds, [`NO_CLIPBOARD_ENV`]
/// included, so a child of this helper touches the clipboard of the machine no
/// more than any other child of this file does.
fn run_with_stdin(gh: &FakeGh, args: &[&str], columns: &str, text: &str) -> Output {
    let mut child = wn(gh, args, columns, false, None)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The pipe closes when it goes out of scope here, and the tool reads
    // standard input to the end. A pipe that stayed open would hold the run.
    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// The command line of one child: the binary that was built, an environment
/// built from nothing, and the arguments.
///
/// The one place that builds the environment of a child, so the clipboard step
/// is off for every test that goes through it and no test has to remember the
/// rule.
fn wn(gh: &FakeGh, args: &[&str], columns: &str, color: bool, start: Option<&str>) -> Command {
    let path = format!("{}:/usr/bin:/bin", gh.path().display());
    let mut command = Command::new(env!("CARGO_BIN_EXE_wn"));
    command
        .env_clear()
        .env("PATH", path)
        .env("COLUMNS", columns)
        .env(NO_CLIPBOARD_ENV, NO_CLIPBOARD);
    if !color {
        command.env("NO_COLOR", "1");
    }
    if let Some(start) = start {
        command.env(START_COMMAND_ENV, start);
    }
    command.args(args);
    command
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn walks_the_chain_and_names_the_issue_to_start() {
    let gh = FakeGh::new(THREE_ISSUES);
    let output = run(&gh, &["--repo", REPO, "#277 → #278 ∥ #279"], "80", false);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(
        stdout(&output),
        concat!(
            "✓ #277  First thing\n",
            "→ #278  Second thing\n",
            "· #279  Third thing\n",
            "\n",
            "Start #278 next with 'si 278'\n",
        )
    );
}

#[test]
fn names_si_when_the_environment_names_no_start_command() {
    // This repository ships no `si`, so the default is a name the reader
    // supplies. It stays the default all the same, because it is the name the
    // plans of this repository are written with.
    let gh = FakeGh::new(THREE_ISSUES);
    let output = run_with_start(&gh, &["--repo", REPO, ONE_OPEN_CHAIN], "80", false, None);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).ends_with("Start #278 next with 'si 278'\n"),
        "the default command stands, in {}",
        stdout(&output)
    );
}

#[test]
fn the_environment_names_the_start_command() {
    let gh = FakeGh::new(THREE_ISSUES);
    let output = run_with_start(
        &gh,
        &["--repo", REPO, ONE_OPEN_CHAIN],
        "80",
        false,
        Some("start"),
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).ends_with("Start #278 next with 'start 278'\n"),
        "the answer names the command of the environment, in {}",
        stdout(&output)
    );
}

#[test]
fn a_start_command_of_more_than_one_word_goes_in_as_it_is_written() {
    // The command is a command line and not a program name, so a reader who
    // has no shell function at all can name a whole command.
    let gh = FakeGh::new(THREE_ISSUES);
    let output = run_with_start(
        &gh,
        &["--repo", REPO, ONE_OPEN_CHAIN],
        "80",
        false,
        Some("gh issue develop"),
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).ends_with("Start #278 next with 'gh issue develop 278'\n"),
        "every word of the command comes through, in {}",
        stdout(&output)
    );
}

#[test]
fn an_empty_start_command_falls_back_to_the_default() {
    // An exported but empty variable is a common accident, and an answer that
    // reads `Start #278 next with ' 278'` names no command at all.
    let gh = FakeGh::new(THREE_ISSUES);
    let output = run_with_start(
        &gh,
        &["--repo", REPO, ONE_OPEN_CHAIN],
        "80",
        false,
        Some(""),
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).ends_with("Start #278 next with 'si 278'\n"),
        "the default command stands, in {}",
        stdout(&output)
    );
}

#[test]
fn a_start_command_of_only_whitespace_falls_back_to_the_default() {
    let gh = FakeGh::new(THREE_ISSUES);
    let output = run_with_start(
        &gh,
        &["--repo", REPO, ONE_OPEN_CHAIN],
        "80",
        false,
        Some("   "),
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).ends_with("Start #278 next with 'si 278'\n"),
        "the default command stands, in {}",
        stdout(&output)
    );
}

#[test]
fn an_unquoted_chain_arrives_as_one_line_again() {
    // A shell splits `#277 → #278` into three arguments, and the answer must
    // not depend on which of the two the user typed.
    let gh = FakeGh::new(THREE_ISSUES);
    let split = run(
        &gh,
        &["--repo", REPO, "#277", "→", "#278", "∥", "#279"],
        "80",
        false,
    );
    let quoted = run(&gh, &["--repo", REPO, "#277 → #278 ∥ #279"], "80", false);
    assert!(split.status.success(), "stderr: {}", stderr(&split));
    assert_eq!(stdout(&split), stdout(&quoted));
}

#[test]
fn asks_about_the_whole_chain_in_one_query() {
    let gh = FakeGh::new(THREE_ISSUES);
    let output = run(&gh, &["--repo", REPO, "#277 → #278 ∥ #279"], "80", false);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let args = gh.recorded_args();
    assert!(
        args.contains("owner=timmattison\n") && args.contains("name=tools\n"),
        "the repository goes in as variables, in {args}"
    );
    for number in [277, 278, 279] {
        assert!(
            args.contains(&format!("i{number}: issueOrPullRequest(number: {number})")),
            "the query asks about {number}, in {args}"
        );
    }
    assert_eq!(
        args.matches("issueOrPullRequest").count(),
        3,
        "one query asked about all three, in {args}"
    );
}

#[test]
fn takes_the_chain_from_standard_input() {
    let gh = FakeGh::new(THREE_ISSUES);
    let path = format!("{}:/usr/bin:/bin", gh.path().display());
    let output = Command::new("sh")
        .env_clear()
        .env("PATH", path)
        .env("COLUMNS", "80")
        .env("NO_COLOR", "1")
        // This test builds its own environment, so it states the switch as
        // well: a pipe that holds a chain must answer without a clipboard.
        .env(NO_CLIPBOARD_ENV, NO_CLIPBOARD)
        .env("WN", env!("CARGO_BIN_EXE_wn"))
        .arg("-c")
        .arg("printf '%s' '#277 → #278 ∥ #279' | \"$WN\" --repo timmattison/tools")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout(&output).ends_with("Start #278 next with 'si 278'\n"),
        "the answer came out of a piped chain, in {}",
        stdout(&output)
    );
}

#[test]
fn refuses_a_run_that_holds_no_chain_in_any_input() {
    // The helper turns the clipboard off, so this is a machine with no
    // clipboard to fall back on. The message is the message the tool printed
    // before the clipboard was an input at all, because a run with the switch
    // on asks for exactly that behavior.
    let gh = FakeGh::new(THREE_ISSUES);
    let output = run(&gh, &["--repo", REPO], "80", false);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert_eq!(stdout(&output), "", "nothing was printed as an answer");
    assert!(
        stderr(&output).contains(NO_CHAIN),
        "the error says no chain was given, in {}",
        stderr(&output)
    );
}

#[test]
fn an_empty_pipe_reaches_the_clipboard_step() {
    // A pipe that holds nothing is not a chain of nothing. The run walks on to
    // the clipboard, which the switch turned off, so it stops with the same
    // message a run with no pipe at all stops with. A run that stopped at
    // standard input would report a chain error about empty text instead.
    let gh = FakeGh::new(THREE_ISSUES);
    let path = format!("{}:/usr/bin:/bin", gh.path().display());
    let output = Command::new("sh")
        .env_clear()
        .env("PATH", path)
        .env("COLUMNS", "80")
        .env("NO_COLOR", "1")
        .env(NO_CLIPBOARD_ENV, NO_CLIPBOARD)
        .env("WN", env!("CARGO_BIN_EXE_wn"))
        .arg("-c")
        .arg("printf '' | \"$WN\" --repo timmattison/tools")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains(NO_CHAIN),
        "the empty pipe fell through to the clipboard step, in {}",
        stderr(&output)
    );
}

#[test]
fn the_run_with_no_argument_reads_the_clipboard() {
    // The one test of this file that leaves the switch out, and the one test
    // that holds the headline behavior: `wn` alone answers the chain the reader
    // just copied. Every other test here turns the clipboard off, and every
    // unit test gives `Sources` a reader of its own, so the line in `run` that
    // names the real clipboard could read `None` and the whole suite stays
    // green.
    //
    // The assertion reads which path the run took. It never reads what the
    // clipboard holds. `no chain given` is the error of a run whose inputs left
    // the clipboard out, and it is the only error that says so: an empty
    // clipboard, a clipboard that does not open, a clipboard of prose, and a
    // clipboard that holds a real chain each take a path of its own. The run is
    // therefore deterministic although it reads a resource of the whole machine
    // that this test does not own, and the assertion fails if and only if the
    // run stops wiring the clipboard in.
    //
    // `env_clear` takes `DISPLAY` and `WAYLAND_DISPLAY` out as well. On Linux
    // `arboard` then opens nothing, and the run reports `the clipboard could not
    // be read (…)`. That is still not the `no chain given` path, so the
    // assertion holds on a machine with no display exactly as it holds on a
    // desktop.
    //
    // The run reads the clipboard and writes nothing to it, so it keeps what
    // the person at the keyboard copied.
    let gh = FakeGh::new(THREE_ISSUES);
    let path = format!("{}:/usr/bin:/bin", gh.path().display());
    let output = Command::new(env!("CARGO_BIN_EXE_wn"))
        .env_clear()
        .env("PATH", path)
        .env("COLUMNS", "80")
        .env("NO_COLOR", "1")
        // NO_CLIPBOARD_ENV stays out of this environment on purpose. A child of
        // `output` reads `/dev/null` on standard input, so the run walks past
        // the argument and past the pipe and arrives at the clipboard.
        .args(["--repo", REPO])
        .output()
        .unwrap();
    assert!(
        !stderr(&output).contains(NO_CHAIN),
        "the clipboard is one of the inputs of the run, in {}",
        stderr(&output)
    );
}

#[test]
fn names_the_repository_of_the_current_directory_when_none_is_given() {
    // The fake `gh` answers `repo view` with the repository name, and the
    // note about the missing number is where that name shows up.
    let body = r#"{"data":{"repository":{
"i277":{"__typename":"Issue","number":277,"title":"First thing","state":"OPEN","stateReason":null},
"i999":null
}},"errors":[{"type":"NOT_FOUND","path":["repository","i999"],"message":"Could not resolve to an issue or pull request with the number of 999."}]}"#;
    let gh = FakeGh::with_status(body, 1);
    let output = run(&gh, &["#277 → #999"], "80", false);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a number the repository does not have is a failed run"
    );
    assert_eq!(
        stdout(&output),
        concat!(
            "→ #277  First thing\n",
            "? #999  (no such issue)\n",
            "\n",
            "#999 is not in timmattison/tools.\n",
            "Start #277 next with 'si 277'\n",
        )
    );
}

#[test]
fn keeps_the_color_when_a_wrapper_says_how_wide_the_window_is() {
    let gh = FakeGh::new(THREE_ISSUES);
    let output = run(&gh, &["--repo", REPO, "#277 → #278"], "80", true);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let painted = stdout(&output);
    assert!(
        painted.contains('\u{1b}'),
        "the rows are painted for the wrapper, in {painted:?}"
    );
    assert!(
        testcolor::strip_ansi(&painted).starts_with("✓ #277  First thing\n"),
        "the paint comes back out, in {painted:?}"
    );
}

#[test]
fn refuses_a_chain_that_holds_a_word() {
    let gh = FakeGh::new(THREE_ISSUES);
    let output = run(&gh, &["--repo", REPO, "#277 an #278"], "80", false);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains("\"an\" is not an issue number"),
        "the error names the word, in {}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "", "nothing was printed as an answer");
}

#[test]
fn refuses_a_repository_that_is_not_two_parts() {
    let gh = FakeGh::new(THREE_ISSUES);
    let output = run(&gh, &["--repo", "tools", "#277"], "80", false);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains("tools"),
        "the error names the argument, in {}",
        stderr(&output)
    );
}

#[test]
fn reports_a_repository_nobody_can_read() {
    let body = r#"{"data":{"repository":null},"errors":[{"type":"NOT_FOUND","message":"Could not resolve to a Repository with the name 'timmattison/nope'."}]}"#;
    let gh = FakeGh::with_status(body, 1);
    let output = run(&gh, &["--repo", "timmattison/nope", "#277"], "80", false);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains("Could not resolve to a Repository"),
        "the error carries what GitHub said, in {}",
        stderr(&output)
    );
}

#[test]
fn reports_a_number_github_would_not_answer_for() {
    // A null answer with a FORBIDDEN beside it is not a number the repository
    // does not have. Printing the `?` row and the note would tell the reader
    // to hunt for a typo they did not make, so the whole run fails instead.
    let body = r#"{"data":{"repository":{
"i277":{"__typename":"Issue","number":277,"title":"First thing","state":"OPEN","stateReason":null},
"i278":null
}},"errors":[{"type":"FORBIDDEN","path":["repository","i278"],"message":"Resource not accessible by integration"}]}"#;
    let gh = FakeGh::with_status(body, 1);
    let output = run(&gh, &["--repo", REPO, "#277 → #278"], "80", false);

    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains("Resource not accessible by integration"),
        "the error carries what GitHub said, in {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("#278"),
        "the error names the number, in {}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "", "nothing was printed as an answer");
}

#[test]
fn says_so_when_the_github_cli_is_not_installed() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_wn"))
        .env_clear()
        .env("PATH", dir.path())
        .env("COLUMNS", "80")
        .env("NO_COLOR", "1")
        // This test builds its own environment, so it states the switch as
        // well: no child of this file reads the clipboard of the machine.
        .env(NO_CLIPBOARD_ENV, NO_CLIPBOARD)
        .args(["--repo", REPO, "#277"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains("GitHub CLI"),
        "the error says what is missing, in {}",
        stderr(&output)
    );
}

#[test]
fn cuts_a_long_title_to_the_window() {
    let body = r#"{"data":{"repository":{
"i1":{"__typename":"Issue","number":1,"title":"A title that is far too long for the window it has to fit in","state":"OPEN","stateReason":null}
}}}"#;
    let gh = FakeGh::new(body);
    let output = run(&gh, &["--repo", REPO, "#1"], "20", false);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    // The window is 20 columns and the last one stays empty, so the title is
    // cut to the 19 columns the row has.
    assert_eq!(
        stdout(&output).lines().next().unwrap(),
        "→ #1  A title that…"
    );
}

#[test]
fn a_row_stops_one_column_short_of_the_window() {
    // A row that fills the window exactly is one column too wide. A terminal
    // with auto-wrap moves the last glyph of such a row to the next line, and
    // right-edge chrome takes the same column, so the window keeps its last
    // column empty and a long title is cut one column earlier.
    let body = r#"{"data":{"repository":{
"i1":{"__typename":"Issue","number":1,"title":"A title that is far too long for the window it has to fit in","state":"OPEN","stateReason":null}
}}}"#;
    let gh = FakeGh::new(body);
    let output = run(&gh, &["--repo", REPO, "#1"], "20", false);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let row = stdout(&output)
        .lines()
        .next()
        .expect("the block holds a row")
        .to_string();
    assert_eq!(
        UnicodeWidthStr::width(row.as_str()),
        19,
        "the row stops one column short of the 20-column window, in {row:?}"
    );
}

#[test]
fn answers_a_whole_plan_of_parallel_work_from_a_pipe() {
    // The headline of the feature: a plan pasted into a pipe gives one block
    // for each stream and one summary that names the issue to start in each of
    // them. No flag says the text is a plan — the shape of the text does.
    let gh = FakeGh::new(PLAN_ISSUES);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", PLAN);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), PLAN_ANSWER);
}

#[test]
fn a_plan_in_one_quoted_argument_answers_the_same_way() {
    // A shell hands a quoted argument over whole, its newlines included, and
    // the arguments join back into one line. So a plan works on the command
    // line exactly as it works in a pipe.
    let gh = FakeGh::new(PLAN_ISSUES);
    let output = run(&gh, &["--repo", REPO, PLAN], "80", false);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), PLAN_ANSWER);
}

#[test]
fn the_table_form_of_a_plan_gives_the_streams_of_the_record_form() {
    // One plan is written two ways: the records a terminal prints, and the
    // table a file holds. Both name the same streams, so both give one answer.
    let gh = FakeGh::new(PLAN_ISSUES);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", PLAN_TABLE);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), PLAN_ANSWER);
}

#[test]
fn a_stream_that_names_a_number_the_repository_does_not_have_still_answers() {
    // The number keeps its row and earns its note, the other stream answers as
    // it always did, and the run exits 1. One typo takes down one row of one
    // block, and never the whole plan.
    let body = r#"{"data":{"repository":{
"i344":{"__typename":"Issue","number":344,"title":"First thing","state":"CLOSED","stateReason":"COMPLETED"},
"i999":null,
"i330":{"__typename":"Issue","number":330,"title":"Second thing","state":"OPEN","stateReason":null},
"i350":{"__typename":"Issue","number":350,"title":"Third thing","state":"OPEN","stateReason":null}
}},"errors":[{"type":"NOT_FOUND","path":["repository","i999"],"message":"Could not resolve to an issue or pull request with the number of 999."}]}"#;
    let gh = FakeGh::with_status(body, 1);
    let plan = "Stream: S1 gitscratch\nOrder: #344 → #999 → #330\nStream: S2 ic\nOrder: #350\n";
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", plan);
    assert_eq!(
        output.status.code(),
        Some(1),
        "a number the repository does not have is a failed run, stderr: {}",
        stderr(&output)
    );
    assert_eq!(
        stdout(&output),
        concat!(
            "S1 gitscratch\n",
            "  ✓ #344  First thing\n",
            "  ? #999  (no such issue)\n",
            "  → #330  Second thing\n",
            "\n",
            "  #999 is not in timmattison/tools.\n",
            "\n",
            "S2 ic\n",
            "  → #350  Third thing\n",
            "\n",
            "Take one from each stream:\n",
            "  S1 gitscratch  → #330  si 330\n",
            "  S2 ic          → #350  si 350\n",
        )
    );
}

#[test]
fn refuses_a_plan_whose_order_field_holds_a_word() {
    // A plan holds several chains, so the message names the stream as well as
    // the token. A message about the token alone leaves the reader to search
    // the whole page for it.
    let gh = FakeGh::new(PLAN_ISSUES);
    let output = run_with_stdin(
        &gh,
        &["--repo", REPO],
        "80",
        "Stream: S2 ic\nOrder: #350 an #187\n",
    );
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains("stream \"S2 ic\": \"an\" is not an issue number"),
        "the error names the stream and the token, in {}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "", "nothing was printed as an answer");
}

#[test]
fn refuses_a_plan_that_names_no_order_field() {
    // A text that names streams and no chain reaches the plan reader, which
    // says which field is missing. The chain reader would complain about the
    // token "Stream:", which tells the reader nothing about what to write.
    let gh = FakeGh::new(PLAN_ISSUES);
    let output = run_with_stdin(
        &gh,
        &["--repo", REPO],
        "80",
        "Stream: S1 gitscratch\nZone: src/gitscratch\nStream: S2 ic\nZone: src/ic\n",
    );
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains("no Order field"),
        "the error names the field that is missing, in {}",
        stderr(&output)
    );
    assert!(
        !stderr(&output).contains("\"Stream:\""),
        "the chain reader never saw the text, in {}",
        stderr(&output)
    );
}

#[test]
fn a_number_that_stands_in_two_streams_is_asked_about_once() {
    // The whole plan is one query, as one chain is. #330 stands in both
    // streams, and it costs one alias and is reported in both.
    let gh = FakeGh::new(PLAN_ISSUES);
    let output = run_with_stdin(
        &gh,
        &["--repo", REPO],
        "80",
        "Stream: S1 gitscratch\nOrder: #344 → #330\nStream: S2 ic\nOrder: #330 → #350\n",
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let args = gh.recorded_args();
    for number in [344, 330, 350] {
        assert!(
            args.contains(&format!("i{number}: issueOrPullRequest(number: {number})")),
            "the query asks about {number}, in {args}"
        );
    }
    assert_eq!(
        args.matches("issueOrPullRequest").count(),
        3,
        "one query asked about all three, and #330 once, in {args}"
    );
    assert!(
        stdout(&output).contains("  → #330  Second thing\n"),
        "the number stands in both blocks, in {}",
        stdout(&output)
    );
}

#[test]
fn a_pull_request_and_the_issue_it_closes_are_one_row() {
    // `PR#344 (#341)` is one step and not two. The state of the row is the
    // state of the pull request, so a merged 344 is walked past although 341
    // is still open — and the two states that disagree earn a note.
    let body = r#"{"data":{"repository":{
"i344":{"__typename":"PullRequest","number":344,"title":"First thing","state":"MERGED"},
"i341":{"__typename":"Issue","number":341,"title":"The bug","state":"OPEN","stateReason":null},
"i330":{"__typename":"Issue","number":330,"title":"Second thing","state":"OPEN","stateReason":null}
}}}"#;
    let gh = FakeGh::new(body);
    let output = run_with_stdin(
        &gh,
        &["--repo", REPO],
        "80",
        "Stream: S1 gitscratch\nOrder: PR#344 (#341) → #330\n",
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(
        stdout(&output),
        concat!(
            "S1 gitscratch\n",
            "  ✓ #344 (#341)  First thing\n",
            "  → #330         Second thing\n",
            "\n",
            "  #344 is closed and #341 is open.\n",
            "\n",
            "Take one from each stream:\n",
            "  S1 gitscratch  → #330  si 330\n",
        )
    );
}

#[test]
fn answers_the_paste_of_the_plan_parallel_work_skill() {
    // The whole point of the feature: copy the report of the skill out of a
    // terminal, type `wn`, and read the issue to start in each stream. The
    // paste draws its table with `│` and `┌─┬─┐`, it wraps two of its rows
    // onto a second line, and its Order fields annotate two steps in
    // parentheses.
    let gh = FakeGh::new(BOX_ISSUES);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", BOX_TABLE);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), BOX_ANSWER);
}

#[test]
fn a_pull_request_an_annotation_names_is_the_work_of_its_step() {
    // `#4 (in flight, PR #15)` is the issue #4 whose work is the pull request
    // #15, so the row is the pull request and the state of the row is the
    // state of it. A merged pull request over an open issue earns the same
    // note the `PR#344 (#341)` order earns, because it is the same step.
    let body = r#"{"data":{"repository":{
"i15":{"__typename":"PullRequest","number":15,"title":"The visualizer branch","state":"MERGED"},
"i4":{"__typename":"Issue","number":4,"title":"The visualizers","state":"OPEN","stateReason":null},
"i7":{"__typename":"Issue","number":7,"title":"Keep or delete","state":"OPEN","stateReason":null}
}}}"#;
    let gh = FakeGh::new(body);
    let output = run_with_stdin(
        &gh,
        &["--repo", REPO],
        "80",
        "Stream: A visualizers\nOrder: #4 (in flight, PR #15) → #7\n",
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(
        stdout(&output),
        concat!(
            "A visualizers\n",
            "  ✓ #15 (#4)  The visualizer branch\n",
            "  → #7        Keep or delete\n",
            "\n",
            "  #15 is closed and #4 is open.\n",
            "\n",
            "Take one from each stream:\n",
            "  A visualizers  → #7  si 7\n",
        )
    );
}

#[test]
fn refuses_a_row_whose_cell_count_the_header_does_not_have() {
    // A note holding a bar it did not escape puts every cell after that bar
    // under the wrong column. The message prints the row, and the run answers
    // nothing.
    let gh = FakeGh::new(PLAN_ISSUES);
    let row = "| S1 | #350 | src/ic | a note with a | bar |";
    let output = run_with_stdin(
        &gh,
        &["--repo", REPO],
        "80",
        &format!("| Stream | Order | Zone | Notes |\n| --- | --- | --- | --- |\n{row}\n"),
    );
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains("row has 5 cells, the header has 4"),
        "the error names both counts, in {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains(row),
        "the error prints the row, in {}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "", "nothing was printed as an answer");
}

#[test]
fn answers_a_plan_drawn_as_a_picture_from_a_pipe() {
    // The headline of the feature: a picture pasted into a pipe gives one row
    // for each step, the work each blocked step waits for, and one start line
    // for each stream that is ready. Two streams that join are two people who
    // work at the same time, and an answer that named one issue loses that.
    // No flag says the text is a picture — the shape of the text does.
    let gh = FakeGh::new(PICTURE_ISSUES);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", PICTURE);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), PICTURE_ANSWER);
}

#[test]
fn a_stream_that_is_finished_leaves_the_other_stream_as_the_one_answer() {
    // The top stream is closed, so one person is free and the other one is
    // still on `#246`. The answer names that one issue, and the row of `#249`
    // names the one step of the bottom stream it still waits for.
    let body = r#"{"data":{"repository":{
"i242":{"__typename":"Issue","number":242,"title":"Read the picture","state":"CLOSED","stateReason":"COMPLETED"},
"i247":{"__typename":"Issue","number":247,"title":"Answer the picture","state":"CLOSED","stateReason":"COMPLETED"},
"i246":{"__typename":"Issue","number":246,"title":"Read the table","state":"OPEN","stateReason":null},
"i248":{"__typename":"Issue","number":248,"title":"Answer the table","state":"OPEN","stateReason":null},
"i249":{"__typename":"Issue","number":249,"title":"Paint the gallery","state":"OPEN","stateReason":null}
}}}"#;
    let gh = FakeGh::new(body);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", PICTURE);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(
        stdout(&output),
        concat!(
            "✓ #242  Read the picture\n",
            "✓ #247  Answer the picture\n",
            "→ #246  Read the table\n",
            "· #248  Answer the table    waits for #246\n",
            "· #249  Paint the gallery   waits for #248\n",
            "\n",
            "Start #246 next with 'si 246'\n",
        )
    );
}

#[test]
fn a_picture_that_names_a_number_the_repository_does_not_have_still_answers() {
    // The number keeps its row and earns the red note, the rows around it read
    // as they always did, and the run exits 1. One typo takes down one row of
    // the picture, and never the whole answer.
    let body = r#"{"data":{"repository":{
"i242":{"__typename":"Issue","number":242,"title":"Read the picture","state":"OPEN","stateReason":null},
"i247":{"__typename":"Issue","number":247,"title":"Answer the picture","state":"OPEN","stateReason":null},
"i246":{"__typename":"Issue","number":246,"title":"Read the table","state":"OPEN","stateReason":null},
"i248":{"__typename":"Issue","number":248,"title":"Answer the table","state":"OPEN","stateReason":null},
"i249":null
}},"errors":[{"type":"NOT_FOUND","path":["repository","i249"],"message":"Could not resolve to an issue or pull request with the number of 249."}]}"#;
    let gh = FakeGh::with_status(body, 1);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", PICTURE);
    assert_eq!(
        output.status.code(),
        Some(1),
        "a number the repository does not have is a failed run, stderr: {}",
        stderr(&output)
    );
    assert_eq!(
        stdout(&output),
        concat!(
            "→ #242  Read the picture\n",
            "· #247  Answer the picture  waits for #242\n",
            "→ #246  Read the table\n",
            "· #248  Answer the table    waits for #246\n",
            "? #249  (no such issue)\n",
            "\n",
            "#249 is not in timmattison/tools.\n",
            "Start #242 next with 'si 242'\n",
            "Start #246 next with 'si 246'\n",
        )
    );
}

#[test]
fn refuses_a_picture_whose_wires_return_to_a_step_before_them() {
    // The wires run from `#1` to `#2` and back to `#1`, so no step of the
    // picture starts first. The message names the numbers of the cycle,
    // because an answer of "nothing is ready" would hide the reason, and the
    // run could not answer at all.
    let gh = FakeGh::new(PICTURE_ISSUES);
    let output = run_with_stdin(
        &gh,
        &["--repo", REPO],
        "80",
        "\
┌──→ #1 ──→ #2 ──┐
│                │
└────────────────┘
",
    );
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains("the wires return to #1 and #2"),
        "the error names the numbers of the cycle, in {}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "", "nothing was printed as an answer");
}

#[test]
fn refuses_a_picture_that_holds_a_leftward_arrowhead() {
    // A picture drawn from right to left says the opposite order, and a guess
    // at it sends somebody to the wrong issue. So the run stops and the
    // message prints the line, which is what the reader must redraw.
    let line = "#246 ←── #248 ──┘";
    let gh = FakeGh::new(PICTURE_ISSUES);
    let output = run_with_stdin(
        &gh,
        &["--repo", REPO],
        "80",
        &format!(
            "\
#242 ──→ #247 ──┐
                ├──→ #249  (gallery)
{line}
"
        ),
    );
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains("holds a leftward arrowhead"),
        "the error says what the picture holds, in {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains(line),
        "the error prints the line, in {}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "", "nothing was printed as an answer");
}

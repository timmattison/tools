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
//! Each test builds its own temporary directory, so concurrent test runs stay
//! isolated (see the parallel-safety note in the project guidelines).

#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error: on the temporary directory and the fixture files the test just created, on spawning the freshly built binary (a spawn failure is a broken harness, not behavior under test), and on reading back a file the fake gh wrote. The error paths of the tool itself are never unwrapped — they are asserted through the exit status and the text on standard error"
)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use unicode_width::UnicodeWidthStr;

/// The repository the fake `gh` answers for.
const REPO: &str = "timmattison/tools";

/// The variable that names the command the answer prints.
const START_COMMAND_ENV: &str = "WN_START_COMMAND";

/// A chain of three issues: one closed, two open.
const THREE_ISSUES: &str = r#"{"data":{"repository":{
"i277":{"__typename":"Issue","number":277,"title":"First thing","state":"CLOSED","stateReason":"COMPLETED"},
"i278":{"__typename":"Issue","number":278,"title":"Second thing","state":"OPEN","stateReason":null},
"i279":{"__typename":"Issue","number":279,"title":"Third thing","state":"OPEN","stateReason":null}
}}}"#;

/// The chain the start-command tests walk. Two of the three issues, so the
/// answer names #278 whichever command it prints.
const ONE_OPEN_CHAIN: &str = "#277 → #278";

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
    let path = format!("{}:/usr/bin:/bin", gh.path().display());
    let mut command = Command::new(env!("CARGO_BIN_EXE_wn"));
    command
        .env_clear()
        .env("PATH", path)
        .env("COLUMNS", columns);
    if !color {
        command.env("NO_COLOR", "1");
    }
    if let Some(start) = start {
        command.env(START_COMMAND_ENV, start);
    }
    command.args(args).output().unwrap()
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

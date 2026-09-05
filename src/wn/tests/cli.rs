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
//! The last tests of this file read the line the tool paints while a run of
//! `claude` works. That line goes to standard error, and indicatif draws
//! nothing at all when standard error is not a terminal. So those runs get a
//! pseudo-terminal of a size this file chose as their standard error, and the
//! test reads the painted frames back off the other end of it.
//!
//! Each test builds its own temporary directory, so concurrent test runs stay
//! isolated (see the parallel-safety note in the project guidelines).

#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error: on the temporary directory and the fixture files the test just created, on spawning the freshly built binary (a spawn failure is a broken harness, not behavior under test), and on reading back a file the fake gh wrote. The error paths of the tool itself are never unwrapped — they are asserted through the exit status and the text on standard error"
)]

use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
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

/// The variable that turns the run of `claude` off.
///
/// Every child of this file gets it, for the reason every child gets
/// [`NO_CLIPBOARD_ENV`]: a run of the real `claude` would cost money, would
/// need an account, and would give a different answer every time. The list of
/// places it looks reaches outside the `PATH` this file builds — `/usr/local/
/// bin/claude` is one — so no `PATH` can hold the run down on its own.
///
/// [`run_building`] takes it back out, and the tests of the run go through
/// that helper and write a `claude` of their own.
const NO_CLAUDE_ENV: &str = "WN_NO_CLAUDE";

/// The value [`NO_CLAUDE_ENV`] carries. Any value with a character in it turns
/// the run off; this one says why it is there.
const NO_CLAUDE: &str = "1";

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

/// A plan of four streams whose `Waits for` cells join three of them.
///
/// The paste of issue #436. `S0` comes before `S1`, and both of them come
/// before `S2`. `S3` waits for nothing, and an empty cell is how a plan writes
/// that. So the plan draws the edges `#96 → #91`, `#96 → #89`, `#91 → #89`,
/// and `#89 → #94`, and the last of those is the chain of `S2` itself.
const WAITS_PLAN: &str = "\
| Stream | Order | Waits for | Zone | Notes |
|--------|-------|-----------|------|-------|
| S0 — daemon leak | #96 | | crates/tsm (serve.rs) | Do first, solo. |
| S1 — lifecycle | #91 | #96 | crates/tsm (kill.rs) | |
| S2 — install | #89 → #94 | #96, #91 | crates/tsm (shell-init) | Same hotspot as S1. |
| S3 — keymap | #86 | | packages/web | Disjoint. |
";

/// What GitHub says about every number of [`WAITS_PLAN`] when each of them is
/// open.
const WAITS_ISSUES: &str = r#"{"data":{"repository":{
"i96":{"__typename":"Issue","number":96,"title":"The daemon leak","state":"OPEN","stateReason":null},
"i91":{"__typename":"Issue","number":91,"title":"The lifecycle","state":"OPEN","stateReason":null},
"i89":{"__typename":"Issue","number":89,"title":"The install","state":"OPEN","stateReason":null},
"i94":{"__typename":"Issue","number":94,"title":"The shell init","state":"OPEN","stateReason":null},
"i86":{"__typename":"Issue","number":86,"title":"The keymap","state":"OPEN","stateReason":null}
}}}"#;

/// The answer [`WAITS_PLAN`] earns while every issue of it is open.
///
/// One row for each step, in the order of the work rather than the order of
/// the plan, and one start line for each of the two streams that are ready.
/// `#91`, `#89`, and `#94` each wait for work that is open, so no line of the
/// answer names them.
const WAITS_ANSWER: &str = concat!(
    "→ #96  The daemon leak\n",
    "· #91  The lifecycle    waits for #96\n",
    "· #89  The install      waits for #96, #91\n",
    "· #94  The shell init   waits for #89\n",
    "→ #86  The keymap\n",
    "\n",
    "Start #96 next with 'si 96'\n",
    "Start #86 next with 'si 86'\n",
);

/// The same four streams as [`WAITS_PLAN`], with no `Waits for` column at all.
///
/// The plan every reader wrote before that column stood. The streams stand
/// apart, so the answer is one block for each of them under one summary.
const WAITS_PLAN_WITHOUT_THE_COLUMN: &str = "\
| Stream | Order | Zone | Notes |
|--------|-------|------|-------|
| S0 — daemon leak | #96 | crates/tsm (serve.rs) | Do first, solo. |
| S1 — lifecycle | #91 | crates/tsm (kill.rs) | |
| S2 — install | #89 → #94 | crates/tsm (shell-init) | Same hotspot as S1. |
| S3 — keymap | #86 | packages/web | Disjoint. |
";

/// The answer [`WAITS_PLAN_WITHOUT_THE_COLUMN`] earns: one block for each of
/// the four streams, and one summary that names an issue to start in each of
/// them.
const WAITS_BLOCK_ANSWER: &str = concat!(
    "S0 — daemon leak\n",
    "  → #96  The daemon leak\n",
    "\n",
    "S1 — lifecycle\n",
    "  → #91  The lifecycle\n",
    "\n",
    "S2 — install\n",
    "  → #89  The install\n",
    "  · #94  The shell init\n",
    "\n",
    "S3 — keymap\n",
    "  → #86  The keymap\n",
    "\n",
    "Take one from each stream:\n",
    "  S0 — daemon leak  → #96  si 96\n",
    "  S1 — lifecycle    → #91  si 91\n",
    "  S2 — install      → #89  si 89\n",
    "  S3 — keymap       → #86  si 86\n",
);

/// What GitHub says about the numbers of [`WAITS_PLAN`] once `#96` is done.
///
/// The one step of `S0` is finished, so `S1` is free. `S2` waits for `S1` as
/// well, so it is not.
const WAITS_ISSUES_ONE_DONE: &str = r#"{"data":{"repository":{
"i96":{"__typename":"Issue","number":96,"title":"The daemon leak","state":"CLOSED","stateReason":"COMPLETED"},
"i91":{"__typename":"Issue","number":91,"title":"The lifecycle","state":"OPEN","stateReason":null},
"i89":{"__typename":"Issue","number":89,"title":"The install","state":"OPEN","stateReason":null},
"i94":{"__typename":"Issue","number":94,"title":"The shell init","state":"OPEN","stateReason":null},
"i86":{"__typename":"Issue","number":86,"title":"The keymap","state":"OPEN","stateReason":null}
}}}"#;

/// What GitHub says about the numbers of [`WAITS_PLAN`] once `#96` and `#91`
/// are done.
///
/// Both blockers of `S2` are finished, so its first step is free at last.
const WAITS_ISSUES_TWO_DONE: &str = r#"{"data":{"repository":{
"i96":{"__typename":"Issue","number":96,"title":"The daemon leak","state":"CLOSED","stateReason":"COMPLETED"},
"i91":{"__typename":"Issue","number":91,"title":"The lifecycle","state":"CLOSED","stateReason":"COMPLETED"},
"i89":{"__typename":"Issue","number":89,"title":"The install","state":"OPEN","stateReason":null},
"i94":{"__typename":"Issue","number":94,"title":"The shell init","state":"OPEN","stateReason":null},
"i86":{"__typename":"Issue","number":86,"title":"The keymap","state":"OPEN","stateReason":null}
}}}"#;

/// A plan whose `Waits for` cell names a number the repository does not have.
///
/// `#999` is the typo. It stands in no `Order` field, so the rows are the only
/// place that can say the repository does not have it.
const WAITS_PLAN_WITH_A_TYPO: &str = "\
| Stream | Order | Waits for |
|--------|-------|-----------|
| S0 — daemon leak | #96 | |
| S1 — lifecycle | #91 | #96, #999 |
";

/// What GitHub says about [`WAITS_PLAN_WITH_A_TYPO`]: two issues, and a
/// refusal for the number nobody has.
const WAITS_ISSUES_WITH_A_TYPO: &str = r#"{"data":{"repository":{
"i96":{"__typename":"Issue","number":96,"title":"The daemon leak","state":"OPEN","stateReason":null},
"i91":{"__typename":"Issue","number":91,"title":"The lifecycle","state":"OPEN","stateReason":null},
"i999":null
}},"errors":[{"type":"NOT_FOUND","path":["repository","i999"],"message":"Could not resolve to an issue or pull request with the number of 999."}]}"#;

/// A plan of two streams that wait for each other.
///
/// Neither of the two starts, so the plan names no work at all. It carries the
/// two fields a cycle is made of and no others, because a `Zone` and a `Notes`
/// say nothing about the order.
const WAITS_CYCLE: &str = "\
| Stream | Order | Waits for |
|--------|-------|-----------|
| S1 — lifecycle | #91 | #96 |
| S0 — daemon leak | #96 | #91 |
";

/// A plan written as JSON, the shape a program hands back.
///
/// The same file the reader of it reads in its own tests, so the document this
/// file drives the binary with is the document that reader was written for.
///
/// It names two streams. `S0` holds `#96`. `S1` holds `#91`, which waits for
/// `#96`, and then `#94`, whose work is the pull request `#102`.
const JSON_PLAN: &str = include_str!("../fixtures/plan-parallel-work.json");

/// What GitHub says about every number of [`JSON_PLAN`] when each of them is
/// open.
///
/// Four numbers and three steps: the pull request and the issue it closes are
/// one step, and the query asks about both of them.
const JSON_ISSUES: &str = r#"{"data":{"repository":{
"i96":{"__typename":"Issue","number":96,"title":"The daemon leak","state":"OPEN","stateReason":null},
"i91":{"__typename":"Issue","number":91,"title":"The lifecycle","state":"OPEN","stateReason":null},
"i102":{"__typename":"PullRequest","number":102,"title":"The shell init","state":"OPEN"},
"i94":{"__typename":"Issue","number":94,"title":"The install","state":"OPEN","stateReason":null}
}}}"#;

/// The answer [`JSON_PLAN`] earns while every issue of it is open.
///
/// The report of a graph, because a JSON plan is a graph: one row for each
/// step in the order of the work, and one start line for each issue somebody
/// can begin now. `#96` is the one of them.
const JSON_ANSWER: &str = concat!(
    "→ #96         The daemon leak\n",
    "· #91         The lifecycle    waits for #96\n",
    "· #102 (#94)  The shell init   waits for #91\n",
    "\n",
    "Start #96 next with 'si 96'\n",
);

/// What GitHub says about the numbers of [`JSON_PLAN`] once `#96` is done.
const JSON_ISSUES_ONE_DONE: &str = r#"{"data":{"repository":{
"i96":{"__typename":"Issue","number":96,"title":"The daemon leak","state":"CLOSED","stateReason":"COMPLETED"},
"i91":{"__typename":"Issue","number":91,"title":"The lifecycle","state":"OPEN","stateReason":null},
"i102":{"__typename":"PullRequest","number":102,"title":"The shell init","state":"OPEN"},
"i94":{"__typename":"Issue","number":94,"title":"The install","state":"OPEN","stateReason":null}
}}}"#;

/// The answer [`JSON_PLAN`] earns once `#96` is done: `#91` is free.
const JSON_ANSWER_ONE_DONE: &str = concat!(
    "✓ #96         The daemon leak\n",
    "→ #91         The lifecycle\n",
    "· #102 (#94)  The shell init   waits for #91\n",
    "\n",
    "Start #91 next with 'si 91'\n",
);

/// A JSON plan whose two streams wait for each other.
///
/// Neither of the two starts, so the plan names no work at all.
const JSON_CYCLE: &str = r#"{
  "version": 1,
  "streams": [
    { "id": "S1", "name": "lifecycle", "order": [{ "issue": 91, "waitsFor": [96] }] },
    { "id": "S0", "name": "daemon leak", "order": [{ "issue": 96, "waitsFor": [91] }] }
  ]
}"#;

/// A JSON plan whose `waitsFor` names a number the repository does not have.
///
/// `#999` is the typo. It stands in no `order` array, so the rows are the only
/// place that can say the repository does not have it.
const JSON_PLAN_WITH_A_TYPO: &str = r#"{
  "version": 1,
  "streams": [
    { "id": "S0", "order": [{ "issue": 96 }] },
    { "id": "S1", "order": [{ "issue": 91, "waitsFor": [96, 999] }] }
  ]
}"#;

/// A JSON plan with no work in it at all.
///
/// Somebody ran the skill on a repository with nothing to do. That is not an
/// error, and the answer says so.
const JSON_EMPTY: &str = "{ \"version\": 1, \"streams\": [] }";

/// The answer [`JSON_EMPTY`] earns.
const JSON_EMPTY_ANSWER: &str = "The plan holds no work. Nothing to start.\n";

/// The words of the message a text that is not an issue number earns.
///
/// The mark of the chain reader. A document that does not parse must never
/// reach it, because a message about a token names the wrong problem.
const NOT_AN_ISSUE: &str = "is not an issue number";

/// The word a message about a drawing holds.
///
/// A reader who wrote a table drew nothing, so the refusal of a plan must hold
/// none of it. One graph carries both forms, so one message answers for both.
const PICTURE_WORD: &str = "picture";

/// The line that opens the summary of a plan, and thus the mark of an answer
/// the plan reader wrote.
const SUMMARY_HEADING: &str = "Take one from each stream:";

/// The words that open the last column of a row of a picture, and thus the
/// mark of an answer the picture reader wrote.
const WAITS_FOR: &str = "waits for ";

/// The file the fake `claude` records the arguments of every call in.
///
/// A file of its own, beside the one the fake `gh` writes, so a test can say
/// that one of the two was never reached while the other was.
const CLAUDE_ARGS_FILE: &str = "claude-args";

/// The file the fake `claude` writes the prompt it was handed into.
const PROMPT_FILE: &str = "prompt";

/// The file the fake `gh` records the arguments of every call in.
///
/// It appears on the first call, so its absence is a run that never reached
/// `gh` at all.
const ARGS_FILE: &str = "args";

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
        let args_file = dir.path().join(ARGS_FILE);
        let script = format!(
            r#"#!/bin/sh
for arg in "$@"; do
    printf '%s\n' "$arg" >> '{args}'
done
if [ "$1" = "repo" ]; then
    printf '%s\n' '{REPO}'
    exit 0
fi
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

    /// Write a `gh` that can name no repository for the current directory.
    ///
    /// The `repo view` of the real `gh` fails that way outside a checkout,
    /// and it writes the reason on standard error.
    fn without_repo() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = format!(
            r#"#!/bin/sh
for arg in "$@"; do
    printf '%s\n' "$arg" >> '{args}'
done
printf 'not a git repository\n' >&2
exit 1
"#,
            args = dir.path().join(ARGS_FILE).display(),
        );
        let gh = dir.path().join("gh");
        std::fs::write(&gh, script).unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    /// Write a `claude` beside the `gh`.
    ///
    /// It answers `--version`, which is how the tool picks it, and it reads
    /// the prompt off standard input into [`PROMPT_FILE`]. `body` is the shell
    /// that runs after that, so each test says what its `claude` does and
    /// nothing more.
    ///
    /// No test of this file runs the real `claude`. A run of it would cost
    /// money, would need an account, and would give a different answer every
    /// time.
    fn with_claude(self, body: &str) -> Self {
        let script = format!(
            r#"#!/bin/sh
for arg in "$@"; do
    printf '%s\n' "$arg" >> '{args}'
done
if [ "$1" = "--version" ]; then
    printf '2.0.0 (Claude Code)\n'
    exit 0
fi
cat > '{prompt}'
{body}
"#,
            args = self.dir.path().join(CLAUDE_ARGS_FILE).display(),
            prompt = self.dir.path().join(PROMPT_FILE).display(),
        );
        let claude = self.dir.path().join("claude");
        std::fs::write(&claude, script).unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        self
    }

    /// The arguments of every call of the fake `claude`, one to a line.
    fn recorded_claude_args(&self) -> String {
        std::fs::read_to_string(self.dir.path().join(CLAUDE_ARGS_FILE)).unwrap()
    }

    /// The prompt the fake `claude` was handed on standard input.
    fn recorded_prompt(&self) -> String {
        std::fs::read_to_string(self.dir.path().join(PROMPT_FILE)).unwrap()
    }

    /// Whether the tool ran the fake `claude` not even once.
    ///
    /// The script writes its arguments on every call, `--version` included, so
    /// a file that never appeared is a run that never reached `claude`.
    fn never_ran_claude(&self) -> bool {
        !self.dir.path().join(CLAUDE_ARGS_FILE).exists()
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The arguments of every call, one to a line.
    fn recorded_args(&self) -> String {
        std::fs::read_to_string(self.dir.path().join(ARGS_FILE)).unwrap()
    }

    /// Whether the tool asked `gh` nothing at all.
    ///
    /// The script writes the file on its first call of any kind, the call that
    /// names the repository included, so a file that never appeared is a run
    /// that never reached `gh`.
    fn asked_nothing(&self) -> bool {
        !self.dir.path().join(ARGS_FILE).exists()
    }

    /// Whether the tool sent no query about any issue.
    ///
    /// A run that named the repository and stopped there sent no query. The
    /// call that names the repository is one cheap round trip, and the query
    /// is the one that costs a unit of the rate limit.
    fn sent_no_query(&self) -> bool {
        self.asked_nothing() || !self.recorded_args().contains("graphql")
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
    run_with_stdin_and_start(gh, args, columns, text, None)
}

/// The same, with [`START_COMMAND_ENV`] set to `start`.
///
/// `None` leaves the variable out of the environment, which is the state of a
/// machine that never set it. One helper opens the pipe for both, so the two
/// can never build a different environment.
fn run_with_stdin_and_start(
    gh: &FakeGh,
    args: &[&str],
    columns: &str,
    text: &str,
    start: Option<&str>,
) -> Output {
    let mut child = wn(gh, args, columns, false, start)
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
        .env(NO_CLIPBOARD_ENV, NO_CLIPBOARD)
        .env(NO_CLAUDE_ENV, NO_CLAUDE);
    if !color {
        command.env("NO_COLOR", "1");
    }
    if let Some(start) = start {
        command.env(START_COMMAND_ENV, start);
    }
    command.args(args);
    command
}

/// Run `wn` with the environment every other child of this file gets, and
/// with `env` on top of it.
///
/// The variables of the run of `claude` are read from the environment, and no
/// other helper of this file sets one. [`NO_CLIPBOARD_ENV`] comes with the
/// environment [`wn`] builds, so a child of this helper writes the clipboard
/// of the machine no more than it reads it.
fn run_building(gh: &FakeGh, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = wn(gh, args, "80", false, None);
    // The switch is on for every other child of this file. A test of the run
    // takes it off and writes its own `claude`, and a test that wants the
    // switch back names it in `env`.
    command.env_remove(NO_CLAUDE_ENV);
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().unwrap()
}

/// The document with its `generated` taken out.
///
/// A plan says its age under the answer, and the age of a fixture written on a
/// fixed day grows every day this file lives. A test that states the whole
/// answer therefore reads a plan that names no moment at all, which earns no
/// such note. The tests of the note itself name the moment they want.
fn undated(document: &str) -> String {
    document
        .lines()
        .filter(|line| !line.contains("\"generated\""))
        .collect::<Vec<_>>()
        .join("\n")
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
    // The helper turns the clipboard off and turns the run of `claude` off,
    // so this is a machine with no input after the pipe. The message is the
    // message the tool printed before either of them was an input at all,
    // because a run with both switches on asks for exactly that behavior.
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
    // the clipboard, which the switch turned off, and then to the run of
    // `claude`, which the second switch turned off, so it stops with the same
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
        .env(NO_CLAUDE_ENV, NO_CLAUDE)
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
fn a_machine_with_no_gh_is_not_told_to_name_a_repository() {
    // The run names no repository, so it asks `gh` for the repository of this
    // directory. `gh` is not on this PATH, so that call fails before it runs.
    //
    // The reason stands alone. Advice to name the repository with --repo
    // cannot help a machine with no `gh`, because the query runs `gh` as well:
    // a run with --repo fails one step later, at the query, with the same
    // reason. Advice that leads the reader to a second failure is worse than
    // no advice.
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_wn"))
        .env_clear()
        .env("PATH", dir.path())
        .env("COLUMNS", "80")
        .env("NO_COLOR", "1")
        // This test builds its own environment, so it states the switches as
        // well: no child of this file reads the clipboard of the machine, and
        // none of them runs `claude`.
        .env(NO_CLIPBOARD_ENV, NO_CLIPBOARD)
        .env(NO_CLAUDE_ENV, NO_CLAUDE)
        .args(["#277"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains("GitHub CLI"),
        "the error says what is missing, in {}",
        stderr(&output)
    );
    assert!(
        !stderr(&output).contains("--repo"),
        "the error gives advice that cannot help, in {}",
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
        stderr(&output).contains("the order returns to #1 and #2"),
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

#[test]
fn refuses_a_picture_whose_wire_reaches_text_that_is_not_a_step() {
    // A stream label beside a wire is a plan this form does not carry. The
    // reader who wrote `A` meant work by it, so the run names the text rather
    // than dropping the wire and the order it draws.
    let gh = FakeGh::new(PICTURE_ISSUES);
    let output = run_with_stdin(
        &gh,
        &["--repo", REPO],
        "80",
        "\
A ──→ #4
#5 ──→ #6 ──┐
            ├──→ #7
#8 ──→ #9 ──┘
",
    );
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains("\"A\" stands beside a wire and is not a step"),
        "the error names the text, in {}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "", "nothing was printed as an answer");
}

#[test]
fn the_environment_names_the_command_of_every_start_line_of_a_picture() {
    // A picture names one issue for each stream that is ready, and the reader
    // who set the variable set it for every one of them. A run that named the
    // command of the first line alone would leave the second line unusable.
    let gh = FakeGh::new(PICTURE_ISSUES);
    let output = run_with_stdin_and_start(&gh, &["--repo", REPO], "80", PICTURE, Some("start"));
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).ends_with(concat!(
            "Start #242 next with 'start 242'\n",
            "Start #246 next with 'start 246'\n",
        )),
        "the answer names the command of the environment in every line, in {}",
        stdout(&output)
    );
}

#[test]
fn a_chain_of_two_issues_on_one_line_is_still_a_chain() {
    // `→` is a wire of a picture, and the net it draws reaches `#277` on its
    // left and `#278` on its right. Both steps stand on one line, so the
    // picture claims nothing and the chain reader answers as it always did.
    // A reader who types a chain must never meet the block of a picture.
    let gh = FakeGh::new(THREE_ISSUES);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", ONE_OPEN_CHAIN);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(
        stdout(&output),
        concat!(
            "✓ #277  First thing\n",
            "→ #278  Second thing\n",
            "\n",
            "Start #278 next with 'si 278'\n",
        )
    );
}

#[test]
fn the_box_drawn_table_of_a_plan_is_still_a_plan() {
    // The border of that table is one net that touches every cell of it, and
    // the table stands on many lines. The plan reader is tried first, so the
    // table keeps its own reader: the answer is one block for each stream
    // under one summary, and no row of it names work it waits for.
    let gh = FakeGh::new(BOX_ISSUES);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", BOX_TABLE);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains(SUMMARY_HEADING),
        "the plan reader answered, in {}",
        stdout(&output)
    );
    assert!(
        !stdout(&output).contains(WAITS_FOR),
        "no block of a plan carries the column of a picture, in {}",
        stdout(&output)
    );
}

#[test]
fn a_picture_inside_a_fenced_code_block_reads_the_same_way() {
    // This is how a reader copies a picture out of an issue: the sentence over
    // it and the fence around it come with it. A line that holds no wire and
    // writes no step is prose, so the sentence and the two fences cost
    // nothing and the answer is the answer of the picture alone.
    let gh = FakeGh::new(PICTURE_ISSUES);
    let pasted = format!("The plan of the gallery:\n\n```\n{PICTURE}```\n");
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", &pasted);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), PICTURE_ANSWER);
}

#[test]
fn a_plan_that_names_a_blocker_answers_as_one_graph() {
    // The headline of the `Waits for` column: one step of one stream blocks
    // another stream, and no block of a plan says that. So a plan that names
    // one crosses to the answer a picture earns — one row for each step, in
    // the order of the work, and one start line for each stream that is ready.
    // `#96` and `#86` are the two people who start now.
    let gh = FakeGh::new(WAITS_ISSUES);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", WAITS_PLAN);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), WAITS_ANSWER);
}

#[test]
fn refuses_a_plan_whose_streams_wait_for_each_other() {
    // Two streams that wait for each other leave no work to start between
    // them, and an answer of "nothing is ready" hides the reason. So the run
    // stops and the message names the two numbers.
    //
    // The reader of this message wrote a table and drew no picture, so the
    // words of it name the order rather than a drawing. One graph carries the
    // table and the picture both, and one message answers for both of them.
    //
    // The run names no repository, and the fake `gh` records every call it is
    // given. So a run that reached `gh` at all wrote the file, and a mistake
    // in the text of the reader costs no round trip.
    let gh = FakeGh::new(WAITS_ISSUES);
    let output = run_with_stdin(&gh, &[], "80", WAITS_CYCLE);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    let message = stderr(&output);
    for number in ["#91", "#96"] {
        assert!(
            message.contains(number),
            "the error names {number} of the cycle, in {message}"
        );
    }
    assert!(
        !message.contains(PICTURE_WORD),
        "the error of a plan names no drawing, in {message}"
    );
    assert_eq!(stdout(&output), "", "nothing was printed as an answer");
    assert!(
        gh.asked_nothing(),
        "the run refused the plan before it asked GitHub, and it asked {}",
        gh.recorded_args()
    );
}

#[test]
fn a_finished_blocker_frees_the_stream_that_waited_for_it() {
    // `#96` is done, so `S1` starts. `S2` waits for `S1` as well, so `#89` is
    // still blocked and no line of the answer names it. An answer that read
    // the cell as a chain would name `#89` here, because the first blocker of
    // it is finished.
    let gh = FakeGh::new(WAITS_ISSUES_ONE_DONE);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", WAITS_PLAN);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(
        stdout(&output),
        concat!(
            "✓ #96  The daemon leak\n",
            "→ #91  The lifecycle\n",
            "· #89  The install      waits for #91\n",
            "· #94  The shell init   waits for #89\n",
            "→ #86  The keymap\n",
            "\n",
            "Start #91 next with 'si 91'\n",
            "Start #86 next with 'si 86'\n",
        )
    );
}

#[test]
fn a_stream_starts_when_every_blocker_of_it_is_finished() {
    // `#96` and `#91` are both done, so the whole cell of `S2` is finished and
    // `#89` is free. `#94` comes after it in the chain of that stream, so it
    // waits for `#89` alone.
    let gh = FakeGh::new(WAITS_ISSUES_TWO_DONE);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", WAITS_PLAN);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(
        stdout(&output),
        concat!(
            "✓ #96  The daemon leak\n",
            "✓ #91  The lifecycle\n",
            "→ #89  The install\n",
            "· #94  The shell init   waits for #89\n",
            "→ #86  The keymap\n",
            "\n",
            "Start #89 next with 'si 89'\n",
            "Start #86 next with 'si 86'\n",
        )
    );
}

#[test]
fn a_blocked_row_names_every_step_it_waits_for() {
    // A `Waits for` cell is a set and not a chain, so a row that named the
    // first blocker alone would tell a reader to start work that two other
    // people still hold. `#89` waits for `#96` and for `#91`, and the row says
    // both.
    let gh = FakeGh::new(WAITS_ISSUES);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", WAITS_PLAN);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let answer = stdout(&output);
    let row = answer
        .lines()
        .find(|line| line.contains("#89"))
        .unwrap_or_else(|| panic!("the answer holds a row of #89, in {answer}"))
        .to_string();
    assert!(
        row.ends_with("waits for #96, #91"),
        "the row names each step it waits for, in {row:?}"
    );
}

#[test]
fn a_blocker_the_repository_does_not_have_still_earns_a_row() {
    // The typo stands in no `Order` field, so a row of the answer is the one
    // place that can say the repository does not have it. The rows around it
    // read as they always did, and the run exits 1.
    let gh = FakeGh::with_status(WAITS_ISSUES_WITH_A_TYPO, 1);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", WAITS_PLAN_WITH_A_TYPO);
    assert_eq!(
        output.status.code(),
        Some(1),
        "a number the repository does not have is a failed run, stderr: {}",
        stderr(&output)
    );
    assert_eq!(
        stdout(&output),
        concat!(
            "→ #96   The daemon leak\n",
            "? #999  (no such issue)\n",
            "· #91   The lifecycle    waits for #96, #999\n",
            "\n",
            "#999 is not in timmattison/tools.\n",
            "Start #96 next with 'si 96'\n",
        )
    );
}

#[test]
fn a_plan_with_no_waits_for_column_answers_as_it_always_did() {
    // The plan every reader wrote before this column stood. Its streams stand
    // apart, so the answer is one block for each of them under one summary,
    // and no row of it names work it waits for. A run that read every plan as
    // a graph would take that answer away from them.
    let gh = FakeGh::new(WAITS_ISSUES);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", WAITS_PLAN_WITHOUT_THE_COLUMN);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), WAITS_BLOCK_ANSWER);
    assert!(
        stdout(&output).contains(SUMMARY_HEADING),
        "the plan reader answered, in {}",
        stdout(&output)
    );
    assert!(
        !stdout(&output).contains(WAITS_FOR),
        "no block of a plan carries the column of a graph, in {}",
        stdout(&output)
    );
}

#[test]
fn answers_a_plan_written_as_json() {
    // The fifth shape of input, and the one a program hands back. A JSON plan
    // is a graph, so it earns the report a picture earns: one row for each
    // step in the order of the work, and one start line for each issue
    // somebody can begin now.
    let gh = FakeGh::new(JSON_ISSUES);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", &undated(JSON_PLAN));
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), JSON_ANSWER);
}

#[test]
fn a_finished_step_of_a_json_plan_frees_the_step_that_waited_for_it() {
    let gh = FakeGh::new(JSON_ISSUES_ONE_DONE);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", &undated(JSON_PLAN));
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), JSON_ANSWER_ONE_DONE);
}

#[test]
fn a_pull_request_of_a_json_step_is_the_pair_the_row_writes() {
    // `"pr": 102` on the step of `#94` is the pair `PR#102 (#94)` writes, and
    // the state of the row is the state of the pull request, because the pull
    // request is the work.
    let gh = FakeGh::new(JSON_ISSUES);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", JSON_PLAN);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let answer = stdout(&output);
    assert!(
        answer.contains("#102 (#94)  The shell init"),
        "the row writes the pair and the title of the work, in {answer}"
    );
}

#[test]
fn refuses_a_json_plan_whose_steps_wait_for_each_other() {
    // The rule of a picture and of a `Waits for` column, unchanged: a cycle
    // has no step to start, so the message names the numbers that hold the
    // knot and the run costs no round trip.
    let gh = FakeGh::new(JSON_ISSUES);
    let output = run_with_stdin(&gh, &[], "80", JSON_CYCLE);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    let message = stderr(&output);
    for number in ["#91", "#96"] {
        assert!(
            message.contains(number),
            "the error names {number} of the cycle, in {message}"
        );
    }
    assert_eq!(stdout(&output), "", "nothing was printed as an answer");
    assert!(
        gh.asked_nothing(),
        "the run refused the plan before it asked GitHub, and it asked {}",
        gh.recorded_args()
    );
}

#[test]
fn a_json_blocker_the_repository_does_not_have_still_earns_a_row() {
    let gh = FakeGh::with_status(WAITS_ISSUES_WITH_A_TYPO, 1);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", JSON_PLAN_WITH_A_TYPO);
    assert_eq!(
        output.status.code(),
        Some(1),
        "a number the repository does not have is a failed run, stderr: {}",
        stderr(&output)
    );
    assert_eq!(
        stdout(&output),
        concat!(
            "→ #96   The daemon leak\n",
            "? #999  (no such issue)\n",
            "· #91   The lifecycle    waits for #96, #999\n",
            "\n",
            "#999 is not in timmattison/tools.\n",
            "Start #96 next with 'si 96'\n",
        )
    );
}

#[test]
fn a_json_plan_with_no_streams_in_it_is_no_error() {
    // Somebody ran the skill on a repository with nothing to do. The answer
    // says the plan is empty and the run exits 0, and it asks GitHub nothing:
    // a query with no field in it is a syntax error, and there is nothing to
    // ask about anyway.
    let gh = FakeGh::new(JSON_ISSUES);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", JSON_EMPTY);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), JSON_EMPTY_ANSWER);
    assert!(
        gh.asked_nothing(),
        "an empty plan asks about nothing, and it asked {}",
        gh.recorded_args()
    );
}

#[test]
fn a_document_that_does_not_parse_never_reaches_the_chain_reader() {
    // A reader that fell through on a broken document would take a document
    // with one missing brace to the chain reader, which would then report
    // `"version" is not an issue number`. That message names the wrong
    // problem.
    let gh = FakeGh::new(JSON_ISSUES);
    let broken = JSON_PLAN.trim_end().trim_end_matches('}');
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", broken);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    let message = stderr(&output);
    assert!(
        message.contains("is not a JSON document"),
        "the error names the shape the reader pasted, in {message}"
    );
    assert!(
        !message.contains(NOT_AN_ISSUE),
        "the chain reader never saw it, in {message}"
    );
    assert!(
        gh.asked_nothing(),
        "the run refused the document before it asked GitHub, and it asked {}",
        gh.recorded_args()
    );
}

#[test]
fn a_json_document_of_a_version_this_reader_does_not_know_is_refused() {
    let gh = FakeGh::new(JSON_ISSUES);
    let ahead = JSON_PLAN.replace("\"version\": 1", "\"version\": 2");
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", &ahead);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    let message = stderr(&output);
    assert!(
        message.contains("version 2") && message.contains("version 1"),
        "the error names the version it read and the version it knows, in {message}"
    );
}

#[test]
fn a_json_document_that_is_not_the_schema_names_the_path() {
    // The path is what says where to look, so the message walks the document
    // rather than naming the key alone.
    let gh = FakeGh::new(JSON_ISSUES);
    let text = JSON_PLAN.replace(
        "{ \"issue\": 91, \"waitsFor\": [96] }",
        "{ \"waitsFor\": [96] }",
    );
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", &text);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains("streams[1].order[0].issue is missing"),
        "the error names the path in the document, in {}",
        stderr(&output)
    );
}

/// The variable that names the seconds the run of `claude` may take.
const PLAN_TIMEOUT_ENV: &str = "WN_PLAN_TIMEOUT";

/// The variable that names the level of effort the run asks for.
const PLAN_EFFORT_ENV: &str = "WN_PLAN_EFFORT";

/// The variable that names the model the run asks for.
const PLAN_MODEL_ENV: &str = "WN_PLAN_MODEL";

/// The envelope a run of `claude --print --output-format json` prints, with
/// `document` in its `result`.
///
/// Built with the JSON writer rather than with `format!`, because a plan holds
/// newlines and quotation marks and every one of them has to be escaped. The
/// numbers are the numbers of one measured run, so a test that reads them
/// reads a shape a real `claude` really printed.
fn envelope(document: &str) -> String {
    serde_json::json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": document,
        "total_cost_usd": 0.054_637_9,
        "duration_ms": 1886,
        "num_turns": 1,
        "modelUsage": {
            "claude-opus-5": {
                "costUSD": 0.054_637_9,
                "inputTokens": 118_000,
                "outputTokens": 9_400,
                "cacheReadInputTokens": 13_629,
                "cacheCreationInputTokens": 26_438,
            }
        },
    })
    .to_string()
}

/// The report line the envelope of [`envelope`] earns, for a run that asked
/// for no level of effort.
const REPORT: &str =
    "plan: $0.05 · claude-opus-5 · 118k in, 9.4k out, 13k cache read, 26k cache write · 1.8s";

/// The shell of a fake `claude` that prints an envelope holding `document`.
fn prints(document: &str) -> String {
    format!(
        "cat <<'WN_FAKE_CLAUDE_ENVELOPE'\n{}\nWN_FAKE_CLAUDE_ENVELOPE\n",
        envelope(document)
    )
}

/// The shell of a fake `claude` that prints the document of [`JSON_PLAN`].
///
/// The plan names no moment, so its answer carries no note about its age. The
/// tests of that note name the moment they want.
fn prints_the_plan() -> String {
    prints(&undated(JSON_PLAN))
}

#[test]
fn a_run_with_nothing_to_read_builds_a_plan_with_claude() {
    // The fourth input. The argument, standard input, and the clipboard all
    // hold nothing, so the tool builds the plan itself rather than stopping
    // with "the clipboard is empty".
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&prints_the_plan());
    let output = run_building(&gh, &["--repo", REPO], &[]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), JSON_ANSWER);
    // The line that says a run is happening goes to standard error, so the
    // answer above reaches a pipe alone.
    assert!(stderr(&output).contains("claude"), "{}", stderr(&output));
}

#[test]
fn the_run_is_handed_the_prompt_and_the_tools_the_skill_needs() {
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&prints_the_plan());
    let output = run_building(&gh, &["--repo", REPO], &[]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(gh.recorded_prompt(), "/plan-parallel-work --json");
    let args = gh.recorded_claude_args();
    assert!(args.contains("--print"), "{args}");
    assert!(args.contains("--allowed-tools"), "{args}");
    assert!(args.contains("Bash"), "{args}");
    assert!(
        !args.contains("--dangerously-skip-permissions"),
        "the run asks for the tools it needs and never for the bypass, in {args}"
    );
}

#[test]
fn an_argument_is_never_a_reason_to_run_claude() {
    // The run is the quietest input of the four. A chain the reader typed
    // outranks it, and a run that costs money must not happen beside one.
    let gh = FakeGh::new(THREE_ISSUES).with_claude(&prints_the_plan());
    let output = run_building(&gh, &["--repo", REPO, ONE_OPEN_CHAIN], &[]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("Start #278"),
        "{}",
        stdout(&output)
    );
    assert!(gh.never_ran_claude(), "{}", gh.recorded_claude_args());
}

#[test]
fn refresh_builds_a_plan_even_when_an_argument_holds_one() {
    // The one way past a plan that is still on the clipboard and no longer
    // true. An argument is louder than a clipboard, so a run that outranks an
    // argument outranks the clipboard as well.
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&prints_the_plan());
    let output = run_building(&gh, &["--refresh", "--repo", REPO, ONE_OPEN_CHAIN], &[]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), JSON_ANSWER);
}

#[test]
fn the_variable_that_turns_the_run_off_leaves_the_error_the_tool_printed_before() {
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&prints_the_plan());
    let output = run_building(&gh, &["--repo", REPO], &[(NO_CLAUDE_ENV, "1")]);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(stderr(&output).contains(NO_CHAIN), "{}", stderr(&output));
    assert!(gh.never_ran_claude(), "{}", gh.recorded_claude_args());
}

#[test]
fn refresh_with_the_run_turned_off_names_the_variable() {
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&prints_the_plan());
    let output = run_building(&gh, &["--refresh", "--repo", REPO], &[(NO_CLAUDE_ENV, "1")]);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains(NO_CLAUDE_ENV),
        "{}",
        stderr(&output)
    );
    assert!(gh.never_ran_claude(), "{}", gh.recorded_claude_args());
}

#[test]
fn a_run_that_printed_nothing_names_claude() {
    let gh = FakeGh::new(JSON_ISSUES).with_claude("exit 0\n");
    let output = run_building(&gh, &["--repo", REPO], &[]);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(stderr(&output).contains("claude"), "{}", stderr(&output));
    assert!(
        gh.sent_no_query(),
        "a run with no plan asks about no issue, and it asked {}",
        gh.recorded_args()
    );
}

#[test]
fn a_run_that_could_not_log_in_names_claude_login() {
    let gh = FakeGh::new(JSON_ISSUES)
        .with_claude("printf 'Invalid API key · Please run /login\\n' >&2\nexit 1\n");
    let output = run_building(&gh, &["--repo", REPO], &[]);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains("claude login"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_reason_written_on_standard_output_reaches_the_reader() {
    // A program writes a reason on standard error, and a run that mixes the
    // two pipes writes it on standard output. The reason is the same reason,
    // so the reader gets it whichever pipe carried it. A refusal that named
    // `claude` and then stopped at the colon tells the reader nothing.
    //
    // This run prints no envelope at all, which is the shape the pipes are
    // still read for. A run that prints one is the test below.
    let gh = FakeGh::new(JSON_ISSUES).with_claude("printf 'the model is overloaded\\n'\nexit 1\n");
    let output = run_building(&gh, &["--repo", REPO], &[]);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    let message = stderr(&output);
    assert!(message.contains("the model is overloaded"), "{message}");
}

#[test]
fn the_envelope_of_a_failing_run_carries_the_reason_and_the_pipes_do_not() {
    // A run that names a model that does not exist exits 1, and it prints an
    // envelope all the same. That envelope carries the sentence a reader can
    // act on, and standard error carries a machine tag on the same mistake.
    // So the envelope stands in front of the pipes.
    //
    // The run also spent money before it failed, and the report of what it
    // cost is the one place a reader reads the price.
    let said = "There's an issue with the selected model (no-such-model-xyz). It may not exist \
                or you may not have access to it. Run --model to pick a different model.";
    let refused = serde_json::json!({
        "type": "result",
        "subtype": "error_during_execution",
        "is_error": true,
        "result": said,
        "total_cost_usd": 0.054_637_9,
        "duration_ms": 1886,
        "num_turns": 1,
    })
    .to_string();
    let body = format!(
        "cat <<'WN_FAKE_CLAUDE_ENVELOPE'\n{refused}\nWN_FAKE_CLAUDE_ENVELOPE\n\
         printf '[claude-code:unrecognized_model] \
         {{\"model\":\"no-such-model-xyz\",\"query_source\":\"sdk\"}}\\n' >&2\n\
         exit 1\n"
    );
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&body);
    let output = run_building(
        &gh,
        &["--repo", REPO],
        &[(PLAN_MODEL_ENV, "no-such-model-xyz")],
    );
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    let message = stderr(&output);
    assert!(
        message.contains("There's an issue with the selected model"),
        "{message}"
    );
    // The machine tag names the mistake and says nothing a reader acts on, so
    // it must not stand in front of the sentence.
    assert!(!message.contains("unrecognized_model"), "{message}");
    // The run cost money before it failed, and the reader paid for it.
    assert!(message.contains("plan: $0.05"), "{message}");
}

#[test]
fn a_document_the_run_built_that_does_not_parse_names_no_clipboard() {
    // The refusal of the reader of a JSON plan, unchanged. A message that
    // named the clipboard would send the reader to look at a clipboard that
    // holds none of it.
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&prints("{ \"version\": 1\n"));
    let output = run_building(&gh, &["--repo", REPO], &[]);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    let message = stderr(&output);
    assert!(message.contains("is not a JSON document"), "{message}");
    assert!(!message.contains("clipboard"), "{message}");
}

#[test]
fn a_run_that_outlives_its_deadline_is_killed_and_says_so() {
    // The fake `claude` replaces itself with the sleep, so the process the
    // tool kills is the process that waits. A run that left a sleep behind
    // would outlive the test that started it.
    let gh = FakeGh::new(JSON_ISSUES).with_claude("exec sleep 30\n");
    let started = std::time::Instant::now();
    let output = run_building(&gh, &["--repo", REPO], &[(PLAN_TIMEOUT_ENV, "1")]);
    let waited = started.elapsed();
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    let message = stderr(&output);
    assert!(message.contains("1 seconds"), "{message}");
    assert!(message.contains(PLAN_TIMEOUT_ENV), "{message}");
    assert!(
        waited < std::time::Duration::from_secs(20),
        "the run stopped at its deadline and did not wait for the sleep, in {waited:?}"
    );
}

#[test]
fn a_timeout_that_names_no_seconds_is_a_refusal_that_costs_no_run() {
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&prints_the_plan());
    let output = run_building(&gh, &["--repo", REPO], &[(PLAN_TIMEOUT_ENV, "10m")]);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains(PLAN_TIMEOUT_ENV),
        "{}",
        stderr(&output)
    );
    assert!(gh.never_ran_claude(), "{}", gh.recorded_claude_args());
}

#[test]
fn a_stale_plan_says_its_age_under_the_answer() {
    // A plan is a claim about a backlog, and a backlog moves. The note costs
    // one line and it is the only thing that would tell the reader.
    let gh = FakeGh::new(JSON_ISSUES);
    let old = JSON_PLAN.replace("2026-09-02T14:03:11Z", "2020-01-01T00:00:00Z");
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", &old);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let answer = stdout(&output);
    assert!(answer.starts_with(JSON_ANSWER), "{answer}");
    assert!(answer.contains("This plan was built "), "{answer}");
    assert!(
        answer.contains("Run wn --refresh to build a new one."),
        "{answer}"
    );
}

#[test]
fn a_plan_that_names_no_moment_says_nothing_about_its_age() {
    let gh = FakeGh::new(JSON_ISSUES);
    let output = run_with_stdin(&gh, &["--repo", REPO], "80", &undated(JSON_PLAN));
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        !stdout(&output).contains("This plan was built"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_directory_that_is_in_no_repository_costs_no_run() {
    // The skill asks `gh` and `git` about the repository of the current
    // directory, and its gather script turns a failure of either into a
    // warning rather than a crash. So a run in such a directory would spend a
    // minute and real money and would then answer that the plan holds no
    // work. The refusal stands before the run, where it costs one cheap call.
    let gh = FakeGh::without_repo().with_claude(&prints_the_plan());
    let output = run_building(&gh, &[], &[]);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    let message = stderr(&output);
    // The reason `gh` gave is carried, and the advice it carries for the
    // reader of a query is not. That advice reads "Name the repository with
    // --repo owner/name", and this refusal stands on the line above it saying
    // that naming one does not help. Two adjacent lines must not disagree.
    assert!(message.contains("not a git repository"), "{message}");
    assert!(!message.contains("Name the repository with"), "{message}");
    assert!(gh.never_ran_claude(), "{}", gh.recorded_claude_args());
}

#[test]
fn the_report_of_the_run_goes_to_standard_error_and_the_plan_goes_to_standard_output() {
    // The reader pays for the run, and the price is written on the pipe the
    // spinner already writes on. The document goes to standard output, so a
    // reader who pipes that output gets the answer alone.
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&prints_the_plan());
    let output = run_building(&gh, &["--repo", REPO], &[]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), JSON_ANSWER);
    assert!(stderr(&output).contains(REPORT), "{}", stderr(&output));
    // The document reaches standard output alone, so the report cannot be on
    // both pipes.
    assert!(!stdout(&output).contains("plan: $"), "{}", stdout(&output));
}

#[test]
fn the_level_the_environment_named_reaches_the_run_and_the_report() {
    // The envelope carries no field that names a level, so the report can
    // only name the level the run asked for. The variable is that level, and
    // it is the lever a reader who thinks the plan cost too much pulls.
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&prints_the_plan());
    let output = run_building(&gh, &["--repo", REPO], &[(PLAN_EFFORT_ENV, "high")]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let args = gh.recorded_claude_args();
    assert!(args.contains("--effort"), "{args}");
    assert!(args.contains("high"), "{args}");
    // The whole line, with the level in it. The level stands beside the
    // models, because the models are what ran at it.
    assert!(
        stderr(&output).contains(&REPORT.replace("claude-opus-5", "claude-opus-5 at effort high")),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_level_that_is_not_one_of_the_five_is_a_refusal_that_costs_no_run() {
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&prints_the_plan());
    let output = run_building(&gh, &["--repo", REPO], &[(PLAN_EFFORT_ENV, "quick")]);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains(PLAN_EFFORT_ENV),
        "{}",
        stderr(&output)
    );
    assert!(gh.never_ran_claude(), "{}", gh.recorded_claude_args());
}

#[test]
fn the_model_the_environment_named_reaches_the_run() {
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&prints_the_plan());
    let output = run_building(
        &gh,
        &["--repo", REPO],
        &[(PLAN_MODEL_ENV, "claude-haiku-4-5")],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let args = gh.recorded_claude_args();
    assert!(args.contains("--model"), "{args}");
    assert!(args.contains("claude-haiku-4-5"), "{args}");
}

#[test]
fn a_model_that_opens_with_a_dash_is_a_refusal_that_costs_no_run() {
    // A variable that can put a flag on the command line of the run decides
    // what the run may do, and that decision is the reader's.
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&prints_the_plan());
    let output = run_building(
        &gh,
        &["--repo", REPO],
        &[(PLAN_MODEL_ENV, "--dangerously-skip-permissions")],
    );
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(
        stderr(&output).contains(PLAN_MODEL_ENV),
        "{}",
        stderr(&output)
    );
    assert!(gh.never_ran_claude(), "{}", gh.recorded_claude_args());
}

#[test]
fn a_run_that_names_neither_a_level_nor_a_model_asks_for_neither() {
    // The two variables cost the reader who sets neither of them nothing at
    // all, so `claude` picks both as it always did.
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&prints_the_plan());
    let output = run_building(&gh, &["--repo", REPO], &[]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let args = gh.recorded_claude_args();
    assert!(!args.contains("--effort"), "{args}");
    assert!(!args.contains("--model"), "{args}");
}

/// The tool the fake `claude` of this file reaches for first.
const FIRST_TOOL: &str = "Read";

/// The words that fake `claude` writes for that reach.
const FIRST_REACH: &str = "Read the open issues";

/// The tool it reaches for last.
const LAST_TOOL: &str = "Bash";

/// The words it writes for that reach.
const LAST_REACH: &str = "Check wn CLI flags";

/// The line of the stream that opens a run.
///
/// The reader walks past every kind of event but the two it reads, and this
/// one is a kind it walks past.
fn opens_the_run() -> String {
    serde_json::json!({
        "type": "system",
        "subtype": "init",
        "model": "claude-opus-5",
    })
    .to_string()
}

/// The line of the stream that says the run reached for `tool`, with
/// `description` as the words it wrote for that reach.
///
/// The shape is the shape of one measured run: an `assistant` event carries a
/// message, the message carries blocks, and a `tool_use` block names the tool
/// and holds what the tool was given.
fn reached_for(tool: &str, description: &str) -> String {
    serde_json::json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "name": tool,
                "input": { "description": description },
            }],
        },
    })
    .to_string()
}

/// The shell of a fake `claude` that writes the whole stream of a run at once,
/// with `document` in the `result` of its last line.
fn writes_the_stream(document: &str) -> String {
    let stream = [
        opens_the_run(),
        reached_for(FIRST_TOOL, FIRST_REACH),
        reached_for(LAST_TOOL, LAST_REACH),
        envelope(document),
    ]
    .join("\n");
    format!("cat <<'WN_FAKE_CLAUDE_STREAM'\n{stream}\nWN_FAKE_CLAUDE_STREAM\n")
}

#[test]
fn the_run_asks_for_the_stream_of_events_it_reads() {
    // The words on the line while the run works come out of that stream, and
    // `claude` writes no stream without both of these flags.
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&writes_the_stream(&undated(JSON_PLAN)));
    let output = run_building(&gh, &["--repo", REPO], &[]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let args = gh.recorded_claude_args();
    assert!(args.contains("stream-json"), "{args}");
    assert!(args.contains("--verbose"), "{args}");
}

#[test]
fn the_plan_of_a_stream_is_the_result_of_its_last_line() {
    // Standard output carries one JSON object for each event of the run, and
    // the plan is the `result` of the last of them. A reader handed the whole
    // stream gets JSON that is no plan at all.
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&writes_the_stream(&undated(JSON_PLAN)));
    let output = run_building(&gh, &["--repo", REPO], &[]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), JSON_ANSWER);
    // The events of the run are not the plan, and none of them reaches the
    // pipe the plan goes to.
    assert!(!stdout(&output).contains("tool_use"), "{}", stdout(&output));
    // The last line is still the envelope, so the run still says what it cost.
    assert!(stderr(&output).contains(REPORT), "{}", stderr(&output));
}

#[test]
fn a_stream_whose_last_line_says_the_run_failed_carries_the_reason() {
    // The envelope of a failing run stands at the end of a stream like every
    // other envelope, and the sentence a reader can act on stands in it. A
    // refusal that quoted the whole stream would hand the reader the events of
    // the run instead of the reason it stopped.
    let said = "There's an issue with the selected model (no-such-model-xyz). It may not exist \
                or you may not have access to it. Run --model to pick a different model.";
    let refused = serde_json::json!({
        "type": "result",
        "subtype": "error_during_execution",
        "is_error": true,
        "result": said,
        "total_cost_usd": 0.054_637_9,
        "duration_ms": 1886,
        "num_turns": 1,
    })
    .to_string();
    let stream = [opens_the_run(), reached_for(LAST_TOOL, LAST_REACH), refused].join("\n");
    let body = format!("cat <<'WN_FAKE_CLAUDE_STREAM'\n{stream}\nWN_FAKE_CLAUDE_STREAM\nexit 1\n");
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&body);
    let output = run_building(
        &gh,
        &["--repo", REPO],
        &[(PLAN_MODEL_ENV, "no-such-model-xyz")],
    );
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    let message = stderr(&output);
    assert!(
        message.contains("There's an issue with the selected model"),
        "{message}"
    );
    // The events of the run say nothing a reader acts on, so none of them
    // stands in the refusal.
    assert!(!message.contains("tool_use"), "{message}");
    // The run spent the money before it failed, and the reader paid for it.
    assert!(message.contains("plan: $0.05"), "{message}");
}

#[test]
fn a_stream_that_carries_no_envelope_answers_nothing() {
    // Every line of this stream is an event of the run, and none of them is
    // the line that carries the plan. An `assistant` event holds no plan, so a
    // reader that took the last line it could parse would answer with one.
    let stream = [opens_the_run(), reached_for(LAST_TOOL, LAST_REACH)].join("\n");
    let body = format!("cat <<'WN_FAKE_CLAUDE_STREAM'\n{stream}\nWN_FAKE_CLAUDE_STREAM\n");
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&body);
    let output = run_building(&gh, &["--repo", REPO], &[]);
    assert_eq!(output.status.code(), Some(2), "the run could not answer");
    assert!(stderr(&output).contains("claude"), "{}", stderr(&output));
    assert!(
        gh.sent_no_query(),
        "a run with no plan asks about no issue, and it asked {}",
        gh.recorded_args()
    );
}

// The line the tool paints while a run works.
//
// `wn` draws that line on standard error, and indicatif draws nothing at all
// when standard error is not a terminal. That is the right rule — a redirected
// run must not collect spinner frames — and it leaves a test that reads a pipe
// with nothing to read. So each run below gets a pseudo-terminal of a size this
// file chose as its standard error, and reads the painted frames back off the
// other end of it.

/// The columns of the terminal every painted run holds.
///
/// The template cuts what the run does now to the width that is left, so a
/// narrow terminal would cut the words these tests assert.
const TERMINAL_COLUMNS: u16 = 200;

/// The rows of that terminal.
///
/// The line takes one row and nothing here reads a row count. It is above zero
/// because a terminal of zero rows carries no window.
const TERMINAL_ROWS: u16 = 24;

/// The terminal the painted runs say they hold.
///
/// indicatif hides the line when `TERM` is unset or names `dumb`, and the
/// environment [`wn`] builds names no terminal at all.
const TERMINAL_KIND: &str = "xterm-256color";

/// The deadline every painted run below is given, as the line writes it.
const DEADLINE: &str = " of 10m0s";

/// The seconds the fake `claude` of a painted run holds the first reach on the
/// line, as the shell writes them.
const PAUSE: &str = "1";

/// The seconds it holds the last reach there.
///
/// Longer than [`PAUSE`], because the clock of the line reads whole seconds
/// and a test that asks whether it moved needs it to pass more than one of
/// them.
const LONGER_PAUSE: &str = "2";

/// What one run painted on its terminal, and what it answered.
struct Painted {
    /// Every frame the run painted, with the escape codes taken out.
    frames: String,
    /// The standard output and the exit status of the run.
    output: Output,
}

/// A pseudo-terminal of a size this file chose.
struct Terminal {
    /// The end a test reads the painted frames back from.
    master: OwnedFd,
    /// The end the child takes as its standard error.
    slave: OwnedFd,
}

impl Terminal {
    /// Open a pseudo-terminal `columns` columns wide.
    ///
    /// A pseudo-terminal that nobody sized reports zero columns, and the call
    /// that reads a window answers that size without an error. So the size
    /// arrives with the `openpty` call, and no window of the wrong size ever
    /// exists.
    fn open(columns: u16) -> Self {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        let mut size = libc::winsize {
            ws_row: TERMINAL_ROWS,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        // SAFETY: `openpty` writes one file descriptor to each of the first two
        // pointers, and both point at a live local variable. The two null
        // pointers are the documented way to ask for the default terminal modes
        // and to ask for no name of the slave device. The last pointer is the
        // size of the window, and it points at a live local variable that
        // outlives the call.
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        assert_eq!(
            opened,
            0,
            "openpty must give a pseudo-terminal: {}",
            std::io::Error::last_os_error()
        );

        // SAFETY: both descriptors came from the one `openpty` call above, they
        // are open, and nothing else in this process owns either of them.
        unsafe {
            Self {
                master: OwnedFd::from_raw_fd(master),
                slave: OwnedFd::from_raw_fd(slave),
            }
        }
    }
}

/// Run `wn` with its standard error on a terminal, and give back what it
/// painted there beside what it answered.
///
/// The environment is the environment [`run_building`] builds, with a terminal
/// named on top of it.
fn run_painting(gh: &FakeGh, args: &[&str], env: &[(&str, &str)]) -> Painted {
    let Terminal { master, slave } = Terminal::open(TERMINAL_COLUMNS);

    let child = {
        let mut command = wn(gh, args, "80", false, None);
        command.env_remove(NO_CLAUDE_ENV);
        command.env("TERM", TERMINAL_KIND);
        for (name, value) in env {
            command.env(name, value);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(slave));
        command.spawn().unwrap()
        // The command holds the last copy of the slave end in this process, and
        // it goes out of scope here. The read of the master end below ends when
        // the child exits, and it would never end while this process still held
        // a writer of the terminal.
    };

    let mut painted = std::fs::File::from(master);
    let frames = std::thread::spawn(move || {
        let mut read = Vec::new();
        // A terminal answers the last close of its other end with an error
        // rather than with an end of file, so what was read up to that point is
        // what the run painted.
        let _ = painted.read_to_end(&mut read);
        String::from_utf8_lossy(&read).into_owned()
    });
    let output = child.wait_with_output().unwrap();
    Painted {
        frames: testcolor::strip_ansi(&frames.join().unwrap()),
        output,
    }
}

/// The shell of a fake `claude` that writes the stream of a run one piece at a
/// time, holding each reach on the line long enough to be painted.
fn writes_the_stream_slowly(document: &str) -> String {
    let opening = [opens_the_run(), reached_for(FIRST_TOOL, FIRST_REACH)].join("\n");
    let reach = reached_for(LAST_TOOL, LAST_REACH);
    let closing = envelope(document);
    format!(
        "cat <<'WN_FAKE_CLAUDE_OPENING'\n{opening}\nWN_FAKE_CLAUDE_OPENING\n\
         sleep {PAUSE}\n\
         cat <<'WN_FAKE_CLAUDE_REACH'\n{reach}\nWN_FAKE_CLAUDE_REACH\n\
         sleep {LONGER_PAUSE}\n\
         cat <<'WN_FAKE_CLAUDE_ENVELOPE'\n{closing}\nWN_FAKE_CLAUDE_ENVELOPE\n"
    )
}

/// Every reading of the clock that `painted` holds, one of each.
///
/// The clock stands in front of [`DEADLINE`], so the reading is the last word
/// before it.
fn readings_of(painted: &str) -> Vec<String> {
    let pieces: Vec<&str> = painted.split(DEADLINE).collect();
    let mut readings: Vec<String> = pieces[..pieces.len().saturating_sub(1)]
        .iter()
        .filter_map(|piece| piece.split_whitespace().next_back())
        .map(ToString::to_string)
        .collect();
    readings.sort();
    readings.dedup();
    readings
}

#[test]
fn the_line_says_how_long_the_run_waited_and_how_long_it_may() {
    // A run that works and a run that died eight minutes ago painted the same
    // line, and the reader could not tell the two apart. The clock is what
    // tells them apart, and it only does that if it moves.
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&writes_the_stream_slowly(&undated(JSON_PLAN)));
    let painted = run_painting(&gh, &["--repo", REPO], &[]);
    assert!(
        painted.output.status.success(),
        "the run answered: {}",
        painted.frames
    );
    // The deadline stands on the line, so the reader knows what the wait is
    // measured against.
    assert!(painted.frames.contains(DEADLINE), "{}", painted.frames);
    let readings = readings_of(&painted.frames);
    assert!(
        readings.len() >= 2,
        "the clock moved while the run worked, and it read {readings:?} in {}",
        painted.frames
    );
}

#[test]
fn the_line_says_what_the_run_does_now() {
    // A steady tick moves the frame while one API call is open, so the
    // animation is no evidence that the run works. The tool the run reached for
    // is such evidence, and it is what a reader who wonders whether to kill the
    // run reads.
    let gh = FakeGh::new(JSON_ISSUES).with_claude(&writes_the_stream_slowly(&undated(JSON_PLAN)));
    let painted = run_painting(&gh, &["--repo", REPO], &[]);
    assert!(
        painted.output.status.success(),
        "the run answered: {}",
        painted.frames
    );
    for reach in [
        format!("{FIRST_TOOL}: {FIRST_REACH}"),
        format!("{LAST_TOOL}: {LAST_REACH}"),
    ] {
        assert!(painted.frames.contains(&reach), "{}", painted.frames);
    }
    // The words the line always carried still open it.
    assert!(
        painted
            .frames
            .contains("plan-parallel-work: reading the backlog"),
        "{}",
        painted.frames
    );
}

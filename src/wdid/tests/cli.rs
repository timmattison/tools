//! Tests that run the built `wdid` binary against stub programs.
//!
//! Each run gets a directory of stubs as its whole `PATH`: an `atuin` that
//! prints fixture rows, a `cargo` that fakes `install --list`, and a `git`
//! that runs the real git. The fixture workspace has two tools,
//! `wdid-fixture-one` and `wdid-fixture-two`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use chrono::{Duration, Utc};
use serde_json::Value;

/// The path of the binary that cargo built for these tests.
const BIN: &str = env!("CARGO_BIN_EXE_wdid");

/// The start of the version line: the tool name, the version, and the opening
/// parenthesis of the commit information.
const VERSION_PREFIX: &str = "wdid 0.1.0 (";

/// The tool that the stub `cargo` reports as installed.
const TOOL_ONE: &str = "wdid-fixture-one";

/// The tool that the stub `cargo` reports as not installed.
const TOOL_TWO: &str = "wdid-fixture-two";

/// The date of the commit that adds the crate `one`.
const ONE_COMMIT_DATE: &str = "2025-01-02T12:00:00Z";

/// The date of the commit that adds the crate `two`.
const TWO_COMMIT_DATE: &str = "2026-03-04T12:00:00Z";

/// The width that the table gets, so that no line wraps.
const WIDE: &str = "200";

/// The format that atuin prints for the time of a row.
const TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// The date format of `--since`.
const DATE_FORMAT: &str = "%Y-%m-%d";

/// One row that the stub `atuin` prints.
struct Row {
    /// The age of the row, in days.
    days_ago: i64,
    /// The exit code that atuin records.
    exit: i64,
    /// The command line.
    command: &'static str,
}

/// A row that ran `days_ago` days ago and succeeded.
const fn row(days_ago: i64, command: &'static str) -> Row {
    Row {
        days_ago,
        exit: 0,
        command,
    }
}

/// Quotes `text` as one word of a POSIX shell script.
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Writes an executable script into `dir`.
fn write_script(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The body of a stub `atuin` that prints `rows` as NUL-separated records.
fn atuin_rows(rows: &[Row]) -> String {
    rows.iter()
        .map(|row| {
            let time = (Utc::now() - Duration::days(row.days_ago)).format(TIME_FORMAT);
            format!(
                "printf '%s\\t%s\\thost\\tuser\\t%s\\0' {} {} {}\n",
                sh_quote(&time.to_string()),
                row.exit,
                sh_quote(row.command)
            )
        })
        .collect()
}

/// Finds the real git in the directories of the `PATH` of the test.
fn real_git() -> PathBuf {
    let path = std::env::var_os("PATH").expect("the test has a PATH");
    std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
        .expect("a git is on the PATH of the test")
}

/// Runs git in `dir` with no inherited git environment. `date` states the
/// author and committer date, after the sweep.
fn git(dir: &Path, args: &[&str], date: Option<&str>) {
    let mut command = Command::new(real_git());
    gitscratch::shed_inherited_git_environment(&mut command);
    if let Some(date) = date {
        command
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date);
    }
    let output = command
        .args([
            "-c",
            "user.name=wdid-test",
            "-c",
            "user.email=wdid-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Makes the fixture workspace in `root`: two crates and two commits.
fn write_workspace(root: &Path) {
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"one\", \"two\"]\n",
    )
    .unwrap();
    for (dir, name) in [("one", TOOL_ONE), ("two", TOOL_TWO)] {
        std::fs::create_dir_all(root.join(dir).join("src")).unwrap();
        std::fs::write(
            root.join(dir).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        std::fs::write(root.join(dir).join("src/main.rs"), "fn main() {}\n").unwrap();
    }
    git(root, &["init", "-q", "-b", "main"], None);
    git(root, &["add", "Cargo.toml", "one"], None);
    git(
        root,
        &["commit", "-q", "-m", "add one"],
        Some(ONE_COMMIT_DATE),
    );
    git(root, &["add", "two"], None);
    git(
        root,
        &["commit", "-q", "-m", "add two"],
        Some(TWO_COMMIT_DATE),
    );
}

/// What the stub `atuin` does.
enum Atuin<'a> {
    /// Print these rows.
    Rows(&'a [Row]),
    /// Write this text to standard error and exit 1.
    Fails(&'a str),
    /// The stub directory holds no `atuin`.
    Missing,
}

/// A run of `wdid`: the stub directory, a home directory, and the workspace.
struct Setup {
    /// The directory that is the whole `PATH` of the run.
    stubs: tempfile::TempDir,
    /// The home directory of the run.
    home: tempfile::TempDir,
    /// The fixture workspace.
    workspace: tempfile::TempDir,
}

impl Setup {
    /// Makes the stubs, the home, and the workspace.
    fn new(atuin: &Atuin<'_>) -> Self {
        let stubs = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write_workspace(workspace.path());
        match atuin {
            Atuin::Rows(rows) => write_script(stubs.path(), "atuin", &atuin_rows(rows)),
            Atuin::Fails(message) => write_script(
                stubs.path(),
                "atuin",
                &format!("printf '%s\\n' {} >&2\nexit 1\n", sh_quote(message)),
            ),
            Atuin::Missing => {}
        }
        write_script(
            stubs.path(),
            "cargo",
            &format!(
                "if [ \"$1\" = install ] && [ \"$2\" = --list ]; then\n\
                 \x20 printf '{TOOL_ONE} v0.1.0 (/somewhere):\\n    {TOOL_ONE}\\n'\n\
                 \x20 exit 0\n\
                 fi\n\
                 exec \"{}\" \"$@\"\n",
                env!("CARGO")
            ),
        );
        write_script(
            stubs.path(),
            "git",
            &format!("exec {} \"$@\"\n", sh_quote(&real_git().to_string_lossy())),
        );
        Self {
            stubs,
            home,
            workspace,
        }
    }

    /// The command for `wdid`, with the stub `PATH` and the width of the
    /// table. The shell is zsh. The caller adds the flags.
    fn command(&self) -> Command {
        let mut command = Command::new(BIN);
        gitscratch::shed_inherited_git_environment(&mut command);
        command
            .env("PATH", self.stubs.path())
            .env("HOME", self.home.path())
            .env_remove("CARGO_HOME")
            .env("COLUMNS", WIDE)
            .env("SHELL", "/bin/zsh")
            .stdin(Stdio::null());
        command
    }

    /// Runs `wdid --no-shell` with `--repo` at the workspace and `args`.
    fn run(&self, args: &[&str]) -> Output {
        self.command()
            .arg("--no-shell")
            .arg("--repo")
            .arg(self.workspace.path())
            .args(args)
            .output()
            .unwrap()
    }
}

/// The standard output of `output` as text.
fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// The standard error of `output` as text.
fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// The JSON document that `wdid --json` printed.
fn json(output: &Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).expect("stdout is one JSON document")
}

/// The entry of the tool `name` in the `tools` array of `document`.
fn tool<'a>(document: &'a Value, name: &str) -> &'a Value {
    document["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("no tool {name} in {document}"))
}

#[test]
fn version_flag_prints_the_name_the_version_and_the_commit() {
    let output = Command::new(BIN).arg("--version").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with(VERSION_PREFIX),
        "unexpected version line: {stdout:?}"
    );
}

#[test]
fn the_text_report_counts_a_typed_use_and_shows_the_install_state() {
    let rows = [
        row(3, "wdid-fixture-one a"),
        row(2, "echo hi | wdid-fixture-one b"),
    ];
    let setup = Setup::new(&Atuin::Rows(&rows));
    let output = setup
        .command()
        .arg("--no-shell")
        .current_dir(setup.workspace.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    let one = text
        .lines()
        .find(|line| line.contains(TOOL_ONE))
        .unwrap_or_else(|| panic!("no row for {TOOL_ONE}:\n{text}"));
    assert!(one.contains("not on PATH"), "{one}");
    assert!(one.split('┆').any(|cell| cell.trim() == "2"), "{one}");
    let two = text
        .lines()
        .find(|line| line.contains(TOOL_TWO))
        .unwrap_or_else(|| panic!("no row for {TOOL_TWO}:\n{text}"));
    assert!(two.contains("not installed"), "{two}");
    assert!(text.contains("History: 2 rows."), "{text}");
    assert!(text.contains("Never used, old"), "{text}");
    assert!(text.contains(TOOL_TWO), "{text}");
    assert!(
        text.contains("Shell: no aliases and no functions"),
        "{text}"
    );
    assert!(text.ends_with('\n'), "{text:?}");
}

/// A reader that closes the pipe before the report arrives, as `wdid | true`
/// does, ends the run with success and no message. Before, the write of the
/// report panicked and the run exited 101.
#[test]
fn a_reader_that_closes_the_pipe_early_ends_the_run_quietly() {
    let rows = [row(3, "wdid-fixture-one a")];
    let setup = Setup::new(&Atuin::Rows(&rows));
    let mut child = setup
        .command()
        .arg("--no-shell")
        .arg("--repo")
        .arg(setup.workspace.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());

    let output = child.wait_with_output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stderr(&output), "");
}

#[test]
fn json_gives_the_counts_and_the_catalog_facts() {
    let rows = [row(3, "wdid-fixture-one a"), row(2, "wdid-fixture-one b")];
    let setup = Setup::new(&Atuin::Rows(&rows));
    let document = json(&setup.run(&["--json", "--new-days", "30", "--idle-days", "7"]));
    assert_eq!(document["new_days"], 30);
    assert_eq!(document["idle_days"], 7);
    assert_eq!(document["history"]["rows"], 2);
    let one = tool(&document, TOOL_ONE);
    assert_eq!(one["uses"], 2);
    assert_eq!(one["typed"], 2);
    assert_eq!(one["installed"], true);
    assert_eq!(one["added"], "2025-01-02");
    assert_eq!(one["crate_dir"], "one");
    assert_eq!(one["classification"], "used");
    let two = tool(&document, TOOL_TWO);
    assert_eq!(two["uses"], 0);
    assert_eq!(two["installed"], false);
    assert_eq!(two["added"], "2026-03-04");
    assert_eq!(document["never_used_old"], serde_json::json!([TOOL_TWO]));
}

#[test]
fn a_missing_atuin_stops_the_run_and_names_atuin() {
    let setup = Setup::new(&Atuin::Missing);
    let output = setup.run(&[]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stdout(&output).is_empty());
    let message = stderr(&output);
    assert!(message.starts_with("wdid: "), "{message}");
    assert!(message.contains("atuin"), "{message}");
}

#[test]
fn a_failing_atuin_stops_the_run_and_shows_its_stderr() {
    let setup = Setup::new(&Atuin::Fails("database is locked"));
    let output = setup.run(&[]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stdout(&output).is_empty());
    let message = stderr(&output);
    assert!(message.starts_with("wdid: "), "{message}");
    assert!(message.contains("database is locked"), "{message}");
}

#[test]
fn an_empty_history_stops_the_run() {
    let setup = Setup::new(&Atuin::Rows(&[]));
    let output = setup.run(&[]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("no history rows"), "{output:?}");
}

#[test]
fn a_repo_that_is_not_a_workspace_stops_the_run_and_names_the_flag() {
    let setup = Setup::new(&Atuin::Rows(&[row(1, "ls")]));
    let empty = tempfile::tempdir().unwrap();
    let output = setup
        .command()
        .arg("--no-shell")
        .arg("--repo")
        .arg(empty.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stdout(&output).is_empty());
    let message = stderr(&output);
    assert!(message.starts_with("wdid: "), "{message}");
    assert!(message.contains("--repo"), "{message}");
}

#[test]
fn a_shell_that_wdid_cannot_read_stops_the_run_and_names_no_shell() {
    let setup = Setup::new(&Atuin::Rows(&[row(1, "ls")]));
    let output = setup
        .command()
        .env("SHELL", "/usr/bin/fish")
        .arg("--repo")
        .arg(setup.workspace.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let message = stderr(&output);
    assert!(message.starts_with("wdid: "), "{message}");
    assert!(message.contains("--no-shell"), "{message}");
}

#[test]
fn json_and_unparsed_conflict_and_clap_gives_exit_two() {
    let setup = Setup::new(&Atuin::Rows(&[row(1, "ls")]));
    let output = setup.run(&["--json", "--unparsed"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(stdout(&output).is_empty());
}

#[test]
fn a_since_date_that_is_not_a_date_gives_exit_two() {
    let setup = Setup::new(&Atuin::Rows(&[row(1, "ls")]));
    let output = setup.run(&["--since", "yesterday"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn unparsed_prints_the_rows_and_the_command_that_did_not_parse() {
    let rows = [
        row(3, "echo $("),
        row(2, "echo $("),
        row(2, "wdid-fixture-one a"),
    ];
    let setup = Setup::new(&Atuin::Rows(&rows));
    let output = setup.run(&["--unparsed"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "2\techo $(\n");
}

#[test]
fn since_drops_a_row_before_the_date() {
    let rows = [
        row(10, "wdid-fixture-one old"),
        row(2, "wdid-fixture-one new"),
    ];
    let setup = Setup::new(&Atuin::Rows(&rows));
    let since = (Utc::now() - Duration::days(5))
        .format(DATE_FORMAT)
        .to_string();
    let document = json(&setup.run(&["--json", "--since", &since]));
    assert_eq!(document["since"], since.as_str());
    assert_eq!(document["history"]["rows"], 2);
    assert_eq!(document["history"]["before_since"], 1);
    assert_eq!(tool(&document, TOOL_ONE)["uses"], 1);
}

#[test]
fn a_row_with_exit_127_does_not_count() {
    let rows = [
        Row {
            days_ago: 1,
            exit: 127,
            command: "wdid-fixture-one a",
        },
        row(1, "wdid-fixture-one b"),
    ];
    let setup = Setup::new(&Atuin::Rows(&rows));
    let document = json(&setup.run(&["--json"]));
    assert_eq!(document["history"]["not_found"], 1);
    assert_eq!(tool(&document, TOOL_ONE)["uses"], 1);
}

/// The zsh that the real-shell test starts.
const ZSH: &str = "/bin/zsh";

#[test]
fn a_use_through_an_alias_of_the_real_zsh_counts_for_the_tool() {
    if !Path::new(ZSH).exists() {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let setup = Setup::new(&Atuin::Rows(&[row(1, "one x")]));
    std::fs::write(
        setup.home.path().join(".zshrc"),
        format!("alias one='{TOOL_ONE} --flag'\n"),
    )
    .unwrap();
    let output = setup
        .command()
        .env("SHELL", ZSH)
        .env("ZDOTDIR", setup.home.path())
        .arg("--json")
        .arg("--repo")
        .arg(setup.workspace.path())
        .output()
        .unwrap();
    let document = json(&output);
    let one = tool(&document, TOOL_ONE);
    assert_eq!(one["uses"], 1, "{document}");
    assert_eq!(one["typed"], 0, "{document}");
    assert_eq!(
        one["through"],
        serde_json::json!([{ "name": "one", "uses": 1 }])
    );
    assert!(
        document["shell"]["aliases"].as_u64().unwrap() >= 1,
        "{document}"
    );
}

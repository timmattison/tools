//! Tests that run the built `wdid` binary against stub programs.
//!
//! Each run gets a directory of stubs as its whole `PATH`: an `atuin` that
//! prints fixture rows, a `cargo` that fakes `install --list`, and a `git`
//! that runs the real git. The fixture workspace has two tools,
//! `wdid-fixture-one` and `wdid-fixture-two`.

mod support;

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

/// The start of the names of the oh-my-zsh variables, for example `ZSH` and
/// `ZSH_CUSTOM`. `wdid defs` reads them from the shell, thus a run sheds each
/// one that the environment of `cargo test` holds.
const OH_MY_ZSH_PREFIX: &str = "ZSH";

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

/// Writes an executable script into `dir`.
fn write_script(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    gitscratch::testing::executable::write_executable(&path, format!("#!/bin/sh\n{body}")).unwrap();
}

/// The body of a stub `atuin` that prints `rows` as NUL-separated records.
fn atuin_rows(rows: &[Row]) -> String {
    rows.iter()
        .map(|row| {
            let time = (Utc::now() - Duration::days(row.days_ago)).format(TIME_FORMAT);
            format!(
                "printf '%s\\t%s\\thost\\tuser\\t%s\\0' {} {} {}\n",
                shellquote::shell_quote(&time.to_string()),
                row.exit,
                shellquote::shell_quote(row.command)
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
    let mut command = childenv::fixture_command(real_git());
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
                &format!(
                    "printf '%s\\n' {} >&2\nexit 1\n",
                    shellquote::shell_quote(message)
                ),
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
            &format!(
                "exec {} \"$@\"\n",
                shellquote::shell_quote(&real_git().to_string_lossy())
            ),
        );
        Self {
            stubs,
            home,
            workspace,
        }
    }

    /// The command for `wdid`, with the stub `PATH` and the width of the
    /// table. The shell is zsh. The command has no variable whose name starts
    /// with [`OH_MY_ZSH_PREFIX`], so the origins that `wdid defs` calls mine
    /// do not depend on the shell that starts `cargo test`. The caller adds
    /// the flags.
    fn command(&self) -> Command {
        let mut command = childenv::fixture_command(BIN);
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with(OH_MY_ZSH_PREFIX) {
                command.env_remove(name);
            }
        }
        command
            .env("PATH", self.stubs.path())
            .env("HOME", self.home.path())
            .env_remove("CARGO_HOME")
            .env_remove("CARGO_INSTALL_ROOT")
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
    let output = childenv::fixture_command(BIN)
        .arg("--version")
        .output()
        .unwrap();
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
    assert_eq!(document["workspace"]["found_by"], "repo_flag", "{document}");
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
fn a_repo_that_is_not_a_workspace_stops_the_run_and_says_why() {
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
    assert!(message.contains("is not a cargo workspace"), "{message}");
    assert!(!message.contains("--repo"), "{message}");
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
    assert_eq!(
        stderr(&output),
        "wdid: wdid reads aliases and functions from zsh and bash only, \
         and SHELL is /usr/bin/fish (wdid --no-shell skips the aliases and functions)\n"
    );
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

/// Says that the `workspace` block of `document` names the checkout of
/// `installed`, found by step 3.
fn assert_installed_workspace(document: &Value, installed: &support::Installed) {
    let workspace = &document["workspace"];
    assert_eq!(workspace["found_by"], "installed_git", "{document}");
    assert_eq!(workspace["url"], support::REPOSITORY_URL, "{document}");
    assert_eq!(workspace["commit"], installed.commit.as_str(), "{document}");
    let root = PathBuf::from(workspace["root"].as_str().unwrap());
    assert_eq!(
        root.canonicalize().unwrap(),
        installed.checkout.canonicalize().unwrap()
    );
}

/// Runs `wdid --no-shell --json` in `cwd`, with `home` as the cargo home.
fn run_in(setup: &Setup, home: &support::CargoHome, cwd: &Path) -> Output {
    setup
        .command()
        .env("CARGO_HOME", home.path())
        .arg("--no-shell")
        .arg("--json")
        .current_dir(cwd)
        .output()
        .unwrap()
}

#[test]
fn outside_each_work_tree_the_report_is_on_the_source_of_the_installed_wdid() {
    let setup = Setup::new(&Atuin::Rows(&[row(1, "wdid-fixture-one a")]));
    let installed = support::Installed::new();
    let cwd = gitscratch::testing::not_a_repository();

    let document = json(&run_in(&setup, &installed.home, cwd.path()));

    assert_eq!(tool(&document, TOOL_ONE)["uses"], 1, "{document}");
    assert!(tool(&document, TOOL_TWO)["added"].is_string(), "{document}");
    assert_installed_workspace(&document, &installed);
}

#[test]
fn in_a_work_tree_without_a_cargo_toml_the_report_is_on_the_source_of_the_installed_wdid() {
    let setup = Setup::new(&Atuin::Rows(&[row(1, "wdid-fixture-one a")]));
    let installed = support::Installed::new();
    let work_tree = gitscratch::testing::TestRepo::init();
    work_tree.commit_file("README.md", "not a workspace\n", "add a readme");

    let document = json(&run_in(&setup, &installed.home, work_tree.path()));

    assert_eq!(tool(&document, TOOL_ONE)["uses"], 1, "{document}");
    assert!(tool(&document, TOOL_TWO)["added"].is_string(), "{document}");
    assert_installed_workspace(&document, &installed);
}

/// In a cargo workspace, the report is on that workspace also when the
/// installed `wdid` came from another one.
#[test]
fn in_a_cargo_workspace_the_report_is_on_that_workspace() {
    let setup = Setup::new(&Atuin::Rows(&[row(1, "wdid-fixture-one a")]));
    let installed = support::Installed::new();

    let document = json(&run_in(&setup, &installed.home, setup.workspace.path()));

    let workspace = &document["workspace"];
    assert_eq!(workspace["found_by"], "work_tree", "{document}");
    let root = PathBuf::from(workspace["root"].as_str().unwrap());
    assert_eq!(
        root.canonicalize().unwrap(),
        setup.workspace.path().canonicalize().unwrap()
    );
}

#[test]
fn outside_each_work_tree_without_a_record_the_error_names_repo_and_the_reason() {
    let setup = Setup::new(&Atuin::Rows(&[row(1, "wdid-fixture-one a")]));
    let home = support::CargoHome::new();
    let cwd = gitscratch::testing::not_a_repository();

    let output = run_in(&setup, &home, cwd.path());

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stdout(&output).is_empty());
    let message = stderr(&output);
    assert!(message.starts_with("wdid: "), "{message}");
    assert!(message.contains("--repo <path>"), "{message}");
    assert!(
        message.contains("there is no record of the install"),
        "{message}"
    );
}

/// The `.zshrc` of the `wdid defs` tests: four kinds of alias, a function
/// that calls another function, a function that nothing calls, and a private
/// function that the report leaves out.
const DEFS_ZSHRC: &str = "alias wt='echo wt'\n\
alias wtf='wt -f'\n\
alias -g G='| cat'\n\
alias -s md=cat\n\
myf() { helper \"$@\" }\n\
helper() { echo hi }\n\
unused() { true }\n\
_private() { true }\n";

/// Writes [`DEFS_ZSHRC`] into the home directory of `setup`.
fn write_defs_zshrc(setup: &Setup) {
    std::fs::write(setup.home.path().join(".zshrc"), DEFS_ZSHRC).unwrap();
}

/// The command `wdid defs` with the real zsh, which reads the `.zshrc` of the
/// home directory of `setup`.
fn defs_command(setup: &Setup) -> Command {
    let mut command = setup.command();
    command
        .env("SHELL", ZSH)
        .env("ZDOTDIR", setup.home.path())
        .arg("defs");
    command
}

/// The entry of the definition `name` in the `definitions` array of
/// `document`.
fn definition<'a>(document: &'a Value, name: &str) -> &'a Value {
    document["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("no definition {name} in {document}"))
}

/// Says if `document` holds a definition `name`.
fn has_definition(document: &Value, name: &str) -> bool {
    document["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["name"] == name)
}

#[test]
fn defs_outside_each_work_tree_prints_the_report_and_starts_no_cargo_and_no_git() {
    if !Path::new(ZSH).exists() {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let setup = Setup::new(&Atuin::Rows(&[row(1, "wtf x"), row(2, "myf")]));
    let log_dir = tempfile::tempdir().unwrap();
    let log = log_dir.path().join("spawned.log");
    let body = format!(
        "printf '%s\\n' \"$*\" >> {}\nexit 1\n",
        shellquote::shell_quote(&log.to_string_lossy())
    );
    write_script(setup.stubs.path(), "cargo", &body);
    write_script(setup.stubs.path(), "git", &body);
    write_defs_zshrc(&setup);
    let cwd = gitscratch::testing::not_a_repository();

    let output = defs_command(&setup)
        .current_dir(cwd.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(text.contains("History: 2 rows."), "{text}");
    assert!(
        text.lines().any(|line| line.starts_with("Shell: /bin/zsh")),
        "{text}"
    );
    let wt = text
        .lines()
        .find(|line| {
            line.trim_start_matches(['\u{2502}', ' '])
                .starts_with("wt ")
        })
        .unwrap_or_else(|| panic!("no row of wt in {text}"));
    assert!(wt.contains("wtf 1"), "{wt}");
    let lines: Vec<&str> = text.lines().collect();
    let heading = lines
        .iter()
        .position(|line| line.starts_with("Never used:"))
        .unwrap_or_else(|| panic!("no Never used heading in {text}"));
    assert!(
        lines[heading + 1..].contains(&"  unused (function)"),
        "{text}"
    );
    assert!(!text.contains("_private"), "{text}");
    assert!(
        !log.exists(),
        "wdid defs started cargo or git: {}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
}

#[test]
fn defs_json_holds_each_definition_also_the_ones_with_zero_uses() {
    if !Path::new(ZSH).exists() {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let rows = [
        row(1, "wtf x"),
        row(40, "myf"),
        row(1, "ls G"),
        row(1, "README.md"),
        row(1, "echo \"wt\""),
        Row {
            days_ago: 1,
            exit: 127,
            command: "wt",
        },
    ];
    let setup = Setup::new(&Atuin::Rows(&rows));
    write_defs_zshrc(&setup);
    let cwd = gitscratch::testing::not_a_repository();

    let output = defs_command(&setup)
        .args(["--json", "--idle-days", "30"])
        .current_dir(cwd.path())
        .output()
        .unwrap();

    let document = json(&output);
    assert_eq!(document["idle_days"], 30, "{document}");
    let unused = definition(&document, "unused");
    assert_eq!(unused["uses"], 0, "{document}");
    assert_eq!(unused["kind"], "function", "{document}");
    assert_eq!(unused["classification"], "never_used_old", "{document}");
    assert!(!has_definition(&document, "_private"), "{document}");
    let wt = definition(&document, "wt");
    assert_eq!(wt["typed"], 0, "{document}");
    assert_eq!(
        wt["through"],
        serde_json::json!([{ "name": "wtf", "uses": 1 }]),
        "{document}"
    );
    let helper = definition(&document, "helper");
    assert_eq!(
        helper["through"],
        serde_json::json!([{ "name": "myf", "uses": 1 }]),
        "{document}"
    );
    assert_eq!(helper["classification"], "idle", "{document}");
    let global = definition(&document, "G");
    assert_eq!(global["kind"], "global_alias", "{document}");
    assert_eq!(global["uses"], 1, "{document}");
    let suffix = definition(&document, "*.md");
    assert_eq!(suffix["kind"], "suffix_alias", "{document}");
    assert_eq!(suffix["uses"], 1, "{document}");
    assert_eq!(document["history"]["not_found"], 1, "{document}");
    assert!(
        document["never_used"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!({ "name": "unused", "kind": "function" })),
        "{document}"
    );
    let idle = document["idle"].as_array().unwrap();
    for name in ["myf", "helper"] {
        assert!(idle.iter().any(|entry| entry["name"] == name), "{document}");
    }
    assert_eq!(document["shell"]["path"], ZSH, "{document}");
}

#[test]
fn defs_since_drops_the_earlier_rows() {
    if !Path::new(ZSH).exists() {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let setup = Setup::new(&Atuin::Rows(&[row(10, "myf"), row(1, "wtf x")]));
    write_defs_zshrc(&setup);
    let cwd = gitscratch::testing::not_a_repository();
    let since = (Utc::now() - Duration::days(5))
        .format(DATE_FORMAT)
        .to_string();

    let output = defs_command(&setup)
        .args(["--json", "--since", &since])
        .current_dir(cwd.path())
        .output()
        .unwrap();

    let document = json(&output);
    assert_eq!(document["history"]["before_since"], 1, "{document}");
    assert_eq!(document["since"], since.as_str(), "{document}");
    assert_eq!(definition(&document, "myf")["uses"], 0, "{document}");
}

#[test]
fn a_flag_of_the_tools_report_before_defs_is_a_usage_error() {
    let setup = Setup::new(&Atuin::Rows(&[row(1, "ls")]));
    let output = setup.command().args(["--json", "defs"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(stdout(&output).is_empty());
}

#[test]
fn defs_with_a_shell_that_wdid_cannot_read_stops_the_run() {
    let setup = Setup::new(&Atuin::Rows(&[row(1, "ls")]));
    let output = setup
        .command()
        .env("SHELL", "/usr/bin/fish")
        .arg("defs")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stdout(&output).is_empty());
    assert_eq!(
        stderr(&output),
        "wdid: wdid reads aliases and functions from zsh and bash only, \
         and SHELL is /usr/bin/fish\n",
        "wdid defs has no --no-shell, thus its message names no such flag"
    );
}

#[test]
fn the_help_texts_name_defs_and_the_flags_of_defs() {
    let setup = Setup::new(&Atuin::Rows(&[row(1, "ls")]));
    let top = setup.command().arg("--help").output().unwrap();
    assert!(top.status.success(), "{top:?}");
    assert!(stdout(&top).contains("defs"), "{}", stdout(&top));

    let defs = setup.command().args(["defs", "--help"]).output().unwrap();
    assert!(defs.status.success(), "{defs:?}");
    let text = stdout(&defs);
    for flag in ["--since", "--json", "--idle-days", "--all"] {
        assert!(text.contains(flag), "{text}");
    }
    assert!(!text.contains("--repo"), "{text}");
    assert!(
        text.contains("run without a typed name"),
        "the help of defs names each group of the report: {text}"
    );
    assert!(
        text.contains("duplicates and shadows"),
        "the help of defs names the group of the findings: {text}"
    );
    assert!(
        text.contains("each finding"),
        "the help of --all says that it shows each finding: {text}"
    );
    let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        words.contains("one or more of my definitions or one of my places"),
        "the help of --all gives the rule of Finding::mine: a place of mine \
         also makes a finding mine: {text}"
    );
    assert!(
        text.contains("ZSH_CUSTOM"),
        "the help of --all names each place that Places::is_mine calls mine: {text}"
    );
}

/// The `.zshrc` of the origin tests of `wdid defs`: an alias of mine on line
/// 2, a stand-in of oh-my-zsh in `$HOME/omz` with one plugin, and a private
/// function.
const ORIGIN_ZSHRC: &str = "export ZSH=$HOME/omz\n\
alias wt='echo wt'\n\
source $ZSH/plugins/git/git.plugin.zsh\n\
_private() { true }\n";

/// The plugin file of [`ORIGIN_ZSHRC`], below the home directory.
const ORIGIN_PLUGIN: &str = "omz/plugins/git/git.plugin.zsh";

/// Writes [`ORIGIN_ZSHRC`] and its plugin into the home directory of
/// `setup`.
fn write_origin_home(setup: &Setup) {
    std::fs::write(setup.home.path().join(".zshrc"), ORIGIN_ZSHRC).unwrap();
    let plugin = setup.home.path().join(ORIGIN_PLUGIN);
    std::fs::create_dir_all(plugin.parent().unwrap()).unwrap();
    std::fs::write(plugin, "alias gst='git status'\n").unwrap();
}

/// Runs `wdid defs` with `args` on the home of [`write_origin_home`], outside
/// each work tree. The history has one use of `gst` and one of `wt`.
fn run_origin_defs(args: &[&str]) -> (Setup, Output) {
    let setup = Setup::new(&Atuin::Rows(&[row(1, "gst"), row(1, "wt")]));
    write_origin_home(&setup);
    let cwd = gitscratch::testing::not_a_repository();
    let output = defs_command(&setup)
        .args(args)
        .current_dir(cwd.path())
        .output()
        .unwrap();
    (setup, output)
}

/// Gives the first table row of `text` whose name cell is `name`.
fn table_row<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines().find(|line| {
        line.strip_prefix('\u{2502}')
            .and_then(|line| line.split('\u{2506}').next())
            .is_some_and(|cell| cell.trim() == name)
    })
}

#[test]
fn defs_shows_only_my_definitions_with_their_origins() {
    if !Path::new(ZSH).exists() {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let (_setup, output) = run_origin_defs(&[]);
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    let wt = table_row(&text, "wt").unwrap_or_else(|| panic!("no row of wt in {text}"));
    assert!(wt.contains("~/.zshrc:2"), "{wt}");
    assert!(table_row(&text, "gst").is_none(), "{text}");
    assert!(!text.contains("git.plugin.zsh"), "{text}");
    assert!(!text.contains("_private"), "{text}");
    assert!(
        text.lines().any(|line| line.starts_with("Definitions: ")
            && line.ends_with(" others (--all shows the others).")),
        "{text}"
    );
}

#[test]
fn defs_all_groups_the_others_by_origin_and_shows_the_private_functions() {
    if !Path::new(ZSH).exists() {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let (_setup, output) = run_origin_defs(&["--all"]);
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(
        text.lines().any(|line| line.starts_with("Mine: ")),
        "{text}"
    );
    assert!(
        text.contains(&format!("\n~/{ORIGIN_PLUGIN}: 1 definition\n")),
        "{text}"
    );
    let gst = table_row(&text, "gst").unwrap_or_else(|| panic!("no row of gst in {text}"));
    assert!(gst.contains(&format!("~/{ORIGIN_PLUGIN}:1")), "{gst}");
    assert!(table_row(&text, "_private").is_some(), "{text}");
    assert!(table_row(&text, "wt").is_some(), "{text}");
}

#[test]
fn defs_json_gives_the_origin_and_mine_of_each_definition() {
    if !Path::new(ZSH).exists() {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let (setup, output) = run_origin_defs(&["--json"]);
    let document = json(&output);
    let home = setup.home.path();
    let wt = definition(&document, "wt");
    assert_eq!(
        wt["origin"],
        serde_json::json!({ "file": home.join(".zshrc"), "line": 2 }),
        "{document}"
    );
    assert_eq!(wt["mine"], true, "{document}");
    let gst = definition(&document, "gst");
    assert_eq!(
        gst["origin"],
        serde_json::json!({ "file": home.join(ORIGIN_PLUGIN), "line": 1 }),
        "{document}"
    );
    assert_eq!(gst["mine"], false, "{document}");
    assert_eq!(gst["uses"], 1, "{document}");
    assert!(!has_definition(&document, "_private"), "{document}");
    let entries = document["definitions"].as_array().unwrap();
    let mine = entries.iter().filter(|entry| entry["mine"] == true).count();
    assert_eq!(document["shell"]["mine"], mine, "{document}");
    assert_eq!(
        document["shell"]["others"],
        entries.len() - mine,
        "{document}"
    );
}

/// The temporary `ZDOTDIR` and the wrapper function `alias` change no count
/// of the tools report. An alias in `.zshenv`, an alias and a function in
/// `.zshrc`, and a typed tool each count as they did before them.
#[test]
fn the_tools_report_counts_through_the_zshenv_and_the_zshrc() {
    if !Path::new(ZSH).exists() {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let setup = Setup::new(&Atuin::Rows(&[
        row(1, "envone"),
        row(1, "one x"),
        row(1, "two"),
        row(1, "wdid-fixture-one"),
    ]));
    std::fs::write(
        setup.home.path().join(".zshenv"),
        format!("alias envone='{TOOL_ONE}'\n"),
    )
    .unwrap();
    std::fs::write(
        setup.home.path().join(".zshrc"),
        format!("alias one='{TOOL_ONE} --flag'\ntwo() {{ {TOOL_TWO} \"$@\" }}\n"),
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
    assert_eq!(one["uses"], 3, "{document}");
    assert_eq!(one["typed"], 1, "{document}");
    assert_eq!(
        one["through"],
        serde_json::json!([{ "name": "envone", "uses": 1 }, { "name": "one", "uses": 1 }]),
        "{document}"
    );
    let two = tool(&document, TOOL_TWO);
    assert_eq!(two["uses"], 1, "{document}");
    assert_eq!(
        two["through"],
        serde_json::json!([{ "name": "two", "uses": 1 }]),
        "{document}"
    );
}

/// The bash of the tests of `wdid defs` that read a `.bashrc`.
const BASH: &str = "/bin/bash";

/// The `.zshrc` of the tests of the definitions that zsh runs without a typed
/// name. Each block defines one start, and `unused` has none.
const STARTS_ZSHRC: &str = "precmd() { prompt_helper }\n\
prompt_helper() { true }\n\
mychpwd() { true }\n\
autoload -Uz add-zsh-hook\n\
add-zsh-hook chpwd mychpwd\n\
TRAPINT() { return 128 }\n\
searchfn() { true }\n\
zle -N history-search-wdid searchfn\n\
bindkey '^R' history-search-wdid\n\
lonelyfn() { true }\n\
zle -N lonely-widget lonelyfn\n\
zle-line-init() { true }\n\
zle -N zle-line-init\n\
kmsel() { true }\n\
autoload -Uz add-zle-hook-widget\n\
add-zle-hook-widget keymap-select kmsel\n\
unused() { true }\n";

/// The `.bashrc` of the test of the definitions that bash runs without a
/// typed name.
const STARTS_BASHRC: &str = "pcfn() { pc_helper; }\n\
pc_helper() { true; }\n\
PROMPT_COMMAND='pcfn'\n\
trapfn() { true; }\n\
trap 'trapfn' EXIT\n\
unused() { true; }\n";

/// Gives the reasons of the definition `name` in `document`, as text.
fn reasons_of(document: &Value, name: &str) -> Vec<String> {
    definition(document, name)["reasons"]
        .as_array()
        .unwrap_or_else(|| panic!("no reasons of {name} in {document}"))
        .iter()
        .map(|reason| reason.as_str().unwrap().to_owned())
        .collect()
}

/// Says if the list `group` of `document` holds the definition `name`.
fn listed_in(document: &Value, group: &str, name: &str) -> bool {
    document[group]
        .as_array()
        .unwrap_or_else(|| panic!("no list {group} in {document}"))
        .iter()
        .any(|entry| entry["name"] == name)
}

/// Gives the lines of the group `heading` of the text report: the lines after
/// the heading line that start with two spaces.
fn group_of<'a>(text: &'a str, heading: &str) -> Vec<&'a str> {
    text.lines()
        .skip_while(|line| !line.starts_with(heading))
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .collect()
}

#[test]
fn defs_gives_the_functions_that_zsh_runs_without_a_typed_name_their_state() {
    if !Path::new(ZSH).exists() {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let setup = Setup::new(&Atuin::Rows(&[row(1, "ls")]));
    std::fs::write(setup.home.path().join(".zshrc"), STARTS_ZSHRC).unwrap();
    let cwd = gitscratch::testing::not_a_repository();

    let output = defs_command(&setup)
        .arg("--json")
        .current_dir(cwd.path())
        .output()
        .unwrap();

    let document = json(&output);
    for (name, reason) in [
        ("precmd", "precmd hook"),
        ("prompt_helper", "precmd hook, through precmd"),
        ("mychpwd", "chpwd hook"),
        ("TRAPINT", "INT trap"),
        ("searchfn", "widget history-search-wdid, key ^R"),
        ("zle-line-init", "widget zle-line-init"),
        ("kmsel", "widget zle-keymap-select"),
    ] {
        let entry = definition(&document, name);
        assert_eq!(
            entry["classification"], "runs_without_typing",
            "{name}: {document}"
        );
        assert_eq!(reasons_of(&document, name), [reason], "{name}: {document}");
        assert!(listed_in(&document, "runs_without_typing", name), "{name}");
        assert!(!listed_in(&document, "never_used", name), "{name}");
        assert!(!listed_in(&document, "idle", name), "{name}");
    }
    let lonely = definition(&document, "lonelyfn");
    assert_eq!(lonely["classification"], "never_used_old", "{document}");
    assert_eq!(reasons_of(&document, "lonelyfn"), ["widget, no key"]);
    assert!(listed_in(&document, "never_used", "lonelyfn"), "{document}");
    assert!(reasons_of(&document, "unused").is_empty(), "{document}");
    assert!(listed_in(&document, "never_used", "unused"), "{document}");

    let output = defs_command(&setup)
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    let never = group_of(&text, "Never used:");
    assert!(
        never.contains(&"  lonelyfn (function, widget, no key)"),
        "{text}"
    );
    assert!(never.contains(&"  unused (function)"), "{text}");
    assert!(!never.iter().any(|line| line.contains("precmd")), "{text}");
    let starts = group_of(&text, "Runs without typing:");
    assert!(
        starts.contains(&"  precmd (function, precmd hook)"),
        "{text}"
    );
    assert!(
        starts.contains(&"  searchfn (function, widget history-search-wdid, key ^R)"),
        "{text}"
    );
}

#[test]
fn defs_gives_the_functions_that_bash_runs_without_a_typed_name_their_state() {
    if !Path::new(BASH).exists() {
        eprintln!("skipped: bash is not installed");
        return;
    }
    let setup = Setup::new(&Atuin::Rows(&[row(1, "ls")]));
    std::fs::write(setup.home.path().join(".bashrc"), STARTS_BASHRC).unwrap();
    let cwd = gitscratch::testing::not_a_repository();
    // The script of bash runs `tail`, which the stub directory does not hold.
    let search_path = format!("{}:/usr/bin:/bin", setup.stubs.path().display());

    let output = setup
        .command()
        .env("SHELL", BASH)
        .env("PATH", search_path)
        .args(["defs", "--json"])
        .current_dir(cwd.path())
        .output()
        .unwrap();

    let document = json(&output);
    for (name, reason) in [
        ("pcfn", "PROMPT_COMMAND"),
        ("pc_helper", "PROMPT_COMMAND, through pcfn"),
        ("trapfn", "EXIT trap"),
    ] {
        let entry = definition(&document, name);
        assert_eq!(
            entry["classification"], "runs_without_typing",
            "{name}: {document}"
        );
        assert_eq!(reasons_of(&document, name), [reason], "{name}: {document}");
        assert!(!listed_in(&document, "never_used", name), "{name}");
    }
    assert!(listed_in(&document, "never_used", "unused"), "{document}");
}

/// The `.zshrc` of the tests of the duplicates and the shadows. Each name
/// starts with `wd`, so a program of the machine seldom shares it. Line 3 is
/// `wdgs`, line 5 is `wdgl`, and line 7 is the first `wdll`.
const FINDINGS_ZSHRC: &str = "export ZSH=$HOME/omz\n\
path=($HOME/bin $path)\n\
alias wdgs='git status'\n\
source $ZSH/plugins/git/git.plugin.zsh\n\
alias wdgl='git log'\n\
wdglog() { git log \"$@\" }\n\
alias wdll='ls -l'\n\
source $HOME/more.zsh\n\
wdx() { true }\n\
alias wdx='echo x'\n\
alias wdhidden='echo hi'\n\
alias wdwrapped='wdwrapped --flag'\n\
wdcmd() { command wdcmd --flag \"$@\" }\n";

/// The plugin of [`FINDINGS_ZSHRC`]. `wdgst` has the same expansion as my
/// `wdgs`. `wdgco` and `wdgcheck` share `git checkout`, and no alias of that
/// finding is mine, so only `--all` shows it.
const FINDINGS_PLUGIN: &str = "alias wdgst='git status'\n\
alias wdgco='git checkout'\n\
alias wdgcheck='git checkout'\n";

/// The file that defines `wdll` a second time, in the home directory.
const FINDINGS_MORE: &str = "alias wdll='ls -la'\n";

/// The programs in the `bin` directory of the home that
/// [`FINDINGS_ZSHRC`] puts first on the path.
const FINDINGS_PROGRAMS: [&str; 3] = ["wdhidden", "wdwrapped", "wdcmd"];

/// The heading of the group of the duplicates and the shadows.
const FINDINGS_HEADING: &str = "Duplicates and shadows:";

/// Runs `wdid defs` with `args` on the home of [`FINDINGS_ZSHRC`], outside each
/// work tree. The history uses `wdgs` twice, `wdglog` once, and `wdll` once.
fn run_findings_defs(args: &[&str]) -> (Setup, Output) {
    let setup = Setup::new(&Atuin::Rows(&[
        row(1, "wdgs"),
        row(2, "wdgs"),
        row(1, "wdglog"),
        row(1, "wdll"),
    ]));
    let home = setup.home.path();
    std::fs::write(home.join(".zshrc"), FINDINGS_ZSHRC).unwrap();
    std::fs::write(home.join("more.zsh"), FINDINGS_MORE).unwrap();
    let plugin = home.join(ORIGIN_PLUGIN);
    std::fs::create_dir_all(plugin.parent().unwrap()).unwrap();
    std::fs::write(plugin, FINDINGS_PLUGIN).unwrap();
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for program in FINDINGS_PROGRAMS {
        write_script(&bin, program, "");
    }
    let cwd = gitscratch::testing::not_a_repository();
    let output = defs_command(&setup)
        .args(args)
        .current_dir(cwd.path())
        .output()
        .unwrap();
    (setup, output)
}

/// Gives the lines of the group of the duplicates and the shadows in `text`:
/// the indented lines after its heading.
fn findings_lines(text: &str) -> Vec<&str> {
    let mut lines = text.lines().skip_while(|line| *line != FINDINGS_HEADING);
    assert!(lines.next().is_some(), "no findings group in:\n{text}");
    lines.take_while(|line| line.starts_with("  ")).collect()
}

#[test]
fn defs_lists_the_duplicates_and_the_shadows_of_my_definitions() {
    if !Path::new(ZSH).exists() {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let (_setup, output) = run_findings_defs(&[]);
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    let lines = findings_lines(&text);
    let has = |entry: &str| lines.iter().any(|line| *line == format!("  {entry}"));
    let plugin = format!("~/{ORIGIN_PLUGIN}");
    assert!(
        has(&format!(
            "Same expansion (git status): wdgs (alias, ~/.zshrc:3, 2 uses), wdgst (alias, {plugin}:1, 0 uses)"
        )),
        "{text}"
    );
    assert!(
        has("Alias and function (git log): wdgl (alias, ~/.zshrc:5, 0 uses), wdglog (function, ~/.zshrc, 1 use)"),
        "{text}"
    );
    assert!(
        has("Defined 2 times: wdll (alias, 1 use) at ~/.zshrc:7, ~/more.zsh:1 (in effect)"),
        "{text}"
    );
    assert!(
        has("Alias hides function: wdx (alias, ~/.zshrc:10, 0 uses), wdx (function, ~/.zshrc, 0 uses)"),
        "{text}"
    );
    assert!(
        has("Hides a program (~/bin/wdhidden): wdhidden (alias, ~/.zshrc:11, 0 uses)"),
        "{text}"
    );
    let block = lines.join("\n");
    assert!(!block.contains("wdwrapped"), "{text}");
    assert!(!block.contains("wdcmd"), "{text}");
    assert!(!block.contains("wdgcheck"), "{text}");
}

#[test]
fn defs_all_also_lists_the_findings_of_the_others() {
    if !Path::new(ZSH).exists() {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let (_setup, output) = run_findings_defs(&["--all"]);
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    let lines = findings_lines(&text);
    let plugin = format!("~/{ORIGIN_PLUGIN}");
    let others = format!(
        "  Same expansion (git checkout): wdgcheck (alias, {plugin}:3, 0 uses), wdgco (alias, {plugin}:2, 0 uses)"
    );
    assert!(lines.iter().any(|line| *line == others), "{text}");
}

#[test]
fn defs_json_lists_each_kind_of_finding_and_marks_the_others() {
    if !Path::new(ZSH).exists() {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let (_setup, output) = run_findings_defs(&["--json"]);
    let document = json(&output);
    let found = document["findings"].as_array().unwrap();
    let names = |entry: &Value| -> Vec<String> {
        entry["definitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|member| member["name"].as_str().unwrap().to_string())
            .collect()
    };
    for (kind, first) in [
        ("same_expansion", "wdgs"),
        ("alias_and_function", "wdgl"),
        ("defined_more_than_once", "wdll"),
        ("alias_hides_function", "wdx"),
        ("hides_program", "wdhidden"),
    ] {
        let entry = found
            .iter()
            .find(|entry| entry["kind"] == kind && names(entry).iter().any(|name| name == first))
            .unwrap_or_else(|| panic!("no {kind} finding of {first} in {document}"));
        assert_eq!(entry["mine"], true, "{entry}");
    }
    let others = found
        .iter()
        .find(|entry| {
            entry["kind"] == "same_expansion" && names(entry).iter().any(|name| name == "wdgcheck")
        })
        .unwrap_or_else(|| panic!("no finding of wdgcheck in {document}"));
    assert_eq!(others["mine"], false, "{others}");
    assert_eq!(others["words"], serde_json::json!(["git", "checkout"]));
    let mut members = names(others);
    members.sort();
    assert_eq!(members, ["wdgcheck", "wdgco"]);
}

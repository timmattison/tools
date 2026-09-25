//! End-to-end proof that `nwt` removed its tmux support, which is issue #527.
//!
//! `nwt` used to open a new worktree in a new tmux window, through the `--tmux`
//! flag and the `tmux` key of `~/.nwt.toml`. That support is gone. The tests
//! here run the real binary and show that each old way to ask for tmux now
//! stops before `nwt` makes anything.
//!
//! Each run gets a fresh, empty home directory. `nwt` reads `~/.nwt.toml` from
//! the home directory, so the configuration of whoever runs the suite cannot
//! change a result here.

mod support;

use std::path::Path;
use std::process::Output;

use support::{git_stdout, init_repo, nanos, nwt_command};
use tempfile::TempDir;

/// The exit code clap gives a usage error, such as an argument it does not
/// know.
const USAGE_ERROR: i32 = 2;

/// The exit code `nwt` gives when it cannot use `~/.nwt.toml`. It is
/// `exit_codes::CONFIG_ERROR` in the binary.
const CONFIG_ERROR: i32 = 12;

/// The name of the configuration file `nwt` reads from the home directory.
const CONFIG_FILE_NAME: &str = ".nwt.toml";

/// The part of the refusal that says why the `tmux` key stops the run.
const NO_LONGER_SUPPORTED: &str = "nwt no longer supports tmux";

/// The part of the refusal that tells the user what to do.
const DELETE_THE_KEY: &str = "Delete the `tmux` key";

/// The prefix of the line that names a worktree in `git worktree list
/// --porcelain`.
const WORKTREE_LINE_PREFIX: &str = "worktree ";

/// Run `nwt` with `args` in `repo`, with `home` as the home directory of the
/// child, and hand back everything the run produced.
///
/// `home` keeps the run away from the `~/.nwt.toml` of whoever runs the suite.
/// A test that needs a configuration file writes it into `home` first.
fn run_nwt_in_home(repo: &Path, home: &Path, args: &[&str]) -> Output {
    nwt_command(repo)
        .args(args)
        .env("HOME", home)
        .output()
        .expect("run the nwt binary")
}

/// Standard output and standard error of `output`, for an assertion message.
fn shown(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

/// Every worktree the repository at `repo` holds.
fn listed_worktrees(repo: &Path) -> Vec<String> {
    git_stdout(repo, &["worktree", "list", "--porcelain"])
        .lines()
        .filter_map(|line| line.strip_prefix(WORKTREE_LINE_PREFIX))
        .map(str::to_string)
        .collect()
}

/// The names of everything directly under `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .expect("read one directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// `nwt --tmux` is an argument clap does not know, so the run stops with a
/// usage error and makes nothing.
///
/// Before #527 the flag was real. The test harness removes `TMUX` from the
/// child, so the old binary stopped with its own "not running inside tmux"
/// error instead, which is exit code 13.
#[test]
fn tmux_flag_is_a_usage_error() {
    let (temp, repo) = init_repo();
    let home = TempDir::new().expect("create the home directory of the run");

    let output = run_nwt_in_home(&repo, home.path(), &["--tmux"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(USAGE_ERROR),
        "nwt --tmux must be a usage error:\n{}",
        shown(&output),
    );
    assert!(
        stderr.contains("unexpected argument '--tmux'"),
        "clap must name --tmux as an unexpected argument:\n{}",
        shown(&output),
    );
    assert_eq!(
        listed_worktrees(&repo).len(),
        1,
        "nwt --tmux must add no worktree:\n{}",
        shown(&output),
    );
    assert_eq!(
        entries(temp.path()),
        vec!["repo".to_string()],
        "nwt --tmux must make no directory beside the repository:\n{}",
        shown(&output),
    );
}

/// Write `tmux = <value>` into `~/.nwt.toml` of a fresh home directory, run
/// `nwt` with a new branch, and assert that the run stops before it makes
/// anything.
///
/// The key is gone, so every value is refused. The refusal says why and names
/// the file to edit, and it uses the exit code of a configuration error.
fn assert_refuses_tmux_key(value: &str) {
    let (temp, repo) = init_repo();
    let home = TempDir::new().expect("create the home directory of the run");
    let config_path = home.path().join(CONFIG_FILE_NAME);
    std::fs::write(&config_path, format!("tmux = {value}\n"))
        .unwrap_or_else(|e| panic!("write {}: {e}", config_path.display()));

    let branch = format!("tmux-key-{}-{}", std::process::id(), nanos());
    let output = run_nwt_in_home(
        &repo,
        home.path(),
        &["-b", &branch, "--no-copy-env", "--no-bootstrap-hooks"],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(CONFIG_ERROR),
        "`tmux = {value}` in ~/.nwt.toml must be a configuration error:\n{}",
        shown(&output),
    );
    assert!(
        stderr.contains(NO_LONGER_SUPPORTED),
        "the refusal of `tmux = {value}` must say that nwt no longer supports tmux:\n{}",
        shown(&output),
    );
    assert!(
        stderr.contains(DELETE_THE_KEY),
        "the refusal of `tmux = {value}` must tell the user to delete the key:\n{}",
        shown(&output),
    );
    assert!(
        stderr.contains(&config_path.display().to_string()),
        "the refusal of `tmux = {value}` must name {}:\n{}",
        config_path.display(),
        shown(&output),
    );
    assert_eq!(
        listed_worktrees(&repo).len(),
        1,
        "`tmux = {value}` must add no worktree:\n{}",
        shown(&output),
    );
    assert_eq!(
        entries(temp.path()),
        vec!["repo".to_string()],
        "`tmux = {value}` must make no directory beside the repository:\n{}",
        shown(&output),
    );
}

/// `tmux = true` asked for the old tmux window, so it is refused.
///
/// Before #527 the harness removed `TMUX` from the child, so the old binary
/// stopped with "not running inside tmux", which is exit code 13.
#[test]
fn tmux_key_true_is_refused() {
    assert_refuses_tmux_key("true");
}

/// `tmux = false` asks for nothing, and it is still refused. A key that does
/// nothing hides from the user that the setting is gone.
///
/// Before #527 this value parsed, and the run made a worktree.
#[test]
fn tmux_key_false_is_refused() {
    assert_refuses_tmux_key("false");
}

/// A `tmux` value of the wrong type gets the same refusal, not a parse error
/// about the type of a key that no longer exists.
///
/// Before #527 serde refused the string with an "invalid type" message, which
/// also exits 12 but does not say that the key is gone.
#[test]
fn tmux_key_of_any_type_is_refused() {
    assert_refuses_tmux_key("\"yes\"");
}

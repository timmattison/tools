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

use support::{git_stdout, init_repo, nwt_command};
use tempfile::TempDir;

/// The exit code clap gives a usage error, such as an argument it does not
/// know.
const USAGE_ERROR: i32 = 2;

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

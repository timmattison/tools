//! Proof that the home directory of whoever runs the suite cannot change the
//! result of a test.
//!
//! `nwt` reads `~/.nwt.toml`, and the git it runs reads `~/.gitconfig` and
//! `$XDG_CONFIG_HOME/git/config`. A child that inherits `HOME` and
//! `XDG_CONFIG_HOME` thus reads the configuration of the developer. One key in
//! one of those files stops every run: a `~/.nwt.toml` that nwt cannot parse,
//! or an empty `nwt.worktreesDir`, gives exit code 12. The suite then fails on
//! one machine and passes on the next, for a reason no test states.
//!
//! The test here puts such a configuration into all three places, points this
//! process at them, and runs `nwt` through `support::nwt_command`. The run must
//! make its worktree as if no configuration existed.
//!
//! This file holds a SINGLE `#[test]`. It changes the environment of the whole
//! process, and cargo runs the tests of one binary on parallel threads. One
//! test in the binary means no sibling thread reads the changed values.

mod support;

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Output;

use support::{git_stdout, init_repo, nanos, nwt_command};
use tempfile::TempDir;

/// The variable that names the home directory.
const HOME: &str = "HOME";

/// The variable that names the base directory of the user configuration. Git
/// reads `$XDG_CONFIG_HOME/git/config` when it is set.
const XDG_CONFIG_HOME: &str = "XDG_CONFIG_HOME";

/// A `~/.nwt.toml` that nwt cannot parse. nwt refuses it with exit code 12.
const UNPARSABLE_NWT_CONFIG: &str = "this line is not TOML\n";

/// A git configuration that sets `nwt.worktreesDir` to an empty value. nwt
/// refuses a value that names no directory with exit code 12.
const EMPTY_WORKTREES_DIR_GIT_CONFIG: &str = "[nwt]\n\tworktreesDir =\n";

/// The prefix of the line that names a worktree in `git worktree list
/// --porcelain`.
const WORKTREE_LINE_PREFIX: &str = "worktree ";

/// Standard output and standard error of `output`, for an assertion message.
fn shown(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

/// The worktrees git lists for `repo`.
fn listed_worktrees(repo: &Path) -> Vec<String> {
    git_stdout(repo, &["worktree", "list", "--porcelain"])
        .lines()
        .filter(|line| line.starts_with(WORKTREE_LINE_PREFIX))
        .map(str::to_owned)
        .collect()
}

/// Write `contents` to `path`, and make the directories above it first.
fn write_config(path: &Path, contents: &str) {
    let parent = path.parent().expect("a config path has a parent");
    fs::create_dir_all(parent).expect("create the directory of a config file");
    fs::write(path, contents).expect("write a config file");
}

/// Put `value` back as the value of `key`, or remove `key` when it had none.
fn restore(key: &str, value: Option<OsString>) {
    match value {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
}

#[test]
fn the_home_of_the_developer_cannot_change_a_run() {
    let home = TempDir::new().expect("create the home directory of the developer");
    write_config(&home.path().join(".nwt.toml"), UNPARSABLE_NWT_CONFIG);
    write_config(
        &home.path().join(".gitconfig"),
        EMPTY_WORKTREES_DIR_GIT_CONFIG,
    );
    let xdg = TempDir::new().expect("create the configuration base directory");
    write_config(
        &xdg.path().join("git").join("config"),
        EMPTY_WORKTREES_DIR_GIT_CONFIG,
    );

    // Make the fixture before the environment changes, so its own git reads
    // none of the configuration above.
    let (_temp, repo) = init_repo();
    let branch = format!("isolated-home-{}-{}", std::process::id(), nanos());

    let saved_home = std::env::var_os(HOME);
    let saved_xdg = std::env::var_os(XDG_CONFIG_HOME);
    std::env::set_var(HOME, home.path());
    std::env::set_var(XDG_CONFIG_HOME, xdg.path());

    let result = nwt_command(&repo)
        .args(["-b", &branch, "--no-copy-env", "--no-bootstrap-hooks"])
        .output();

    // Put the environment back before any assertion. A panic leaves the
    // changed values in place for the rest of the process.
    restore(HOME, saved_home);
    restore(XDG_CONFIG_HOME, saved_xdg);

    let output = result.expect("run the nwt binary");
    assert_eq!(
        output.status.code(),
        Some(0),
        "a configuration in the home of the developer must not reach the nwt \
         child, but the run failed:\n{}",
        shown(&output),
    );
    assert_eq!(
        listed_worktrees(&repo).len(),
        2,
        "the run must make its worktree:\n{}",
        shown(&output),
    );
}

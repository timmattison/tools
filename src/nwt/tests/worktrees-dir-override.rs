//! End-to-end coverage for `nwt.worktreesDir`, the git configuration key that
//! names where a repository keeps its worktrees.
//!
//! `nwt` puts every new worktree in `<main worktree>-worktrees`, beside the main
//! worktree. That default is right for a normal repository and for a repository
//! whose git directory is detached from its work tree, but it is not right for
//! every layout. A repository states its own answer with this key, and the key
//! lives in the repository configuration, which `yadm` already tracks.
//!
//! The default itself stays under test in `tests/detached-git-dir.rs`, which
//! runs `nwt` with the key unset in both repository shapes.

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

use gitscratch::testing::DetachedGitDirRepo;
use support::{git_stdout, init_repo, nanos, nwt_command, run_git};
use tempfile::TempDir;

/// The git configuration key that names the worktrees directory.
const WORKTREES_DIR_KEY: &str = "nwt.worktreesDir";

/// The exit code `nwt` gives a configuration error, from its `exit_codes`
/// module.
const CONFIG_ERROR: i32 = 12;

/// The prefix of the line that names a worktree in `git worktree list
/// --porcelain`.
const WORKTREE_LINE_PREFIX: &str = "worktree ";

/// Resolve a path before an assertion reads it.
///
/// Git prints resolved paths, and every fixture lives under a temporary
/// directory that macOS reaches through a symbolic link: `/var` resolves to
/// `/private/var`.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
}

/// A branch name that no concurrent copy of this suite can also hold.
///
/// Two `cargo test` runs share one machine, and a branch name is a shared
/// resource. The process id and a nanosecond clock reading keep them apart.
fn unique_branch(label: &str) -> String {
    format!("{label}-{}-{}", std::process::id(), nanos())
}

/// Run `nwt -b <branch>` in `from`, and hand back everything the run produced.
///
/// The two flags keep the run short and silent. `--no-copy-env` stops a walk of
/// the repository for `.env` files, and `--no-bootstrap-hooks` stops a package
/// manager install. Neither one has anything to do with where the worktree
/// lands.
///
/// `home`, when it is there, becomes the home directory of the child. Git
/// expands a leading `~` in a path value against that directory, so a test of
/// the expansion names a temporary directory and never writes into the home
/// directory of whoever runs the suite.
fn run_nwt(from: &Path, branch: &str, home: Option<&Path>) -> Output {
    let mut command = nwt_command(from);
    command.args(["-b", branch, "--no-copy-env", "--no-bootstrap-hooks"]);
    if let Some(home) = home {
        command.env("HOME", home);
    }
    command.output().expect("run the nwt binary")
}

/// Run `nwt -b <branch>` in `from`, and hand back the directory it made.
fn created_worktree(from: &Path, branch: &str) -> PathBuf {
    created_worktree_under_home(from, branch, None)
}

/// Run `nwt -b <branch>` in `from` under the home directory `home`, and hand
/// back the directory it made.
fn created_worktree_under_home(from: &Path, branch: &str, home: Option<&Path>) -> PathBuf {
    let output = run_nwt(from, branch, home);

    assert!(
        output.status.success(),
        "nwt -b {branch} failed in {}:\n{}\n{}",
        from.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let printed = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string());
    assert!(
        printed.is_dir(),
        "nwt printed {}, which is no directory",
        printed.display()
    );

    printed
}

/// Point the repository at `worktrees_dir` through the configuration key.
fn set_override(repo: &Path, worktrees_dir: &str) {
    assert!(
        run_git(repo, &["config", WORKTREES_DIR_KEY, worktrees_dir]),
        "git config {WORKTREES_DIR_KEY} failed in {}",
        repo.display()
    );
}

/// Check a new `branch` out into a linked worktree of `repo` at `path`.
///
/// A run from a linked worktree stands somewhere other than the main worktree.
/// That is what tells the two candidate answers apart when the stated value is
/// relative.
fn add_linked_worktree(repo: &Path, path: &Path, branch: &str) {
    assert!(
        run_git(
            repo,
            &[
                "worktree",
                "add",
                path.to_str().expect("utf-8 worktree path"),
                "-b",
                branch,
            ],
        ),
        "git worktree add failed in {}",
        repo.display()
    );
}

#[test]
fn an_absolute_value_places_the_worktree_in_a_detached_repository() {
    let repo = DetachedGitDirRepo::nested();
    let elsewhere = TempDir::new().expect("create the directory that holds the override");
    let worktrees_dir = elsewhere.path().join("stated-worktrees");
    repo.git(&[
        "config",
        WORKTREES_DIR_KEY,
        worktrees_dir.to_str().expect("utf-8 override path"),
    ]);
    let branch = unique_branch("detached-absolute");

    let created = created_worktree(repo.git_dir(), &branch);

    assert_eq!(
        canonical(&created),
        canonical(elsewhere.path())
            .join("stated-worktrees")
            .join(&branch),
        "nwt must obey {WORKTREES_DIR_KEY} in a repository with a detached git directory"
    );
}

#[test]
fn an_absolute_value_places_the_worktree_in_a_normal_repository() {
    let (temp, repo) = init_repo();
    let worktrees_dir = temp.path().join("elsewhere").join("stated-worktrees");
    set_override(&repo, worktrees_dir.to_str().expect("utf-8 override path"));
    let branch = unique_branch("normal-absolute");

    let created = created_worktree(&repo, &branch);

    assert_eq!(
        canonical(&created),
        canonical(temp.path())
            .join("elsewhere")
            .join("stated-worktrees")
            .join(&branch),
        "nwt must obey {WORKTREES_DIR_KEY} in a normal repository"
    );
}

#[test]
fn a_relative_value_resolves_against_the_main_worktree() {
    let (temp, repo) = init_repo();
    set_override(&repo, "stated-worktrees");
    let linked = temp.path().join("linked");
    add_linked_worktree(&repo, &linked, &unique_branch("relative-linked"));
    let branch = unique_branch("relative");

    // The run starts in the linked worktree, so the current directory and the
    // main worktree are two different answers.
    let created = created_worktree(&linked, &branch);

    assert_eq!(
        canonical(&created),
        canonical(&repo).join("stated-worktrees").join(&branch),
        "a relative {WORKTREES_DIR_KEY} must resolve against the main worktree"
    );
}

#[test]
fn a_leading_tilde_expands_to_the_home_directory() {
    let (_temp, repo) = init_repo();
    set_override(&repo, "~/stated-worktrees");
    let home = TempDir::new().expect("create the home directory of the run");
    let branch = unique_branch("tilde");

    let created = created_worktree_under_home(&repo, &branch, Some(home.path()));

    assert_eq!(
        canonical(&created),
        canonical(home.path())
            .join("stated-worktrees")
            .join(&branch),
        "a leading tilde in {WORKTREES_DIR_KEY} must expand to the home directory"
    );
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

/// Run `nwt -b <branch>` in `repo` with a `value` that names no directory, and
/// prove the run stops without making anything.
fn assert_refuses_the_value(value: &str, label: &str) {
    let (temp, repo) = init_repo();
    set_override(&repo, value);
    let branch = unique_branch(label);

    let output = run_nwt(&repo, &branch, None);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_eq!(
        output.status.code(),
        Some(CONFIG_ERROR),
        "nwt must refuse a {WORKTREES_DIR_KEY} that names no directory:\n{}\n{stderr}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        stderr.contains(WORKTREES_DIR_KEY),
        "the message must name the key the user has to fix, but it reads:\n{stderr}"
    );
    assert_eq!(
        entries(temp.path()),
        vec!["repo".to_string()],
        "a refused run must make no directory beside the repository"
    );
    assert_eq!(
        listed_worktrees(&repo).len(),
        1,
        "a refused run must add no worktree"
    );
}

#[test]
fn an_empty_value_is_refused() {
    assert_refuses_the_value("", "empty");
}

#[test]
fn a_value_of_only_whitespace_is_refused() {
    assert_refuses_the_value("   ", "whitespace");
}

#[test]
fn a_value_git_cannot_expand_is_refused() {
    // The home directory of a user who does not exist. Git refuses to expand
    // it, and the process id and the clock keep the name away from every real
    // user of the machine.
    let stranger = format!(
        "~nwt-no-such-user-{}-{}/worktrees",
        std::process::id(),
        nanos()
    );

    assert_refuses_the_value(&stranger, "unknown-user");
}

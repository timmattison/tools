//! A child directory that is a repository but owns no worktrees.
//!
//! `git init --bare <child>/.git` makes such a directory: the scan for children
//! accepts it, because `.git` is there, but `git worktree list --porcelain`
//! answers with a `worktree` line and `bare` and no `HEAD`, so the repository
//! contributes nothing to list. It must not be able to answer for another
//! repository, and it must not be able to crash `cwt`.

// These mirror the crate-root attributes in src/main.rs. A crate-root attribute
// reaches only its own target, so the binary raising them does nothing for this
// test target; repeating them here is what keeps the whole crate under one lint
// set now that they no longer live in a manifest `[lints]` table.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use std::path::{Path, PathBuf};

use support::{code, combined, cwt, git, stdout, Family};

use tempfile::TempDir;

/// The exit code `cwt --help` publishes for a name it cannot find.
const WORKTREE_NOT_FOUND: i32 = 3;

/// An anchor repository with children below it, built fresh for one test.
///
/// The shared [`Family`] fixture is deliberately left alone: several tests
/// assert its listing exactly, so a bare child added there would be read as a
/// change to those expectations.
struct Anchor {
    /// Kept alive so the temp directory outlives the test.
    _tmp: TempDir,
    /// The canonical path of the temp directory. Canonical because git prints
    /// resolved paths, and on macOS the temp directory is reached through a
    /// symbolic link.
    root: PathBuf,
}

impl Anchor {
    /// An anchor repository on branch `main`, with nothing below it yet.
    fn new() -> Self {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let root = tmp
            .path()
            .canonicalize()
            .expect("failed to canonicalize temp dir");
        make_repo(&root.join("anchor"), "main");
        Self { _tmp: tmp, root }
    }

    /// Add a normal child repository named `name`, on branch `branch`.
    fn child(&self, name: &str, branch: &str) -> &Self {
        make_repo(&self.at(name), branch);
        self
    }

    /// Add a child directory whose `.git` is itself a bare repository, so it
    /// looks like a repository to the scan and lists no worktrees.
    fn empty_child(&self, name: &str) -> &Self {
        let dir = self.at(name);
        std::fs::create_dir_all(&dir).expect("failed to create empty child dir");
        git(&dir, &["init", "--bare", ".git"]);
        self
    }

    /// Resolve a path below the anchor, for example `z-real`.
    fn at(&self, relative: &str) -> PathBuf {
        self.root.join("anchor").join(relative)
    }

    /// The anchor repository itself, which is where these tests run `cwt`.
    fn root(&self) -> PathBuf {
        self.root.join("anchor")
    }

    /// The path of a directory below the anchor, as `cwt` would print it.
    fn path_of(&self, relative: &str) -> String {
        self.at(relative).display().to_string()
    }
}

/// Create a repository at `path` whose first branch is `branch`, with one
/// commit so `git worktree list` reports a real HEAD.
fn make_repo(path: &Path, branch: &str) {
    std::fs::create_dir_all(path).expect("failed to create repo dir");
    git(path, &["init", "--initial-branch", branch]);
    std::fs::write(path.join("README.md"), "fixture\n").expect("failed to write README");
    git(path, &["add", "README.md"]);
    git(path, &["commit", "--no-verify", "-m", "init"]);
}

#[test]
fn a_repository_with_no_worktrees_never_answers_for_another_one() {
    // `a-empty` sorts before `z-real`, so a repository that mistakes one for the
    // other lands the user in `z-real` and says nothing about it.
    let anchor = Anchor::new();
    anchor.empty_child("a-empty").child("z-real", "trunk");

    let output = cwt(&anchor.root(), &["a-empty:"]);

    assert_ne!(
        stdout(&output).trim_end(),
        anchor.path_of("z-real"),
        "a repository with no worktrees must never answer with another repository's path"
    );
    assert_eq!(
        code(&output),
        WORKTREE_NOT_FOUND,
        "a repository with no worktrees has nothing to select: {}",
        combined(&output)
    );
    assert_eq!(
        stdout(&output),
        "",
        "a name that cannot be resolved prints no path, or the shell function changes directory"
    );
}

#[test]
fn a_repository_with_no_worktrees_does_not_crash_when_it_is_last() {
    // The only child lists no worktrees, so its group is the last one and every
    // index it could hold is one past the end of the entries.
    let anchor = Anchor::new();
    anchor.empty_child("oddchild");

    let output = cwt(&anchor.root(), &["oddchild:"]);

    assert_eq!(
        code(&output),
        WORKTREE_NOT_FOUND,
        "cwt must report the name instead of panicking: {}",
        combined(&output)
    );
    assert_eq!(
        stdout(&output),
        "",
        "a name that cannot be resolved prints no path"
    );
}

#[test]
fn the_listing_reports_the_repository_it_left_out() {
    // Leaving a repository out silently is the other half of the problem: the
    // user is never told why the directory they can see is not in the list.
    let anchor = Anchor::new();
    anchor.empty_child("a-empty").child("z-real", "trunk");

    let output = cwt(&anchor.root(), &[]);

    assert_eq!(code(&output), 0, "cwt failed: {}", combined(&output));
    assert!(
        stdout(&output).contains(&anchor.path_of("z-real")),
        "the readable repositories still list: {}",
        stdout(&output)
    );

    let message = combined(&output);
    assert!(
        message.contains("Warning: skipped"),
        "a repository left out of the family goes to the warnings channel: {message}"
    );
    assert!(
        message.contains("a-empty"),
        "the warning has to name the repository it left out: {message}"
    );
}

#[test]
fn a_family_of_readable_repositories_is_unchanged() {
    // The guard against empty repositories must not cost a normal family
    // anything, so the shared fixture still answers a bare prefix.
    let family = Family::build();
    let output = cwt(&family.at("family"), &["child-b:"]);

    assert_eq!(
        code(&output),
        0,
        "cwt child-b: failed: {}",
        combined(&output)
    );
    assert_eq!(
        stdout(&output).trim_end(),
        family.path_of("family/child-b"),
        "a repository that has worktrees still selects its main one"
    );
}

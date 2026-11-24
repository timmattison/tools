//! The walk, read through the public API.
//!
//! Every tree here goes under `tempfile::tempdir()`, so two copies of this file
//! running at once never read each other's fixtures, and nothing here shells
//! out — no `git`, no subprocess at all — so no environment of the caller can
//! point a step of it at a real repository.
//!
//! The ignore-file test writes a fixture under a name no global ignore file
//! would plausibly list. A name such as `ignored.rs` could be excluded by the
//! configuration of whoever runs the test, and the test would then pass or fail
//! for a reason that has nothing to do with the walk.

use cdva::{walk, WalkOptions};
use std::path::{Path, PathBuf};

/// The name the ignore-file test hides. It is deliberately unmistakable: no
/// global `.gitignore` names it, so the verdict of that test is the verdict of
/// this walk and of nothing else.
const IGNORED_NAME: &str = "cdva-ignored-fixture.rs";

/// Writes one file, making the directories above it.
fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the fixture directory is made");
    }
    std::fs::write(&path, contents).expect("the fixture file is written");
}

/// The relative half of each pair, as a sorted list of strings.
fn relatives(found: &[(PathBuf, PathBuf)]) -> Vec<String> {
    found
        .iter()
        .map(|(_, relative)| relative.to_string_lossy().replace('\\', "/"))
        .collect()
}

#[test]
fn an_ignored_file_is_skipped_by_default_and_no_ignore_takes_it_back() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "keep.rs", "fn main() {}\n");
    write(root.path(), IGNORED_NAME, "fn ignored() {}\n");
    write(root.path(), ".gitignore", &format!("{IGNORED_NAME}\n"));

    let default =
        walk(&[root.path().to_path_buf()], WalkOptions::default()).expect("the fixture tree walks");
    assert_eq!(
        relatives(&default),
        vec!["keep.rs".to_string()],
        "a .gitignore holds even where no .git is beside it"
    );

    let everything = walk(
        &[root.path().to_path_buf()],
        WalkOptions {
            hidden: true,
            no_ignore: true,
        },
    )
    .expect("the fixture tree walks");
    assert_eq!(
        relatives(&everything),
        vec![
            ".gitignore".to_string(),
            IGNORED_NAME.to_string(),
            "keep.rs".to_string(),
        ],
        "no_ignore takes the ignored file back"
    );
}

#[test]
fn a_hidden_file_is_skipped_by_default_and_hidden_takes_it_back() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "shown.rs", "fn main() {}\n");
    write(root.path(), ".hidden.rs", "fn hidden() {}\n");

    let default =
        walk(&[root.path().to_path_buf()], WalkOptions::default()).expect("the fixture tree walks");
    assert_eq!(relatives(&default), vec!["shown.rs".to_string()]);

    let with_hidden = walk(
        &[root.path().to_path_buf()],
        WalkOptions {
            hidden: true,
            no_ignore: false,
        },
    )
    .expect("the fixture tree walks");
    assert_eq!(
        relatives(&with_hidden),
        vec![".hidden.rs".to_string(), "shown.rs".to_string()]
    );
}

#[test]
fn the_walk_yields_files_only_and_sorts_them() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "zebra/last.rs", "fn last() {}\n");
    write(root.path(), "alpha/first.rs", "fn first() {}\n");
    write(root.path(), "middle.rs", "fn middle() {}\n");
    std::fs::create_dir_all(root.path().join("empty-directory"))
        .expect("the empty directory is made");

    let found =
        walk(&[root.path().to_path_buf()], WalkOptions::default()).expect("the fixture tree walks");

    assert_eq!(
        relatives(&found),
        vec![
            "alpha/first.rs".to_string(),
            "middle.rs".to_string(),
            "zebra/last.rs".to_string(),
        ],
        "a directory is never yielded, and the order is the order of the paths"
    );

    for (path, _) in &found {
        assert!(path.is_file(), "{} is a file", path.display());
    }

    let paths: Vec<&PathBuf> = found.iter().map(|(path, _)| path).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "the walk sorts what it found");
}

#[test]
fn two_roots_both_contribute_and_each_path_is_relative_to_its_own_root() {
    let first = tempfile::tempdir().expect("a temporary directory is made");
    let second = tempfile::tempdir().expect("a temporary directory is made");
    write(first.path(), "src/one.rs", "fn one() {}\n");
    write(second.path(), "src/two.rs", "fn two() {}\n");

    let found = walk(
        &[first.path().to_path_buf(), second.path().to_path_buf()],
        WalkOptions::default(),
    )
    .expect("both fixture trees walk");

    let mut names = relatives(&found);
    names.sort();
    assert_eq!(
        names,
        vec!["src/one.rs".to_string(), "src/two.rs".to_string()],
        "each file is named relative to the root that found it"
    );

    let absolute: Vec<&PathBuf> = found.iter().map(|(path, _)| path).collect();
    assert!(absolute.contains(&&first.path().join("src/one.rs")));
    assert!(absolute.contains(&&second.path().join("src/two.rs")));
}

#[test]
fn a_root_that_is_a_file_yields_that_file() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/only.rs", "fn only() {}\n");
    write(root.path(), "src/other.rs", "fn other() {}\n");
    let only = root.path().join("src/only.rs");

    let found =
        walk(std::slice::from_ref(&only), WalkOptions::default()).expect("the single file walks");

    assert_eq!(found.len(), 1, "exactly the file that was named");
    assert_eq!(found[0].0, only);
    assert_eq!(
        found[0].1,
        PathBuf::from("only.rs"),
        "a file named on its own answers with its own name"
    );
}

#[test]
fn a_root_that_does_not_exist_is_an_error_that_names_it() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    let missing = root.path().join("no-such-directory");

    let error = walk(std::slice::from_ref(&missing), WalkOptions::default())
        .expect_err("a root that is not there cannot be walked");

    assert!(
        error.to_string().contains("no-such-directory"),
        "the error names the missing root: {error}"
    );
}

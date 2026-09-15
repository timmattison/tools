//! Guard tests for `repo_guards::doc_placement::misplaced`.
//!
//! The first test runs the guard on this repository. Every other test builds a
//! small tree in its own `tempfile::TempDir`, whose name the OS makes unique, so
//! two copies of this file can run at the same time. A fixture that expects no
//! finding also holds one misplaced control file, so a walk that reads nothing
//! cannot pass it.

use std::fs;
use std::path::{Path, PathBuf};

use repo_guards::doc_placement;
use tempfile::TempDir;

/// A misplaced file that each "not flagged" fixture also holds. The walk must
/// find it, so a walk that reads nothing fails the fixture.
const CONTROL: &str = "a/specs/control.md";

/// Absolute, canonical path to this repository's root, derived from the crate
/// being compiled rather than the working directory (which `cargo test` does
/// not pin).
fn repo_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::canonicalize(&root)
        .unwrap_or_else(|e| panic!("cannot canonicalize {}: {e}", root.display()))
}

/// Build a tree that holds one empty file at each path in `files`.
fn tree(files: &[&str]) -> TempDir {
    let dir = TempDir::new().expect("a temp dir");
    for relative in files {
        let path = dir.path().join(relative);
        let parent = path.parent().expect("a path with a parent");
        fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("cannot create {}: {e}", parent.display()));
        fs::write(&path, "").unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
    }
    dir
}

/// The misplaced files of a tree that holds `files`.
fn misplaced_in(files: &[&str]) -> Vec<PathBuf> {
    let dir = tree(files);
    doc_placement::misplaced(dir.path()).expect("the walk reads every directory")
}

/// `relative` as paths, for comparison with a result.
fn paths(relative: &[&str]) -> Vec<PathBuf> {
    relative.iter().map(PathBuf::from).collect()
}

#[test]
fn no_spec_or_plan_of_this_repository_is_below_the_root() {
    let root = repo_root();
    assert!(
        root.join("Cargo.toml").is_file() && root.join("specs").is_dir(),
        "{} holds no Cargo.toml or no specs/ directory, so it is not the workspace root \
         and a clean scan of it proves nothing",
        root.display()
    );

    let found = doc_placement::misplaced(&root).expect("the walk reads every directory");

    assert!(found.is_empty(), "{}", doc_placement::remediation(&found));
}

#[test]
fn a_spec_under_docs_superpowers_is_flagged() {
    assert_eq!(
        misplaced_in(&["docs/superpowers/specs/x.md"]),
        paths(&["docs/superpowers/specs/x.md"])
    );
}

#[test]
fn a_plan_under_docs_superpowers_is_flagged() {
    assert_eq!(
        misplaced_in(&["docs/superpowers/plans/x.md"]),
        paths(&["docs/superpowers/plans/x.md"])
    );
}

#[test]
fn a_file_in_any_nested_specs_directory_is_flagged() {
    assert_eq!(
        misplaced_in(&["a/b/specs/x.md"]),
        paths(&["a/b/specs/x.md"])
    );
}

#[test]
fn every_misplaced_file_comes_back_sorted_and_relative() {
    // `a/plans/sub/y.md` is two levels below its `plans` directory, so the
    // walk must read every directory above a file, not only its parent.
    assert_eq!(
        misplaced_in(&["z/specs/x.md", "a/plans/sub/y.md", "m/specs/w.md"]),
        paths(&["a/plans/sub/y.md", "m/specs/w.md", "z/specs/x.md"])
    );
}

#[test]
fn specs_and_plans_at_the_root_are_not_flagged() {
    assert_eq!(
        misplaced_in(&["specs/x.md", "plans/x.md", CONTROL]),
        paths(&[CONTROL])
    );
}

#[test]
fn build_output_installed_packages_and_git_metadata_are_not_walked() {
    assert_eq!(
        misplaced_in(&[
            "target/x/specs/y.md",
            "node_modules/x/plans/y.md",
            "src/tool/node_modules/x/plans/y.md",
            ".git/x/specs/y.md",
            CONTROL,
        ]),
        paths(&[CONTROL])
    );
}

#[test]
fn a_file_named_specs_or_plans_is_not_flagged() {
    assert_eq!(
        misplaced_in(&["a/b/specs", "a/plans", CONTROL]),
        paths(&[CONTROL])
    );
}

#[test]
fn an_empty_misplaced_directory_is_not_flagged() {
    let dir = tree(&[CONTROL]);
    let empty = dir.path().join("docs/superpowers/specs");
    fs::create_dir_all(&empty).unwrap_or_else(|e| panic!("cannot create {}: {e}", empty.display()));

    let found = doc_placement::misplaced(dir.path()).expect("the walk reads every directory");

    assert_eq!(found, paths(&[CONTROL]));
}

#[test]
fn the_remediation_names_each_file_and_the_move() {
    let found = misplaced_in(&["docs/superpowers/specs/x.md", "docs/superpowers/plans/y.md"]);

    let message = doc_placement::remediation(&found);

    for expected in [
        "docs/superpowers/specs/x.md",
        "docs/superpowers/plans/y.md",
        "git mv",
        "`specs/`",
        "`plans/`",
    ] {
        assert!(
            message.contains(expected),
            "the message does not contain {expected:?}:\n{message}"
        );
    }
}

#[cfg(unix)]
#[test]
fn an_unreadable_directory_is_an_error() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tree(&["locked/specs/x.md"]);
    let locked = dir.path().join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock the directory");

    let result = doc_placement::misplaced(dir.path());
    // Unlock before any assertion, so that the TempDir can remove the tree.
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("unlock the directory");

    let error =
        result.expect_err("a directory the walk cannot read is an error, never a clean result");
    assert!(
        error.to_string().contains("locked"),
        "the error names the directory: {error}"
    );
}

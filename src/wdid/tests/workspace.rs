//! Tests of the search for the workspace: `--repo`, the git work tree of the
//! current directory, and the source of the installed binary.
//!
//! Each test of step 3 uses a cargo home in a temporary directory, with a
//! fixture records file and a fixture checkout that `gitscratch` builds.

mod support;

use std::path::{Path, PathBuf};

use gitscratch::testing::{not_a_repository, TestRepo};
use support::{
    git_key, short, workspace_repository, CargoHome, BINARY, CHECKOUT_DIRECTORY, REPOSITORY_URL,
};
use wdid::catalog::{InstallLocation, Programs};
use wdid::workspace::{find, FoundBy, Workspace};

/// A commit that no fixture repository holds.
const OTHER_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// Canonicalizes a path for a comparison.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

/// Runs [`find`] without `--repo`, from `cwd`, with `location`.
fn find_from(cwd: &Path, location: &InstallLocation<'_>) -> Workspace {
    find(None, cwd, &Programs::default(), location, BINARY)
        .unwrap_or_else(|error| panic!("no workspace: {error}"))
}

/// A cargo home with a `git+` record of [`BINARY`] and the checkout of its
/// commit, beside the records of other binaries.
struct Installed {
    /// The cargo home.
    home: CargoHome,
    /// The checkout of the commit of the record.
    checkout: PathBuf,
    /// The full commit of the record.
    commit: String,
}

impl Installed {
    /// Makes the cargo home. The records file holds the record of
    /// [`BINARY`], a `git+` record of another binary at another commit, and a
    /// record from the registry.
    fn new() -> Self {
        let source = workspace_repository();
        let commit = source.rev_parse("HEAD");
        let home = CargoHome::new();
        let checkout = home.add_checkout(&source, CHECKOUT_DIRECTORY, short(&commit));
        home.write_records(&[
            (&git_key("gsw", REPOSITORY_URL, OTHER_COMMIT), &["gsw"]),
            (&git_key(BINARY, REPOSITORY_URL, &commit), &[BINARY]),
            (
                "ripgrep 14.1.1 (registry+https://github.com/rust-lang/crates.io-index)",
                &["rg"],
            ),
        ]);
        Self {
            home,
            checkout,
            commit,
        }
    }

    /// Says that `workspace` is the checkout, found by step 3.
    fn assert_found(&self, workspace: &Workspace) {
        assert_eq!(canonical(&workspace.root), canonical(&self.checkout));
        let FoundBy::InstalledGit { url, commit } = &workspace.found_by else {
            panic!("not found by the git record: {workspace:?}");
        };
        assert_eq!(url, REPOSITORY_URL);
        assert_eq!(commit.as_str(), self.commit);
        assert_eq!(commit.short(), short(&self.commit));
    }
}

#[test]
fn outside_each_work_tree_a_git_record_gives_the_checkout_of_its_commit() {
    let installed = Installed::new();
    let cwd = not_a_repository();

    let workspace = find_from(cwd.path(), &installed.home.location());

    installed.assert_found(&workspace);
}

#[test]
fn in_a_work_tree_without_a_cargo_toml_a_git_record_gives_the_checkout_of_its_commit() {
    let installed = Installed::new();
    let work_tree = TestRepo::init();
    work_tree.commit_file("README.md", "not a workspace\n", "add a readme");

    let workspace = find_from(work_tree.path(), &installed.home.location());

    installed.assert_found(&workspace);
}

/// cargo writes the records into the install root, which `CARGO_INSTALL_ROOT`
/// moves, and it keeps the checkouts in the cargo home.
#[test]
fn the_records_file_is_in_the_install_root_and_the_checkout_is_in_the_cargo_home() {
    let source = workspace_repository();
    let commit = source.rev_parse("HEAD");
    let home = CargoHome::new();
    let checkout = home.add_checkout(&source, CHECKOUT_DIRECTORY, short(&commit));
    let install_root = tempfile::tempdir().unwrap();
    support::write_records(
        install_root.path(),
        &[(&git_key(BINARY, REPOSITORY_URL, &commit), &[BINARY])],
    );
    let location = InstallLocation {
        install_root: Some(install_root.path().as_os_str()),
        ..home.location()
    };
    let cwd = not_a_repository();

    let workspace = find_from(cwd.path(), &location);

    assert_eq!(canonical(&workspace.root), canonical(&checkout));
}

/// Without `CARGO_HOME`, the cargo home is `.cargo` in the home directory.
#[test]
fn without_cargo_home_the_cargo_home_is_in_the_home_directory() {
    let installed = Installed::new();
    let home = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(installed.home.path(), home.path().join(".cargo")).unwrap();
    let location = InstallLocation {
        home: Some(home.path()),
        ..InstallLocation::default()
    };
    let cwd = not_a_repository();

    let workspace = find_from(cwd.path(), &location);

    installed.assert_found(&workspace);
}

/// libgit2 makes the short form of a commit longer than seven characters when
/// seven characters are not unique. The name of the checkout directory is then
/// longer too.
#[test]
fn a_checkout_directory_with_more_than_seven_characters_of_the_commit_counts() {
    let source = workspace_repository();
    let commit = source.rev_parse("HEAD");
    let home = CargoHome::new();
    let longer = commit.get(..9).unwrap();
    let checkout = home.add_checkout(&source, CHECKOUT_DIRECTORY, longer);
    home.write_records(&[(&git_key(BINARY, REPOSITORY_URL, &commit), &[BINARY])]);
    let cwd = not_a_repository();

    let workspace = find_from(cwd.path(), &home.location());

    assert_eq!(canonical(&workspace.root), canonical(&checkout));
}

/// cargo names the checkout directory after the last part of the canonical
/// URL. That URL has no `.git` suffix, no query, and, on GitHub, no capital
/// letters.
#[test]
fn the_checkout_directory_matches_the_canonical_name_of_the_repository() {
    let source = workspace_repository();
    let commit = source.rev_parse("HEAD");
    let home = CargoHome::new();
    let checkout = home.add_checkout(&source, CHECKOUT_DIRECTORY, short(&commit));
    let url = "https://github.com/TimMattison/Tools.git?branch=main";
    home.write_records(&[(&git_key(BINARY, url, &commit), &[BINARY])]);
    let cwd = not_a_repository();

    let workspace = find_from(cwd.path(), &home.location());

    assert_eq!(canonical(&workspace.root), canonical(&checkout));
    let FoundBy::InstalledGit { url: found, .. } = &workspace.found_by else {
        panic!("not found by the git record: {workspace:?}");
    };
    assert_eq!(found, url);
}

/// A checkout of another repository with the same short commit is not the
/// source, because its directory has another name.
#[test]
fn a_checkout_of_another_repository_does_not_count() {
    let installed = Installed::new();
    let decoy = workspace_repository();
    installed
        .home
        .add_checkout(&decoy, "other-0123456789abcdef", short(&installed.commit));

    let cwd = not_a_repository();
    let workspace = find_from(cwd.path(), &installed.home.location());

    installed.assert_found(&workspace);
}

/// Step 2 comes before step 3: in a cargo workspace, `wdid` reports on that
/// workspace also when a record names another one.
#[test]
fn in_a_cargo_workspace_the_work_tree_wins_over_the_record() {
    let installed = Installed::new();
    let other = workspace_repository();
    let cwd = other.path().join("one").join("src");

    let workspace = find_from(&cwd, &installed.home.location());

    assert_eq!(canonical(&workspace.root), canonical(other.path()));
    assert_eq!(workspace.found_by, FoundBy::WorkTree);
}

/// Step 1 comes before each other step.
#[test]
fn with_repo_the_flag_wins_over_the_work_tree_and_the_record() {
    let installed = Installed::new();
    let other = workspace_repository();
    let flagged = workspace_repository();

    let workspace = find(
        Some(flagged.path()),
        other.path(),
        &Programs::default(),
        &installed.home.location(),
        BINARY,
    )
    .unwrap();

    assert_eq!(workspace.root, canonical(flagged.path()));
    assert_eq!(workspace.found_by, FoundBy::RepoFlag);
}

/// A `--repo` that is not a cargo workspace is an error. `wdid` does not try
/// the record then, because the person named the workspace.
#[test]
fn a_repo_that_is_not_a_workspace_is_an_error_also_with_a_record() {
    let installed = Installed::new();
    let empty = tempfile::tempdir().unwrap();

    let error = find(
        Some(empty.path()),
        empty.path(),
        &Programs::default(),
        &installed.home.location(),
        BINARY,
    )
    .unwrap_err();

    assert!(error.to_string().contains("--repo <path>"), "{error}");
}

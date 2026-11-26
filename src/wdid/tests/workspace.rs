//! Tests of the search for the workspace: `--repo`, the git work tree of the
//! current directory, and the source of the installed binary.
//!
//! Each test of step 3 uses a cargo home in a temporary directory, with a
//! fixture records file and a fixture checkout that `gitscratch` builds.

mod support;

use std::path::{Path, PathBuf};

use gitscratch::testing::{not_a_repository, TestRepo};
use support::{
    git_key, short, workspace_repository, CargoHome, Installed, BINARY, CHECKOUT_DIRECTORY,
    OTHER_COMMIT, REPOSITORY_URL,
};
use wdid::catalog::{InstallLocation, Programs};
use wdid::workspace::{find, FoundBy, Workspace};

/// Canonicalizes a path for a comparison.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

/// Runs [`find`] without `--repo`, from `cwd`, with `location`.
fn find_from(cwd: &Path, location: &InstallLocation<'_>) -> Workspace {
    find(None, cwd, &Programs::default(), location, BINARY)
        .unwrap_or_else(|error| panic!("no workspace: {error}"))
}

impl Installed {
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

/// A checkout that misses does not stop the search. Here the first directory
/// holds no repository, and the second is at another commit. Both sort before
/// the directory that holds the checkout of the commit.
#[test]
fn a_checkout_that_misses_does_not_hide_a_later_checkout_of_the_commit() {
    let installed = Installed::new();
    let name = short(&installed.commit);
    let checkouts = installed.home.path().join("git").join("checkouts");
    std::fs::create_dir_all(checkouts.join("tools-0000000000000000").join(name)).unwrap();
    let other = TestRepo::init();
    other.commit_file("README.md", "other\n", "add a readme");
    installed
        .home
        .add_checkout(&other, "tools-0000000000000001", name);

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

    assert_eq!(
        error.to_string(),
        format!(
            "{} is not a cargo workspace (it has no Cargo.toml)",
            canonical(empty.path()).display()
        )
    );
}

/// Gives the key of a `path+file` record of `package` in `crate_dir`, as cargo
/// writes it: a file URL, with a percent escape for each byte outside the
/// safe set.
fn path_key(package: &str, crate_dir: &Path) -> String {
    let url = url::Url::from_file_path(crate_dir).unwrap();
    format!("{package} 0.1.0 (path+{url})")
}

/// Makes a cargo home whose records file holds only `key`, for [`BINARY`].
fn home_with_record(key: &str) -> CargoHome {
    let home = CargoHome::new();
    home.write_records(&[(key, &[BINARY])]);
    home
}

#[test]
fn a_path_record_gives_the_workspace_that_holds_its_crate_directory() {
    let source = workspace_repository();
    let crate_dir = source.path().join("one");
    let home = home_with_record(&path_key(BINARY, &crate_dir));
    let cwd = not_a_repository();

    let workspace = find_from(cwd.path(), &home.location());

    assert_eq!(canonical(&workspace.root), canonical(source.path()));
    assert_eq!(workspace.found_by, FoundBy::InstalledPath { crate_dir });
}

/// A crate that is not a member of a workspace is its own workspace.
#[test]
fn a_path_record_of_a_crate_without_a_workspace_gives_the_crate_directory() {
    let crate_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(crate_dir.path().join("src")).unwrap();
    std::fs::write(
        crate_dir.path().join("Cargo.toml"),
        "[package]\nname = \"wdid-fixture-alone\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(crate_dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    let home = home_with_record(&path_key(BINARY, crate_dir.path()));
    let cwd = not_a_repository();

    let workspace = find_from(cwd.path(), &home.location());

    assert_eq!(canonical(&workspace.root), canonical(crate_dir.path()));
}

/// cargo writes a space and each multibyte character of the path as percent
/// escapes. wdid reads them back.
#[test]
fn a_path_record_with_a_space_and_multibyte_characters_in_the_path_counts() {
    let root = tempfile::tempdir().unwrap();
    let member = "日本語 🎉 café";
    std::fs::write(
        root.path().join("Cargo.toml"),
        format!("[workspace]\nresolver = \"2\"\nmembers = [\"{member}\"]\n"),
    )
    .unwrap();
    let crate_dir = root.path().join(member);
    std::fs::create_dir_all(crate_dir.join("src")).unwrap();
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        "[package]\nname = \"wdid-fixture-multibyte\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(crate_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    let key = path_key(BINARY, &crate_dir);
    assert!(key.contains("%20"), "the fixture key has no escape: {key}");
    let home = home_with_record(&key);
    let cwd = not_a_repository();

    let workspace = find_from(cwd.path(), &home.location());

    assert_eq!(canonical(&workspace.root), canonical(root.path()));
    assert_eq!(workspace.found_by, FoundBy::InstalledPath { crate_dir });
}

/// Runs [`find`] without `--repo` in `cwd`, and gives the text of its error.
fn error_in(cwd: &Path, location: &InstallLocation<'_>) -> String {
    find(None, cwd, &Programs::default(), location, BINARY)
        .expect_err("a workspace was found")
        .to_string()
}

/// Runs [`find`] without `--repo` outside each work tree, and gives the text
/// of its error.
fn error_outside(location: &InstallLocation<'_>) -> String {
    let cwd = not_a_repository();
    error_in(cwd.path(), location)
}

/// Says that `message` names `--repo` and holds `reason`.
fn assert_names_repo_and(message: &str, reason: &str) {
    assert!(message.contains("--repo <path>"), "{message}");
    assert!(message.contains(reason), "no {reason:?} in: {message}");
}

#[test]
fn a_missing_records_file_is_an_error_that_names_repo_and_the_reason() {
    let home = CargoHome::new();

    let message = error_outside(&home.location());

    assert_names_repo_and(&message, "there is no record of the install");
    assert_names_repo_and(&message, ".crates2.json");
}

#[test]
fn a_records_file_without_the_binary_is_an_error_that_names_repo_and_the_reason() {
    let home = CargoHome::new();
    home.write_records(&[(&git_key("gsw", REPOSITORY_URL, OTHER_COMMIT), &["gsw"])]);

    let message = error_outside(&home.location());

    assert_names_repo_and(&message, "holds no record of the binary wdid");
}

#[test]
fn a_records_file_that_is_not_json_is_an_error_that_names_repo_and_the_reason() {
    let home = CargoHome::new();
    std::fs::write(home.path().join(support::RECORDS_FILE), "not json").unwrap();

    let message = error_outside(&home.location());

    assert_names_repo_and(&message, "is not a records file that wdid can read");
}

#[test]
fn a_record_from_the_registry_is_an_error_that_names_repo_and_the_reason() {
    let home =
        home_with_record("wdid 0.1.0 (registry+https://github.com/rust-lang/crates.io-index)");

    let message = error_outside(&home.location());

    assert_names_repo_and(&message, "names a source that is not git+ or path+file");
}

#[test]
fn a_git_record_without_a_full_commit_is_an_error_that_names_repo_and_the_reason() {
    let home = home_with_record(&git_key(BINARY, REPOSITORY_URL, "12baa41"));

    let message = error_outside(&home.location());

    assert_names_repo_and(&message, "names no full commit");
}

/// `cargo cache --autoclean` removes the checkouts.
#[test]
fn a_missing_checkout_is_an_error_that_names_repo_and_the_reason() {
    let home = home_with_record(&git_key(BINARY, REPOSITORY_URL, OTHER_COMMIT));

    let message = error_outside(&home.location());

    assert_names_repo_and(&message, "holds no checkout of commit 0123456 of tools");
    assert_names_repo_and(&message, "cargo cache --autoclean");
}

#[test]
fn a_checkout_at_a_different_commit_is_an_error_that_names_repo_and_the_reason() {
    let source = workspace_repository();
    let head = source.rev_parse("HEAD");
    let home = home_with_record(&git_key(BINARY, REPOSITORY_URL, OTHER_COMMIT));
    home.add_checkout(&source, CHECKOUT_DIRECTORY, short(OTHER_COMMIT));

    let message = error_outside(&home.location());

    assert_names_repo_and(
        &message,
        &format!("is at commit {head}, not at commit {OTHER_COMMIT} of the record"),
    );
}

/// git does not look above the checkout. A checkout directory without a
/// repository is thus not a checkout, also inside another work tree. The
/// cargo home is a real directory in that work tree, not a symlink: git starts
/// from the physical directory, and a symlink thus hides the work tree.
#[test]
fn a_checkout_directory_without_a_repository_is_an_error_that_names_repo_and_the_reason() {
    let outer = TestRepo::init();
    outer.commit_file("README.md", "outer\n", "add a readme");
    let cargo_home = outer.path().join("cargo-home");
    let checkouts = cargo_home.join("git").join("checkouts");
    std::fs::create_dir_all(checkouts.join(CHECKOUT_DIRECTORY).join(short(OTHER_COMMIT))).unwrap();
    support::write_records(
        &cargo_home,
        &[(&git_key(BINARY, REPOSITORY_URL, OTHER_COMMIT), &[BINARY])],
    );
    let inside = InstallLocation {
        cargo_home: Some(cargo_home.as_os_str()),
        ..InstallLocation::default()
    };

    let message = error_outside(&inside);

    assert_names_repo_and(&message, "git cannot read the checkout");
}

#[test]
fn a_checkout_without_a_cargo_toml_is_an_error_that_names_repo_and_the_reason() {
    let source = TestRepo::init();
    source.commit_file("README.md", "no manifest\n", "add a readme");
    let commit = source.rev_parse("HEAD");
    let home = home_with_record(&git_key(BINARY, REPOSITORY_URL, &commit));
    home.add_checkout(&source, CHECKOUT_DIRECTORY, short(&commit));

    let message = error_outside(&home.location());

    assert_names_repo_and(&message, "has no Cargo.toml");
}

#[test]
fn a_path_record_whose_crate_directory_is_gone_is_an_error_that_names_repo_and_the_reason() {
    let gone = tempfile::tempdir().unwrap();
    let crate_dir = gone.path().join("wdid");
    let home = home_with_record(&path_key(BINARY, &crate_dir));

    let message = error_outside(&home.location());

    assert_names_repo_and(
        &message,
        &format!(
            "the crate directory {} of the record does not exist",
            crate_dir.display()
        ),
    );
}

#[test]
fn without_cargo_home_and_home_the_error_names_repo_and_the_reason() {
    let message = error_outside(&InstallLocation::default());

    assert_names_repo_and(&message, "neither CARGO_HOME nor HOME is set");
}

/// The error also says why step 2 found no workspace.
#[test]
fn the_error_outside_each_work_tree_says_that_the_directory_is_in_no_work_tree() {
    let home = CargoHome::new();
    let cwd = not_a_repository();

    let message = error_in(cwd.path(), &home.location());

    assert!(
        message.starts_with(&format!(
            "{} is not in a git work tree, and the installed wdid gives no workspace: ",
            cwd.path().display()
        )),
        "{message}"
    );
}

#[test]
fn the_error_in_a_work_tree_without_a_cargo_toml_names_the_work_tree() {
    let home = CargoHome::new();
    let work_tree = TestRepo::init();

    let message = error_in(work_tree.path(), &home.location());

    assert!(message.starts_with("the git work tree "), "{message}");
    assert!(
        message.contains(" has no Cargo.toml, and the installed wdid gives no workspace: "),
        "{message}"
    );
    assert!(
        message.ends_with("Run wdid in a cargo workspace, or name one with --repo <path>"),
        "{message}"
    );
}

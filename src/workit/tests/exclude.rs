//! An exclusion names a directory, not a piece of a path.
//!
//! `workit` keeps `target` and `node_modules` out of its walk, and `--exclude`
//! adds more names to that list. Each name matches a whole directory below the
//! search root. A directory named `targets`, `targeting` or
//! `node_modules_backup` is a different directory and stays in the walk, and
//! the search root the user chose is never judged against the list at all —
//! the user asked for that directory by name, so a `target` somewhere in its
//! own path says nothing about what is under it.
//!
//! Every test here builds a tree of packages in its own temporary directory,
//! runs the binary over that tree with `--dry-run`, and reads back the members
//! the binary printed. Nothing is written and nothing outside the temporary
//! directory is read, so two copies of this file can run at the same time.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// Writes a package manifest at `relative` under `root`, so the walk has
/// something to find there. Every package in a workspace needs a name of its
/// own, so each call states one.
fn package_at(root: &Path, relative: &str, name: &str) {
    let dir = root.join(relative);
    fs::create_dir_all(&dir).expect("the fixture directory is created");
    fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    )
    .expect("the fixture manifest is written");
}

/// A temporary directory holding a search root named `name`.
///
/// The root is canonical, because `workit` reports each member relative to the
/// canonical form of the path it was given. The `TempDir` is returned with it:
/// it deletes the tree when it drops, so the caller has to keep it alive.
fn fixture(name: &str) -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new().expect("a temporary directory is created");
    let root = fs::canonicalize(temp.path())
        .expect("the temporary directory has a canonical path")
        .join(name);
    fs::create_dir_all(&root).expect("the search root is created");
    (temp, root)
}

/// Runs the binary over `root` and returns the members it printed, in the
/// order it printed them. `--dry-run` keeps it from writing a manifest, and
/// the output path names a file inside the fixture so no run can touch a
/// manifest of the repository it was started from.
fn members_of(root: &Path, extra_args: &[&str]) -> Vec<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_workit"))
        .arg("--path")
        .arg(root)
        .arg("--output")
        .arg(root.join("workspace-manifest.toml"))
        .arg("--dry-run")
        .args(extra_args)
        .output()
        .expect("the workit binary runs");

    assert!(
        output.status.success(),
        "workit over {} exited with {}, stderr: {}",
        root.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let printed = String::from_utf8(output.stdout).expect("workit writes UTF-8");
    printed
        .lines()
        .filter_map(|line| line.strip_prefix("  - "))
        .map(str::to_string)
        .collect()
}

#[test]
fn a_search_root_whose_own_path_holds_an_exclusion_is_still_searched() {
    let (_temp, root) = fixture("targeting");
    package_at(&root, "widget", "workit-fixture-root-widget");

    assert_eq!(
        members_of(&root, &[]),
        vec!["widget".to_string()],
        "the user named this root; `target` inside its own path is not a reason to skip it"
    );
}

#[test]
fn a_directory_named_target_is_excluded() {
    let (_temp, root) = fixture("tree");
    package_at(&root, "keep", "workit-fixture-target-keep");
    package_at(&root, "target/buried", "workit-fixture-target-buried");

    assert_eq!(
        members_of(&root, &[]),
        vec!["keep".to_string()],
        "a directory named exactly `target` stays out of the walk"
    );
}

#[test]
fn a_directory_whose_name_merely_starts_with_an_exclusion_is_kept() {
    let (_temp, root) = fixture("tree");
    package_at(&root, "targets", "workit-fixture-targets");
    package_at(&root, "targeting", "workit-fixture-targeting");

    assert_eq!(
        members_of(&root, &[]),
        vec!["targeting".to_string(), "targets".to_string()],
        "`targets` and `targeting` are not `target`"
    );
}

#[test]
fn a_directory_whose_name_merely_contains_node_modules_is_kept() {
    let (_temp, root) = fixture("tree");
    package_at(
        &root,
        "node_modules/buried",
        "workit-fixture-node-modules-buried",
    );
    package_at(
        &root,
        "node_modules_backup",
        "workit-fixture-node-modules-backup",
    );

    assert_eq!(
        members_of(&root, &[]),
        vec!["node_modules_backup".to_string()],
        "`node_modules` stays out and `node_modules_backup` stays in"
    );
}

#[test]
fn an_exclusion_the_user_passes_names_a_whole_directory() {
    let (_temp, root) = fixture("tree");
    package_at(&root, "vendor", "workit-fixture-vendor");
    package_at(&root, "vendored", "workit-fixture-vendored");

    assert_eq!(
        members_of(&root, &["--exclude", "vendor"]),
        vec!["vendored".to_string()],
        "`--exclude vendor` names the directory `vendor`, not every path that spells it"
    );
}

#[test]
fn no_default_excludes_lets_a_target_directory_through() {
    let (_temp, root) = fixture("tree");
    package_at(&root, "target/inner", "workit-fixture-no-defaults-inner");

    assert_eq!(
        members_of(&root, &["--no-default-excludes"]),
        vec!["target/inner".to_string()],
        "with the defaults off, nothing keeps a `target` directory out"
    );
}

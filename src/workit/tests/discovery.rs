//! Which `Cargo.toml` files the walk turns into members.
//!
//! `workit` walks a tree and lists what it found. Two decisions run through
//! that walk and neither one was pinned: which directories it descends into,
//! and which of the manifests it meets name a package rather than a workspace.
//!
//! The predicate is `[package]` and no `[workspace]`. A manifest that carries
//! only `[workspace]` describes a workspace, which cargo cannot hold as a
//! member of another one, and a manifest that carries both is a workspace root
//! that is also a package — the same answer, for the same reason. The manifest
//! the run is about to write is not a member of itself either, so the search
//! path's own `Cargo.toml` is left out whatever it holds.
//!
//! The walk itself descends into everything below the search path except a
//! hidden directory and an excluded one. It does not stop at a package, so a
//! package inside another package is found, and it does not stop at a nested
//! workspace either, so the packages below one are listed by the outer
//! manifest. That last one is the behaviour a reader is most likely to guess
//! wrong, so it is written down here: `cargo metadata` accepts the manifest
//! that comes out, because the nested workspace root is itself no member of it.
//!
//! Every test here builds a tree of packages in its own temporary directory,
//! runs the binary over that tree with `--dry-run`, and reads back the members
//! the binary printed. `--output` names a file inside the fixture, so nothing
//! is written and nothing outside the temporary directory is read: two copies
//! of this file can run at the same time, and neither one can reach a real
//! repository. No test here runs git, so none of them inherits a git
//! environment either.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// A temporary directory and the canonical path of the search root inside it.
///
/// The path is canonical because `workit` reports each member relative to the
/// canonical form of the path it was given, and a temporary directory on macOS
/// is reached through a symbolic link. The `TempDir` is returned with it: it
/// deletes the tree when it drops, so the caller has to keep it alive.
fn fixture() -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new().expect("a temporary directory is created");
    let root = fs::canonicalize(temp.path()).expect("the temporary directory has a canonical path");
    (temp, root)
}

/// Writes `content` as the `Cargo.toml` of the directory `relative` under
/// `root`. A `relative` of `"."` names the search root itself.
fn manifest_at(root: &Path, relative: &str, content: &str) {
    let dir = root.join(relative);
    fs::create_dir_all(&dir).expect("the fixture directory is created");
    fs::write(dir.join("Cargo.toml"), content).expect("the fixture manifest is written");
}

/// Writes a package manifest at `relative` under `root`, so the walk has
/// something to find there. Every package in a workspace needs a name of its
/// own, so each call states one.
fn package_at(root: &Path, relative: &str, name: &str) {
    manifest_at(
        root,
        relative,
        &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    );
}

/// Writes a manifest that describes a workspace and no package at all.
fn workspace_at(root: &Path, relative: &str, members: &str) {
    manifest_at(
        root,
        relative,
        &format!("[workspace]\nmembers = [{members}]\nresolver = \"2\"\n"),
    );
}

/// Runs the binary over `root` and returns the members it printed, in the
/// order it printed them. `--dry-run` keeps it from writing a manifest, and
/// the output path names a file inside the fixture so no run can touch a
/// manifest of the repository it was started from.
fn members_of(root: &Path) -> Vec<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_workit"))
        .current_dir(root)
        .arg("--path")
        .arg(root)
        .arg("--output")
        .arg(root.join("workspace-manifest.toml"))
        .arg("--dry-run")
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
fn a_manifest_that_describes_only_a_workspace_is_not_a_member() {
    let (_temp, root) = fixture();
    package_at(&root, "pkg", "workit-fixture-workspace-only-pkg");
    workspace_at(&root, "ws", "");

    assert_eq!(
        members_of(&root),
        vec!["pkg".to_string()],
        "a manifest with no [package] describes a workspace, and cargo cannot hold \
         a workspace as a member of another one"
    );
}

#[test]
fn a_manifest_that_is_both_a_package_and_a_workspace_is_not_a_member() {
    let (_temp, root) = fixture();
    package_at(&root, "pkg", "workit-fixture-both-pkg");
    manifest_at(
        &root,
        "both",
        "[package]\nname = \"workit-fixture-both-root\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         \n[workspace]\nmembers = []\nresolver = \"2\"\n",
    );

    assert_eq!(
        members_of(&root),
        vec!["pkg".to_string()],
        "a manifest that carries [workspace] is a workspace root, whether or not it \
         also carries [package]"
    );
}

#[test]
fn the_search_paths_own_manifest_is_not_a_member_of_itself() {
    let (_temp, root) = fixture();
    package_at(&root, ".", "workit-fixture-root-itself");
    package_at(&root, "pkg", "workit-fixture-root-itself-pkg");

    assert_eq!(
        members_of(&root),
        vec!["pkg".to_string()],
        "the manifest at the search path is the one being written; a workspace that \
         listed itself as a member would name the directory it lives in"
    );
}

#[test]
fn a_package_inside_a_hidden_directory_is_not_found() {
    let (_temp, root) = fixture();
    package_at(&root, "visible", "workit-fixture-hidden-visible");
    package_at(&root, ".hidden/buried", "workit-fixture-hidden-buried");

    assert_eq!(
        members_of(&root),
        vec!["visible".to_string()],
        "the walk does not descend into a directory whose name starts with a dot, so \
         nothing under one is found"
    );
}

#[test]
fn a_package_nested_inside_another_package_is_found() {
    let (_temp, root) = fixture();
    package_at(&root, "outer", "workit-fixture-nested-outer");
    package_at(&root, "outer/inner", "workit-fixture-nested-inner");

    assert_eq!(
        members_of(&root),
        vec!["outer".to_string(), "outer/inner".to_string()],
        "a package is not the end of the walk: both the package and the one below it \
         are listed"
    );
}

#[test]
fn packages_below_a_nested_workspace_are_listed_by_the_outer_manifest() {
    let (_temp, root) = fixture();
    package_at(&root, "plain", "workit-fixture-nested-ws-plain");
    workspace_at(&root, "nested-ws", "\"a\"");
    package_at(&root, "nested-ws/a", "workit-fixture-nested-ws-a");
    package_at(&root, "nested-ws/b", "workit-fixture-nested-ws-b");

    assert_eq!(
        members_of(&root),
        vec![
            "nested-ws/a".to_string(),
            "nested-ws/b".to_string(),
            "plain".to_string(),
        ],
        "a nested workspace is not the end of the walk either: the packages below it \
         are listed by the outer manifest, the one the nested workspace already lists \
         included. Cargo accepts that manifest, because the nested workspace root is \
         no member of it"
    );
}

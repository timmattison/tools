//! What a rewrite does to the manifest it is handed.
//!
//! `workit` does not write a manifest from scratch when one is already there.
//! It reads the file, replaces the members array, and writes the document back
//! — which is the whole reason the crate parses with `toml_edit` rather than
//! with `toml`. A round trip through `toml` would answer with the same *data*
//! and a different *file*: the comments gone, the tables reordered, the
//! inline tables expanded. A workspace manifest is a file people wrote by
//! hand, so `[workspace.package]`, `[workspace.dependencies]`,
//! `[profile.release]` and a comment at the top of the file all have to come
//! back out exactly as they went in.
//!
//! One key is added rather than preserved. A workspace with no `resolver` gets
//! `resolver = "2"`, because a virtual manifest that names none falls back to
//! the first resolver and cargo warns about it on every command. A `resolver`
//! the user already set is left alone: the fallback is a default, and a default
//! that overwrote a decision would be a bug rather than a convenience.
//!
//! Every test here builds its own tree in its own temporary directory and
//! points the run at a manifest inside it, so nothing outside the fixture is
//! written. These tests write for real rather than with `--dry-run`, because
//! what reaches the file is the thing under test. No test here runs git, so
//! none of them inherits a git environment; the one that runs `cargo` sheds
//! that environment all the same, and gives it a target directory inside the
//! fixture so it cannot disturb the workspace it was started from.

use gitscratch::shed_inherited_git_environment;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// A manifest of the shape people write by hand: a comment at the top, a
/// members array to be replaced, a resolver the user chose, two tables under
/// `[workspace]`, and one that is nothing to do with the workspace at all.
const HANDWRITTEN_MANIFEST: &str = r#"# The workspace of the fixture, and a comment no rewrite may take off.

[workspace]
members = ["stale"]
resolver = "3"

[workspace.package]
version = "1.2.3"
edition = "2021"
license = "MIT"

[workspace.dependencies]
anyhow = "1.0"
serde = { version = "1", features = ["derive"] }

[profile.release]
lto = true
codegen-units = 1

[patch.crates-io]
foo = { path = "vendor/foo" }
"#;

/// The members array of [`HANDWRITTEN_MANIFEST`], as it reads before a run.
const STALE_MEMBERS: &str = r#"members = ["stale"]"#;

/// A temporary directory and its canonical path.
///
/// The path is canonical because `workit` subtracts canonical paths to build a
/// member, and a temporary directory on macOS is reached through a symbolic
/// link. The `TempDir` is returned with it: it deletes the tree when it drops,
/// so the caller has to keep it alive.
fn fixture() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("a temporary directory is created");
    let root = fs::canonicalize(temp.path()).expect("the temporary directory has a canonical path");
    (temp, root)
}

/// Writes a package at `relative` under `root`: a manifest naming it, and the
/// library root cargo needs before it will call the directory a package at all.
fn package_at(root: &Path, relative: &str, name: &str) {
    let dir = root.join(relative);
    fs::create_dir_all(dir.join("src")).expect("the fixture directory is created");
    fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    )
    .expect("the fixture manifest is written");
    fs::write(dir.join("src/lib.rs"), "").expect("the fixture library root is written");
}

/// Runs the binary over `root`, writing for real, and fails the test with what
/// the run printed when it did not succeed.
fn workit(root: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_workit"))
        .current_dir(root)
        .arg("--path")
        .arg(root)
        .output()
        .expect("the workit binary runs");

    assert!(
        output.status.success(),
        "workit over {} exited with {}, stderr: {}",
        root.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The text of the manifest at the root of `root`.
fn manifest_text(root: &Path) -> String {
    let manifest = root.join("Cargo.toml");
    fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("{} is readable: {e}", manifest.display()))
}

/// Writes `content` as the manifest at the root of `root`.
fn write_manifest(root: &Path, content: &str) {
    fs::write(root.join("Cargo.toml"), content).expect("the existing manifest is written");
}

#[test]
fn an_existing_manifest_keeps_everything_but_its_members() {
    let (_temp, root) = fixture();
    package_at(&root, "crate-a", "workit-fixture-keep-crate-a");
    write_manifest(&root, HANDWRITTEN_MANIFEST);

    workit(&root);

    assert_eq!(
        manifest_text(&root),
        HANDWRITTEN_MANIFEST.replace(STALE_MEMBERS, r#"members = ["crate-a"]"#),
        "the members array is the one thing a rewrite is about. The comment, the two \
         tables under [workspace], the profile, the patch table and the inline table \
         inside [workspace.dependencies] all come back byte for byte — which is what \
         parsing with toml_edit rather than toml buys, and nothing else was measuring it"
    );
}

#[test]
fn a_workspace_that_names_no_resolver_is_given_the_second_one() {
    let (_temp, root) = fixture();
    package_at(&root, "crate-a", "workit-fixture-resolver-added-crate-a");
    write_manifest(&root, "[workspace]\nmembers = [\"stale\"]\n");

    workit(&root);

    assert_eq!(
        manifest_text(&root),
        "[workspace]\nmembers = [\"crate-a\"]\nresolver = \"2\"\n",
        "a virtual manifest that names no resolver falls back to the first one, and \
         cargo says so on every command, so the rewrite states the second"
    );
}

#[test]
fn a_resolver_the_user_chose_is_not_overwritten() {
    let (_temp, root) = fixture();
    package_at(&root, "crate-a", "workit-fixture-resolver-kept-crate-a");
    write_manifest(&root, HANDWRITTEN_MANIFEST);

    workit(&root);

    let after = manifest_text(&root);
    assert!(
        after.contains("resolver = \"3\""),
        "the resolver is a default the run supplies, not a decision it makes: {after}"
    );
    assert!(
        !after.contains("resolver = \"2\""),
        "and supplying it twice would leave the manifest naming two resolvers: {after}"
    );
}

#[test]
fn a_manifest_written_where_none_existed_is_one_cargo_accepts() {
    let (_temp, root) = fixture();
    package_at(&root, "crate-a", "workit-fixture-fresh-crate-a");
    package_at(&root, "crate-b", "workit-fixture-fresh-crate-b");

    workit(&root);

    let manifest = root.join("Cargo.toml");
    let mut cargo = Command::new(env!("CARGO"));
    // Nothing here reads a repository, but a `GIT_*` inherited from a
    // pre-commit hook aims every tool that does. The rule is the prefix.
    shed_inherited_git_environment(&mut cargo);
    let metadata = cargo
        // The build products of this run belong inside the fixture, which is
        // deleted with it. Sharing the workspace's own `./target` would let a
        // test disturb the build it was started by.
        .env("CARGO_TARGET_DIR", root.join("target"))
        .arg("metadata")
        .arg("--no-deps")
        .arg("--offline")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .expect("cargo runs");

    let complaint = String::from_utf8_lossy(&metadata.stderr);
    assert!(
        metadata.status.success(),
        "cargo is the reader of what this tool writes, so it is what says the manifest \
         is well formed. It exited with {}: {complaint}\n{}",
        metadata.status,
        manifest_text(&root)
    );

    let reported = String::from_utf8(metadata.stdout).expect("cargo metadata writes UTF-8");
    for name in [
        "workit-fixture-fresh-crate-a",
        "workit-fixture-fresh-crate-b",
    ] {
        assert!(
            reported.contains(name),
            "cargo reads the workspace as holding '{name}'"
        );
    }
}

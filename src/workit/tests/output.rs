//! Where the manifest lands, and what its members are relative to.
//!
//! `workit` reads one tree and writes one manifest. The two used to be chosen
//! independently: the walk started at `--path`, and the manifest went to
//! `Cargo.toml` in whatever directory the run was started from. A run that
//! named another tree therefore rewrote the members of the manifest beside it,
//! naming packages that directory does not hold. The manifest now lands beside
//! the tree that was scanned, and an explicit `--output` is still honored.
//!
//! A member path is relative to the directory that holds the manifest, because
//! that is the directory cargo resolves it against. Both sides of that
//! subtraction are canonical, so the `./` a relative `--path` used to leave on
//! every member is gone, and `--prefix src/` writes `src/crate-a` rather than
//! `src/./crate-a`.
//!
//! Every test here builds its own tree in its own temporary directory and runs
//! the binary with an explicit working directory, so two copies of this file
//! can run at the same time and neither one can reach a real repository.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;
use toml_edit::DocumentMut;

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

/// Creates the directory `relative` under `root` and returns its path.
fn directory_at(root: &Path, relative: &str) -> PathBuf {
    let dir = root.join(relative);
    fs::create_dir_all(&dir).expect("the fixture directory is created");
    dir
}

/// Writes a package manifest at `relative` under `root`, so the walk has
/// something to find there. Every package in a workspace needs a name of its
/// own, so each call states one.
fn package_at(root: &Path, relative: &str, name: &str) -> PathBuf {
    let dir = directory_at(root, relative);
    fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    )
    .expect("the fixture manifest is written");
    dir
}

/// Runs the binary from `cwd`, and returns what it wrote once it has exited
/// successfully. The working directory is stated on every run, because the
/// directory a run is started from is one of the two inputs under test.
fn workit(cwd: &Path, args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_workit"))
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("the workit binary runs");

    assert!(
        output.status.success(),
        "workit {args:?} in {} exited with {}, stderr: {}",
        cwd.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    output
}

/// The members `manifest` lists, in the order it lists them.
fn members_of(manifest: &Path) -> Vec<String> {
    let content = fs::read_to_string(manifest)
        .unwrap_or_else(|e| panic!("{} is readable: {e}", manifest.display()));
    let doc = content
        .parse::<DocumentMut>()
        .unwrap_or_else(|e| panic!("{} parses as TOML: {e}", manifest.display()));

    doc["workspace"]["members"]
        .as_array()
        .unwrap_or_else(|| panic!("{} carries a workspace members array", manifest.display()))
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("every member is a string")
                .to_string()
        })
        .collect()
}

/// The stderr of a run, as text.
fn stderr_of(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("workit writes UTF-8")
}

#[test]
fn scanning_another_tree_leaves_the_manifest_of_the_current_directory_alone() {
    let (_temp, root) = fixture();

    // The repository the run is started from. It carries a workspace of its
    // own, and nothing about this run is about it.
    let victim = directory_at(&root, "victim");
    package_at(&victim, "pkg-x", "workit-fixture-victim-pkg-x");
    let victim_manifest = victim.join("Cargo.toml");
    let victim_bytes = b"[workspace]\nmembers = [\"pkg-x\"]\nresolver = \"2\"\n";
    fs::write(&victim_manifest, victim_bytes).expect("the victim manifest is written");

    // The tree the run was asked to scan.
    let elsewhere = directory_at(&root, "elsewhere");
    package_at(&elsewhere, "pkg-y", "workit-fixture-elsewhere-pkg-y");

    workit(&victim, &["--path", &elsewhere.to_string_lossy()]);

    assert_eq!(
        fs::read(&victim_manifest).expect("the victim manifest is still readable"),
        victim_bytes.to_vec(),
        "a run that scans another tree must not touch the manifest beside it"
    );

    let written = elsewhere.join("Cargo.toml");
    assert!(
        written.exists(),
        "the manifest belongs beside the tree that was scanned"
    );
    assert_eq!(
        members_of(&written),
        vec!["pkg-y".to_string()],
        "the members name the packages of the tree that was scanned"
    );
}

#[test]
fn a_default_run_writes_members_with_no_leading_dot_segment() {
    let (_temp, root) = fixture();
    package_at(&root, "crate-a", "workit-fixture-default-crate-a");

    workit(&root, &[]);

    assert_eq!(
        members_of(&root.join("Cargo.toml")),
        vec!["crate-a".to_string()],
        "a member is the package directory relative to the manifest, with no `./` on the front"
    );
}

#[test]
fn a_prefix_lands_directly_on_the_member() {
    let (_temp, root) = fixture();
    package_at(&root, "crate-a", "workit-fixture-prefix-crate-a");

    workit(&root, &["--prefix", "src/"]);

    assert_eq!(
        members_of(&root.join("Cargo.toml")),
        vec!["src/crate-a".to_string()],
        "`--prefix src/` names `src/crate-a`, not `src/./crate-a`"
    );
}

#[test]
fn an_explicit_output_roots_the_members_at_its_own_directory() {
    let (_temp, root) = fixture();
    let scanned = directory_at(&root, "workspace");
    package_at(&scanned, "pkg-a", "workit-fixture-explicit-pkg-a");

    let manifest = root.join("Cargo.toml");
    workit(
        &root,
        &[
            "--path",
            &scanned.to_string_lossy(),
            "--output",
            &manifest.to_string_lossy(),
        ],
    );

    assert_eq!(
        members_of(&manifest),
        vec!["workspace/pkg-a".to_string()],
        "cargo resolves a member against the directory of the manifest that lists it"
    );
}

#[test]
fn a_package_outside_the_manifest_directory_is_written_as_an_absolute_path() {
    let (_temp, root) = fixture();
    let scanned = directory_at(&root, "packages");
    let package = package_at(&scanned, "pkg-a", "workit-fixture-outside-pkg-a");
    let build = directory_at(&root, "build");

    let manifest = build.join("Cargo.toml");
    workit(
        &root,
        &[
            "--path",
            &scanned.to_string_lossy(),
            "--output",
            &manifest.to_string_lossy(),
        ],
    );

    assert_eq!(
        members_of(&manifest),
        vec![package.to_string_lossy().to_string()],
        "a package the manifest directory does not hold cannot be named relative to it, \
         so it is named in full"
    );
}

#[test]
fn a_member_the_scan_did_not_find_is_named_before_it_is_dropped() {
    let (_temp, root) = fixture();
    package_at(&root, "keep", "workit-fixture-drop-keep");

    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        "[workspace]\nmembers = [\"ghost\", \"keep\"]\nresolver = \"2\"\n",
    )
    .expect("the existing manifest is written");

    let output = workit(&root, &[]);
    let complaint = stderr_of(&output);

    assert!(
        complaint.contains("ghost"),
        "a member the scan did not find is dropped, so the run has to say so; stderr was {complaint:?}"
    );
    assert!(
        !complaint.contains("keep"),
        "`keep` is still a member, so nothing about it is being dropped; stderr was {complaint:?}"
    );
    assert_eq!(
        members_of(&manifest),
        vec!["keep".to_string()],
        "the members that were named on stderr really are gone"
    );
}

#[test]
fn a_dry_run_leaves_an_existing_manifest_byte_identical() {
    let (_temp, root) = fixture();
    let scanned = directory_at(&root, "tree");
    package_at(&scanned, "pkg-a", "workit-fixture-dry-run-pkg-a");

    let manifest = scanned.join("Cargo.toml");
    let before = b"[workspace]\nmembers = [\"stale\"]\nresolver = \"2\"\n";
    fs::write(&manifest, before).expect("the existing manifest is written");

    let elsewhere = directory_at(&root, "elsewhere");
    workit(
        &elsewhere,
        &["--path", &scanned.to_string_lossy(), "--dry-run"],
    );

    assert_eq!(
        fs::read(&manifest).expect("the manifest is still readable"),
        before.to_vec(),
        "a dry run writes nothing"
    );
}

#[test]
fn a_dry_run_creates_no_manifest_that_was_not_there() {
    let (_temp, root) = fixture();
    let scanned = directory_at(&root, "tree");
    package_at(&scanned, "pkg-a", "workit-fixture-dry-run-absent-pkg-a");

    let elsewhere = directory_at(&root, "elsewhere");
    workit(
        &elsewhere,
        &["--path", &scanned.to_string_lossy(), "--dry-run"],
    );

    assert!(
        !scanned.join("Cargo.toml").exists(),
        "a dry run writes nothing, so the manifest it would have written is still absent"
    );
    assert!(
        !elsewhere.join("Cargo.toml").exists(),
        "and nothing lands in the directory the run was started from either"
    );
}

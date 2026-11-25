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
//! every member is gone. A package in the manifest directory itself is the
//! member cargo calls `.`: the subtraction leaves nothing there, so the
//! manifest used to hold an empty member and the run printed a bullet with
//! nothing behind it.
//!
//! There is no `--prefix`: a member is already the exact path from the manifest
//! to its package, so a prefix named a directory that holds no package. An
//! `--output` above the scanned tree writes the path that a prefix used to add.
//!
//! Cargo holds a member only where the manifest that lists it sits above that
//! member on the file system. It refuses every other member, an absolute path
//! and a `../` path alike. A package the manifest directory does not hold has
//! no spelling at all, so the run refuses and names every such package. It
//! used to write those packages as absolute paths, and no checkout could load
//! the manifest that came out, not even the one that wrote it.
//!
//! Every test here builds its own tree in its own temporary directory and runs
//! the binary with an explicit working directory, so two copies of this file
//! can run at the same time and neither one can reach a real repository.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
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
    let output = childenv::fixture_command(env!("CARGO_BIN_EXE_workit"))
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

/// Runs the binary from `cwd`, and returns what it wrote once it has failed.
/// A tree the run refuses is the thing under test here, so this one asserts the
/// opposite status: a run that wrote a manifest cargo cannot load is the defect,
/// and it reads as success from the exit status alone.
fn workit_fails(cwd: &Path, args: &[&str]) -> Output {
    let output = childenv::fixture_command(env!("CARGO_BIN_EXE_workit"))
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("the workit binary runs");

    assert!(
        !output.status.success(),
        "workit {args:?} in {} succeeded, and it has to refuse the tree; stdout: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stdout)
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

/// The stdout of a run, as text. The list of members the run found goes here,
/// and the manifest the run wrote has to agree with it.
fn stdout_of(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("workit writes UTF-8")
}

/// Whether the run listed `member` as one of the packages it found.
///
/// The whole line is compared, because a bullet with nothing behind it is the
/// defect under test and a substring test passes on it.
fn lists_member(output: &Output, member: &str) -> bool {
    let bullet = format!("  - {member}");
    stdout_of(output).lines().any(|line| line == bullet)
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
fn a_prefix_is_refused_because_each_member_already_names_its_package() {
    let (_temp, root) = fixture();
    package_at(&root, "crate-a", "workit-fixture-prefix-refused-crate-a");

    workit_fails(&root, &["--prefix", "src/"]);

    assert!(
        !root.join("Cargo.toml").exists(),
        "a member is the exact path from the manifest to its package, so a prefix names a \
         directory that holds no package, and the run writes no manifest"
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
fn the_manifest_directory_itself_is_named_as_a_member_rather_than_as_nothing() {
    let (_temp, root) = fixture();
    let scanned = directory_at(&root, "tree");
    let package = package_at(&scanned, "sub", "workit-fixture-dot-member-sub");

    // The manifest lands in the package directory itself. Its name is not
    // `Cargo.toml`, so the walk still meets the package beside it and lists it:
    // the one file the walk leaves out is the file the run writes.
    let manifest = package.join("workspace-manifest.toml");
    let output = workit(
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
        vec![".".to_string()],
        "the manifest directory itself is the member cargo calls `.`, and an empty \
         member says nothing to the reader"
    );
    assert!(
        lists_member(&output, "."),
        "the printed list and the manifest name the same member; stdout was {:?}",
        stdout_of(&output)
    );
}

#[test]
fn a_package_the_manifest_directory_does_not_hold_is_refused() {
    let (_temp, root) = fixture();
    let scanned = directory_at(&root, "packages");
    let package = package_at(&scanned, "pkg-a", "workit-fixture-outside-pkg-a");
    let build = directory_at(&root, "build");

    let manifest = build.join("Cargo.toml");
    let output = workit_fails(
        &root,
        &[
            "--path",
            &scanned.to_string_lossy(),
            "--output",
            &manifest.to_string_lossy(),
        ],
    );
    let complaint = stderr_of(&output);

    assert!(
        complaint.contains(&package.to_string_lossy().to_string()),
        "cargo refuses a member the manifest directory does not hold, so the run refuses \
         first and names the package; stderr was {complaint:?}"
    );
    assert!(
        complaint.contains(&build.to_string_lossy().to_string()),
        "the package is measured against the directory that holds the manifest, so the \
         report names that directory; stderr was {complaint:?}"
    );
}

#[test]
fn a_refused_run_writes_no_manifest() {
    let (_temp, root) = fixture();
    let scanned = directory_at(&root, "packages");
    package_at(&scanned, "pkg-a", "workit-fixture-refused-pkg-a");
    let build = directory_at(&root, "build");

    let manifest = build.join("Cargo.toml");
    workit_fails(
        &root,
        &[
            "--path",
            &scanned.to_string_lossy(),
            "--output",
            &manifest.to_string_lossy(),
        ],
    );

    assert!(
        !manifest.exists(),
        "a run that refuses the tree writes nothing: a manifest cargo cannot load is \
         worse than no manifest at all"
    );
}

#[test]
fn a_tree_with_one_package_outside_the_manifest_directory_is_refused() {
    let (_temp, root) = fixture();
    let scanned = directory_at(&root, "packages");
    let inside = package_at(&scanned, "build/pkg-inside", "workit-fixture-mixed-inside");
    let outside = package_at(&scanned, "pkg-outside", "workit-fixture-mixed-outside");

    // The manifest lands under `build`, which holds one of the two packages.
    let manifest = scanned.join("build").join("Cargo.toml");
    let output = workit_fails(
        &root,
        &[
            "--path",
            &scanned.to_string_lossy(),
            "--output",
            &manifest.to_string_lossy(),
        ],
    );
    let complaint = stderr_of(&output);

    assert!(
        complaint.contains(&outside.to_string_lossy().to_string()),
        "one package the manifest directory does not hold refuses the whole run, because \
         --output below the packages is one mistake in the invocation; stderr was \
         {complaint:?}"
    );
    assert!(
        !complaint.contains(&inside.to_string_lossy().to_string()),
        "the package the manifest directory does hold is named nowhere in the report: it \
         is a member cargo accepts; stderr was {complaint:?}"
    );
    assert!(
        !manifest.exists(),
        "and the run that refused wrote no manifest"
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

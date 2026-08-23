//! Guard tests for `repo_guards::doc_links::audit`.
//!
//! Every fixture is a real crate on disk, and every test feeds it to the real
//! `audit()` — the cargo invocation, the environment scrub, the JSON Lines
//! parse, and the join back onto `cargo metadata` together, rather than a
//! string predicate in isolation. A guard that cannot fail is worse than no
//! guard, because "clean" and "I never looked" print identically.
//!
//! Hermetic: each fixture manifest carries an empty `[workspace]` table, so it
//! detaches from every ancestor workspace, and declares no dependencies, so the
//! documentation build needs no network and no registry.
//!
//! Parallel safety: this workspace's tests share `./target` with the pre-commit
//! hook's own `cargo test`, so two copies of any test here can run at the same
//! moment. Every fixture lives in its own `tempfile::TempDir`, whose name the
//! OS makes unique, and therefore builds into its own target directory.
//! Nothing is keyed on a fixed path under the temp dir, the repo, or the home
//! dir.

use std::fs;
use std::path::Path;

use repo_guards::doc_links;
use tempfile::TempDir;

/// The manifest every fixture carries.
///
/// The empty `[workspace]` table is what detaches the fixture from whatever
/// workspace its temp dir happens to sit under. Without it, cargo walks up the
/// tree and the fixture inherits a lint set and a member list it never asked
/// for.
const FIXTURE_MANIFEST: &str = "\
[workspace]

[package]
name = \"fixture\"
version = \"0.1.0\"
edition = \"2021\"
";

/// A library whose doc comment links to an item that does not exist.
const LINK_TO_NOWHERE: &str = "\
/// Calls [`nowhere`] first.
pub fn thing() {}
";

/// Build a fixture crate in its own temp dir and return the dir.
///
/// The `TempDir` is returned rather than its path, so the caller holds it for
/// the length of the test and the fixture is removed afterwards.
fn fixture(lib_rs: &str) -> TempDir {
    let dir = TempDir::new().expect("create fixture crate dir");
    fs::write(dir.path().join("Cargo.toml"), FIXTURE_MANIFEST)
        .expect("write fixture crate manifest");
    fs::create_dir_all(dir.path().join("src")).expect("create fixture crate src dir");
    fs::write(dir.path().join("src/lib.rs"), lib_rs).expect("write fixture crate library");
    dir
}

/// Scan a fixture and require a verdict, rendering the refusal when one came
/// back instead.
fn scan(dir: &TempDir) -> doc_links::DocScan {
    doc_links::audit(dir.path())
        .unwrap_or_else(|e| panic!("the audit refused a fixture it should have scanned: {e}"))
}

/// The defect this guard exists for: a doc comment links to an item that is not
/// there, every other build step reads the file happily, and the link renders
/// as nothing.
#[test]
fn unresolved_intra_doc_link_is_reported() {
    let dir = fixture(LINK_TO_NOWHERE);

    let scan = scan(&dir);

    assert!(
        !scan.is_clean(),
        "a link to an item that does not exist must be reported, but the scan was clean:\n{scan}"
    );
    let broken = scan.broken();
    assert_eq!(
        broken.len(),
        1,
        "the fixture holds exactly one unresolved link; got:\n{scan}"
    );
    let link = &broken[0];
    assert_eq!(
        link.package(),
        "fixture",
        "the report must name the package"
    );
    assert_eq!(link.target_name(), "fixture");
    assert_eq!(
        link.target_kind(),
        "lib",
        "the report must say which kind of target holds the link"
    );
    assert!(
        link.message().contains("nowhere"),
        "the report must carry rustdoc's own words, got: {}",
        link.message()
    );
    assert_eq!(
        link.file(),
        Path::new("src/lib.rs"),
        "the report must name the file, relative to the workspace root"
    );
    assert_eq!(link.line(), 1, "the report must name the line");
}

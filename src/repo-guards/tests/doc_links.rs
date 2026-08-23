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

/// Source rustdoc cannot parse, so the unit that holds it never documents.
const UNPARSABLE_SOURCE: &str = "pub fn broken( {\n";

/// A library whose one link names two items at once.
///
/// This is the second spelling of the same lint. Rustdoc writes "`trace` is
/// both a function and a module" rather than "unresolved link to …", so a guard
/// that matched the words instead of the code would be blind to it. `krt` holds
/// this exact shape today.
const AMBIGUOUS_LINK: &str = "\
/// Calls [`trace`] first.
pub fn thing() {}

/// A function named the same as the module.
pub fn trace() {}

/// A module named the same as the function.
pub mod trace {}
";

/// A library whose every intra-doc link resolves, and which still raises four
/// other rustdoc lints.
///
/// The four are the four this workspace emits today: `rustdoc::bare_urls` from
/// the bare URL in the crate header, `rustdoc::redundant_explicit_links` from
/// the link that repeats its own target, `rustdoc::invalid_html_tags` from the
/// unclosed `<div>`, and `rustdoc::private_intra_doc_links` from the public
/// item that links to a private one.
///
/// The last of those matters most. It *is* an intra-doc link, and it resolves —
/// rustdoc simply cannot render a page for the private target. A guard that
/// reported every rustdoc warning, or every warning whose text mentions a link,
/// would report all four and be permanently red on this repository.
const ALL_LINKS_RESOLVE: &str = "\
//! Crate docs.
//!
//! see https://example.com

/// Calls [`thing`] first, and [`thing`](thing) again.
///
/// The prose holds a <div> that rustdoc reads as markup.
pub fn other() {}

/// The thing. It uses [`hidden`].
pub fn thing() {}

/// A private helper.
fn hidden() {}
";

/// Build a fixture crate whose library is `lib_rs`, in its own temp dir.
fn fixture(lib_rs: &str) -> TempDir {
    fixture_files(&[("src/lib.rs", lib_rs)])
}

/// Build a fixture crate from `files`, each a path relative to the crate root,
/// in its own temp dir.
///
/// The `TempDir` is returned rather than its path, so the caller holds it for
/// the length of the test and the fixture is removed afterwards.
fn fixture_files(files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().expect("create fixture crate dir");
    fs::write(dir.path().join("Cargo.toml"), FIXTURE_MANIFEST)
        .expect("write fixture crate manifest");
    for (relative, contents) in files {
        let path = dir.path().join(relative);
        let parent = path.parent().expect("a fixture file has a parent dir");
        fs::create_dir_all(parent).expect("create fixture crate source dir");
        fs::write(&path, contents).expect("write fixture crate source file");
    }
    dir
}

/// Scan a fixture and require a verdict, rendering the refusal when one came
/// back instead.
fn scan(dir: &TempDir) -> doc_links::DocScan {
    doc_links::audit(dir.path())
        .unwrap_or_else(|e| panic!("the audit refused a fixture it should have scanned: {e}"))
}

/// Scan a fixture and require a refusal.
///
/// The panic renders the *scan* when one came back, so a fail-closed regression
/// says what the guard wrongly concluded rather than only "expected Err".
fn must_refuse(dir: &TempDir) -> doc_links::DocLinksError {
    match doc_links::audit(dir.path()) {
        Err(e) => e,
        Ok(scan) => panic!(
            "the audit should have refused this fixture, but returned a verdict:\n{scan}\n\
             (clean = {})",
            scan.is_clean()
        ),
    }
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

/// A link that names two items at once is unresolved too, and rustdoc says so
/// under the same lint code in entirely different words.
///
/// This test guards the change the next commit makes. Narrowing the filter from
/// "every rustdoc lint" to one lint code is the right narrowing; narrowing it to
/// the words "unresolved link" is the wrong one, and only this fixture tells the
/// two apart.
#[test]
fn a_link_that_names_two_items_is_reported() {
    let dir = fixture(AMBIGUOUS_LINK);

    let scan = scan(&dir);

    assert_eq!(
        scan.broken().len(),
        1,
        "an ambiguous link is an unresolved link; got:\n{scan}"
    );
    assert!(
        scan.broken()[0].message().contains("both a function"),
        "the report must carry rustdoc's own words, got: {}",
        scan.broken()[0].message()
    );
}

/// The other half of the rule: a rustdoc warning that is not this lint is not a
/// finding.
///
/// This workspace raises `rustdoc::private_intra_doc_links`,
/// `rustdoc::bare_urls`, `rustdoc::redundant_explicit_links`, and
/// `rustdoc::invalid_html_tags` today. A guard that reported every rustdoc
/// warning would be red on the day it landed and would stay red, so it would be
/// switched off, and the workspace would be back where it started.
#[test]
fn other_rustdoc_warnings_are_not_findings() {
    let dir = fixture(ALL_LINKS_RESOLVE);

    let scan = scan(&dir);

    assert!(
        scan.is_clean(),
        "only unresolved links are findings; got:\n{scan}"
    );
}

/// A documentation build that fails is a refusal, never a verdict.
///
/// A unit that fails to document takes every unit downstream of it with it, so
/// the link list is short by an unknown amount. "No broken links" printed over
/// forty crates that were never read is indistinguishable from a guard doing
/// real work.
#[test]
fn a_failed_documentation_build_refuses() {
    let dir = fixture(UNPARSABLE_SOURCE);

    let error = must_refuse(&dir);

    assert!(
        matches!(&error, doc_links::DocLinksError::DocBuildFailed { .. }),
        "a failed documentation build must be a refusal, got: {error}"
    );
    assert!(
        error.to_string().contains("could not document"),
        "the refusal must carry what cargo said, got: {error}"
    );
}

/// The refusal holds even when the build reported findings before it failed.
///
/// This is the whole rule. The fixture documents its library — rustdoc reports
/// the unresolved link in it — and then fails on the binary beside it. A guard
/// that refused only when it had found nothing would print "1 broken link" here
/// and say nothing about the target it never read.
#[test]
fn a_failed_build_refuses_even_when_it_found_a_broken_link() {
    let dir = fixture_files(&[
        ("src/lib.rs", LINK_TO_NOWHERE),
        ("src/bin/other.rs", UNPARSABLE_SOURCE),
    ]);

    let error = must_refuse(&dir);

    assert!(
        matches!(&error, doc_links::DocLinksError::DocBuildFailed { .. }),
        "a partial build must be a refusal however much it found, got: {error}"
    );
}

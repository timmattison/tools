//! Guard tests for `repo_guards::trippy_wall::audit`.
//!
//! Every test builds a real source tree on disk and feeds it to the real
//! `audit_sources()` — the directory walk, the parse, and the syntax-tree walk
//! together, rather than a string predicate in isolation.
//!
//! Each fixture is one *syntactic form* that a trippy mention arrives in. One
//! fixture for the one spelling that prompted the guard is not enough: a form
//! the guard never learned reports *clean*, which reads the same as a guard
//! that does real work.
//!
//! Parallel safety: this workspace's tests share `./target` with the pre-commit
//! hook's own `cargo test`, so two copies of any test here can run at the same
//! moment. Every fixture lives in its own `tempfile::TempDir`, whose name the
//! OS makes unique. Nothing is keyed on a fixed path under the temp dir, the
//! repo, or the home dir.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use repo_guards::trippy_wall::{self, Report, TrippyWallError};
use tempfile::TempDir;

/// The module the fixtures let through, which is the module the real guard
/// lets through.
const ALLOWED: &str = "trace.rs";

/// A module that names nothing, so every fixture has a file the guard must
/// leave alone.
const PLAIN: &str = "fn main() {}\n";

// ---------------------------------------------------------------------------
// One constant per syntactic form a trippy mention arrives in.
//
// The same six drive both halves of the wall: each one is fed to the guard in a
// module that is not allowed, where it must be an offender, and all six
// together are fed to the guard inside `trace.rs`, where they must be clean.
// ---------------------------------------------------------------------------

/// A plain import. The only shape a text search would reliably find.
const USE_DECLARATION: &str = "\
use trippy_core::Builder;

pub fn build_from_use() -> Builder {
    todo!()
}
";

/// A fully-qualified call, written twice on purpose, beside a second path.
/// The report must hold two entries, sorted, not three.
const QUALIFIED_CALL: &str = "\
pub fn build_qualified(addr: IpAddr) {
    let _ = trippy_core::Builder::new(addr);
    let _ = trippy_core::Builder::new(addr);
    let _ = trippy_core::Round::default();
}
";

/// A type in a signature. The mention is a `syn::Type`, never a statement.
const TYPE_IN_SIGNATURE: &str = "\
pub fn summarize(round: &trippy_core::Round<'_>) {}
";

/// An `extern crate`, which holds a bare identifier rather than a path.
const EXTERN_CRATE: &str = "\
extern crate trippy_privilege;
";

/// An identifier inside a macro body. `syn` hands the body over as unparsed
/// tokens, so a path visitor alone never sees this one.
const MACRO_BODY: &str = "\
pub fn show() {
    println!(\"{:?}\", trippy_core::MAX_TTL);
}
";

/// An aliased import. Every later mention in the file says `tc`, so the `use`
/// tree is the only place the wall can catch it.
const ALIASED_IMPORT: &str = "\
use trippy_core as tc;

pub fn build_aliased() -> tc::Builder {
    todo!()
}
";

/// Every form above, in the order they are declared.
const EVERY_FORM: [&str; 6] = [
    USE_DECLARATION,
    QUALIFIED_CALL,
    TYPE_IN_SIGNATURE,
    EXTERN_CRATE,
    MACRO_BODY,
    ALIASED_IMPORT,
];

/// A module whose only mention of trippy is a comment and two string literals,
/// one of them inside a macro body.
///
/// This is the case a text-matching guard gets wrong, and the case that proves
/// the token walk reads an identifier and never a literal.
const COMMENT_AND_LITERAL: &str = "\
// trippy_core::Builder is the type this module never names.
pub fn describe() -> &'static str {
    \"trippy_core::Builder\"
}

pub fn show() {
    println!(\"trippy_core::Builder\");
}
";

/// A module whose paths are rooted at `crate`. Only the first segment of a path
/// names a crate, so a trippy word later in the path names nothing outside this
/// workspace.
const CRATE_ROOTED: &str = "\
use crate::trippy_helper::Thing;

pub fn helper() -> Thing {
    crate::trippy_helper::thing()
}
";

/// Write a source tree in its own temp dir, one file per `(relative, contents)`
/// pair, and return the directory that holds it.
fn tree(files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().expect("a temp dir");
    for (relative, contents) in files {
        let path = dir.path().join(relative);
        let parent = path.parent().expect("a path with a parent");
        fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("cannot create {}: {e}", parent.display()));
        fs::write(&path, contents)
            .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
    }
    dir
}

/// Audit a fixture and require a verdict, not a refusal.
fn audit_fixture(dir: &TempDir) -> Report {
    trippy_wall::audit_sources(dir.path(), ALLOWED).expect("the audit reaches a verdict")
}

/// Audit a fixture and require a refusal. Returning the error lets a caller
/// assert on the variant, and the message renders the *report* when one came
/// back, so a fail-closed regression says what the guard wrongly concluded.
fn audit_must_refuse(dir: &TempDir) -> TrippyWallError {
    match trippy_wall::audit_sources(dir.path(), ALLOWED) {
        Err(e) => e,
        Ok(report) => panic!(
            "the audit should have refused this tree, but returned a verdict:\n{report}\n\
             (compliant = {})",
            report.is_compliant()
        ),
    }
}

/// The offender list as `(path relative to the fixture root, trippy paths)`.
fn offenders(dir: &TempDir, report: &Report) -> Vec<(PathBuf, Vec<String>)> {
    report
        .offenders()
        .iter()
        .map(|offender| {
            (
                offender
                    .path()
                    .strip_prefix(dir.path())
                    .unwrap_or(offender.path())
                    .to_path_buf(),
                offender.trippy_paths().to_vec(),
            )
        })
        .collect()
}

/// Build the one-offender expectation these tests assert against.
fn one_offender(file: &str, trippy_paths: &[&str]) -> Vec<(PathBuf, Vec<String>)> {
    vec![(
        PathBuf::from(file),
        trippy_paths.iter().map(|path| (*path).to_owned()).collect(),
    )]
}

// ---------------------------------------------------------------------------
// Offenders: one test per syntactic form, each in a module that is not allowed
// ---------------------------------------------------------------------------

#[test]
fn a_use_declaration_outside_the_allowed_module_is_an_offender() {
    let dir = tree(&[("main.rs", PLAIN), ("record.rs", USE_DECLARATION)]);

    let report = audit_fixture(&dir);

    assert!(!report.is_compliant(), "an import names the type: {report}");
    assert_eq!(
        offenders(&dir, &report),
        one_offender("record.rs", &["trippy_core::Builder"])
    );
    assert_eq!(report.files_examined(), 2);
}

#[test]
fn a_qualified_call_outside_the_allowed_module_is_an_offender() {
    let dir = tree(&[("main.rs", PLAIN), ("record.rs", QUALIFIED_CALL)]);

    let report = audit_fixture(&dir);

    assert_eq!(
        offenders(&dir, &report),
        one_offender(
            "record.rs",
            &["trippy_core::Builder::new", "trippy_core::Round::default"]
        ),
        "the report names each path once, in sorted order, however often the file names it"
    );
}

#[test]
fn a_type_in_a_signature_outside_the_allowed_module_is_an_offender() {
    let dir = tree(&[("main.rs", PLAIN), ("record.rs", TYPE_IN_SIGNATURE)]);

    let report = audit_fixture(&dir);

    assert_eq!(
        offenders(&dir, &report),
        one_offender("record.rs", &["trippy_core::Round"])
    );
}

#[test]
fn an_extern_crate_outside_the_allowed_module_is_an_offender() {
    let dir = tree(&[("main.rs", PLAIN), ("record.rs", EXTERN_CRATE)]);

    let report = audit_fixture(&dir);

    assert_eq!(
        offenders(&dir, &report),
        one_offender("record.rs", &["trippy_privilege"]),
        "an `extern crate` holds an identifier, which no path visitor sees"
    );
}

#[test]
fn a_macro_body_mention_outside_the_allowed_module_is_an_offender() {
    let dir = tree(&[("main.rs", PLAIN), ("record.rs", MACRO_BODY)]);

    let report = audit_fixture(&dir);

    assert_eq!(
        offenders(&dir, &report),
        one_offender("record.rs", &["trippy_core"]),
        "a macro body arrives as unparsed tokens, so the walk must read them"
    );
}

#[test]
fn an_aliased_import_outside_the_allowed_module_is_an_offender() {
    let dir = tree(&[("main.rs", PLAIN), ("record.rs", ALIASED_IMPORT)]);

    let report = audit_fixture(&dir);

    assert_eq!(
        offenders(&dir, &report),
        one_offender("record.rs", &["trippy_core"]),
        "the alias hides every later mention, so the `use` tree is the only catch"
    );
}

// ---------------------------------------------------------------------------
// Clean: the wall lets the tracer through, and it never fires on text
// ---------------------------------------------------------------------------

#[test]
fn the_allowed_module_carries_every_form() {
    let dir = tree(&[("main.rs", PLAIN), (ALLOWED, &EVERY_FORM.join("\n"))]);

    let report = audit_fixture(&dir);

    assert!(
        report.is_compliant(),
        "the tracer is the one module that names a trippy type: {report}"
    );
    assert_eq!(
        report.files_examined(),
        2,
        "the allowed module is read like every other file"
    );
}

#[test]
fn a_comment_or_a_string_literal_is_not_a_mention() {
    let dir = tree(&[("main.rs", PLAIN), ("record.rs", COMMENT_AND_LITERAL)]);

    let report = audit_fixture(&dir);

    assert!(
        report.is_compliant(),
        "a text search fires on this file, and a parser does not: {report}"
    );
}

#[test]
fn a_path_rooted_at_crate_is_not_a_mention() {
    let dir = tree(&[("main.rs", PLAIN), ("record.rs", CRATE_ROOTED)]);

    let report = audit_fixture(&dir);

    assert!(
        report.is_compliant(),
        "only the first segment of a path names a crate: {report}"
    );
}

#[test]
fn a_file_under_the_allowed_module_directory_carries_a_trippy_path() {
    let dir = tree(&[
        ("main.rs", PLAIN),
        ("trace/mod.rs", USE_DECLARATION),
        ("trace/probe.rs", QUALIFIED_CALL),
    ]);

    let report = audit_fixture(&dir);

    assert!(
        report.is_compliant(),
        "a split of the tracer into submodules keeps the wall where it is: {report}"
    );
    assert_eq!(report.files_examined(), 3);
}

// ---------------------------------------------------------------------------
// The message has to be usable without opening the source
// ---------------------------------------------------------------------------

#[test]
fn the_failure_message_names_the_file_and_the_remedy() {
    let dir = tree(&[("main.rs", PLAIN), ("record.rs", USE_DECLARATION)]);

    let rendered = audit_fixture(&dir).to_string();

    assert!(
        rendered.contains("record.rs names: trippy_core::Builder"),
        "the message must name the file and what it names, got:\n{rendered}"
    );
    assert!(
        rendered.contains("Move that code into trace.rs"),
        "the message must say what to do, got:\n{rendered}"
    );
}

// ---------------------------------------------------------------------------
// Fail closed: none of these may yield a clean verdict
// ---------------------------------------------------------------------------

#[test]
fn an_unparsable_source_refuses() {
    let dir = tree(&[("main.rs", PLAIN), ("record.rs", "pub fn broken( {\n")]);

    let error = audit_must_refuse(&dir);

    assert!(
        matches!(&error, TrippyWallError::Unparsable { path, .. } if path.ends_with("record.rs")),
        "a file the guard cannot parse is a file whose paths it cannot see, got: {error}"
    );
}

#[test]
fn a_directory_with_no_rust_file_refuses() {
    let dir = tree(&[("notes.txt", "no source here\n")]);

    let error = audit_must_refuse(&dir);

    assert!(
        matches!(&error, TrippyWallError::NoSources { dir: named } if named == dir.path()),
        "a guard pointed at the wrong directory must say so, got: {error}"
    );
}

// ---------------------------------------------------------------------------
// The real repository
// ---------------------------------------------------------------------------

/// Absolute, canonical path to this repository's root, derived from the crate
/// being compiled rather than the working directory (which `cargo test` does
/// not pin).
fn repo_root() -> PathBuf {
    canonical(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// Resolve a path to its one true spelling, failing loudly when it cannot be.
///
/// Both sides of the file-set comparison go through this. This repository is
/// reachable as both `/Users/...` and `/Volumes/...`, and `repo_root()` arrives
/// with a `../..` in it, so two names for one file would otherwise read as two
/// files.
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|e| panic!("cannot canonicalize {}: {e}", path.display()))
}

/// The two constants [`trippy_wall::audit`] holds — the source directory it
/// reads, and the module it lets through — are pinned here against a repository
/// built for the purpose.
///
/// Every other fixture calls `audit_sources` and passes both of them in, so a
/// wrong constant inside `audit` would change nothing that those tests see.
/// `src/krt/src/trace.rs` does not exist yet, so the live repository cannot pin
/// them either: a guard that let `tracer.rs` through, or that read
/// `src/krt/source`, would report the same clean verdict it reports today.
#[test]
fn the_repository_audit_reads_krt_and_lets_the_tracer_through() {
    let dir = tree(&[
        ("src/krt/src/main.rs", PLAIN),
        ("src/krt/src/trace.rs", &EVERY_FORM.join("\n")),
        ("src/krt/src/record.rs", USE_DECLARATION),
    ]);

    let report = trippy_wall::audit(dir.path()).expect("the audit reaches a verdict");

    assert_eq!(
        offenders(&dir, &report),
        one_offender("src/krt/src/record.rs", &["trippy_core::Builder"]),
        "the audit must read the sources of krt, and let trace.rs alone"
    );
    assert_eq!(report.files_examined(), 3);
}

#[test]
fn no_module_of_krt_names_a_trippy_type() {
    let report = trippy_wall::audit(&repo_root()).expect("the audit reaches a verdict");

    assert!(
        report.files_examined() > 0,
        "the audit read zero files; a guard that reads nothing reports clean for the wrong reason"
    );
    assert!(report.is_compliant(), "{report}");
}

/// The set of files the guard reads must equal the set that is on disk.
///
/// Every other test here asks what the guard *concludes* about a file. This one
/// asks the prior question, which a verdict cannot: whether the guard found
/// every file there is. A perfect matcher pointed at the wrong directory
/// reports clean with the same silence as a broken matcher, so the file set is
/// enumerated a second time, by a different means, and the two are compared.
#[test]
fn the_guard_reads_exactly_the_krt_sources_on_disk() {
    let root = repo_root();
    let report = trippy_wall::audit(&root).expect("the audit reaches a verdict");

    let read: BTreeSet<PathBuf> = report.files().iter().map(|file| canonical(file)).collect();
    let on_disk = globbed_krt_sources(&root);

    assert!(
        !on_disk.is_empty(),
        "the glob found no source under src/krt/src, so the comparison would prove nothing"
    );
    assert_eq!(
        read, on_disk,
        "the guard reads a different set of files than the one on disk; a file it never \
         opens cannot be reported as naming a trippy type, so the crate comes back clean \
         on the strength of the files that were read"
    );
}

/// Every Rust source under `src/krt/src`, found with a glob rather than with
/// the guard's own directory walk.
///
/// The repo root is escaped before it is joined, so a checkout whose path holds
/// a glob metacharacter still matches itself.
fn globbed_krt_sources(repo_root: &Path) -> BTreeSet<PathBuf> {
    let root = repo_root
        .to_str()
        .unwrap_or_else(|| panic!("{} is not valid UTF-8", repo_root.display()));
    let pattern = format!("{}/src/krt/src/**/*.rs", glob::Pattern::escape(root));

    glob::glob(&pattern)
        .unwrap_or_else(|e| panic!("`{pattern}` is not a valid glob: {e}"))
        .map(|entry| {
            let path = entry.unwrap_or_else(|e| panic!("cannot read a match of `{pattern}`: {e}"));
            canonical(&path)
        })
        .collect()
}

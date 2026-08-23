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
use std::process::Command;

use repo_guards::trippy_wall::{self, Report, TrippyWallError};
use tempfile::TempDir;

/// The package the wall protects, as `cargo metadata` names it.
const KRT_PACKAGE: &str = "krt";

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

// ---------------------------------------------------------------------------
// One constant per visibility a `use` inside the allowed module arrives in.
//
// The forms above ask whether a file names a trippy type. These ask the
// question the allowed module raises instead: whether the name it holds stays
// inside it. A `use` that carries a trippy type out puts that type into every
// module of the crate under a `crate::` name, and the wall promises that an
// upgrade of the trippy crates breaks one file and no other.
// ---------------------------------------------------------------------------

/// A public re-export. The tracer holds the name, and so does every caller of
/// the tracer.
const PUBLIC_RE_EXPORT: &str = "\
pub use trippy_core::Port;
";

/// A crate-visible re-export, under an alias. This is the exact shape that got
/// past the wall: the alias hides the name, and `pub(crate)` still carries the
/// type to every other module of `krt`.
const CRATE_RE_EXPORT: &str = "\
pub(crate) use trippy_core::Port as LeakedPort;
";

/// Every visibility that carries a name out of the module that writes it.
///
/// `pub(super)` and `pub(in ...)` are the two spellings a reader forgets. Each
/// one reaches at least one module that is not the tracer, so each one is the
/// fault the wall exists to stop.
const EVERY_RE_EXPORT_VISIBILITY: [&str; 4] = [
    "pub use trippy_core::Port;\n",
    "pub(crate) use trippy_core::Round as R;\n",
    "pub(super) use trippy_core::Builder;\n",
    "pub(in crate::trace) use trippy_core::MAX_TTL;\n",
];

/// A `use` restricted to the module that writes it. `pub(self)` is the long
/// spelling of private, and it carries nothing out of the tracer.
const SELF_RESTRICTED_USE: &str = "\
pub(self) use trippy_core::Port;
";

/// A public re-export of types this workspace owns. The tracer hands its
/// callers types that `krt` owns, which is the whole point of the wall, so
/// these must not fire.
const OWNED_RE_EXPORT: &str = "\
pub use crate::record::Hop;
pub(crate) use self::inner::Thing;
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
// The allowed module keeps what it holds: a re-export out of it is an offender
// ---------------------------------------------------------------------------

#[test]
fn a_public_re_export_out_of_the_allowed_module_is_an_offender() {
    let dir = tree(&[("main.rs", PLAIN), (ALLOWED, PUBLIC_RE_EXPORT)]);

    let report = audit_fixture(&dir);

    assert!(
        !report.is_compliant(),
        "a public re-export puts the trippy type into every module of the crate: {report}"
    );
    assert_eq!(
        offenders(&dir, &report),
        one_offender(ALLOWED, &["trippy_core::Port"])
    );
}

#[test]
fn a_crate_visible_re_export_out_of_the_allowed_module_is_an_offender() {
    let dir = tree(&[("main.rs", PLAIN), (ALLOWED, CRATE_RE_EXPORT)]);

    let report = audit_fixture(&dir);

    assert!(
        !report.is_compliant(),
        "`pub(crate)` reaches every module of krt, and the alias hides the name: {report}"
    );
    assert_eq!(
        offenders(&dir, &report),
        one_offender(ALLOWED, &["trippy_core::Port"]),
        "the report names the trippy path, not the alias a caller reads"
    );
}

#[test]
fn every_visibility_that_leaves_the_allowed_module_is_an_offender() {
    for source in EVERY_RE_EXPORT_VISIBILITY {
        let dir = tree(&[("main.rs", PLAIN), (ALLOWED, source)]);

        let report = audit_fixture(&dir);

        assert!(
            !report.is_compliant(),
            "`{source}` carries a trippy type out of {ALLOWED}: {report}"
        );
    }
}

#[test]
fn a_private_use_inside_the_allowed_module_is_not_a_re_export() {
    let dir = tree(&[("main.rs", PLAIN), (ALLOWED, USE_DECLARATION)]);

    let report = audit_fixture(&dir);

    assert!(
        report.is_compliant(),
        "a private import is how the tracer names a trippy type, and it carries nothing \
         out: {report}"
    );
}

#[test]
fn a_use_restricted_to_the_allowed_module_is_not_a_re_export() {
    let dir = tree(&[("main.rs", PLAIN), (ALLOWED, SELF_RESTRICTED_USE)]);

    let report = audit_fixture(&dir);

    assert!(
        report.is_compliant(),
        "`pub(self)` is the long spelling of private; no module outside the tracer can \
         name what it holds: {report}"
    );
}

#[test]
fn a_re_export_of_a_type_this_workspace_owns_is_not_an_offender() {
    let dir = tree(&[("main.rs", PLAIN), (ALLOWED, OWNED_RE_EXPORT)]);

    let report = audit_fixture(&dir);

    assert!(
        report.is_compliant(),
        "handing the caller a type this crate owns is the purpose of the wall: {report}"
    );
}

#[test]
fn a_re_export_under_the_allowed_module_directory_is_an_offender() {
    let dir = tree(&[
        ("main.rs", PLAIN),
        ("trace/mod.rs", USE_DECLARATION),
        ("trace/probe.rs", PUBLIC_RE_EXPORT),
    ]);

    let report = audit_fixture(&dir);

    assert_eq!(
        offenders(&dir, &report),
        one_offender("trace/probe.rs", &["trippy_core::Port"]),
        "a split of the tracer into submodules keeps the wall where it is, and a re-export \
         out of one of them reaches the same callers"
    );
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

/// The two faults have two remedies, and a message that gives the wrong one
/// sends a reader to move code that is already in the right file.
#[test]
fn the_failure_message_names_the_re_export_and_its_own_remedy() {
    let dir = tree(&[("main.rs", PLAIN), (ALLOWED, CRATE_RE_EXPORT)]);

    let rendered = audit_fixture(&dir).to_string();

    assert!(
        rendered.contains("trace.rs re-exports: trippy_core::Port"),
        "the message must say the file carries the type out, got:\n{rendered}"
    );
    assert!(
        rendered.contains("Do not re-export a trippy type out of trace.rs"),
        "the message must say what to do, got:\n{rendered}"
    );
    assert!(
        !rendered.contains("Move that code into trace.rs"),
        "the code is already in trace.rs; the remedy of the other fault does not apply, \
         got:\n{rendered}"
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

/// The table [`trippy_wall::audit`] holds — the directories of `krt` it reads,
/// and the module each one lets through — is pinned here against a repository
/// built for the purpose.
///
/// Every other fixture calls `audit_sources` and passes a directory and a
/// module in, so a wrong entry inside `audit` would change nothing that those
/// tests see.
#[test]
fn the_repository_audit_reads_krt_and_lets_the_tracer_through() {
    let dir = tree(&[
        ("src/krt/src/main.rs", PLAIN),
        ("src/krt/src/trace.rs", &EVERY_FORM.join("\n")),
        ("src/krt/src/record.rs", USE_DECLARATION),
        ("src/krt/tests/cli.rs", PLAIN),
    ]);

    let report = trippy_wall::audit(dir.path()).expect("the audit reaches a verdict");

    assert_eq!(
        offenders(&dir, &report),
        one_offender("src/krt/src/record.rs", &["trippy_core::Builder"]),
        "the audit must read the sources of krt, and let trace.rs alone"
    );
    assert_eq!(
        report.files_examined(),
        4,
        "the integration tests of krt are read like every other file"
    );
}

/// An integration test of `krt` is not the tracer, so a trippy type in one is
/// an offender.
///
/// `trippy-core` is an ordinary `[dependencies]` entry, so every target of the
/// package can name a trippy type — the integration tests included. A wall that
/// reads `src/` alone keeps a smaller promise than the one it makes: the rule
/// is that no *file* of `krt` except `trace.rs` names a trippy type, not that
/// no *module* does.
#[test]
fn a_trippy_type_in_an_integration_test_of_krt_is_an_offender() {
    let dir = tree(&[
        ("src/krt/src/main.rs", PLAIN),
        ("src/krt/src/trace.rs", &EVERY_FORM.join("\n")),
        ("src/krt/tests/cli.rs", USE_DECLARATION),
    ]);

    let report = trippy_wall::audit(dir.path()).expect("the audit reaches a verdict");

    assert_eq!(
        offenders(&dir, &report),
        one_offender("src/krt/tests/cli.rs", &["trippy_core::Builder"]),
        "an integration test that names a trippy type breaks on the same upgrade every \
         other file would"
    );
}

/// The tracer is one file in one directory, not a file name that opens the wall
/// wherever it is written.
///
/// `src/krt/tests/trace.rs` is an integration test called `trace`, which cargo
/// builds like any other. A wall that let a file through on its name alone
/// would let this one through, and the exemption would read as a coincidence of
/// naming rather than as a decision.
#[test]
fn a_test_named_like_the_tracer_is_still_an_offender() {
    let dir = tree(&[
        ("src/krt/src/main.rs", PLAIN),
        ("src/krt/src/trace.rs", &EVERY_FORM.join("\n")),
        ("src/krt/tests/trace.rs", USE_DECLARATION),
    ]);

    let report = trippy_wall::audit(dir.path()).expect("the audit reaches a verdict");

    assert_eq!(
        offenders(&dir, &report),
        one_offender("src/krt/tests/trace.rs", &["trippy_core::Builder"]),
        "the wall lets one file through, and that file lives in the source directory"
    );
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

/// Every target root cargo builds for `krt` must be a file the guard read.
///
/// Every other test here asks what the guard *concludes* about a file. This one
/// asks the prior question, which a verdict cannot: whether the guard looked
/// where the code is. A perfect matcher pointed at three of four directories
/// reports clean with the same silence as a broken matcher.
///
/// The guard keeps its own cheap directory walk, because a guard that shells
/// out to cargo on every run is a guard nobody keeps in a test suite. So the
/// true set is asked of `cargo metadata` here instead, exactly as the sibling
/// guards `target_lints` and `tool_index` do. A target kind nobody taught the
/// guard about — a bench, an example, a second test directory — then shows up
/// as a set difference rather than as a clean report.
#[test]
fn every_target_root_of_krt_is_a_file_the_guard_read() {
    let root = repo_root();
    let report = trippy_wall::audit(&root).expect("the audit reaches a verdict");

    let read: BTreeSet<PathBuf> = report.files().iter().map(|file| canonical(file)).collect();
    let target_roots = cargo_target_roots_of_krt(&root);

    let missed: BTreeSet<PathBuf> = target_roots.difference(&read).cloned().collect();
    assert!(
        missed.is_empty(),
        "cargo builds {} target root(s) of krt that the guard never read:\n{}\n\
         Every one of them can name a trippy type, because trippy-core is an ordinary \
         dependency of the package. A file the guard does not open cannot be reported as \
         naming one, so krt comes back clean on the strength of the files that were read.",
        missed.len(),
        render_paths(&missed, &root)
    );
}

/// The set of files the guard reads must equal the set that is on disk.
///
/// A target root is one file, and cargo names only that one. The modules beside
/// it are the rest of the directory, so the comparison set here is every Rust
/// source in the directory of every root cargo reports — enumerated with a
/// glob, rather than with the guard's own directory walk, so the two answers
/// are independent. Both halves are derived from `cargo metadata`: a directory
/// the guard reads and cargo builds nothing from, and a directory cargo builds
/// from and the guard never opens, are both a difference of these two sets.
#[test]
fn the_guard_reads_exactly_the_krt_sources_on_disk() {
    let root = repo_root();
    let report = trippy_wall::audit(&root).expect("the audit reaches a verdict");

    let read: BTreeSet<PathBuf> = report.files().iter().map(|file| canonical(file)).collect();
    let on_disk = rust_sources_beside(&cargo_target_roots_of_krt(&root));

    assert!(
        !on_disk.is_empty(),
        "the glob found no source beside a target root of krt, so the comparison would \
         prove nothing"
    );
    assert_eq!(
        read, on_disk,
        "the guard reads a different set of files than the one on disk; a file it never \
         opens cannot be reported as naming a trippy type, so the crate comes back clean \
         on the strength of the files that were read"
    );
}

/// The `src_path` of every target `cargo metadata` reports for `krt`,
/// canonicalized.
///
/// This is the *ground truth* the guard is measured against. Asking cargo is
/// what turns "the guard found nothing wrong" into a claim about the files that
/// exist, rather than about the files the guard happened to look for.
///
/// `--no-deps` keeps the answer to this workspace's own packages, and is
/// deliberately not paired with `--offline`: resolution is skipped either way,
/// and `--offline` only adds a way for a cold cache to fail.
///
/// Every step here fails loudly. A parity test that skipped when cargo could
/// not be spawned, or compared against a set it quietly failed to parse, would
/// be an instance of the very defect it exists to catch: a check that reports
/// clean because it never looked.
fn cargo_target_roots_of_krt(repo_root: &Path) -> BTreeSet<PathBuf> {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(repo_root)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "cannot run `cargo metadata` in {}: {e}",
                repo_root.display()
            )
        });

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "`cargo metadata` in {} exited with {}:\n{stderr}",
        repo_root.display(),
        output.status
    );

    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!("cannot parse the output of `cargo metadata` as JSON: {e}\nstderr:\n{stderr}")
    });

    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| {
            panic!("`cargo metadata` returned no `packages` array\nstderr:\n{stderr}")
        });

    let krt = packages
        .iter()
        .find(|package| {
            package.get("name").and_then(serde_json::Value::as_str) == Some(KRT_PACKAGE)
        })
        .unwrap_or_else(|| {
            panic!(
                "`cargo metadata` reports no package named {KRT_PACKAGE}; the wall guards a \
                    package that is no longer in this workspace"
            )
        });

    let roots: BTreeSet<PathBuf> = krt
        .get("targets")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("the package {KRT_PACKAGE} has no `targets` array: {krt}"))
        .iter()
        .map(|target| {
            let src_path = target
                .get("src_path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("a target of {KRT_PACKAGE} has no `src_path`: {target}"));
            canonical(Path::new(src_path))
        })
        .collect();

    assert!(
        !roots.is_empty(),
        "`cargo metadata` reports no target at all for {KRT_PACKAGE}, so the comparison \
         would prove nothing"
    );
    roots
}

/// Every Rust source in the directory of every root, at any depth, found with a
/// glob rather than with the guard's own directory walk.
///
/// Each directory is escaped before it is joined, so a checkout whose path holds
/// a glob metacharacter still matches itself.
fn rust_sources_beside(roots: &BTreeSet<PathBuf>) -> BTreeSet<PathBuf> {
    let directories: BTreeSet<&Path> = roots
        .iter()
        .map(|root| {
            root.parent()
                .unwrap_or_else(|| panic!("the target root {} has no directory", root.display()))
        })
        .collect();

    let mut sources = BTreeSet::new();
    for directory in directories {
        let text = directory
            .to_str()
            .unwrap_or_else(|| panic!("{} is not valid UTF-8", directory.display()));
        let pattern = format!("{}/**/*.rs", glob::Pattern::escape(text));
        for entry in
            glob::glob(&pattern).unwrap_or_else(|e| panic!("`{pattern}` is not a valid glob: {e}"))
        {
            let path = entry.unwrap_or_else(|e| panic!("cannot read a match of `{pattern}`: {e}"));
            sources.insert(canonical(&path));
        }
    }
    sources
}

/// Render a set of absolute paths as one indented repo-relative path per line,
/// so a failure reads as a list of files rather than as two sets to diff by eye.
fn render_paths(paths: &BTreeSet<PathBuf>, repo_root: &Path) -> String {
    paths
        .iter()
        .map(|path| {
            format!(
                "    {}",
                path.strip_prefix(repo_root).unwrap_or(path).display()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

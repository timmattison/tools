//! Guard tests for `repo_guards::op_wall`.
//!
//! Every test builds a real source tree on disk and feeds it to the real
//! `audit_sources()` — the directory walk, the parse, and the syntax-tree walk
//! together, rather than a string predicate in isolation.
//!
//! Each fixture is one *syntactic form* that a spawn of the `op` binary arrives
//! in. One fixture for the one spelling that prompted the guard is not enough:
//! a form the guard never learned reports *clean*, which reads the same as a
//! guard that does real work.
//!
//! Parallel safety: this workspace's tests share `./target` with the pre-commit
//! hook's own `cargo test`, so two copies of any test here can run at the same
//! moment. Every fixture lives in its own `tempfile::TempDir`, whose name the
//! OS makes unique. Nothing is keyed on a fixed path under the temp dir, the
//! repo, or the home dir.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use repo_guards::op_wall::{self, OpWallError};
use repo_guards::workspace_lints;
use tempfile::TempDir;

/// The crate that is allowed to run `op`, relative to the repository root.
const OP_CACHE: &str = "src/op-cache";

/// The file the finding that prompted this guard was written about. The guard
/// must have read it, or the workspace comes back clean because nobody looked.
const THE_FILE_THE_RULE_WAS_WRITTEN_FOR: &str = "src/ufa/src/config.rs";

/// A module that runs nothing, so every fixture holds a file the guard must
/// leave alone.
const PLAIN: &str = "pub fn main() {}\n";

// ---------------------------------------------------------------------------
// One constant per syntactic form a spawn of `op` arrives in.
// ---------------------------------------------------------------------------

/// The spelling the offender used. The only shape a text search would reliably
/// find.
const FULLY_QUALIFIED: &str = "\
pub fn controllers() {
    let _ = std::process::Command::new(\"op\")
        .args([\"item\", \"get\", \"ufa\"])
        .output();
}
";

/// The same call behind an import, which a search for the qualified spelling
/// never sees.
const IMPORTED: &str = "\
use std::process::Command;

pub fn controllers() {
    let _ = Command::new(\"op\").arg(\"whoami\").output();
}
";

/// The asynchronous type of the same name. A different module path in front of
/// the same two segments.
const ASYNC: &str = "\
pub async fn controllers() {
    let _ = tokio::process::Command::new(\"op\").output().await;
}
";

/// The same binary named by its path. A search for the literal `\"op\"` never
/// sees it.
const NAMED_BY_PATH: &str = "\
use std::process::Command;

pub fn controllers() {
    let _ = Command::new(\"/opt/homebrew/bin/op\").arg(\"whoami\").output();
}
";

/// A call inside a macro body. `syn` hands a macro body over as unparsed
/// tokens, so the call check never reaches it.
const INSIDE_A_MACRO: &str = "\
use std::process::Command;

pub fn controllers() {
    println!(\"{:?}\", Command::new(\"op\").arg(\"whoami\").output());
}
";

/// Everything that names `op` and runs nothing.
///
/// `which` locates the binary. `op-cache` is the sanctioned route and is a
/// different program. A JSON patch document carries an `\"op\"` key, and it is
/// a macro body, which is where the loosest of the three checks reads.
const RUNS_SOMETHING_ELSE: &str = "\
use std::process::Command;

pub fn controllers() {
    let _ = which::which(\"op\");
    let _ = Command::new(\"op-cache\").args([\"read\", \"op://Private/ufa/key\"]).output();
    let _ = Command::new(\"git\").arg(\"status\").output();
    let _ = serde_json::json!({ \"op\": \"replace\", \"path\": \"/a\" });
}
";

/// Write `sources` into a fresh temporary directory, one file per entry.
///
/// # Arguments
///
/// * `sources` - The file name and the content of each source.
///
/// # Returns
///
/// The directory, which deletes itself when the test ends.
fn tree(sources: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().expect("a temporary directory");
    for (name, content) in sources {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("the parent of a fixture source");
        }
        fs::write(&path, content).expect("a fixture source");
    }
    dir
}

/// The repository this workspace lives in.
fn repo_root() -> PathBuf {
    canonical(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// `path`, with every symbolic link resolved.
///
/// This checkout is reachable as both `/Users/...` and `/Volumes/...`, so two
/// paths that name one file compare unequal until both are resolved.
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|error| panic!("{} exists: {error}", path.display()))
}

/// Every syntactic form of a spawn is reported, and the file that holds it is
/// named.
#[test]
fn every_spelling_of_a_spawn_is_an_offender() {
    for (form, source) in [
        ("fully qualified", FULLY_QUALIFIED),
        ("imported", IMPORTED),
        ("asynchronous", ASYNC),
        ("named by path", NAMED_BY_PATH),
        ("inside a macro", INSIDE_A_MACRO),
    ] {
        let dir = tree(&[("lib.rs", source), ("plain.rs", PLAIN)]);
        let report = op_wall::audit_sources(dir.path()).expect("the audit reaches a verdict");

        assert_eq!(
            report.files_examined(),
            2,
            "the {form} fixture holds two sources"
        );
        assert!(
            !report.is_compliant(),
            "the {form} spelling runs the op binary, got {report}"
        );
        assert_eq!(
            report.offenders().len(),
            1,
            "one file of the {form} fixture runs it, got {report}"
        );
        assert!(
            report.offenders()[0].path().ends_with("lib.rs"),
            "the offender must be named, got {report}"
        );
        assert!(
            !report.offenders()[0].spawns().is_empty(),
            "the report must say what the file runs, got {report}"
        );
    }
}

/// Naming the binary is not running it, and a program of another name is
/// another program.
#[test]
fn a_file_that_runs_something_else_is_left_alone() {
    let dir = tree(&[("lib.rs", RUNS_SOMETHING_ELSE), ("plain.rs", PLAIN)]);
    let report = op_wall::audit_sources(dir.path()).expect("the audit reaches a verdict");

    assert!(
        report.is_compliant(),
        "locating op, running op-cache, running git, and a JSON patch key run no 1Password \
         CLI, got {report}"
    );
    assert_eq!(report.files_examined(), 2, "both sources must be read");
}

/// A file the guard cannot parse is a file whose calls it cannot see, which is
/// not the same as a file that holds none.
#[test]
fn a_source_that_is_not_rust_is_refused() {
    let dir = tree(&[("lib.rs", "pub fn broken( {")]);

    let error = op_wall::audit_sources(dir.path()).expect_err("a source that will not parse");
    assert!(
        matches!(error, OpWallError::Unparsable { .. }),
        "an unparsable source must refuse the audit, got {error}"
    );
}

/// "I examined nothing" reads exactly like "everything is clean".
#[test]
fn a_directory_with_no_rust_source_is_refused() {
    let dir = tree(&[("README.md", "no sources here\n")]);

    let error = op_wall::audit_sources(dir.path()).expect_err("a directory with no source");
    assert!(
        matches!(error, OpWallError::NoSources { .. }),
        "a directory with no Rust source must refuse the audit, got {error}"
    );
}

/// A directory that is not there cannot be listed, and a guard pointed at one
/// must say so.
#[test]
fn a_directory_that_is_not_there_is_refused() {
    let dir = tree(&[("plain.rs", PLAIN)]);

    let error = op_wall::audit_sources(&dir.path().join("nowhere"))
        .expect_err("a directory that is not there");
    assert!(
        matches!(error, OpWallError::ReadDir { .. }),
        "a directory that cannot be listed must refuse the audit, got {error}"
    );
}

/// A fixture is data a test reads, not code a crate compiles, and this
/// workspace keeps Rust there that is deliberately not valid Rust. Reading one
/// as a source refuses every run of the guard, on a fault that is the whole
/// point of the file.
///
/// The cost of that rule is the second half of this test: a spawn inside a
/// fixture is invisible. The rule is stated so a reader sees the hole.
#[test]
fn a_fixture_is_neither_read_nor_parsed() {
    let dir = tree(&[
        ("lib.rs", PLAIN),
        ("fixtures/rust/syntax_error.rs", "pub fn broken( {"),
        ("fixtures/rust/spawn.rs", IMPORTED),
    ]);

    let report = op_wall::audit_sources(dir.path()).expect("a fixture must not refuse the audit");

    assert_eq!(
        report.files_examined(),
        1,
        "only the source outside the fixtures may be read, got {report}"
    );
    assert!(report.is_compliant(), "{report}");
}

/// The rule itself, against the workspace it protects.
#[test]
fn only_op_cache_runs_the_op_binary() {
    let report = op_wall::audit(&repo_root()).expect("the audit reaches a verdict");

    assert!(report.is_compliant(), "{report}");
}

/// A perfect matcher pointed at the wrong directories reports clean with the
/// same silence as a broken one. Every member but `op-cache` must contribute a
/// file the guard read, and `op-cache` must contribute none.
#[test]
fn every_member_but_op_cache_is_read() {
    let root = repo_root();
    let report = op_wall::audit(&root).expect("the audit reaches a verdict");

    let read: BTreeSet<PathBuf> = report.files().iter().map(|file| canonical(file)).collect();
    let op_cache = canonical(&root.join(OP_CACHE));

    let mut unread = Vec::new();
    for member in workspace_lints::members(&root).expect("the workspace members") {
        let member = canonical(&member);
        let covered = read.iter().any(|file| file.starts_with(&member));

        if member == op_cache {
            assert!(
                !covered,
                "the crate that owns the op binary must not be audited, or the guard reports \
                 its own two calls"
            );
        } else if !covered {
            unread.push(member);
        }
    }

    assert!(
        unread.is_empty(),
        "the guard read no file of {} workspace member(s), so those crates come back clean \
         because nobody looked:\n{}",
        unread.len(),
        unread
            .iter()
            .map(|member| format!("  {}", member.display()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The finding that prompted this guard was about one file. A guard that never
/// opens it would have reported the workspace clean on the day it was written.
#[test]
fn the_guard_reads_the_file_the_rule_was_written_for() {
    let root = repo_root();
    let report = op_wall::audit(&root).expect("the audit reaches a verdict");

    let wanted = canonical(&root.join(THE_FILE_THE_RULE_WAS_WRITTEN_FOR));
    assert!(
        report.files().iter().any(|file| canonical(file) == wanted),
        "the guard never read {THE_FILE_THE_RULE_WAS_WRITTEN_FOR}, which held the call this \
         rule was written about"
    );
}

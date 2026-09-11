//! Guard tests for `repo_guards::git_env_sweep::audit`.
//!
//! Every test builds a real workspace on disk and feeds it to the real
//! `audit()` — member enumeration, the source walk, and the matcher together,
//! rather than a string predicate in isolation.
//!
//! The fixtures are organised by *syntactic form*, not by the one spelling that
//! prompted the guard. A named removal can arrive as a bare literal, behind a
//! reference, inside a macro body, or through a constant, and each of those is
//! a separate way for a matcher to go quiet. The negative half matters as much:
//! a comment, a doc comment, and a string that merely *spells* the call are all
//! prose, and a guard that reads prose as code gets deleted by whoever it
//! blocks first.
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

use repo_guards::git_env_sweep::{self, GitEnvSweepError};
use tempfile::TempDir;

/// Build a workspace on disk holding one member crate, whose `src/lib.rs` is
/// `source`.
fn workspace(source: &str) -> TempDir {
    workspace_at("src/one/src/lib.rs", source)
}

/// Build a workspace on disk holding one member crate, with `source` written to
/// `relative` — a path from the workspace root, so a test can place a file
/// exactly where an exemption names it.
fn workspace_at(relative: &str, source: &str) -> TempDir {
    let dir = TempDir::new().expect("a temp dir");
    let root = dir.path();

    write(root, "Cargo.toml", "[workspace]\nmembers = [\"src/*\"]\n");

    let member = Path::new(relative)
        .parent()
        .and_then(Path::parent)
        .expect("a path of the shape src/<crate>/src/<file>.rs");
    write(
        &root.join(member),
        "Cargo.toml",
        "[package]\nname = \"one\"\nedition = \"2021\"\n",
    );
    write(root, relative, source);

    dir
}

/// Write `contents` to `dir/relative`, creating the parent directories.
fn write(dir: &Path, relative: &str, contents: &str) {
    let path = dir.join(relative);
    let parent = path.parent().expect("a path with a parent");
    fs::create_dir_all(parent)
        .unwrap_or_else(|e| panic!("cannot create {}: {e}", parent.display()));
    fs::write(&path, contents).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

/// The verdict for a one-crate workspace whose only source file is `source`.
fn verdict(source: &str) -> git_env_sweep::Report {
    let ws = workspace(source);
    git_env_sweep::audit(ws.path()).expect("the audit reaches a verdict")
}

/// Every variable the report says is removed by name, across all offenders.
fn named(report: &git_env_sweep::Report) -> Vec<String> {
    report
        .offenders()
        .iter()
        .flat_map(|offender| offender.variables().iter().cloned())
        .collect()
}

// ---------------------------------------------------------------------------
// The forms a named removal arrives in. Each must be reported.
// ---------------------------------------------------------------------------

#[test]
fn a_named_removal_is_reported() {
    let report = verdict(
        "pub fn run(command: &mut std::process::Command) {\n    \
         command.env_remove(\"GIT_DIR\");\n}\n",
    );

    assert_eq!(named(&report), ["GIT_DIR".to_owned()], "{report}");
    assert!(!report.is_compliant(), "{report}");
    assert_eq!(
        report.offenders()[0].path(),
        Path::new("src/one/src/lib.rs"),
        "the offender is named by its path relative to the workspace root"
    );
}

#[test]
fn every_named_removal_of_a_builder_chain_is_reported() {
    let report = verdict(
        "pub fn run() -> std::process::Command {\n    \
         let mut command = std::process::Command::new(\"git\");\n    \
         command\n        .env_remove(\"GIT_DIR\")\n        \
         .env_remove(\"GIT_WORK_TREE\")\n        .env_remove(\"GIT_INDEX_FILE\");\n    \
         command\n}\n",
    );

    assert_eq!(
        named(&report),
        [
            "GIT_DIR".to_owned(),
            "GIT_INDEX_FILE".to_owned(),
            "GIT_WORK_TREE".to_owned(),
        ],
        "a chain of removals is a chain of violations, not one: {report}"
    );
}

#[test]
fn a_named_removal_behind_a_reference_is_reported() {
    let report = verdict(
        "pub fn run(command: &mut std::process::Command) {\n    \
         command.env_remove(&\"GIT_DIR\");\n}\n",
    );

    assert_eq!(named(&report), ["GIT_DIR".to_owned()], "{report}");
}

#[test]
fn a_named_removal_inside_a_macro_body_is_reported() {
    // The reason this guard reads tokens rather than the syntax tree: `syn`
    // leaves a macro's body as an unparsed stream, so a tree walk sees nothing
    // here at all.
    let report = verdict(
        "pub fn run(command: &mut std::process::Command) {\n    \
         assert!({ command.env_remove(\"GIT_DIR\"); true });\n}\n",
    );

    assert_eq!(named(&report), ["GIT_DIR".to_owned()], "{report}");
}

#[test]
fn a_named_removal_inside_a_macro_definition_is_reported() {
    let report = verdict(
        "#[macro_export]\nmacro_rules! scrub {\n    ($command:expr) => {\n        \
         $command.env_remove(\"GIT_DIR\")\n    };\n}\n",
    );

    assert_eq!(
        named(&report),
        ["GIT_DIR".to_owned()],
        "a macro that expands to a named removal writes one at every call site: {report}"
    );
}

#[test]
fn a_named_removal_built_by_a_macro_is_reported() {
    let report = verdict(
        "pub fn run(command: &mut std::process::Command, name: &str) {\n    \
         command.env_remove(format!(\"GIT_{name}\"));\n}\n",
    );

    assert_eq!(
        named(&report),
        ["GIT_{name}".to_owned()],
        "a name assembled from a GIT_ literal is still a name, and the literal is what the \
         report quotes back: {report}"
    );
}

#[test]
fn a_named_removal_through_an_associated_function_is_reported() {
    let report = verdict(
        "pub fn run(command: &mut std::process::Command) {\n    \
         std::process::Command::env_remove(command, \"GIT_PREFIX\");\n}\n",
    );

    assert_eq!(
        named(&report),
        ["GIT_PREFIX".to_owned()],
        "the call is the violation, not one spelling of it: {report}"
    );
}

// ---------------------------------------------------------------------------
// What is not a violation. Each of these would be one to a text search.
// ---------------------------------------------------------------------------

#[test]
fn a_removal_of_a_key_read_from_the_environment_is_not_reported() {
    // The shape of the shared sweep. It passes a key it read rather than a
    // name it knows, which is the whole difference between a rule and a list.
    let report = verdict(
        "pub fn sweep(command: &mut std::process::Command) {\n    \
         for (key, _) in std::env::vars_os() {\n        \
         if key.to_string_lossy().starts_with(\"GIT_\") {\n            \
         command.env_remove(&key);\n        }\n    }\n}\n",
    );

    assert!(
        report.is_compliant(),
        "the sweep must not trip the rule it implements: {report}"
    );
}

#[test]
fn a_removal_of_a_variable_that_is_not_gits_is_not_reported() {
    let report = verdict(
        "pub fn run(command: &mut std::process::Command) {\n    \
         command.env_remove(\"NO_COLOR\");\n    command.env_remove(\"COLUMNS\");\n}\n",
    );

    assert!(report.is_compliant(), "{report}");
}

#[test]
fn a_named_removal_written_in_a_comment_is_not_reported() {
    let report = verdict(
        "// Once this read command.env_remove(\"GIT_DIR\") and it was wrong.\n\
         /// Replaces `command.env_remove(\"GIT_WORK_TREE\")` with the sweep.\n\
         pub fn run() {}\n",
    );

    assert!(
        report.is_compliant(),
        "prose that names the call is data, not code: {report}"
    );
}

#[test]
fn a_named_removal_inside_a_string_is_not_reported() {
    let report =
        verdict("pub const ADVICE: &str = \"do not write command.env_remove(\\\"GIT_DIR\\\")\";\n");

    assert!(
        report.is_compliant(),
        "the text of a string is one token, not the words it spells: {report}"
    );
}

#[test]
fn a_macro_of_the_same_name_is_not_a_call() {
    let report = verdict(
        "macro_rules! env_remove {\n    ($name:expr) => {\n        $name\n    };\n}\n\
         pub fn run() -> &'static str {\n    env_remove!(\"GIT_DIR\")\n}\n",
    );

    assert!(
        report.is_compliant(),
        "a `!` between the name and the arguments makes it a macro, not the method: {report}"
    );
}

#[test]
fn setting_a_git_variable_is_not_a_removal() {
    let report = verdict(
        "pub fn run(command: &mut std::process::Command) {\n    \
         command.env(\"GIT_CONFIG_GLOBAL\", \"/dev/null\");\n}\n",
    );

    assert!(
        report.is_compliant(),
        "pinning a variable is a decision, and only removing one by name is the defect: {report}"
    );
}

// ---------------------------------------------------------------------------
// The exemptions. Keyed on (file, variable), so one excused removal does not
// excuse the file that holds it.
// ---------------------------------------------------------------------------

/// The file both entries of `EXEMPTIONS` name.
const EXEMPT_FILE: &str = "src/gitscratch/src/git.rs";

#[test]
fn an_exempt_removal_is_not_reported() {
    let ws = workspace_at(
        EXEMPT_FILE,
        "pub fn run(command: &mut std::process::Command) {\n    \
         command.env_remove(\"GIT_AUTHOR_DATE\");\n    \
         command.env_remove(\"GIT_COMMITTER_DATE\");\n}\n",
    );

    let report = git_env_sweep::audit(ws.path()).expect("the audit reaches a verdict");

    assert!(report.is_compliant(), "{report}");
    assert!(
        report.unused_exemptions().is_empty(),
        "both entries matched, so neither is stale: {:?}",
        report.unused_exemptions()
    );
}

#[test]
fn a_new_named_removal_in_an_exempt_file_is_reported() {
    let ws = workspace_at(
        EXEMPT_FILE,
        "pub fn run(command: &mut std::process::Command) {\n    \
         command.env_remove(\"GIT_AUTHOR_DATE\");\n    \
         command.env_remove(\"GIT_COMMITTER_DATE\");\n    \
         command.env_remove(\"GIT_DIR\");\n}\n",
    );

    let report = git_env_sweep::audit(ws.path()).expect("the audit reaches a verdict");

    assert_eq!(
        named(&report),
        ["GIT_DIR".to_owned()],
        "an exemption excuses one variable in one file, never the file: {report}"
    );
}

#[test]
fn an_exemption_that_matches_nothing_is_reported() {
    let report = verdict("pub fn run() {}\n");

    let stale: Vec<&str> = report
        .unused_exemptions()
        .iter()
        .map(|exemption| exemption.variable)
        .collect();
    assert_eq!(
        stale,
        ["GIT_AUTHOR_DATE", "GIT_COMMITTER_DATE"],
        "a workspace holding neither removal leaves both entries unused"
    );
}

// ---------------------------------------------------------------------------
// Refusals. Everything that could shrink the read set is an error rather than
// a clean verdict.
// ---------------------------------------------------------------------------

#[test]
fn a_source_file_that_is_not_rust_refuses() {
    let ws = workspace("pub fn run( {\n");

    let error = git_env_sweep::audit(ws.path()).expect_err("an unparsable file is a refusal");

    assert!(
        matches!(error, GitEnvSweepError::ParseSource { .. }),
        "expected a parse refusal, got {error}"
    );
}

#[test]
fn a_workspace_with_no_rust_refuses() {
    let dir = TempDir::new().expect("a temp dir");
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"src/*\"]\n",
    );
    write(
        &dir.path().join("src/one"),
        "Cargo.toml",
        "[package]\nname = \"one\"\nedition = \"2021\"\n",
    );

    let error = git_env_sweep::audit(dir.path()).expect_err("an empty read set is a refusal");

    assert!(
        matches!(error, GitEnvSweepError::NoSourceFiles { .. }),
        "expected an empty-read-set refusal, got {error}"
    );
}

#[test]
fn a_directory_of_build_artifacts_is_not_read() {
    let ws = workspace("pub fn run() {}\n");
    let generated = ws.path().join("src/one/target");
    write(&generated, "CACHEDIR.TAG", "Signature: 8a477f597d28d172\n");
    write(&generated, "left-by-a-build.rs", "fn x( {\n");

    let report = git_env_sweep::audit(ws.path()).expect("the audit reaches a verdict");

    assert!(
        report
            .files()
            .iter()
            .all(|file| !file.starts_with("src/one/target")),
        "a directory carrying CACHEDIR.TAG holds artifacts, not source: {:?}",
        report.files()
    );
}

// ---------------------------------------------------------------------------
// Scope. A perfect matcher pointed at part of the workspace reports clean for
// the wrong reason, with the same silence as one that works.
// ---------------------------------------------------------------------------

/// Absolute, canonical path to this repository's root, derived from the crate
/// being compiled rather than the working directory, which `cargo test` does
/// not pin.
fn repo_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::canonicalize(&root)
        .unwrap_or_else(|e| panic!("cannot canonicalize {}: {e}", root.display()))
}

#[test]
fn the_read_set_holds_every_target_root_cargo_builds() {
    let root = repo_root();

    let read: BTreeSet<PathBuf> = git_env_sweep::source_files(&root)
        .expect("the source files are enumerable")
        .into_iter()
        .collect();
    let cargo = cargo_target_roots(&root);

    let unread: Vec<&PathBuf> = cargo.difference(&read).collect();
    assert!(
        unread.is_empty(),
        "cargo compiles these roots and the guard never reads them, so a named removal in one \
         would be invisible: {unread:?}"
    );
    assert!(
        read.len() > cargo.len(),
        "the guard reads modules as well as roots, so its set must be the larger one; \
         read {} files against {} roots",
        read.len(),
        cargo.len()
    );
}

/// Every target root `cargo metadata` reports for the workspace at `repo_root`,
/// relative to it.
///
/// This is the *ground truth* the read set is measured against. Asking cargo is
/// what turns "the guard found nothing wrong" into a claim about the code that
/// exists, rather than about the code the guard happened to open.
fn cargo_target_roots(repo_root: &Path) -> BTreeSet<PathBuf> {
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
        .unwrap_or_else(|| panic!("`cargo metadata` reported no `packages` array"));

    let mut roots = BTreeSet::new();
    for package in packages {
        let targets = package
            .get("targets")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("a package in `cargo metadata` has no `targets` array"));
        for target in targets {
            let src = target
                .get("src_path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("a target in `cargo metadata` has no `src_path`"));
            let path = Path::new(src);
            roots.insert(path.strip_prefix(repo_root).unwrap_or(path).to_path_buf());
        }
    }

    assert!(
        !roots.is_empty(),
        "`cargo metadata` reported no target roots for {}",
        repo_root.display()
    );
    roots
}

//! Guard tests for `repo_guards::clippy_superset::audit`.
//!
//! The headline test — [`every_crate_clippy_config_restates_the_workspace_bans`]
//! — runs the audit against this repository. It was red when this file was
//! written: `src/ufa/clippy.toml` had just been added to ban clap's parse entry
//! points, and clippy reads the nearest configuration file and no other, so
//! `ufa` had silently stopped inheriting the workspace's ban on the `colored`
//! crate's process-global override and on the two window-size calls that report
//! a zero. The commit that follows restates them.
//!
//! [`the_walk_finds_every_config_beside_a_member_manifest`] is the companion
//! test the sibling guards carry. The audit's file set is derived twice — once
//! by walking the repository, once from `workspace_lints::members` — and the
//! two are compared as sets, so a walk blind to a crate shows up as a set
//! difference rather than as a clean report.
//!
//! Every other test is a mutation test. A guard that cannot fail is worse than
//! no guard, because "clean" and "I never looked" print identically. So each
//! shape is built as a real repository on disk and fed to the real `audit()` —
//! enumeration, parsing and comparison together, not a predicate in isolation —
//! and each fail-closed condition is asserted to produce an `Err` rather than a
//! clean verdict.
//!
//! Parallel safety: this workspace's tests share `./target` with the pre-commit
//! hook's own `cargo test`, so two copies of any test here can run at the same
//! moment. Every fixture lives in its own `tempfile::TempDir`, whose name the
//! OS makes unique; nothing is keyed on a fixed path under the temp dir, the
//! repo, or the home dir.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use repo_guards::clippy_superset::{self, ClippySupersetError, Report, CONFIG_NAMES};
use repo_guards::workspace_lints;
use tempfile::TempDir;

/// A root configuration with two bans, standing in for the real one.
const ROOT_CONFIG: &str = r#"disallowed-methods = [
    { path = "colored::control::set_override", reason = "call testcolor::with_forced_ansi instead" },
    { path = "crossterm::terminal::size", reason = "call termsize::controlling_size instead" },
]
"#;

/// A crate file that restates both root bans and adds one of its own. This is
/// the shape the guard exists to require.
const SUPERSET_CONFIG: &str = r#"disallowed-methods = [
    { path = "colored::control::set_override", reason = "call testcolor::with_forced_ansi instead" },
    { path = "crossterm::terminal::size", reason = "call termsize::controlling_size instead" },
    { path = "clap::Parser::try_parse_from", reason = "parse through the environment lock" },
]
"#;

/// A crate file that declares only its own ban. This is the shape `ufa` shipped
/// with, and the one that quietly turns the root bans off for that crate.
const REPLACEMENT_CONFIG: &str = r#"disallowed-methods = [
    { path = "clap::Parser::try_parse_from", reason = "parse through the environment lock" },
]
"#;

/// Absolute, canonical path to this repository's root, derived from the crate
/// being compiled rather than the working directory (which `cargo test` does
/// not pin).
fn repo_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::canonicalize(&root)
        .unwrap_or_else(|e| panic!("cannot canonicalize repo root {}: {e}", root.display()))
}

/// Create a throwaway repository whose root configuration is `root_config`.
fn synthetic_repo(root_config: &str) -> TempDir {
    let dir = TempDir::new().expect("create fixture repo dir");
    fs::write(dir.path().join(CONFIG_NAMES[0]), root_config).expect("write fixture root config");
    dir
}

/// Create `src/<name>/clippy.toml` under `root`, spelled with `file_name`.
fn write_crate_config(root: &Path, name: &str, file_name: &str, contents: &str) {
    let dir = root.join("src").join(name);
    fs::create_dir_all(&dir).expect("create fixture crate dir");
    fs::write(dir.join(file_name), contents).expect("write fixture crate config");
}

/// Run the audit against a fixture and require a verdict (not an error).
fn audit_fixture(dir: &TempDir) -> Report {
    clippy_superset::audit(dir.path()).expect("audit the fixture repository")
}

/// Run the audit against a fixture and require a refusal. Returning the error
/// lets a caller assert on the variant; the assertion message renders the
/// *report* when one came back, so a fail-closed regression says what the guard
/// wrongly concluded instead of just "expected Err".
fn audit_must_refuse(dir: &TempDir) -> ClippySupersetError {
    match clippy_superset::audit(dir.path()) {
        Err(e) => e,
        Ok(report) => panic!(
            "audit should have refused this repository, but returned a verdict:\n{report}\n\
             (compliant = {})",
            report.is_compliant()
        ),
    }
}

/// The offending configuration paths of a report.
fn offending_paths(report: &Report) -> Vec<PathBuf> {
    report
        .offenders()
        .iter()
        .map(|offender| offender.config().to_path_buf())
        .collect()
}

// ---------------------------------------------------------------------------
// The real repository
// ---------------------------------------------------------------------------

/// The guard, pointed at this repo: every crate-local configuration restates
/// the whole root configuration.
///
/// The file-count assertion runs first on purpose. This repository has at least
/// one crate-local configuration, so an audit that examined none would report
/// "compliant" for entirely the wrong reason, and that false green is the
/// failure mode this whole file exists to prevent.
#[test]
fn every_crate_clippy_config_restates_the_workspace_bans() {
    let report = clippy_superset::audit(&repo_root()).expect("audit the repository");

    assert!(
        report.files_examined() > 0,
        "the audit examined no crate-local clippy configuration; a guard that \
         scans nothing reports clean for the wrong reason"
    );
    assert!(report.is_compliant(), "{report}");
}

/// The walk must see every configuration file sitting beside a member manifest.
///
/// The set is derived a second way — from `workspace_lints::members`, which is
/// how the sibling guards enumerate this workspace — and compared. A walk that
/// skipped a crate directory would report clean for the same reason a correct
/// walk does, and only a second derivation can tell the two apart.
///
/// The walk is allowed to find *more* than this: it covers the whole repository,
/// including any directory that is not a workspace member. It is not allowed to
/// find less.
#[test]
fn the_walk_finds_every_config_beside_a_member_manifest() {
    let root = repo_root();

    let walked: BTreeSet<PathBuf> = clippy_superset::configs(&root)
        .expect("walk the repository for clippy configurations")
        .into_iter()
        .collect();

    let beside_a_manifest: BTreeSet<PathBuf> = workspace_lints::members(&root)
        .expect("enumerate the workspace members")
        .into_iter()
        .flat_map(|member| CONFIG_NAMES.iter().map(move |name| member.join(name)))
        .filter(|path| path.is_file())
        .collect();

    assert!(
        !beside_a_manifest.is_empty(),
        "this repository carries at least one crate-local clippy configuration; \
         finding none means the member enumeration broke, not that the file went away"
    );

    let missed: Vec<&PathBuf> = beside_a_manifest.difference(&walked).collect();
    assert!(
        missed.is_empty(),
        "the walk missed clippy configurations that sit beside a member manifest: {missed:?}"
    );
}

// ---------------------------------------------------------------------------
// Mutation tests: prove the guard fires on each non-compliant shape
// ---------------------------------------------------------------------------

/// A crate file that declares only its own bans is flagged, and every root ban
/// it dropped is named.
#[test]
fn a_crate_config_that_replaces_the_root_is_flagged() {
    let repo = synthetic_repo(ROOT_CONFIG);
    write_crate_config(repo.path(), "narrow", CONFIG_NAMES[0], REPLACEMENT_CONFIG);

    let report = audit_fixture(&repo);

    assert!(
        !report.is_compliant(),
        "a crate file that drops both root bans must be flagged, but the audit \
         reported clean"
    );
    assert_eq!(
        offending_paths(&report),
        vec![PathBuf::from("src/narrow/clippy.toml")],
        "exactly the offending file should be named"
    );

    let missing: Vec<String> = report.offenders()[0]
        .missing()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        missing,
        vec![
            "disallowed-methods entry `colored::control::set_override`".to_string(),
            "disallowed-methods entry `crossterm::terminal::size`".to_string(),
        ],
        "both dropped bans should be named, so the fix needs no detective work"
    );
}

/// A crate file that restates the root bans and adds one of its own is clean.
/// Without this, every assertion above would still pass for a guard that flags
/// everything unconditionally.
#[test]
fn a_crate_config_that_extends_the_root_is_clean() {
    let repo = synthetic_repo(ROOT_CONFIG);
    write_crate_config(repo.path(), "wide", CONFIG_NAMES[0], SUPERSET_CONFIG);

    let report = audit_fixture(&repo);

    assert!(
        report.is_compliant(),
        "extra bans are the whole point of a crate-local file; got:\n{report}"
    );
    assert_eq!(report.files_examined(), 1);
}

/// The `.clippy.toml` spelling shadows the root exactly as `clippy.toml` does,
/// so the guard must read it too. A guard that knew only the first name would
/// report clean for the second — the same silence, a different filename.
#[test]
fn the_dotted_spelling_is_audited_too() {
    let repo = synthetic_repo(ROOT_CONFIG);
    write_crate_config(repo.path(), "dotted", CONFIG_NAMES[1], REPLACEMENT_CONFIG);

    let report = audit_fixture(&repo);

    assert_eq!(
        offending_paths(&report),
        vec![PathBuf::from("src/dotted/.clippy.toml")],
        "a `.clippy.toml` must be audited like a `clippy.toml`"
    );
}

/// A configuration nested well below a crate directory is audited too. Clippy
/// walks up from the crate directory, so such a file is usually inert — but a
/// guard that only looked one level down would report clean for a file it never
/// opened, and the day the directory becomes a crate the exemption is live.
#[test]
fn a_deeply_nested_config_is_audited() {
    let repo = synthetic_repo(ROOT_CONFIG);
    let deep = repo.path().join("src").join("tool").join("sub").join("dir");
    fs::create_dir_all(&deep).expect("create nested fixture dir");
    fs::write(deep.join(CONFIG_NAMES[0]), REPLACEMENT_CONFIG).expect("write nested config");

    let report = audit_fixture(&repo);

    assert_eq!(
        offending_paths(&report),
        vec![PathBuf::from("src/tool/sub/dir/clippy.toml")],
        "the walk must reach a configuration at any depth"
    );
}

/// A key the root declares and a crate file omits entirely is a dropped
/// declaration, not a clean pass. The crate file below bans nothing at all
/// under `disallowed-methods` — it has no such key.
#[test]
fn a_crate_config_missing_the_key_altogether_is_flagged() {
    let repo = synthetic_repo(ROOT_CONFIG);
    write_crate_config(
        repo.path(),
        "elsewhere",
        CONFIG_NAMES[0],
        "disallowed-types = [{ path = \"std::collections::HashMap\" }]\n",
    );

    let report = audit_fixture(&repo);

    assert_eq!(
        report.offenders()[0].missing().len(),
        2,
        "a file with no `disallowed-methods` key at all drops both root bans; got:\n{report}"
    );
}

/// A setting is not a list, so a crate file can only match it. One that gives
/// the same key a different value has replaced the workspace's decision, and
/// the guard says so rather than letting the change happen in silence.
#[test]
fn a_crate_config_that_changes_a_scalar_setting_is_flagged() {
    let repo = synthetic_repo("msrv = \"1.80\"\n");
    write_crate_config(repo.path(), "older", CONFIG_NAMES[0], "msrv = \"1.70\"\n");

    let report = audit_fixture(&repo);

    assert_eq!(
        report.offenders()[0]
            .missing()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["msrv = \"1.80\"".to_string()],
        "a changed setting must be reported with the value the root declares; got:\n{report}"
    );
}

/// A bare-string array element, which is how `disallowed-names` is written, is
/// identified by itself rather than by a `path` key.
#[test]
fn a_bare_string_entry_is_compared_by_its_text() {
    let repo = synthetic_repo("disallowed-names = [\"foo\", \"bar\"]\n");
    write_crate_config(
        repo.path(),
        "partial",
        CONFIG_NAMES[0],
        "disallowed-names = [\"foo\"]\n",
    );

    let report = audit_fixture(&repo);

    assert_eq!(
        report.offenders()[0]
            .missing()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["disallowed-names entry `bar`".to_string()],
        "a dropped string entry must be reported; got:\n{report}"
    );
}

/// A ban named only inside a comment is not a declaration. Every one of these
/// files carries paragraphs explaining the bans it holds, so a text search over
/// the file would call a crate compliant for reciting the root's reasoning.
#[test]
fn a_ban_named_in_a_comment_is_not_a_declaration() {
    let repo = synthetic_repo(ROOT_CONFIG);
    write_crate_config(
        repo.path(),
        "commented",
        CONFIG_NAMES[0],
        "# colored::control::set_override and crossterm::terminal::size are\n\
         # banned at the workspace root.\n\
         disallowed-methods = [\n\
         \x20   { path = \"clap::Parser::try_parse_from\", reason = \"lock\" },\n\
         ]\n",
    );

    let report = audit_fixture(&repo);

    assert_eq!(
        report.offenders()[0].missing().len(),
        2,
        "naming a ban in a comment must not count as declaring it; got:\n{report}"
    );
}

/// Build output is not this repository's source. A vendored crate under
/// `target/` carries configuration of its own, and clippy never reads it as a
/// crate configuration because it only ever walks *up*.
#[test]
fn build_output_is_not_walked() {
    let repo = synthetic_repo(ROOT_CONFIG);
    let vendored = repo.path().join("target").join("vendor").join("crate");
    fs::create_dir_all(&vendored).expect("create fixture vendor dir");
    fs::write(vendored.join(CONFIG_NAMES[0]), REPLACEMENT_CONFIG).expect("write vendored config");

    let report = audit_fixture(&repo);

    assert!(
        report.is_compliant(),
        "a configuration under target/ is not ours to audit; got:\n{report}"
    );
    assert_eq!(report.files_examined(), 0);
}

// ---------------------------------------------------------------------------
// Fail closed: none of these may yield a clean verdict
// ---------------------------------------------------------------------------

/// No root configuration at all. There is nothing to be a superset of, and a
/// crate file that shadows nothing is still not something to report clean.
#[test]
fn a_repository_without_a_root_config_refuses() {
    let dir = TempDir::new().expect("create fixture dir");

    assert!(
        matches!(
            clippy_superset::audit(dir.path()),
            Err(ClippySupersetError::NoRootConfig { .. })
        ),
        "a missing root configuration must be an error, not a clean repository"
    );
}

/// A root configuration that is not valid TOML.
#[test]
fn an_unparsable_root_config_refuses() {
    let repo = synthetic_repo("disallowed-methods = [ oops\n");

    assert!(
        matches!(
            audit_must_refuse(&repo),
            ClippySupersetError::ParseConfig { .. }
        ),
        "an unparsable root configuration must be an error"
    );
}

/// A crate configuration that is not valid TOML. Unreadable is not compliant.
#[test]
fn an_unparsable_crate_config_refuses() {
    let repo = synthetic_repo(ROOT_CONFIG);
    write_crate_config(
        repo.path(),
        "broken",
        CONFIG_NAMES[0],
        "disallowed-methods = [ {",
    );

    assert!(
        matches!(
            audit_must_refuse(&repo),
            ClippySupersetError::ParseConfig { .. }
        ),
        "an unparsable crate configuration must be an error, never a pass"
    );
}

/// One directory holding both spellings. Clippy itself refuses such a
/// directory, so the guard must not pick one and carry on.
#[test]
fn a_directory_holding_both_spellings_refuses() {
    let repo = synthetic_repo(ROOT_CONFIG);
    write_crate_config(repo.path(), "twice", CONFIG_NAMES[0], SUPERSET_CONFIG);
    write_crate_config(repo.path(), "twice", CONFIG_NAMES[1], SUPERSET_CONFIG);

    assert!(
        matches!(
            audit_must_refuse(&repo),
            ClippySupersetError::BothSpellings { .. }
        ),
        "a directory with both configuration names must be an error"
    );
}

/// An array entry shaped in a way the guard does not model. Skipping it would
/// drop a root ban from the requirement list in silence, which is the audited
/// set shrinking by itself.
#[test]
fn an_entry_the_guard_cannot_read_refuses() {
    let repo = synthetic_repo("disallowed-methods = [ 42 ]\n");
    write_crate_config(repo.path(), "any", CONFIG_NAMES[0], REPLACEMENT_CONFIG);

    let error = audit_must_refuse(&repo);

    assert!(
        matches!(
            &error,
            ClippySupersetError::UnsupportedEntry { key, index: 0, .. } if key == "disallowed-methods"
        ),
        "an entry the guard cannot read must be an error naming the key, got: {error}"
    );
}

// ---------------------------------------------------------------------------
// Display: the message has to be usable without opening the source
// ---------------------------------------------------------------------------

/// The failure text names the offending file, the bans it dropped, and why a
/// crate-local file loses them at all.
#[test]
fn failure_message_names_the_file_and_the_reason() {
    let repo = synthetic_repo(ROOT_CONFIG);
    write_crate_config(repo.path(), "narrow", CONFIG_NAMES[0], REPLACEMENT_CONFIG);

    let rendered = audit_fixture(&repo).to_string();

    assert!(
        rendered.contains("src/narrow/clippy.toml is missing 2 of the root's declarations:"),
        "the message must name the offending file, got:\n{rendered}"
    );
    assert!(
        rendered.contains("disallowed-methods entry `colored::control::set_override`"),
        "the message must name each dropped ban, got:\n{rendered}"
    );
    assert!(
        rendered.contains("nearest configuration file and no other"),
        "the message must say why the bans were lost, got:\n{rendered}"
    );
}

/// A clean audit says so, and says how much it looked at — so a passing CI log
/// distinguishes "checked one crate file" from "checked nothing".
#[test]
fn clean_message_reports_the_file_count() {
    let repo = synthetic_repo(ROOT_CONFIG);
    write_crate_config(repo.path(), "wide", CONFIG_NAMES[0], SUPERSET_CONFIG);

    let rendered = audit_fixture(&repo).to_string();

    assert!(
        rendered.contains(
            "Checked 1 crate-local clippy configurations; all restate the workspace configuration."
        ),
        "a clean report should state the file count, got:\n{rendered}"
    );
}

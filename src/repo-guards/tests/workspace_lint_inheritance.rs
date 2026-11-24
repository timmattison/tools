//! Guard tests for `repo_guards::workspace_lints::audit`.
//!
//! The headline test — [`every_workspace_member_inherits_the_workspace_lints`]
//! — runs the audit against this repository and checks the verdict: every
//! member manifest here declares `[lints] workspace = true`, so cargo hands
//! each of them the workspace lint set. It was red when this file was written
//! — six crates had never typed the stanza, and the exemption was spelled as
//! an absence, so nothing warned and nothing failed — and the commits that
//! followed cleared it, five of the six in one and `beta` in the next.
//!
//! Every other test is a mutation test. A guard that cannot fail is worse than
//! no guard, because "clean" and "I never looked" print identically. So each
//! non-compliant shape is built as a real workspace on disk and fed to the real
//! `audit()` — enumeration and checking together, not a string predicate in
//! isolation — and each fail-closed condition is asserted to produce an `Err`
//! rather than a clean verdict.
//!
//! Parallel safety: this workspace's tests share `./target` with the pre-commit
//! hook's own `cargo test`, so two copies of any test here can run at the same
//! moment. Every fixture lives in its own `tempfile::TempDir`, whose name the
//! OS makes unique; nothing is keyed on a fixed path under the temp dir, the
//! repo, or the home dir.

use std::fs;
use std::path::{Path, PathBuf};

use repo_guards::workspace_lints::{self, Report, WorkspaceLintsError};
use tempfile::TempDir;

/// The stanza a compliant member carries.
const COMPLIANT: &str = "[lints]\nworkspace = true\n";

/// Opting *out* explicitly. Reads like a lint declaration, inherits nothing.
const OPTED_OUT: &str = "[lints]\nworkspace = false\n";

/// A `[lints.clippy]` table with no `workspace` key. This was the exact shape
/// of the six offenders this guard found in this repo: the manifest mentions
/// lints, so a grep for "lints" calls it clean, while cargo hands it none of
/// the workspace set.
const CLIPPY_ONLY: &str = "[lints.clippy]\npedantic = { level = \"warn\", priority = -1 }\n";

/// Absolute, canonical path to this repository's root, derived from the crate
/// being compiled rather than the working directory (which `cargo test` does
/// not pin).
fn repo_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::canonicalize(&root)
        .unwrap_or_else(|e| panic!("cannot canonicalize repo root {}: {e}", root.display()))
}

/// Create a throwaway workspace root whose `workspace.members` is exactly
/// `members_array`, written verbatim as TOML (e.g. `["crates/*"]`).
fn synthetic_workspace(members_array: &str) -> TempDir {
    let dir = TempDir::new().expect("create fixture workspace dir");
    write_root_manifest(
        dir.path(),
        &format!("[workspace]\nresolver = \"2\"\nmembers = {members_array}\n"),
    );
    dir
}

/// Overwrite the root manifest of `root` with `contents`.
fn write_root_manifest(root: &Path, contents: &str) {
    fs::write(root.join("Cargo.toml"), contents).expect("write fixture root manifest");
}

/// Create `crates/<name>/Cargo.toml` under `root`, appending `lints_stanza`
/// verbatim after the `[package]` table.
fn write_member(root: &Path, name: &str, lints_stanza: &str) {
    let dir = root.join("crates").join(name);
    fs::create_dir_all(&dir).expect("create fixture member dir");
    fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n{lints_stanza}"
        ),
    )
    .expect("write fixture member manifest");
}

/// Run the audit against a fixture and require a verdict (not an error).
fn audit_fixture(dir: &TempDir) -> Report {
    workspace_lints::audit(dir.path()).expect("audit the fixture workspace")
}

/// Run the audit against a fixture and require a refusal. Returning the error
/// lets a caller assert on the variant; the assertion message renders the
/// *report* when one came back, so a fail-closed regression says what the guard
/// wrongly concluded instead of just "expected Err".
fn audit_must_refuse(dir: &TempDir) -> WorkspaceLintsError {
    match workspace_lints::audit(dir.path()) {
        Err(e) => e,
        Ok(report) => panic!(
            "audit should have refused this workspace, but returned a verdict:\n{report}\n\
             (compliant = {})",
            report.is_compliant()
        ),
    }
}

/// Convenience: the offender list as comparable paths.
fn offenders(report: &Report) -> Vec<PathBuf> {
    report.offenders().to_vec()
}

// ---------------------------------------------------------------------------
// The real repository
// ---------------------------------------------------------------------------

/// The guard, pointed at this repo: every member inherits the workspace lint
/// set.
///
/// The member-count assertion runs first on purpose: a guard that examined zero
/// crates would report "compliant" for entirely the wrong reason, and that
/// false green is the failure mode this whole file exists to prevent.
#[test]
fn every_workspace_member_inherits_the_workspace_lints() {
    let report = workspace_lints::audit(&repo_root()).expect("audit the workspace");

    assert!(
        report.members_examined() > 0,
        "the audit examined zero workspace members; a guard that scans nothing \
         reports clean for the wrong reason"
    );
    assert!(report.is_compliant(), "{report}");
}

// ---------------------------------------------------------------------------
// Mutation tests: prove the guard fires on each non-compliant shape
// ---------------------------------------------------------------------------

/// A member with no `[lints]` table at all is flagged, and its compliant
/// neighbour is not.
#[test]
fn member_without_a_lints_table_is_flagged() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    write_member(ws.path(), "compliant", COMPLIANT);
    write_member(ws.path(), "bare", "");

    let report = audit_fixture(&ws);

    assert!(
        !report.is_compliant(),
        "a member with no [lints] table must be flagged, but the audit reported clean"
    );
    assert_eq!(
        offenders(&report),
        vec![PathBuf::from("crates/bare/Cargo.toml")],
        "exactly the non-compliant member should be named"
    );
    assert_eq!(
        report.members_examined(),
        2,
        "both members should have been examined"
    );
}

/// `[lints] workspace = false` is an explicit opt-*out*, not inheritance.
#[test]
fn member_that_opts_out_explicitly_is_flagged() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    write_member(ws.path(), "compliant", COMPLIANT);
    write_member(ws.path(), "optedout", OPTED_OUT);

    let report = audit_fixture(&ws);

    assert_eq!(
        offenders(&report),
        vec![PathBuf::from("crates/optedout/Cargo.toml")],
        "`workspace = false` must be flagged; it inherits nothing"
    );
}

/// A `[lints.clippy]` table with no `workspace = true` key is flagged. This was
/// the shape of every offender this guard found here, and it is the shape a
/// naive "does the manifest mention lints?" check would wave through.
#[test]
fn member_with_only_a_clippy_lints_table_is_flagged() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    write_member(ws.path(), "compliant", COMPLIANT);
    write_member(ws.path(), "clippyonly", CLIPPY_ONLY);

    let report = audit_fixture(&ws);

    assert_eq!(
        offenders(&report),
        vec![PathBuf::from("crates/clippyonly/Cargo.toml")],
        "a [lints.clippy] table without `workspace = true` inherits nothing"
    );
}

/// An all-compliant workspace comes back clean. Without this, every assertion
/// above would still pass for a guard that flags everything unconditionally.
#[test]
fn fully_compliant_workspace_is_clean() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    write_member(ws.path(), "first", COMPLIANT);
    write_member(ws.path(), "second", COMPLIANT);

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "a fully compliant workspace must be clean, got:\n{report}"
    );
    assert!(report.offenders().is_empty(), "no member should be named");
    assert_eq!(report.members_examined(), 2);
}

/// The inline spelling `lints = { workspace = true }` is the same declaration
/// as the `[lints]` table, and the parser sees that for free. A regex tuned to
/// the table form would report this crate as an offender — a false positive
/// that would push someone toward "just allowlist it".
#[test]
fn inline_lints_table_counts_as_inheritance() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    let dir = ws.path().join("crates").join("inline");
    fs::create_dir_all(&dir).expect("create fixture member dir");
    fs::write(
        dir.join("Cargo.toml"),
        "lints = { workspace = true }\n\n[package]\nname = \"inline\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write fixture member manifest");

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "the inline lints table is the same declaration as [lints]; got:\n{report}"
    );
}

/// A single literal (glob-free) member path is enumerated like any other.
#[test]
fn literal_member_paths_are_audited() {
    let ws = synthetic_workspace("[\"crates/only\"]");
    write_member(ws.path(), "only", CLIPPY_ONLY);

    let report = audit_fixture(&ws);

    assert_eq!(
        offenders(&report),
        vec![PathBuf::from("crates/only/Cargo.toml")],
        "a literal member path must be audited, not just glob patterns"
    );
}

/// `workspace.exclude` is honored: a non-compliant crate that cargo does not
/// treat as a member must not be reported. Ignoring `exclude` would produce a
/// confusing false positive the first time this repo excludes anything.
#[test]
fn excluded_members_are_not_audited() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    write_member(ws.path(), "compliant", COMPLIANT);
    write_member(ws.path(), "vendored", CLIPPY_ONLY);
    write_root_manifest(
        ws.path(),
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\nexclude = [\"crates/vendored\"]\n",
    );

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "an excluded crate is not a workspace member and must not be flagged; got:\n{report}"
    );
    assert_eq!(
        report.members_examined(),
        1,
        "only the non-excluded member should have been examined"
    );
}

/// A glob written in `workspace.exclude` must not remove anything, because
/// cargo does not glob-expand `exclude` — it prefix-matches the entries as
/// *literal* paths.
///
/// Verified against cargo itself: a workspace whose root declares
/// `members = ["crates/*"]` and `exclude = ["crates/b*"]` still lists
/// `crates/bad` in `cargo metadata --no-deps`, because no directory is
/// literally named `crates/b*`. So `crates/bad` is a member, cargo builds it,
/// and the guard must audit it.
///
/// A guard that glob-expanded `exclude` instead would audit strictly *fewer*
/// crates than cargo builds, and the crate that slipped out could omit the
/// stanza forever while the guard kept reporting clean. That is the false-green
/// direction this module refuses everywhere else — it is why a *member* pattern
/// matching nothing is a hard error — and it must not sneak back in through
/// `exclude`.
#[test]
fn a_glob_in_exclude_does_not_shrink_the_audited_set() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    write_member(ws.path(), "good", COMPLIANT);
    write_member(ws.path(), "bad", CLIPPY_ONLY);
    write_root_manifest(
        ws.path(),
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\nexclude = [\"crates/b*\"]\n",
    );

    let report = audit_fixture(&ws);

    assert_eq!(
        report.members_examined(),
        2,
        "cargo treats `crates/b*` literally, so both crates are still members; got:\n{report}"
    );
    assert_eq!(
        offenders(&report),
        vec![PathBuf::from("crates/bad/Cargo.toml")],
        "a glob in `exclude` excludes nothing, so the non-compliant member must \
         still be flagged; got:\n{report}"
    );
}

// ---------------------------------------------------------------------------
// Display: the message has to be usable without opening the source
// ---------------------------------------------------------------------------

/// The failure text names the offending manifest and carries the exact stanza
/// to paste. Formatting lives in `Report`, not at the call site, so this is
/// where it is pinned.
#[test]
fn failure_message_names_the_offender_and_the_stanza() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    write_member(ws.path(), "newtool", "");

    let rendered = audit_fixture(&ws).to_string();

    assert!(
        rendered.contains("crates/newtool/Cargo.toml is missing workspace lint inheritance."),
        "the message must name the offending manifest, got:\n{rendered}"
    );
    assert!(
        rendered.contains("    [lints]\n    workspace = true"),
        "the message must show the stanza to paste, got:\n{rendered}"
    );
}

/// A clean audit says so, and says how much it looked at — so a passing CI log
/// distinguishes "checked 73 crates" from "checked nothing".
#[test]
fn clean_message_reports_the_member_count() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    write_member(ws.path(), "only", COMPLIANT);

    let rendered = audit_fixture(&ws).to_string();

    assert!(
        rendered.contains("Checked 1 workspace members; all inherit the workspace lint set."),
        "a clean report should state the member count, got:\n{rendered}"
    );
}

// ---------------------------------------------------------------------------
// Fail closed: none of these may yield a clean verdict
// ---------------------------------------------------------------------------

/// No root manifest at all.
#[test]
fn missing_root_manifest_refuses() {
    let dir = TempDir::new().expect("create fixture dir");

    assert!(
        matches!(
            workspace_lints::audit(dir.path()),
            Err(WorkspaceLintsError::ReadManifest { .. })
        ),
        "a missing root manifest must be an error, not a clean workspace"
    );
}

/// A root manifest that is not valid TOML.
#[test]
fn unparsable_root_manifest_refuses() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    write_root_manifest(ws.path(), "[workspace\nmembers = oops");

    assert!(
        matches!(
            audit_must_refuse(&ws),
            WorkspaceLintsError::ParseManifest { .. }
        ),
        "an unparsable root manifest must be an error"
    );
}

/// A root manifest with no `workspace.members` key.
#[test]
fn missing_members_key_refuses() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    write_root_manifest(ws.path(), "[workspace]\nresolver = \"2\"\n");

    assert!(
        matches!(
            audit_must_refuse(&ws),
            WorkspaceLintsError::NoMembersKey { .. }
        ),
        "a root manifest without `workspace.members` must be an error"
    );
}

/// An empty `members` array. Nothing to audit is not the same as nothing wrong.
#[test]
fn empty_members_list_refuses() {
    let ws = synthetic_workspace("[]");

    assert!(
        matches!(
            audit_must_refuse(&ws),
            WorkspaceLintsError::EmptyMembers { .. }
        ),
        "an empty `workspace.members` must be an error"
    );
}

/// A member pattern that matches nothing. A typo here would silently shrink the
/// audited set — the guard would keep passing while covering fewer crates.
#[test]
fn member_pattern_matching_nothing_refuses() {
    let ws = synthetic_workspace("[\"crates/*\", \"crtaes/*\"]");
    write_member(ws.path(), "real", COMPLIANT);

    let error = audit_must_refuse(&ws);

    assert!(
        matches!(
            &error,
            WorkspaceLintsError::PatternMatchedNothing { pattern } if pattern == "crtaes/*"
        ),
        "a member pattern matching nothing must be an error naming the pattern, got: {error}"
    );
}

/// A member directory with no `Cargo.toml`. Cargo itself refuses to load such a
/// workspace, so silently skipping it would let the guard disagree with the
/// build about which crates exist.
#[test]
fn member_directory_without_a_manifest_refuses() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    write_member(ws.path(), "real", COMPLIANT);
    fs::create_dir_all(ws.path().join("crates").join("empty")).expect("create manifest-less dir");

    assert!(
        matches!(
            audit_must_refuse(&ws),
            WorkspaceLintsError::MissingMemberManifest { .. }
        ),
        "a member directory without a manifest must be an error"
    );
}

/// A member manifest that is not valid TOML. Unreadable is not compliant.
#[test]
fn unparsable_member_manifest_refuses() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    write_member(ws.path(), "broken", COMPLIANT);
    fs::write(
        ws.path().join("crates").join("broken").join("Cargo.toml"),
        "[package\nname = ",
    )
    .expect("overwrite member manifest with invalid TOML");

    assert!(
        matches!(
            audit_must_refuse(&ws),
            WorkspaceLintsError::ParseManifest { .. }
        ),
        "an unparsable member manifest must be an error, never a pass"
    );
}

/// Every member excluded leaves nothing to audit — which must refuse rather
/// than report a vacuously clean workspace.
#[test]
fn excluding_every_member_refuses() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    write_member(ws.path(), "only", CLIPPY_ONLY);
    write_root_manifest(
        ws.path(),
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\nexclude = [\"crates/only\"]\n",
    );

    assert!(
        matches!(audit_must_refuse(&ws), WorkspaceLintsError::NoMembersRemain),
        "excluding every member must be an error, not a clean verdict"
    );
}

/// A non-string entry in `workspace.members`.
#[test]
fn non_string_member_entry_refuses() {
    let ws = synthetic_workspace("[42]");

    assert!(
        matches!(
            audit_must_refuse(&ws),
            WorkspaceLintsError::NonStringEntry { key: "members", .. }
        ),
        "a non-string `workspace.members` entry must be an error"
    );
}

/// Stray files beside the crate directories are ignored, exactly as cargo
/// ignores them. Without this, a Finder-dropped `.DS_Store` would take the
/// whole guard down with a `MissingMemberManifest` refusal.
#[test]
fn stray_files_beside_members_are_not_members() {
    let ws = synthetic_workspace("[\"crates/*\"]");
    write_member(ws.path(), "real", COMPLIANT);
    fs::write(
        ws.path().join("crates").join("README.md"),
        "# not a crate\n",
    )
    .expect("write stray file");
    fs::write(ws.path().join("crates").join(".DS_Store"), "junk").expect("write stray dotfile");

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "stray files are not members; got:\n{report}"
    );
    assert_eq!(
        report.members_examined(),
        1,
        "only the real crate directory should count as a member"
    );
}

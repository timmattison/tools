//! Guard tests for `repo_guards::arboard_wall::audit`.
//!
//! Each fixture is a real cargo workspace in its own `tempfile::TempDir`, and
//! the real `audit` reads it through `cargo metadata`. Nothing here names a
//! fixed path or sets an environment variable, so two copies of a test can run
//! at the same moment.

use std::fs;
use std::path::{Path, PathBuf};

use repo_guards::arboard_wall::{self, ArboardWallError, Offender};
use tempfile::TempDir;

const ROOT_MANIFEST: &str = "\
[workspace]
resolver = \"2\"
members = [\"crates/*\"]

[workspace.dependencies]
arboard = \"3.6\"
";

const SYSCLIP_MANIFEST: &str = "\
[package]
name = \"sysclip\"
version = \"0.1.0\"
edition = \"2021\"

[dependencies]
arboard = \"3\"
";

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("path has a parent")).expect("create dirs");
    fs::write(path, text).expect("write file");
}

/// A workspace that holds `sysclip` and nothing else.
fn base() -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    write(&dir.path().join("Cargo.toml"), ROOT_MANIFEST);
    write(
        &dir.path().join("crates/sysclip/Cargo.toml"),
        SYSCLIP_MANIFEST,
    );
    write(&dir.path().join("crates/sysclip/src/lib.rs"), "");
    dir
}

/// Add member `other` whose manifest holds `body` after its `[package]` table.
fn with_member(body: &str) -> TempDir {
    let dir = base();
    let manifest =
        format!("[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n{body}\n");
    write(&dir.path().join("crates/other/Cargo.toml"), &manifest);
    write(&dir.path().join("crates/other/src/lib.rs"), "");
    dir
}

fn offenders_of(dir: &TempDir) -> Vec<Offender> {
    let report = arboard_wall::audit(dir.path()).expect("the audit reaches a verdict");
    assert!(!report.is_compliant(), "expected an offender");
    report.offenders().to_vec()
}

fn assert_one(offenders: &[Offender], kind: &str, rename: Option<&str>, target: Option<&str>) {
    assert_eq!(offenders.len(), 1, "offenders: {offenders:?}");
    let offender = &offenders[0];
    assert_eq!(offender.member, "other");
    assert_eq!(offender.kind, kind);
    assert_eq!(offender.rename.as_deref(), rename);
    assert_eq!(offender.target.as_deref(), target);
}

#[test]
fn a_normal_dependency_is_found() {
    let dir = with_member("[dependencies]\narboard = \"3\"");
    assert_one(&offenders_of(&dir), "normal", None, None);
}

#[test]
fn a_dev_dependency_is_found() {
    let dir = with_member("[dev-dependencies]\narboard = \"3\"");
    assert_one(&offenders_of(&dir), "dev", None, None);
}

#[test]
fn a_build_dependency_is_found() {
    let dir = with_member("[build-dependencies]\narboard = \"3\"");
    write(&dir.path().join("crates/other/build.rs"), "fn main() {}\n");
    assert_one(&offenders_of(&dir), "build", None, None);
}

#[test]
fn a_renamed_dependency_is_found() {
    let dir = with_member("[dependencies]\nclip = { package = \"arboard\", version = \"3\" }");
    assert_one(&offenders_of(&dir), "normal", Some("clip"), None);
}

#[test]
fn a_target_table_dependency_is_found() {
    let dir = with_member("[target.'cfg(unix)'.dependencies]\narboard = \"3\"");
    assert_one(&offenders_of(&dir), "normal", None, Some("cfg(unix)"));
}

#[test]
fn an_inherited_dependency_is_found() {
    let dir = with_member("[dependencies]\narboard.workspace = true");
    assert_one(&offenders_of(&dir), "normal", None, None);
}

#[test]
fn another_package_under_the_name_arboard_is_not_an_offender() {
    let dir =
        with_member("[dependencies]\narboard = { package = \"copypasta\", version = \"0.10\" }");
    let report = arboard_wall::audit(dir.path()).expect("the audit reaches a verdict");
    assert!(report.is_compliant(), "{report}");
}

#[test]
fn a_workspace_with_only_sysclip_is_compliant() {
    let dir = base();
    let report = arboard_wall::audit(dir.path()).expect("the audit reaches a verdict");
    assert!(report.is_compliant(), "{report}");
    assert_eq!(report.members_examined(), 1);
}

#[test]
fn a_workspace_without_sysclip_is_an_error() {
    let dir = TempDir::new().expect("temp dir");
    write(&dir.path().join("Cargo.toml"), ROOT_MANIFEST);
    write(
        &dir.path().join("crates/other/Cargo.toml"),
        "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&dir.path().join("crates/other/src/lib.rs"), "");
    let error = arboard_wall::audit(dir.path()).expect_err("no owner is a refusal");
    assert!(matches!(error, ArboardWallError::OwnerMissing), "{error}");
}

#[test]
fn a_broken_member_manifest_is_an_error() {
    let dir = base();
    write(
        &dir.path().join("crates/other/Cargo.toml"),
        "[package\nname = ",
    );
    write(&dir.path().join("crates/other/src/lib.rs"), "");
    let result = arboard_wall::audit(dir.path());
    assert!(result.is_err(), "a manifest cargo cannot read is a refusal");
}

fn repo_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::canonicalize(&root)
        .unwrap_or_else(|e| panic!("cannot canonicalize {}: {e}", root.display()))
}

#[test]
fn this_workspace_lets_only_sysclip_depend_on_arboard() {
    let report = arboard_wall::audit(&repo_root()).expect("the audit reaches a verdict");
    assert!(report.is_compliant(), "{report}");
    assert!(
        report.members_examined() > 50,
        "the audit read only {} members",
        report.members_examined()
    );
}

//! Tests of the catalog: the pure parsers, the PATH rules, and the reads of a
//! fixture workspace.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::NaiveDate;
use wdid::catalog::{
    added_dates, binaries, cargo_bin, installed_binaries, path_state, workspace_root, CatalogError,
    PathState, Programs,
};
use wdid::tool::ToolName;

/// Makes a date for the expected values.
fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// A package of the fixture document: name, manifest path, and targets as
/// pairs of a name and a kind.
type FixturePackage<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

/// Makes a `cargo metadata` document with the given packages.
fn metadata(packages: &[FixturePackage<'_>]) -> String {
    let packages: Vec<String> = packages
        .iter()
        .map(|(name, manifest, targets)| {
            let targets: Vec<String> = targets
                .iter()
                .map(|(target, kind)| {
                    format!(
                        r#"{{"name":"{target}","kind":["{kind}"],"crate_types":["{kind}"],"src_path":"/x/src/main.rs"}}"#
                    )
                })
                .collect();
            format!(
                r#"{{"name":"{name}","version":"0.1.0","manifest_path":"{manifest}","targets":[{}]}}"#,
                targets.join(",")
            )
        })
        .collect();
    format!(
        r#"{{"packages":[{}],"workspace_root":"/x","version":1}}"#,
        packages.join(",")
    )
}

#[test]
fn binaries_gives_the_bin_targets_with_relative_crate_dirs() {
    let root = tempfile::tempdir().unwrap();
    for dir in ["src/aa", "src/bb"] {
        std::fs::create_dir_all(root.path().join(dir)).unwrap();
    }
    let a_manifest = root.path().join("src/aa/Cargo.toml");
    let b_manifest = root.path().join("src/bb/Cargo.toml");
    let json = metadata(&[
        (
            "aa",
            a_manifest.to_str().unwrap(),
            &[("aa", "bin"), ("aa-lib", "lib")],
        ),
        ("bb-lib", b_manifest.to_str().unwrap(), &[("bb_lib", "lib")]),
        ("bb", b_manifest.to_str().unwrap(), &[("bb-two", "bin")]),
    ]);
    let found = binaries(&json, root.path()).unwrap();
    assert_eq!(
        found,
        vec![
            (ToolName::new("aa"), PathBuf::from("src/aa")),
            (ToolName::new("bb-two"), PathBuf::from("src/bb")),
        ]
    );
}

#[test]
fn binaries_keeps_an_absolute_path_outside_the_root() {
    let root = tempfile::tempdir().unwrap();
    let json = metadata(&[("far", "/no/such/place/Cargo.toml", &[("far", "bin")])]);
    let found = binaries(&json, root.path()).unwrap();
    assert_eq!(
        found,
        vec![(ToolName::new("far"), PathBuf::from("/no/such/place"))]
    );
}

#[test]
fn binaries_gives_an_empty_crate_dir_for_a_root_package() {
    let root = tempfile::tempdir().unwrap();
    let manifest = root.path().join("Cargo.toml");
    let json = metadata(&[("top", manifest.to_str().unwrap(), &[("top", "bin")])]);
    let found = binaries(&json, root.path()).unwrap();
    assert_eq!(found, vec![(ToolName::new("top"), PathBuf::new())]);
}

#[test]
fn binaries_refuses_a_workspace_without_binaries() {
    let root = tempfile::tempdir().unwrap();
    let json = metadata(&[("only-lib", "/x/Cargo.toml", &[("only_lib", "lib")])]);
    let error = binaries(&json, root.path()).unwrap_err();
    assert!(matches!(error, CatalogError::NoBinaries { .. }), "{error}");
    assert!(error.to_string().starts_with("the cargo workspace at "));
}

#[test]
fn binaries_refuses_json_it_cannot_read() {
    let root = tempfile::tempdir().unwrap();
    for text in ["", "not json", r#"{"packages":"x"}"#, "{}"] {
        let error = binaries(text, root.path()).unwrap_err();
        assert!(
            matches!(error, CatalogError::Metadata { .. }),
            "{text}: {error}"
        );
        assert!(error
            .to_string()
            .starts_with("cargo metadata gave output that wdid cannot read: "));
    }
}

#[test]
fn installed_binaries_reads_the_real_format() {
    let list = "aa v0.1.0 (https://github.com/timmattison/tools#e595fa47):\n    aa\n\
                bottom v0.14.9:\n    btm\n\
                multi v1.0.0 (/path):\n    one\n    two\n";
    let expected: BTreeSet<String> = ["aa", "btm", "one", "two"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(installed_binaries(list), expected);
}

#[test]
fn installed_binaries_gives_nothing_for_empty_text() {
    assert!(installed_binaries("").is_empty());
    assert!(installed_binaries("\n\n").is_empty());
}

#[test]
fn added_dates_gives_the_oldest_date_for_each_crate_dir() {
    let log = "\x002026-10-14\n\ngsw/src/issue.rs\ngsw/src/shell.rs\n\
               \x002026-03-04\n\ngsw/Cargo.toml\nprcp/src/main.rs\n\
               \x002025-01-02\n\nprcp/Cargo.toml\nREADME.md\n";
    let dirs = vec![
        PathBuf::from("gsw"),
        PathBuf::from("prcp"),
        PathBuf::from("none"),
    ];
    let found = added_dates(log, &dirs);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[&PathBuf::from("gsw")], date(2026, 3, 4));
    assert_eq!(found[&PathBuf::from("prcp")], date(2025, 1, 2));
}

#[test]
fn added_dates_matches_whole_directory_names_only() {
    let log = "\x002025-01-02\n\ngsw-extra/src/main.rs\n";
    let found = added_dates(log, &[PathBuf::from("gsw")]);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn added_dates_handles_nested_crate_dirs_and_the_root() {
    let log = "\x002025-06-01\n\nsrc/aa/src/main.rs\nCargo.toml\n\
               \x002024-02-03\n\nsrc/aa/Cargo.toml\n";
    let dirs = vec![PathBuf::from("src/aa"), PathBuf::new()];
    let found = added_dates(log, &dirs);
    assert_eq!(found[&PathBuf::from("src/aa")], date(2024, 2, 3));
    assert_eq!(found[&PathBuf::new()], date(2024, 2, 3));
}

#[test]
fn added_dates_skips_a_record_with_a_bad_date_and_reads_multibyte_paths() {
    let log = "\x00not-a-date\n\nsrc/aa/x\n\x002025-01-02\n\nsrc/日本語/🎉.rs\n";
    let dirs = vec![PathBuf::from("src/aa"), PathBuf::from("src/日本語")];
    let found = added_dates(log, &dirs);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[&PathBuf::from("src/日本語")], date(2025, 1, 2));
}

#[test]
fn added_dates_gives_nothing_for_an_empty_log() {
    assert!(added_dates("", &[PathBuf::from("aa")]).is_empty());
}

/// Writes a file in `dir` with the given Unix mode.
fn write_file(dir: &Path, name: &str, mode: u32) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    path
}

/// Joins directories into a PATH value.
fn search_path(dirs: &[&Path]) -> std::ffi::OsString {
    std::env::join_paths(dirs).unwrap()
}

#[test]
fn path_state_is_cargo_when_the_cargo_bin_dir_comes_first() {
    let cargo = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    write_file(cargo.path(), "aa", 0o755);
    write_file(other.path(), "aa", 0o755);
    let path = search_path(&[cargo.path(), other.path()]);
    assert_eq!(path_state("aa", &path, cargo.path()), PathState::Cargo);
}

#[test]
fn path_state_is_shadowed_when_another_program_comes_first() {
    let cargo = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    write_file(cargo.path(), "aa", 0o755);
    let shadow = write_file(other.path(), "aa", 0o755);
    let path = search_path(&[other.path(), cargo.path()]);
    assert_eq!(
        path_state("aa", &path, cargo.path()),
        PathState::Shadowed(shadow)
    );
}

#[test]
fn path_state_is_not_on_path_when_no_directory_holds_the_name() {
    let cargo = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    write_file(other.path(), "bb", 0o755);
    let path = search_path(&[other.path(), cargo.path()]);
    assert_eq!(path_state("aa", &path, cargo.path()), PathState::NotOnPath);
    assert_eq!(
        path_state("aa", OsStr::new(""), cargo.path()),
        PathState::NotOnPath
    );
}

#[test]
fn path_state_skips_a_file_without_the_execute_bit() {
    let cargo = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    write_file(other.path(), "aa", 0o644);
    write_file(cargo.path(), "aa", 0o755);
    let path = search_path(&[other.path(), cargo.path()]);
    assert_eq!(path_state("aa", &path, cargo.path()), PathState::Cargo);
}

#[test]
fn path_state_skips_a_directory_with_the_name() {
    let cargo = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    std::fs::create_dir(other.path().join("aa")).unwrap();
    write_file(cargo.path(), "aa", 0o755);
    let path = search_path(&[other.path(), cargo.path()]);
    assert_eq!(path_state("aa", &path, cargo.path()), PathState::Cargo);
}

#[test]
fn path_state_follows_a_symlink_to_an_executable() {
    let cargo = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let target = write_file(other.path(), "real", 0o755);
    let link = other.path().join("aa");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let path = search_path(&[other.path(), cargo.path()]);
    assert_eq!(
        path_state("aa", &path, cargo.path()),
        PathState::Shadowed(link)
    );
}

#[test]
fn path_state_skips_an_empty_directory_of_the_path() {
    let cargo = tempfile::tempdir().unwrap();
    write_file(cargo.path(), "aa", 0o755);
    let mut path = std::ffi::OsString::from(":");
    path.push(cargo.path());
    assert_eq!(path_state("aa", &path, cargo.path()), PathState::Cargo);
}

#[test]
fn path_state_compares_canonical_paths_for_the_cargo_bin_dir() {
    let base = tempfile::tempdir().unwrap();
    let real = base.path().join("real");
    std::fs::create_dir(&real).unwrap();
    write_file(&real, "aa", 0o755);
    let link = base.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let path = search_path(&[&link]);
    assert_eq!(path_state("aa", &path, &real), PathState::Cargo);
}

#[test]
fn path_state_compares_plain_paths_when_the_cargo_bin_dir_does_not_exist() {
    let other = tempfile::tempdir().unwrap();
    let shadow = write_file(other.path(), "aa", 0o755);
    let missing = other.path().join("no-such-dir");
    let path = search_path(&[other.path()]);
    assert_eq!(
        path_state("aa", &path, &missing),
        PathState::Shadowed(shadow)
    );
}

#[test]
fn cargo_bin_prefers_cargo_home() {
    assert_eq!(
        cargo_bin(Some(OsStr::new("/c/home")), Some(Path::new("/u"))),
        Some(PathBuf::from("/c/home/bin"))
    );
}

#[test]
fn cargo_bin_falls_back_to_the_home_directory() {
    assert_eq!(
        cargo_bin(None, Some(Path::new("/u"))),
        Some(PathBuf::from("/u/.cargo/bin"))
    );
    assert_eq!(
        cargo_bin(Some(OsStr::new("")), Some(Path::new("/u"))),
        Some(PathBuf::from("/u/.cargo/bin"))
    );
}

#[test]
fn cargo_bin_is_none_without_cargo_home_and_home() {
    assert_eq!(cargo_bin(None, None), None);
    assert_eq!(cargo_bin(Some(OsStr::new("")), None), None);
}

/// The date of the commit that adds the crate `one`.
const ONE_COMMIT_DATE: &str = "2025-01-02T12:00:00Z";

/// The date of the commit that adds the crate `two`.
const TWO_COMMIT_DATE: &str = "2026-03-04T12:00:00Z";

/// Runs git in `dir` with no inherited git environment. `date` states the
/// author and committer date, after the sweep.
fn git(dir: &Path, args: &[&str], date: Option<&str>) {
    let mut command = Command::new("git");
    gitscratch::shed_inherited_git_environment(&mut command);
    if let Some(date) = date {
        command
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date);
    }
    let output = command
        .args([
            "-c",
            "user.name=wdid-test",
            "-c",
            "user.email=wdid-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Writes the workspace files of the fixture: a root manifest and the crates
/// `one` and `two`.
fn write_workspace(root: &Path) {
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"one\", \"two\"]\n",
    )
    .unwrap();
    for (dir, name) in [("one", "wdid-fixture-one"), ("two", "wdid-fixture-two")] {
        std::fs::create_dir_all(root.join(dir).join("src")).unwrap();
        std::fs::write(
            root.join(dir).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        std::fs::write(root.join(dir).join("src/main.rs"), "fn main() {}\n").unwrap();
    }
}

/// A fixture workspace in a temporary directory, with a git history.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    /// The workspace without a git repository.
    fn without_history() -> Self {
        let dir = tempfile::tempdir().unwrap();
        write_workspace(dir.path());
        Self { dir }
    }

    /// The workspace with two commits: `one` first, then `two`.
    fn with_history() -> Self {
        let fixture = Self::without_history();
        let root = fixture.root();
        git(root, &["init", "-q", "-b", "main"], None);
        git(root, &["add", "Cargo.toml", "one"], None);
        git(
            root,
            &["commit", "-q", "-m", "add one"],
            Some(ONE_COMMIT_DATE),
        );
        git(root, &["add", "two"], None);
        git(
            root,
            &["commit", "-q", "-m", "add two"],
            Some(TWO_COMMIT_DATE),
        );
        fixture
    }

    /// The root of the workspace.
    fn root(&self) -> &Path {
        self.dir.path()
    }
}

/// Canonicalizes a path for a comparison.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

#[test]
fn workspace_root_from_a_subdirectory_gives_the_work_tree_root() {
    let fixture = Fixture::with_history();
    let cwd = fixture.root().join("one/src");
    let root = workspace_root(None, &cwd, &Programs::default()).unwrap();
    assert_eq!(canonical(&root), canonical(fixture.root()));
}

#[test]
fn workspace_root_with_repo_gives_the_canonical_repo() {
    let fixture = Fixture::without_history();
    let elsewhere = tempfile::tempdir().unwrap();
    let root =
        workspace_root(Some(fixture.root()), elsewhere.path(), &Programs::default()).unwrap();
    assert_eq!(root, canonical(fixture.root()));
}

#[test]
fn workspace_root_with_repo_refuses_a_directory_without_cargo_toml() {
    let empty = tempfile::tempdir().unwrap();
    let error = workspace_root(Some(empty.path()), empty.path(), &Programs::default()).unwrap_err();
    assert!(
        matches!(error, CatalogError::NotAWorkspace { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("--repo <path>"), "{error}");
}

#[test]
fn workspace_root_with_repo_refuses_a_missing_directory() {
    let base = tempfile::tempdir().unwrap();
    let missing = base.path().join("no-such-dir");
    let error = workspace_root(Some(&missing), base.path(), &Programs::default()).unwrap_err();
    assert!(
        matches!(error, CatalogError::NotAWorkspace { .. }),
        "{error}"
    );
}

#[test]
fn workspace_root_refuses_a_work_tree_without_cargo_toml() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"], None);
    let error = workspace_root(None, dir.path(), &Programs::default()).unwrap_err();
    assert!(
        matches!(error, CatalogError::NotAWorkspace { .. }),
        "{error}"
    );
}

#[test]
fn workspace_root_outside_any_work_tree_is_not_a_work_tree() {
    let dir = tempfile::tempdir().unwrap();
    let mut probe = Command::new("git");
    gitscratch::shed_inherited_git_environment(&mut probe);
    let inside = probe
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir.path())
        .output()
        .unwrap()
        .status
        .success();
    if inside {
        eprintln!("skipped: the temporary directory is inside a git work tree");
        return;
    }
    let error = workspace_root(None, dir.path(), &Programs::default()).unwrap_err();
    assert!(
        matches!(error, CatalogError::NotAWorkTree { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("--repo <path>"), "{error}");
}

#[test]
fn workspace_root_names_a_git_that_does_not_start() {
    let dir = tempfile::tempdir().unwrap();
    let programs = Programs {
        git: dir.path().join("no-such-git").into_os_string(),
        ..Programs::default()
    };
    let error = workspace_root(None, dir.path(), &programs).unwrap_err();
    assert!(matches!(error, CatalogError::Spawn { .. }), "{error}");
    assert!(error.to_string().contains("no-such-git"), "{error}");
}

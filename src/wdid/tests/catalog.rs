//! Tests of the catalog: the pure parsers, the PATH rules, and the reads of a
//! fixture workspace.

use std::collections::BTreeSet;
use std::path::PathBuf;

use chrono::NaiveDate;
use wdid::catalog::{added_dates, binaries, installed_binaries, CatalogError};
use wdid::tool::ToolName;

/// Makes a date for the expected values.
fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// Makes a `cargo metadata` document with the given packages. Each package is
/// a name, a manifest path, and the kinds of its targets.
fn metadata(packages: &[(&str, &str, &[(&str, &str)])]) -> String {
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
    let log = "\02026-10-14\n\ngsw/src/issue.rs\ngsw/src/shell.rs\n\
               \02026-03-04\n\ngsw/Cargo.toml\nprcp/src/main.rs\n\
               \02025-01-02\n\nprcp/Cargo.toml\nREADME.md\n";
    let dirs = vec![
        PathBuf::from("gsw"),
        PathBuf::from("prcp"),
        PathBuf::from("none"),
    ];
    let found = added_dates(&log, &dirs);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[&PathBuf::from("gsw")], date(2026, 3, 4));
    assert_eq!(found[&PathBuf::from("prcp")], date(2025, 1, 2));
}

#[test]
fn added_dates_matches_whole_directory_names_only() {
    let log = "\02025-01-02\n\ngsw-extra/src/main.rs\n";
    let found = added_dates(log, &[PathBuf::from("gsw")]);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn added_dates_handles_nested_crate_dirs_and_the_root() {
    let log = "\02025-06-01\n\nsrc/aa/src/main.rs\nCargo.toml\n\
               \02024-02-03\n\nsrc/aa/Cargo.toml\n";
    let dirs = vec![PathBuf::from("src/aa"), PathBuf::new()];
    let found = added_dates(log, &dirs);
    assert_eq!(found[&PathBuf::from("src/aa")], date(2024, 2, 3));
    assert_eq!(found[&PathBuf::new()], date(2024, 2, 3));
}

#[test]
fn added_dates_skips_a_record_with_a_bad_date_and_reads_multibyte_paths() {
    let log = "\0not-a-date\n\nsrc/aa/x\n\02025-01-02\n\nsrc/日本語/🎉.rs\n";
    let dirs = vec![PathBuf::from("src/aa"), PathBuf::from("src/日本語")];
    let found = added_dates(log, &dirs);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[&PathBuf::from("src/日本語")], date(2025, 1, 2));
}

#[test]
fn added_dates_gives_nothing_for_an_empty_log() {
    assert!(added_dates("", &[PathBuf::from("aa")]).is_empty());
}

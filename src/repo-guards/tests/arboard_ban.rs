//! Mutation test of the clippy ban on `arboard::Clipboard`.
//!
//! `clippy.toml` bans `arboard::Clipboard` in `disallowed-types` and
//! `arboard::Clipboard::new` in `disallowed-methods`. Only `sysclip` may name
//! them. This test builds a small workspace on disk, runs the real clippy on
//! it, and checks which lines of the fixture the ban reports.
//!
//! Each syntactic form lives in its own module file. A diagnostic then maps to
//! one form by the name of its file. A path with no import, an import, a type
//! annotation, an alias, and a renamed dependency are separate ways for a ban
//! to go quiet. Two files are the negative half: an exempt call and a clean
//! file.
//!
//! The fixture reads the real `clippy.toml` of this repository. The test sets
//! `CLIPPY_CONF_DIR` to the repository root, so no copy of the file can drift
//! from the original.
//!
//! The fixture holds a stand-in crate that is named `arboard`. Clippy matches a
//! banned path by the name of the crate and the path of the item. The stand-in
//! is thus enough, and the fixture builds in seconds without a registry.
//!
//! A run that gives no compiler message passes the sanity check of `run_clippy`
//! only when clippy exits with status 0. A run that failed to build and gave no
//! message fails that check. A healthy run that gave no message fails the
//! positive test, because it found no ban diagnostic. Thus a broken fixture and
//! a quiet ban both fail, and each one fails with its own message.
//!
//! Parallel safety: the fixture and its target directory live in one
//! `tempfile::TempDir`. The test sets no variable in its own process.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::Value;
use tempfile::TempDir;

const TYPES_LINT: &str = "clippy::disallowed_types";
const METHODS_LINT: &str = "clippy::disallowed_methods";

/// Each module file of the fixture, the lint that the ban must raise on it,
/// and its source. A `None` lint means that the file must raise nothing.
/// The source of the stand-in crate.
const STAND_IN: &str = "pub struct Clipboard;\n\nimpl Clipboard {\n    pub fn new() -> Result<Self, String> {\n        Ok(Self)\n    }\n}\n";

const FORMS: &[(&str, Option<&str>, &str)] = &[
    (
        "imported.rs",
        Some(TYPES_LINT),
        "use arboard::Clipboard;\n\npub fn f(_c: Option<Clipboard>) {}\n",
    ),
    (
        "called.rs",
        Some(METHODS_LINT),
        "pub fn g() {\n    let _ = arboard::Clipboard::new();\n}\n",
    ),
    (
        "annotated.rs",
        Some(TYPES_LINT),
        "pub fn h(_c: &arboard::Clipboard) {}\n",
    ),
    (
        "aliased.rs",
        Some(TYPES_LINT),
        "use arboard::Clipboard as Board;\n\npub fn i(_c: Option<Board>) {}\n",
    ),
    (
        "renamed_crate.rs",
        Some(METHODS_LINT),
        "pub fn k() {\n    let _ = clip::Clipboard::new();\n}\n",
    ),
    (
        "exempt.rs",
        None,
        "#[expect(\n    clippy::disallowed_methods,\n    clippy::disallowed_types,\n    reason = \"the fixture of an exemption\"\n)]\npub fn m() {\n    let _ = arboard::Clipboard::new();\n}\n",
    ),
    ("clean.rs", None, "pub fn n() -> u32 {\n    7\n}\n"),
];

/// The forms that must raise a diagnostic, with the lint of each.
const FLAGGED: &[(&str, &str)] = &[
    ("imported.rs", TYPES_LINT),
    ("called.rs", METHODS_LINT),
    ("annotated.rs", TYPES_LINT),
    ("aliased.rs", TYPES_LINT),
    ("renamed_crate.rs", METHODS_LINT),
];

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("a fixture path has a parent"))
        .expect("create the fixture directory");
    fs::write(&path, text).expect("write the fixture file");
}

/// Builds the fixture workspace and returns its root.
fn build_fixture(temp: &TempDir) -> PathBuf {
    let root = temp.path().join("fixture");
    write(
        &root,
        "Cargo.toml",
        "[workspace]\nresolver = \"2\"\nmembers = [\"user\"]\nexclude = [\"arboard\", \"arboard-copy\"]\n",
    );
    write(
        &root,
        "arboard/Cargo.toml",
        "[package]\nname = \"arboard\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n[workspace]\n",
    );
    write(&root, "arboard/src/lib.rs", STAND_IN);
    // Cargo refuses two dependencies on one package under two names. A second
    // package of the same name, at another path, is a different package.
    write(
        &root,
        "arboard-copy/Cargo.toml",
        "[package]\nname = \"arboard\"\nversion = \"0.0.1\"\nedition = \"2021\"\npublish = false\n\n[workspace]\n",
    );
    write(&root, "arboard-copy/src/lib.rs", STAND_IN);
    write(
        &root,
        "user/Cargo.toml",
        "[package]\nname = \"user\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n[dependencies]\narboard = { path = \"../arboard\" }\nclip = { package = \"arboard\", path = \"../arboard-copy\" }\n",
    );
    let mut lib = String::new();
    for (file, _, source) in FORMS {
        let module = file.trim_end_matches(".rs");
        lib.push_str(&format!("pub mod {module};\n"));
        write(&root, &format!("user/src/{file}"), source);
    }
    write(&root, "user/src/lib.rs", &lib);
    root
}

/// Runs clippy on the fixture and returns each ban diagnostic as a pair of the
/// module file name and the lint code.
fn ban_diagnostics() -> &'static BTreeSet<(String, String)> {
    static RESULT: OnceLock<BTreeSet<(String, String)>> = OnceLock::new();
    RESULT.get_or_init(run_clippy)
}

fn run_clippy() -> BTreeSet<(String, String)> {
    let temp = TempDir::new().expect("create the temporary directory");
    let root = build_fixture(&temp);
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve the repository root");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());

    let output = childenv::fixture_command(cargo)
        .args([
            "clippy",
            "--offline",
            "--quiet",
            "--message-format=json",
            "-p",
            "user",
        ])
        .current_dir(&root)
        .env("CLIPPY_CONF_DIR", &repository)
        .env("CARGO_TARGET_DIR", temp.path().join("target"))
        .env_remove("RUSTFLAGS")
        .env_remove("RUSTDOCFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("CARGO_BUILD_RUSTFLAGS")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")
        .output()
        .expect("start cargo clippy");

    let mut messages = 0_usize;
    let mut found = BTreeSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value["reason"] != "compiler-message" {
            continue;
        }
        messages += 1;
        let Some(code) = value["message"]["code"]["code"].as_str() else {
            continue;
        };
        if code != TYPES_LINT && code != METHODS_LINT {
            continue;
        }
        let spans = value["message"]["spans"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let Some(file) = spans
            .iter()
            .find(|span| span["is_primary"] == true)
            .and_then(|span| span["file_name"].as_str())
        else {
            continue;
        };
        let Some(name) = Path::new(file)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|_| file.contains("user/src/"))
        else {
            continue;
        };
        found.insert((name.to_string(), code.to_string()));
    }
    // The stand-in compiles without a message, so a run that is healthy and
    // finds nothing gives no message at all. That run cannot be told from a
    // run that failed to build, so the exit status decides between the two.
    assert!(
        messages > 0 || output.status.success(),
        "cargo clippy produced no compiler message and failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    found
}

#[test]
fn clippy_bans_the_clipboard_of_arboard_in_each_syntactic_form() {
    let found = ban_diagnostics();
    let expected: BTreeSet<(String, String)> = FLAGGED
        .iter()
        .map(|(file, lint)| ((*file).to_string(), (*lint).to_string()))
        .collect();
    assert!(
        expected.is_subset(found),
        "the ban missed a form.\nexpected at least: {expected:#?}\nfound: {found:#?}"
    );
}

#[test]
fn clippy_leaves_the_exempt_and_the_clean_forms_alone() {
    let found = ban_diagnostics();
    let wrongly: Vec<_> = found
        .iter()
        .filter(|(file, _)| file == "exempt.rs" || file == "clean.rs")
        .collect();
    assert!(wrongly.is_empty(), "the ban flagged {wrongly:#?}");
}

#[test]
fn clippy_raises_no_other_ban_diagnostic_than_the_expected_ones() {
    let found = ban_diagnostics();
    let flagged_files: BTreeSet<&str> = FLAGGED.iter().map(|(file, _)| *file).collect();
    let stray: Vec<_> = found
        .iter()
        .filter(|(file, _)| !flagged_files.contains(file.as_str()))
        .collect();
    assert!(
        stray.is_empty(),
        "the ban flagged an unexpected file: {stray:#?}"
    );
}

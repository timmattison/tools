//! Guard tests for `repo_guards::tool_index::audit`.
//!
//! Every test builds a real workspace on disk and feeds it to the real
//! `audit()` — member enumeration, binary discovery, markdown parsing, and
//! comparison together, rather than a string predicate in isolation.
//!
//! Parallel safety: this workspace's tests share `./target` with the pre-commit
//! hook's own `cargo test`, so two copies of any test here can run at the same
//! moment. Every fixture lives in its own `tempfile::TempDir`, whose name the
//! OS makes unique. Nothing is keyed on a fixed path under the temp dir, the
//! repo, or the home dir.

use std::fs;
use std::path::Path;

use repo_guards::tool_index;
use tempfile::TempDir;

/// A `README.md` that documents `aa` as a list entry and `bb` as a section.
///
/// Both forms are in use in the real README, so a fixture that carried only one
/// of them would leave the other untested.
const README_WITH_BOTH: &str = "\
# Tools

## The tools

- aa
  - Prints the caller identity.
  - To install: `cargo install aa`

## bb (the other one)

Does the other thing.
";

/// A `TLDR.md` whose table holds a row for `aa` and none for `bb`.
const TLDR_WITH_AA_ONLY: &str = "\
# TL;DR

| Tool | What it does |
| --- | --- |
| `aa` | Prints the caller identity. |
";

/// Build a workspace on disk: one member crate per name in `bins`, each with a
/// `src/main.rs`, plus the two index files at the root.
fn workspace(bins: &[&str], readme: &str, tldr: &str) -> TempDir {
    let dir = TempDir::new().expect("a temp dir");
    let root = dir.path();

    write(root, "Cargo.toml", "[workspace]\nmembers = [\"src/*\"]\n");
    write(root, "README.md", readme);
    write(root, "TLDR.md", tldr);

    for name in bins {
        let member = root.join("src").join(name);
        write(
            &member,
            "Cargo.toml",
            &format!("[package]\nname = \"{name}\"\nedition = \"2021\"\n"),
        );
        write(&member, "src/main.rs", "fn main() {}\n");
    }

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

#[test]
fn a_binary_with_no_tldr_row_is_reported() {
    let ws = workspace(&["aa", "bb"], README_WITH_BOTH, TLDR_WITH_AA_ONLY);

    let report = tool_index::audit(ws.path()).expect("the audit reaches a verdict");

    assert_eq!(report.missing_from_tldr(), ["bb".to_owned()]);
    assert!(
        report.missing_from_readme().is_empty(),
        "both tools are in the README, so nothing is missing from it: {:?}",
        report.missing_from_readme()
    );
    assert!(!report.is_compliant(), "a missing row is not compliant");
    assert_eq!(report.binaries_examined(), 2);
}

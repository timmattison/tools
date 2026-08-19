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

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// A `TLDR.md` whose table holds a row for both tools, used as the fixed half
/// of a fixture that varies `README.md`.
const TLDR_WITH_BOTH: &str = "\
# TL;DR

| Tool | What it does |
| --- | --- |
| `aa` | Prints the caller identity. |
| `bb` | Does the other thing. |
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

/// Absolute, canonical path to this repository's root, derived from the crate
/// being compiled rather than the working directory (which `cargo test` does
/// not pin).
fn repo_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::canonicalize(&root)
        .unwrap_or_else(|e| panic!("cannot canonicalize {}: {e}", root.display()))
}

#[test]
fn every_binary_of_this_repository_appears_in_both_indexes() {
    let report = tool_index::audit(&repo_root()).expect("the audit reaches a verdict");

    assert!(
        report.binaries_examined() > 50,
        "this workspace builds dozens of binaries, so a smaller count means the guard \
         enumerated the wrong thing and would report clean for the wrong reason: {report:?}"
    );
    assert!(report.is_compliant(), "{report}");
}

// ---------------------------------------------------------------------------
// Mutation tests.
//
// A guard that cannot fail is worse than no guard, because "clean" and "I never
// looked" print identically. Each test below feeds `audit()` a form the tool
// name can arrive in and asserts the verdict the form deserves.
//
// The set is organised by *syntactic form*, not by the one spelling that
// prompted the guard. A fixture set with one shape per construct is what stops
// the guard from silently accepting the next shape somebody writes.
// ---------------------------------------------------------------------------

/// A `README.md` that documents `aa` and nothing else, used as the fixed half
/// of a fixture that varies `TLDR.md`.
const README_AA_ONLY: &str = "\
# Tools

## The tools

- aa
  - Prints the caller identity.
";

/// A `TLDR.md` that lists `aa` and nothing else, used as the fixed half of a
/// fixture that varies `README.md`.
const TLDR_AA_ONLY: &str = "\
# TL;DR

| Tool | What it does |
| --- | --- |
| `aa` | Prints the caller identity. |
";

/// The verdict for a workspace of `aa` and `bb`, where only `bb` is in doubt.
fn verdict(readme: &str, tldr: &str) -> tool_index::Report {
    let ws = workspace(&["aa", "bb"], readme, tldr);
    tool_index::audit(ws.path()).expect("the audit reaches a verdict")
}

#[test]
fn a_name_only_in_a_tldr_description_is_not_an_entry() {
    // The second row's description is *exactly* a tool name, not a sentence
    // that merely contains one. A guard that read every cell would take the
    // whole cell as one name, so a fixture whose description is prose cannot
    // tell the two apart: "Pairs with `bb`." is not the string "bb" either way.
    // This shape is the one that fails when the guard stops reading only the
    // first cell.
    let report = verdict(
        README_WITH_BOTH,
        "\
# TL;DR

| Tool | What it does |
| --- | --- |
| `aa` | Prints the caller identity. Pairs with `bb`. |
| `zz` | bb |
",
    );

    assert_eq!(
        report.missing_from_tldr(),
        ["bb".to_owned()],
        "a name in another row's description is not a row of its own"
    );
}

#[test]
fn a_name_only_in_the_tldr_head_row_is_not_an_entry() {
    let report = verdict(
        README_WITH_BOTH,
        "\
# TL;DR

| bb | What it does |
| --- | --- |
| `aa` | Prints the caller identity. |
",
    );

    assert_eq!(report.missing_from_tldr(), ["bb".to_owned()]);
}

#[test]
fn a_tldr_row_without_backticks_is_an_entry() {
    let report = verdict(
        README_WITH_BOTH,
        "\
# TL;DR

| Tool | What it does |
| --- | --- |
| aa | Prints the caller identity. |
| bb | Does the other thing. |
",
    );

    assert!(
        report.missing_from_tldr().is_empty(),
        "the guard reads the cell, not its decoration: {report}"
    );
}

#[test]
fn a_name_only_in_a_readme_code_block_is_not_an_entry() {
    let report = verdict(
        "\
# Tools

## The tools

- aa
  - Prints the caller identity.

## Examples

```bash
bb --help
```
",
        TLDR_WITH_BOTH,
    );

    assert_eq!(
        report.missing_from_readme(),
        ["bb".to_owned()],
        "a command line in a code block documents nothing"
    );
}

#[test]
fn a_name_only_in_a_nested_readme_item_is_not_an_entry() {
    let report = verdict(
        "\
# Tools

## The tools

- aa
  - Prints the caller identity.
  - bb
",
        TLDR_WITH_BOTH,
    );

    assert_eq!(
        report.missing_from_readme(),
        ["bb".to_owned()],
        "a nested item describes its parent; it is not an entry of its own"
    );
}

#[test]
fn a_name_only_in_readme_prose_is_not_an_entry() {
    let report = verdict(
        "\
# Tools

## The tools

- aa
  - Prints the caller identity.

## Notes

Run bb when you need the other thing.
",
        TLDR_WITH_BOTH,
    );

    assert_eq!(report.missing_from_readme(), ["bb".to_owned()]);
}

#[test]
fn a_readme_item_or_heading_with_a_parenthetical_is_an_entry() {
    let report = verdict(
        "\
# Tools

## The tools

- aa (the identity one)
  - Prints the caller identity.

## bb (the other one)

Does the other thing.
",
        TLDR_WITH_BOTH,
    );

    assert!(
        report.missing_from_readme().is_empty(),
        "the entry is the first word of the item or heading: {report}"
    );
}

#[test]
fn a_binary_declared_by_an_explicit_bin_table_is_examined() {
    let dir = TempDir::new().expect("a temp dir");
    let root = dir.path();
    write(root, "Cargo.toml", "[workspace]\nmembers = [\"src/*\"]\n");
    write(root, "README.md", README_AA_ONLY);
    write(root, "TLDR.md", TLDR_AA_ONLY);
    // The package is named `renamed`, and the binary it builds is named `aa`.
    // Counting the package name here would report a tool that does not exist.
    let member = root.join("src").join("renamed");
    write(
        &member,
        "Cargo.toml",
        "[package]\nname = \"renamed\"\nedition = \"2021\"\n\n[[bin]]\nname = \"aa\"\npath = \"src/main.rs\"\n",
    );
    write(&member, "src/main.rs", "fn main() {}\n");

    let report = tool_index::audit(root).expect("the audit reaches a verdict");

    assert_eq!(report.binaries_examined(), 1);
    assert!(report.is_compliant(), "{report}");
}

#[test]
fn a_binary_under_src_bin_is_examined() {
    let dir = TempDir::new().expect("a temp dir");
    let root = dir.path();
    write(root, "Cargo.toml", "[workspace]\nmembers = [\"src/*\"]\n");
    write(root, "README.md", README_AA_ONLY);
    write(root, "TLDR.md", TLDR_AA_ONLY);
    let member = root.join("src").join("host");
    write(
        &member,
        "Cargo.toml",
        "[package]\nname = \"host\"\nedition = \"2021\"\n",
    );
    write(&member, "src/lib.rs", "");
    write(&member, "src/bin/bb.rs", "fn main() {}\n");

    let report = tool_index::audit(root).expect("the audit reaches a verdict");

    assert_eq!(
        report.missing_from_readme(),
        ["bb".to_owned()],
        "a binary cargo builds from src/bin is a tool like any other: {report}"
    );
}

// ---------------------------------------------------------------------------
// Fail-closed refusals. Each asks the guard to report on something it cannot
// see, and asserts it refuses rather than returning a clean verdict.
// ---------------------------------------------------------------------------

#[test]
fn a_readme_with_no_tools_section_refuses() {
    let ws = workspace(&["aa"], "# Tools\n\nNothing here.\n", TLDR_AA_ONLY);

    let error = tool_index::audit(ws.path()).expect_err("a README with no tool list is a refusal");

    assert!(
        matches!(error, tool_index::ToolIndexError::NoToolsSection { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn a_tldr_with_no_table_refuses() {
    let ws = workspace(&["aa"], README_AA_ONLY, "# TL;DR\n\nNothing here.\n");

    let error = tool_index::audit(ws.path()).expect_err("a TLDR with no table is a refusal");

    assert!(
        matches!(error, tool_index::ToolIndexError::NoTableRows { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn a_workspace_with_no_binaries_refuses() {
    let dir = TempDir::new().expect("a temp dir");
    let root = dir.path();
    write(root, "Cargo.toml", "[workspace]\nmembers = [\"src/*\"]\n");
    write(root, "README.md", README_AA_ONLY);
    write(root, "TLDR.md", TLDR_AA_ONLY);
    let member = root.join("src").join("libonly");
    write(
        &member,
        "Cargo.toml",
        "[package]\nname = \"libonly\"\nedition = \"2021\"\n",
    );
    write(&member, "src/lib.rs", "");

    let error = tool_index::audit(root).expect_err("a workspace with no tools is a refusal");

    assert!(
        matches!(error, tool_index::ToolIndexError::NoBinaries),
        "unexpected error: {error}"
    );
}

#[test]
fn a_missing_index_refuses() {
    let dir = TempDir::new().expect("a temp dir");
    let root = dir.path();
    write(root, "Cargo.toml", "[workspace]\nmembers = [\"src/*\"]\n");
    write(root, "TLDR.md", TLDR_AA_ONLY);
    let member = root.join("src").join("aa");
    write(
        &member,
        "Cargo.toml",
        "[package]\nname = \"aa\"\nedition = \"2021\"\n",
    );
    write(&member, "src/main.rs", "fn main() {}\n");

    let error = tool_index::audit(root).expect_err("an absent index is a refusal");

    assert!(
        matches!(error, tool_index::ToolIndexError::ReadIndex { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn a_manifest_that_moves_binary_discovery_refuses() {
    let dir = TempDir::new().expect("a temp dir");
    let root = dir.path();
    write(root, "Cargo.toml", "[workspace]\nmembers = [\"src/*\"]\n");
    write(root, "README.md", README_AA_ONLY);
    write(root, "TLDR.md", TLDR_AA_ONLY);
    let member = root.join("src").join("aa");
    write(
        &member,
        "Cargo.toml",
        "[package]\nname = \"aa\"\nedition = \"2021\"\nautobins = false\n",
    );
    write(&member, "src/main.rs", "fn main() {}\n");

    let error = tool_index::audit(root).expect_err("a moved discovery rule is a refusal");

    assert!(
        matches!(
            error,
            tool_index::ToolIndexError::AutoDiscoveryOverride { .. }
        ),
        "unexpected error: {error}"
    );
}

// ---------------------------------------------------------------------------
// Parity with cargo.
// ---------------------------------------------------------------------------

#[test]
fn the_guard_enumerates_exactly_the_binaries_cargo_builds() {
    let root = repo_root();

    let guard: BTreeSet<String> = tool_index::binaries(&root).expect("the binaries are enumerable");
    let cargo = cargo_binary_names(&root);

    assert_eq!(
        guard, cargo,
        "the guard's model of cargo's binary discovery has drifted from cargo's own answer; \
         a binary the guard never enumerates is one it can never report as undocumented"
    );
}

/// Every binary target name `cargo metadata` reports for the workspace at
/// `repo_root`.
///
/// This is the *ground truth* the guard is measured against. Asking cargo is
/// what turns "the guard found nothing wrong" into a claim about the tools that
/// exist, rather than about the tools the guard happened to look for.
fn cargo_binary_names(repo_root: &Path) -> BTreeSet<String> {
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

    let mut names = BTreeSet::new();
    for package in packages {
        let targets = package
            .get("targets")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("a package in `cargo metadata` has no `targets` array"));
        for target in targets {
            let kinds = target
                .get("kind")
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| panic!("a target in `cargo metadata` has no `kind` array"));
            if kinds.iter().any(|kind| kind.as_str() == Some("bin")) {
                names.insert(
                    target
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_else(|| {
                            panic!("a binary target in `cargo metadata` has no name")
                        })
                        .to_owned(),
                );
            }
        }
    }

    assert!(
        !names.is_empty(),
        "`cargo metadata` reported no binaries at all, so the comparison would prove nothing"
    );
    names
}

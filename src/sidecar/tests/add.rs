#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
//! `sidecar add <name> [--yes]`, tested through the binary.

mod support;

use std::fs;
use std::path::Path;

use support::Fixture;

/// The name of the new sidecar in these tests.
const NAME: &str = "pics";

/// The text of a file, or an empty string when the file is missing.
fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// The lines of `gh-calls.log` that are not reads.
fn posts(fixture: &Fixture) -> Vec<String> {
    fixture
        .gh_calls()
        .into_iter()
        .filter(|call| call.contains("-X POST"))
        .collect()
}

/// The two files that `sidecar add` edits in the worktree.
fn edited_files(worktree: &Path) -> (String, String) {
    (
        read(&worktree.join("sidecar.toml")),
        read(&worktree.join(".gitignore")),
    )
}

#[test]
fn an_invalid_name_is_refused_before_any_call() {
    let fixture = Fixture::new(&[]);
    let checkout = fixture.main_checkout();
    let before = (
        read(&checkout.join("sidecar.toml")),
        read(&checkout.join(".gitignore")),
    );
    let run = fixture.run(&checkout, &["add", "Bad_Name", "--yes"]);
    assert_eq!(run.code, 1, "{run:?}");
    assert!(run.stderr.contains("Bad_Name"), "{run:?}");
    assert!(
        run.stderr
            .contains("lowercase letters, digits, hyphens and dots"),
        "{run:?}"
    );
    assert!(fixture.gh_calls().is_empty(), "{:?}", fixture.gh_calls());
    assert_eq!(
        before,
        (
            read(&checkout.join("sidecar.toml")),
            read(&checkout.join(".gitignore"))
        )
    );
    assert!(!fixture.main_clone("Bad_Name").exists());
}

#[test]
fn sidecar_add_prints_the_plan_before_it_does_it() {
    let fixture = Fixture::new(&[]);
    let run = fixture.run(&fixture.main_checkout(), &["add", NAME, "--yes"]);
    assert!(
        run.stdout.contains("sidecar add will do these steps:"),
        "{run:?}"
    );
    assert!(
        run.stdout
            .contains("  - make the private repository fixture-owner/megarepo-data-pics on GitHub"),
        "{run:?}"
    );
}

#[test]
fn an_existing_public_repository_is_refused_and_nothing_changes() {
    let fixture = Fixture::new(&[]);
    fixture.make_sidecar_remote(NAME);
    fixture.set_visibility("megarepo-data-pics", "public");
    let checkout = fixture.main_checkout();
    let before = edited_files(&checkout);
    let run = fixture.run(&checkout, &["add", NAME, "--yes"]);
    assert_eq!(run.code, 1, "{run:?}");
    assert!(run.stderr.contains("public"), "{run:?}");
    assert_eq!(before, edited_files(&checkout));
    assert!(!fixture.main_clone(NAME).exists());
    assert!(fs::symlink_metadata(fixture.link(&checkout, NAME)).is_err());
    assert!(posts(&fixture).is_empty(), "{:?}", fixture.gh_calls());
}

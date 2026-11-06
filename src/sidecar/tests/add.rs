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

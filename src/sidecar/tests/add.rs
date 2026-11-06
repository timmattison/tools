#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
//! `sidecar add <name> [--yes]`, tested through the binary.

mod support;

use std::fs;
use std::path::Path;

use sidecar::registry::Registry;

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

#[test]
fn without_a_terminal_and_without_yes_add_refuses_and_changes_nothing() {
    let fixture = Fixture::new(&[]);
    let checkout = fixture.main_checkout();
    let before = edited_files(&checkout);
    let run = fixture.run(&checkout, &["add", NAME]);
    assert_eq!(run.code, 1, "{run:?}");
    assert!(run.stderr.contains("--yes"), "{run:?}");
    assert!(posts(&fixture).is_empty(), "{:?}", fixture.gh_calls());
    assert_eq!(before, edited_files(&checkout));
    assert!(!fixture.remote(&format!("megarepo-data-{NAME}")).exists());
    assert!(!fixture.main_clone(NAME).exists());
}

/// The names that the file `sidecar.toml` of `worktree` registers.
fn registered(worktree: &Path) -> Vec<String> {
    Registry::parse(&read(&worktree.join("sidecar.toml")))
        .expect("sidecar.toml is a valid registry")
        .sidecars()
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn a_new_name_makes_a_private_repository_and_links_it() {
    let fixture = Fixture::new(&[]);
    let worktree = fixture.add_host_worktree("feature");
    let run = fixture.run(&worktree, &["add", NAME, "--yes"]);
    assert_eq!(run.code, 0, "{run:?}");
    assert!(
        fixture.gh_calls().contains(
            &"api -X POST user/repos -f name=megarepo-data-pics -F private=true --jq .full_name"
                .to_string()
        ),
        "{:?}",
        fixture.gh_calls()
    );
    let remote = fixture.sidecar_remote(NAME);
    let files = fixture.git(&remote, &["ls-tree", "--name-only", "main"]);
    let mut files: Vec<&str> = files.lines().collect();
    files.sort_unstable();
    assert_eq!(files, [".gitattributes", "README.md"]);
    let attributes = fixture.try_git(&remote, &["show", "main:.gitattributes"]);
    assert_eq!(String::from_utf8_lossy(&attributes.stdout), "* -text\n");
    let readme = fixture.git(&remote, &["show", "main:README.md"]);
    assert!(
        readme.contains("megarepo") && readme.contains(NAME),
        "{readme}"
    );
    assert_eq!(registered(&worktree), [NAME]);
    let clone = fixture.main_clone(NAME);
    assert!(clone.join(".git").exists());
    assert_eq!(
        fs::canonicalize(fixture.link(&worktree, NAME)).unwrap(),
        fs::canonicalize(&clone).unwrap()
    );
    assert!(fixture
        .try_git(&worktree, &["check-ignore", "-q", "data/"])
        .status
        .success());
}

#[test]
fn a_new_name_for_an_organization_uses_the_org_path() {
    let fixture = Fixture::new(&[]);
    fixture.set_gh_user("someone-else");
    let run = fixture.run(&fixture.main_checkout(), &["add", NAME, "--yes"]);
    assert_eq!(run.code, 0, "{run:?}");
    assert_eq!(
        posts(&fixture),
        ["api -X POST orgs/fixture-owner/repos -f name=megarepo-data-pics -F private=true --jq .full_name"]
    );
    assert!(fixture.sidecar_remote(NAME).exists());
}

/// Asserts that `sidecar add` adds `/data/` on its own line to a `.gitignore` that
/// starts with `before`.
fn assert_ignore_line_added(before: &str) {
    let fixture = Fixture::new(&[]);
    let checkout = fixture.main_checkout();
    fixture.write(&checkout.join(".gitignore"), before);
    let run = fixture.run(&checkout, &["add", NAME, "--yes"]);
    assert_eq!(run.code, 0, "{run:?}");
    assert!(
        run.stdout.contains("add \"/data/\" to .gitignore"),
        "{run:?}"
    );
    let lines: Vec<String> = read(&checkout.join(".gitignore"))
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(lines, ["target/", "/data/"]);
    assert!(fixture
        .try_git(&checkout, &["check-ignore", "-q", "data/"])
        .status
        .success());
}

#[test]
fn a_missing_gitignore_line_is_added() {
    assert_ignore_line_added("target/\n");
}

#[test]
fn a_missing_gitignore_line_is_added_after_a_last_line_with_no_newline() {
    assert_ignore_line_added("target/");
}

#[test]
fn add_prints_the_token_step_with_the_settings_url() {
    let fixture = Fixture::new(&[]);
    let run = fixture.run(&fixture.main_checkout(), &["add", NAME, "--yes"]);
    assert_eq!(run.code, 0, "{run:?}");
    assert!(run.stdout.contains("Do these steps by hand:"), "{run:?}");
    assert!(
        run.stdout
            .contains("https://github.com/settings/personal-access-tokens"),
        "{run:?}"
    );
    assert!(
        run.stdout.contains("fixture-owner/megarepo-data-pics"),
        "{run:?}"
    );
}

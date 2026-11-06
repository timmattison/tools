#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
//! `sidecar worktree <name> [<branch>]`, tested through the binary.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use support::Fixture;

/// The name of the sidecar in these tests.
const NAME: &str = "testdata";

/// Makes a fixture and runs `sidecar setup` in the main checkout.
fn after_setup() -> Fixture {
    let fixture = Fixture::new(&[NAME]);
    set_up_in(&fixture, &fixture.main_checkout());
    fixture
}

/// Runs `sidecar setup` in `worktree` and asserts success.
fn set_up_in(fixture: &Fixture, worktree: &Path) {
    let run = fixture.run(worktree, &["setup"]);
    assert_eq!(run.code, 0, "{run:?}");
}

/// What the link of `name` in `worktree` points at, canonical.
fn link_target(fixture: &Fixture, worktree: &Path, name: &str) -> PathBuf {
    fs::canonicalize(fixture.link(worktree, name)).expect("the link points at a path")
}

/// The branch checked out in the directory `place`.
fn branch_of(fixture: &Fixture, place: &Path) -> String {
    fixture.git(place, &["symbolic-ref", "--short", "HEAD"])
}

/// Asserts that a run of `sidecar worktree` made the sidecar worktree for `branch`,
/// points the link of `worktree` at it, and names the link, the target, and the branch.
fn assert_pointed_at_new_worktree(
    fixture: &Fixture,
    worktree: &Path,
    run: &support::Run,
    branch: &str,
) {
    assert_eq!(run.code, 0, "{run:?}");
    let target = fixture.worktrees_dir(NAME).join(branch);
    assert!(target.is_dir(), "{} is missing", target.display());
    assert_eq!(branch_of(fixture, &target), branch);
    assert_eq!(
        link_target(fixture, worktree, NAME),
        fs::canonicalize(&target).unwrap()
    );
    assert!(run.stdout.contains("data/testdata"), "{run:?}");
    assert!(run.stdout.contains(&*target.to_string_lossy()), "{run:?}");
    assert!(
        run.stdout.contains(&format!("(branch {branch})")),
        "{run:?}"
    );
}

#[test]
fn no_branch_uses_the_current_branch_of_the_host() {
    let fixture = after_setup();
    let host = fixture.add_host_worktree("issue-123");
    set_up_in(&fixture, &host);

    let run = fixture.run(&host, &["worktree", NAME]);

    assert_pointed_at_new_worktree(&fixture, &host, &run, "issue-123");
}

#[test]
fn a_named_branch_is_used() {
    let fixture = after_setup();
    let host = fixture.add_host_worktree("issue-123");
    set_up_in(&fixture, &host);

    let run = fixture.run(&host, &["worktree", NAME, "other"]);

    assert_pointed_at_new_worktree(&fixture, &host, &run, "other");
}

#[test]
fn a_branch_with_a_slash_makes_nested_directories() {
    let fixture = after_setup();
    let host = fixture.main_checkout();

    let run = fixture.run(&host, &["worktree", NAME, "team/topic"]);

    assert_pointed_at_new_worktree(&fixture, &host, &run, "team/topic");
    assert!(fixture.worktrees_dir(NAME).join("team").is_dir());
}

/// The names in `dir`, sorted. The list is empty when `dir` is missing.
fn entries(dir: &Path) -> Vec<String> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The records of `git worktree list --porcelain -z` in the main clone. A record is
/// the list of fields up to an empty field.
fn worktree_records(fixture: &Fixture) -> Vec<Vec<String>> {
    let output = fixture.try_git(
        &fixture.main_clone(NAME),
        &["worktree", "list", "--porcelain", "-z"],
    );
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut records = Vec::new();
    let mut current = Vec::new();
    for field in text.split('\0') {
        if field.is_empty() {
            if !current.is_empty() {
                records.push(std::mem::take(&mut current));
            }
        } else {
            current.push(field.to_string());
        }
    }
    records
}

#[test]
fn main_points_the_link_at_the_main_clone_and_makes_no_worktree() {
    let fixture = after_setup();
    let host = fixture.add_host_worktree("issue-123");
    set_up_in(&fixture, &host);
    let first = fixture.run(&host, &["worktree", NAME]);
    assert_eq!(first.code, 0, "{first:?}");
    let before_entries = entries(&fixture.worktrees_dir(NAME));
    let before_records = worktree_records(&fixture);

    let run = fixture.run(&host, &["worktree", NAME, "main"]);

    assert_eq!(run.code, 0, "{run:?}");
    assert_eq!(
        link_target(&fixture, &host, NAME),
        fs::canonicalize(fixture.main_clone(NAME)).unwrap()
    );
    assert_eq!(entries(&fixture.worktrees_dir(NAME)), before_entries);
    assert_eq!(worktree_records(&fixture), before_records);
    assert!(run.stdout.contains("(branch main)"), "{run:?}");
}

#[test]
fn two_host_worktrees_share_one_sidecar_worktree() {
    let fixture = after_setup();
    let a = fixture.add_host_worktree("a");
    let b = fixture.add_host_worktree("b");
    set_up_in(&fixture, &a);
    set_up_in(&fixture, &b);

    let first = fixture.run(&a, &["worktree", NAME, "shared"]);
    let second = fixture.run(&b, &["worktree", NAME, "shared"]);

    assert_eq!(first.code, 0, "{first:?}");
    assert_eq!(second.code, 0, "{second:?}");
    assert_eq!(
        link_target(&fixture, &a, NAME),
        link_target(&fixture, &b, NAME)
    );
    let on_shared = worktree_records(&fixture)
        .iter()
        .filter(|record| {
            record
                .iter()
                .any(|field| field == "branch refs/heads/shared")
        })
        .count();
    assert_eq!(on_shared, 1);
}

/// The commit that `rev` names in `place`.
fn commit_of(fixture: &Fixture, place: &Path, rev: &str) -> String {
    fixture.git(place, &["rev-parse", rev])
}

/// Makes a commit on a new branch `branch` of a scratch clone of the bare sidecar
/// remote and pushes it. Returns the commit.
fn push_branch_to_remote(fixture: &Fixture, branch: &str) -> String {
    let scratch = fixture.root().join(format!("scratch-{branch}"));
    fixture.git(
        fixture.root(),
        &[
            "clone",
            "--quiet",
            &fixture.sidecar_remote(NAME).to_string_lossy(),
            &scratch.to_string_lossy(),
        ],
    );
    fixture.git(&scratch, &["checkout", "--quiet", "-b", branch]);
    fixture.write(&scratch.join("from-remote.txt"), branch);
    fixture.git(&scratch, &["add", "from-remote.txt"]);
    fixture.git(&scratch, &["commit", "--quiet", "-m", branch]);
    fixture.git(&scratch, &["push", "--quiet", "origin", branch]);
    commit_of(fixture, &scratch, "HEAD")
}

#[test]
fn a_local_branch_is_taken_first() {
    let fixture = after_setup();
    let host = fixture.main_checkout();
    let clone = fixture.main_clone(NAME);
    fixture.git(&clone, &["branch", "feature"]);
    fixture.git(&clone, &["checkout", "--quiet", "feature"]);
    fixture.write(&clone.join("local-only.txt"), "local");
    fixture.git(&clone, &["add", "local-only.txt"]);
    fixture.git(&clone, &["commit", "--quiet", "-m", "local"]);
    let local = commit_of(&fixture, &clone, "HEAD");
    fixture.git(&clone, &["checkout", "--quiet", "main"]);
    push_branch_to_remote(&fixture, "feature");

    let run = fixture.run(&host, &["worktree", NAME, "feature"]);

    assert_pointed_at_new_worktree(&fixture, &host, &run, "feature");
    let target = fixture.worktrees_dir(NAME).join("feature");
    assert_eq!(commit_of(&fixture, &target, "HEAD"), local);
}

#[test]
fn origin_branch_is_taken_after_a_fetch() {
    let fixture = after_setup();
    let host = fixture.main_checkout();
    let pushed = push_branch_to_remote(&fixture, "remote-only");

    let run = fixture.run(&host, &["worktree", NAME, "remote-only"]);

    assert_pointed_at_new_worktree(&fixture, &host, &run, "remote-only");
    let target = fixture.worktrees_dir(NAME).join("remote-only");
    assert_eq!(commit_of(&fixture, &target, "HEAD"), pushed);
    assert_eq!(
        fixture.git(&target, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        "origin/remote-only"
    );
}

#[test]
fn a_new_branch_from_main_is_the_last_choice() {
    let fixture = after_setup();
    let host = fixture.main_checkout();

    let run = fixture.run(&host, &["worktree", NAME, "fresh"]);

    assert_pointed_at_new_worktree(&fixture, &host, &run, "fresh");
    let target = fixture.worktrees_dir(NAME).join("fresh");
    assert_eq!(
        commit_of(&fixture, &target, "HEAD"),
        commit_of(&fixture, &fixture.main_clone(NAME), "main")
    );
}

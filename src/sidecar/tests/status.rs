#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
//! `sidecar status`, tested through the binary.

mod support;

use std::fs;
use std::path::Path;

use support::{Fixture, Run};

/// The name of the sidecar in these tests.
const NAME: &str = "testdata";

/// The cells of the row that starts with `name`. Two spaces or more split the cells.
fn cells(run: &Run, name: &str) -> Vec<String> {
    let line = run
        .stdout
        .lines()
        .find(|line| line.starts_with(name))
        .unwrap_or_else(|| panic!("no row for {name}: {run:?}"));
    line.split("  ")
        .map(str::trim)
        .filter(|cell| !cell.is_empty())
        .map(str::to_string)
        .collect()
}

/// Asserts the exit code and the two state cells of the row of `NAME`.
fn assert_row(run: &Run, code: i32, clone: &str, link: &str) {
    assert_eq!(run.code, code, "{run:?}");
    assert_eq!(cells(run, NAME), [NAME, clone, link], "{run:?}");
}

/// Makes a fixture, runs `sidecar setup` in the main checkout, and returns both.
fn after_setup() -> Fixture {
    let fixture = Fixture::new(&[NAME]);
    let run = fixture.run(&fixture.main_checkout(), &["setup"]);
    assert_eq!(run.code, 0, "{run:?}");
    fixture
}

/// Makes a worktree of the main clone of `NAME` and points the link of the main
/// checkout at it. `extra` holds the flags of `git worktree add`.
fn point_link_at_worktree(fixture: &Fixture, extra: &[&str]) {
    let place = fixture.worktrees_dir(NAME).join("feature");
    let place_text = place.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "add", "--quiet"];
    args.extend_from_slice(extra);
    args.push(&place_text);
    fixture.git(&fixture.main_clone(NAME), &args);
    replace_link(fixture, |link| {
        std::os::unix::fs::symlink(&place, link).unwrap();
    });
}

/// Removes the link of the main checkout and lets `make` put something else there.
fn replace_link(fixture: &Fixture, make: impl FnOnce(&Path)) {
    let link = fixture.link(&fixture.main_checkout(), NAME);
    fs::remove_file(&link).unwrap();
    make(&link);
}

#[test]
fn clone_missing() {
    let fixture = Fixture::new(&[NAME]);
    let run = fixture.run(&fixture.main_checkout(), &["status"]);
    assert_row(&run, 1, "missing", "missing");
}

#[test]
fn clone_not_a_clone_of_the_sidecar() {
    let fixture = Fixture::new(&[NAME]);
    let place = fixture.main_clone(NAME);
    fs::create_dir_all(&place).unwrap();
    fixture.git(&place, &["init", "--quiet"]);
    let other = fixture.remote("other-repository");
    fixture.git(
        &place,
        &["remote", "add", "origin", &other.to_string_lossy()],
    );
    let run = fixture.run(&fixture.main_checkout(), &["status"]);
    assert_row(&run, 1, "not a clone of the sidecar", "missing");
}

#[test]
fn clone_did_not_end() {
    let fixture = Fixture::new(&[NAME]);
    let place = fixture.main_clone(NAME);
    fs::create_dir_all(&place).unwrap();
    fixture.git(&place, &["init", "--quiet"]);
    let remote = fixture.sidecar_remote(NAME);
    fixture.git(
        &place,
        &["remote", "add", "origin", &remote.to_string_lossy()],
    );
    let run = fixture.run(&fixture.main_checkout(), &["status"]);
    assert_row(&run, 1, "clone did not end", "missing");
}

#[test]
fn clone_ready_and_link_at_the_main_clone() {
    let fixture = after_setup();
    let run = fixture.run(&fixture.main_checkout(), &["status"]);
    assert_row(&run, 0, "ready", "at the main clone");
}

#[test]
fn link_missing_in_a_new_host_worktree() {
    let fixture = after_setup();
    let worktree = fixture.add_host_worktree("feature");
    let run = fixture.run(&worktree, &["status"]);
    assert_row(&run, 1, "ready", "missing");
}

#[test]
fn link_at_a_worktree_on_a_branch() {
    let fixture = after_setup();
    point_link_at_worktree(&fixture, &["-b", "feature"]);
    let run = fixture.run(&fixture.main_checkout(), &["status"]);
    assert_row(&run, 0, "ready", "at a worktree on branch feature");
}

#[test]
fn link_at_a_worktree_on_a_detached_head() {
    let fixture = after_setup();
    point_link_at_worktree(&fixture, &["--detach"]);
    let run = fixture.run(&fixture.main_checkout(), &["status"]);
    assert_row(&run, 0, "ready", "at a worktree on a detached HEAD");
}

#[test]
fn link_broken() {
    let fixture = after_setup();
    let gone = fixture.root().join("gone");
    replace_link(&fixture, |link| {
        std::os::unix::fs::symlink(&gone, link).unwrap();
    });
    let run = fixture.run(&fixture.main_checkout(), &["status"]);
    assert_row(&run, 1, "ready", "broken");
}

#[test]
fn link_foreign_directory() {
    let fixture = after_setup();
    replace_link(&fixture, |link| fs::create_dir(link).unwrap());
    let run = fixture.run(&fixture.main_checkout(), &["status"]);
    assert_row(&run, 1, "ready", "foreign");
}

#[test]
fn link_foreign_other_target() {
    let fixture = after_setup();
    let other = fixture.root().join("elsewhere");
    fs::create_dir_all(&other).unwrap();
    replace_link(&fixture, |link| {
        std::os::unix::fs::symlink(&other, link).unwrap();
    });
    let run = fixture.run(&fixture.main_checkout(), &["status"]);
    assert_row(&run, 1, "ready", "foreign");
}

#[test]
fn a_directory_that_looks_like_a_clone_but_is_not_registered_gives_a_row() {
    let fixture = after_setup();
    let stray = fixture.root().join("code").join("megarepo-data-stray");
    fs::create_dir_all(&stray).unwrap();
    fixture.git(&stray, &["init", "--quiet"]);
    let run = fixture.run(&fixture.main_checkout(), &["status"]);
    assert_eq!(run.code, 1, "{run:?}");
    assert_eq!(cells(&run, "stray"), ["stray", "not registered", "-"]);
    assert_eq!(
        cells(&run, NAME),
        [NAME, "ready", "at the main clone"],
        "{run:?}"
    );
}

#[test]
fn the_worktree_directory_of_a_sidecar_gives_no_row() {
    let fixture = after_setup();
    point_link_at_worktree(&fixture, &["-b", "feature"]);
    assert!(fixture.worktrees_dir(NAME).is_dir());
    let run = fixture.run(&fixture.main_checkout(), &["status"]);
    assert_eq!(run.code, 0, "{run:?}");
    assert!(!run.stdout.contains("worktrees"), "{run:?}");
    assert_eq!(run.stdout.lines().count(), 2, "{run:?}");
}

#[test]
fn status_makes_no_network_call() {
    let fixture = after_setup();
    let before = fixture.run(&fixture.main_checkout(), &["status"]);
    fs::rename(
        fixture.sidecar_remote(NAME),
        fixture.root().join("moved-away.git"),
    )
    .unwrap();
    let after = fixture.run(&fixture.main_checkout(), &["status"]);
    assert_eq!(after.code, 0, "{after:?}");
    assert_eq!(after.stdout, before.stdout);
    assert!(fixture.gh_calls().is_empty());
}

#[test]
fn the_table_has_a_header_padding_and_no_trailing_spaces() {
    let fixture = after_setup();
    let run = fixture.run(&fixture.main_checkout(), &["status"]);
    assert_eq!(
        run.stdout,
        "NAME      CLONE  LINK\ntestdata  ready  at the main clone\n"
    );
    assert!(run.stdout.lines().all(|line| line == line.trim_end()));
}

#[test]
fn a_missing_registry_is_an_error() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    fixture.git(&host, &["rm", "--quiet", "sidecar.toml"]);
    let run = fixture.run(&host, &["status"]);
    assert_eq!(run.code, 1, "{run:?}");
    assert!(run.stderr.starts_with("sidecar: "), "{run:?}");
    assert_eq!(run.stdout, "");
}

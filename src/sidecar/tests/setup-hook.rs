#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
//! `sidecar setup --hook`, tested through the binary.

mod support;

use std::fs;

use support::{Fixture, Run};

/// The name of the sidecar in these tests.
const NAME: &str = "testdata";

/// Two object names, as git gives them to a `post-checkout` hook.
const OLD_HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const NEW_HEAD: &str = "fedcba9876543210fedcba9876543210fedcba98";

/// Moves the bare remote of the sidecar away, so each clone fails.
fn move_remote_away(fixture: &Fixture) {
    fs::rename(
        fixture.sidecar_remote(NAME),
        fixture.root().join("moved-away-testdata.git"),
    )
    .unwrap();
}

/// Asserts that the run printed nothing and made neither a clone nor a link.
fn assert_did_nothing(fixture: &Fixture, host: &std::path::Path, run: &Run) {
    assert_eq!(run.code, 0, "{run:?}");
    assert_eq!(run.stdout, "", "{run:?}");
    assert_eq!(run.stderr, "", "{run:?}");
    assert!(!fixture.main_clone(NAME).exists());
    assert!(fs::symlink_metadata(fixture.link(host, NAME)).is_err());
}

#[test]
fn hook_exits_0_when_a_clone_fails() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    move_remote_away(&fixture);

    let run = fixture.run(&host, &["setup", "--hook"]);

    assert_eq!(run.code, 0, "{run:?}");
    assert!(
        run.stderr
            .lines()
            .any(|line| line.starts_with("warning: ") && line.contains("sidecar setup")),
        "{run:?}"
    );
}

#[test]
fn hook_with_flag_0_does_nothing() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();

    let run = fixture.run(&host, &["setup", "--hook", OLD_HEAD, NEW_HEAD, "0"]);

    assert_did_nothing(&fixture, &host, &run);
}

#[test]
fn hook_with_flag_1_runs_the_setup() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();

    let run = fixture.run(&host, &["setup", "--hook", OLD_HEAD, NEW_HEAD, "1"]);

    assert_eq!(run.code, 0, "{run:?}");
    assert!(fs::symlink_metadata(fixture.link(&host, NAME)).is_ok());
    assert!(fixture.main_clone(NAME).join("data.txt").is_file());
}

#[test]
fn hook_does_nothing_when_ci_is_set() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();

    let run = fixture.run_with_env(&host, &["setup", "--hook"], &[("CI", "true")]);

    assert_did_nothing(&fixture, &host, &run);
}

#[test]
fn hook_runs_the_setup_when_ci_is_0() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();

    let run = fixture.run_with_env(&host, &["setup", "--hook"], &[("CI", "0")]);

    assert_eq!(run.code, 0, "{run:?}");
    assert!(fs::symlink_metadata(fixture.link(&host, NAME)).is_ok());
}

#[test]
fn hook_exits_0_when_git_does_not_ignore_the_link_dir() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    fs::write(host.join(".gitignore"), "").unwrap();

    let run = fixture.run(&host, &["setup", "--hook"]);

    assert_eq!(run.code, 0, "{run:?}");
    assert!(
        run.stderr
            .lines()
            .any(|line| line.starts_with("warning: ") && line.contains("/data/")),
        "{run:?}"
    );
    assert!(!fixture.main_clone(NAME).exists());
}

#[test]
fn hook_exits_0_when_sidecar_toml_is_missing() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    fs::remove_file(host.join("sidecar.toml")).unwrap();

    let run = fixture.run(&host, &["setup", "--hook"]);

    assert_eq!(run.code, 0, "{run:?}");
    assert!(
        run.stderr.lines().any(|line| line.starts_with("warning: ")),
        "{run:?}"
    );
}

#[test]
fn hook_args_need_the_hook_flag() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();

    let run = fixture.run(&host, &["setup", "a", "b", "1"]);

    assert_eq!(run.code, 2, "{run:?}");
}

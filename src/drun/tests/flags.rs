//! End-to-end tests of `drun check-flags`, which compares the flag tables of
//! `drun` with the help of the docker CLI.
//!
//! The fake docker prints the committed help of Docker, so no test needs a
//! real docker and no test skips when docker is missing.

// The fake docker is a POSIX shell script.
#![cfg(unix)]
// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use std::path::Path;

use support::Fixture;

/// The docker commands whose flags `drun` keeps in a table.
const COMMANDS: [&str; 5] = ["run", "logs", "exec", "cp", "rm"];

/// The committed help of the docker command `command`.
fn committed_help(command: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/help")
        .join(format!("{command}.txt"));
    std::fs::read_to_string(&path).expect("the committed help exists")
}

/// A fixture whose docker prints the committed help for every command.
fn committed() -> Fixture {
    let fixture = Fixture::new();
    for command in COMMANDS {
        fixture.help(command, &committed_help(command));
    }
    fixture
}

#[test]
fn check_flags_agrees_with_the_committed_help() {
    let run = committed().run(&["check-flags"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(run.stdout.contains("agree"), "{}", run.stdout);
}

#[test]
fn check_flags_asks_docker_for_help_without_an_engine() {
    let fixture = committed();
    fixture.run_with(
        &["check-flags"],
        &[
            ("DOCKER_CONTEXT", "box-a"),
            ("DOCKER_HOST", "tcp://box-a.example:2376"),
            ("DOCKER_API_VERSION", "1.40"),
            ("DOCKER_TLS_VERIFY", "1"),
        ],
    );
    for command in COMMANDS {
        assert_eq!(
            fixture.help_environment(command),
            "DOCKER_HOST=unix:///dev/null/drun-no-engine.sock\n",
            "the environment of docker {command} --help",
        );
    }
}

#[test]
fn check_flags_needs_no_hosts() {
    let fixture = committed();
    let run = fixture.run_with(&["check-flags"], &[("DRUN_CONTEXTS", "")]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
}

#[test]
fn check_flags_reports_a_flag_that_docker_adds_and_exits_with_1() {
    let fixture = committed();
    fixture.help(
        "run",
        &format!(
            "{}      --new-flag string                  Do something new\n",
            committed_help("run")
        ),
    );
    let run = fixture.run(&["check-flags"]);
    assert_eq!(run.code, Some(1), "{}", run.stderr);
    assert!(
        run.stdout.contains("docker run: docker lists --new-flag"),
        "{}",
        run.stdout
    );
    assert!(!run.stdout.contains("agree"), "{}", run.stdout);
}

#[test]
fn check_flags_fails_when_it_cannot_read_the_help() {
    let fixture = committed();
    fixture.help("exec", "Options:\n  this is not a flag\n");
    let run = fixture.run(&["check-flags"]);
    assert_eq!(run.code, Some(125), "{}", run.stderr);
    assert!(
        run.stderr.contains("cannot read the help of docker exec"),
        "{}",
        run.stderr
    );
    assert!(!run.stdout.contains("agree"), "{}", run.stdout);
}

#[test]
fn check_flags_fails_when_docker_gives_no_help() {
    let fixture = Fixture::new();
    let run = fixture.run(&["check-flags"]);
    assert_eq!(run.code, Some(125), "{}", run.stderr);
    assert!(run.stderr.contains("no help for run"), "{}", run.stderr);
    assert!(!run.stdout.contains("agree"), "{}", run.stdout);
}

#[test]
fn check_flags_takes_no_arguments() {
    let run = committed().run(&["check-flags", "run"]);
    assert_eq!(run.code, Some(2), "{}", run.stderr);
    assert!(
        run.stderr.contains("drun check-flags takes no arguments"),
        "{}",
        run.stderr
    );
}

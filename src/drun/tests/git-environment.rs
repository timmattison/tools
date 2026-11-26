//! End-to-end tests of the `GIT_` environment of each docker that `drun`
//! starts.
//!
//! A git hook exports `GIT_DIR`, `GIT_INDEX_FILE`, and
//! `GIT_CONFIG_PARAMETERS` into each program that it starts, and docker runs
//! git for a build whose context is a git URL. Thus `drun` must not give those
//! variables to docker. A person sets `GIT_SSH_COMMAND` and
//! `GIT_TERMINAL_PROMPT` on purpose, so `drun` gives those to docker.

// The fake docker is a POSIX shell script.
#![cfg(unix)]
// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use support::{Fixture, LOCAL};

/// The `GIT_` environment of `drun` when a hook starts it, for a person who
/// also set two variables of their own.
const HOOK_AND_USER_ENVIRONMENT: [(&str, &str); 5] = [
    ("GIT_DIR", "/decoy/.git"),
    ("GIT_INDEX_FILE", ".git/index"),
    ("GIT_CONFIG_PARAMETERS", "'user.email'='hook@example.com'"),
    ("GIT_SSH_COMMAND", "ssh -i /keys/deploy"),
    ("GIT_TERMINAL_PROMPT", "0"),
];

/// The `GIT_` variables that docker must get: only those of the person.
const USER_ENVIRONMENT: &str = "GIT_SSH_COMMAND=ssh -i /keys/deploy\nGIT_TERMINAL_PROMPT=0\n";

/// The one host of the tests that ask a host.
const HOST: &str = "box-a";

#[test]
fn a_call_to_a_host_gets_the_git_variables_of_the_person_and_not_of_the_hook() {
    let fixture = Fixture::new();
    fixture.hosts(&[HOST]);
    let run = fixture.run_with(&["ps"], &HOOK_AND_USER_ENVIRONMENT);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(fixture.git_environment(HOST), USER_ENVIRONMENT);
}

#[test]
fn a_passed_through_command_gets_the_git_variables_of_the_person_and_not_of_the_hook() {
    let fixture = Fixture::new();
    let run = fixture.run_with(&["frobnicate"], &HOOK_AND_USER_ENVIRONMENT);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(fixture.git_environment(LOCAL), USER_ENVIRONMENT);
}

#[test]
fn a_help_call_of_check_flags_gets_the_git_variables_of_the_person_and_not_of_the_hook() {
    let fixture = Fixture::new();
    fixture.run_with(&["check-flags"], &HOOK_AND_USER_ENVIRONMENT);
    assert_eq!(fixture.git_environment(LOCAL), USER_ENVIRONMENT);
}

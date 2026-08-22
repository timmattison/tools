//! The harness's own contract: whatever the suite spawns — a fixture's git or
//! the real `swt` binary — is sandboxed from the host the same way.
//!
//! Every other file here asserts something about `swt`. This one asserts
//! something about the harness, because the harness is what makes those
//! assertions mean anything. Two entrances build a child process
//! ([`support::git_command`] and [`support::swt_command`]) and a rule applied at
//! only one of them is worse than no rule at all: the suite reads as sandboxed
//! while half of it inherits the developer's or CI machine's git configuration —
//! `core.hooksPath`, `pull.rebase`, aliases, credential helpers — and fails on a
//! machine nobody can reproduce. So the isolation is pinned here rather than
//! trusted to a comment.

mod support;

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;

use support::{git_command, swt_command};

/// Git's global and system configuration, pointed at an empty file. A child that
/// carries these reads no configuration but what the fixture pinned itself.
const NEUTRALIZED_CONFIG: [(&str, &str); 2] = [
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_SYSTEM", "/dev/null"),
];

/// A directory to build the commands *for*. Nothing is spawned and nothing is
/// written — only the environment the commands carry is inspected.
fn scratch() -> &'static Path {
    Path::new("/")
}

/// The environment overrides a built command carries: `Some(value)` for a
/// variable it sets, `None` for one it removes.
fn env_overrides(command: &Command) -> BTreeMap<OsString, Option<OsString>> {
    command
        .get_envs()
        .map(|(name, value)| (name.to_os_string(), value.map(OsStr::to_os_string)))
        .collect()
}

/// The binary under test runs against an empty global and system git config, so
/// its behavior is decided by the fixture repository and nothing on the host.
#[test]
fn spawning_the_binary_under_test_neutralizes_the_host_git_config() {
    let overrides = env_overrides(&swt_command(scratch()));

    for (name, value) in NEUTRALIZED_CONFIG {
        assert_eq!(
            overrides.get(OsStr::new(name)),
            Some(&Some(OsString::from(value))),
            "swt_command should set {name}={value}, or the binary under test reads \
             the host's git configuration"
        );
    }
}

/// Neither entrance is more sandboxed than the other. This is the assertion that
/// keeps them from drifting: a rule added to one and forgotten at the other
/// fails here instead of quietly halving the suite's isolation.
#[test]
fn both_spawn_entrances_apply_the_same_isolation() {
    assert_eq!(
        env_overrides(&swt_command(scratch())),
        env_overrides(&git_command(scratch(), &["status"])),
        "the fixture-git and binary-under-test entrances should scrub and pin the \
         same environment"
    );
}

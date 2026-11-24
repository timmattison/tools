//! The harness's own contract: whatever the suite spawns — a fixture's git, the
//! real `swt` binary, or a shell that runs git — is sandboxed from the host the
//! same way.
//!
//! Every other file here asserts something about `swt`. This one asserts
//! something about the harness, because the harness is what makes those
//! assertions mean anything. Three entrances build a child process
//! ([`support::git_command`], [`support::swt_command`] and
//! [`support::shell_command`]), and a rule applied at only some of them is
//! worse than no rule at all: the suite reads as sandboxed while part of it
//! inherits the developer's or CI machine's git configuration —
//! `core.hooksPath`, `pull.rebase`, aliases, credential helpers — and fails on
//! a machine nobody can reproduce. So the isolation is pinned here rather than
//! trusted to a comment.

mod support;

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;

use gitscratch::testing::{child_ran, run_child_half};
use support::{git_command, shell_command, swt_command};

/// The environment overrides a built command carries: `Some(value)` for a
/// variable it sets, `None` for one it removes.
type Overrides = BTreeMap<OsString, Option<OsString>>;

/// Git's global and system configuration, pointed at an empty file. A child that
/// carries these reads no configuration but what the fixture pinned itself.
const NEUTRALIZED_CONFIG: [(&str, &str); 2] = [
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_SYSTEM", "/dev/null"),
];

/// The prefix of every variable git reads from the environment. The harness
/// sheds each inherited variable that carries it, whatever its name.
const GIT_PREFIX: &str = "GIT_";

/// Marks the re-executed child half of
/// [`every_spawn_entrance_sheds_every_inherited_git_variable`]. The child reads
/// it to know that it runs under the probe environment.
const CHILD_MARKER: &str = "SWT_SPAWN_ISOLATION_CHILD";

/// The libtest filter for the one test that the child half runs. A test of an
/// integration-test crate sits at the crate root, so the filter is the bare
/// function name.
///
/// The compiler does not check this string against the test it names. So
/// `run_child_half` requires the child to say that it ran.
const SHED_TEST: &str = "every_spawn_entrance_sheds_every_inherited_git_variable";

/// The value every probe carries. The child spawns nothing, so the content does
/// not matter. It is not empty, so the variable is present in the environment of
/// the child.
const PROBE_VALUE: &str = "swt-spawn-isolation-probe";

/// The script that the shell entrance is built with. Nothing is spawned, so the
/// script never runs.
const SHELL_SCRIPT: &str = "true";

/// `GIT_` variables that a list of location names does not hold, each with the
/// cost of a child that inherits it. A failure quotes the cost, so the failure
/// explains itself.
///
/// This is a sample of the family and not its definition. The last name is one
/// that git has never defined. No list holds it, and the prefix holds it.
const UNLISTED_GIT_VARIABLES: [(&str, &str); 3] = [
    (
        "GIT_CONFIG_PARAMETERS",
        "git exports it into every hook, and it injects configuration such as `user.email`, \
         `core.bare` and `core.hooksPath` into the child",
    ),
    (
        "GIT_OBJECT_DIRECTORY",
        "the child writes every object into the store of another repository",
    ),
    (
        "GIT_SWT_PROBE_NEVER_INVENTED",
        "the rule is the prefix, because no list holds a name that git adds later",
    ),
];

/// A directory to build the commands *for*. Nothing is spawned and nothing is
/// written — only the environment the commands carry is inspected.
fn scratch() -> &'static Path {
    Path::new("/")
}

/// The environment overrides a built command carries.
fn env_overrides(command: &Command) -> Overrides {
    command
        .get_envs()
        .map(|(name, value)| (name.to_os_string(), value.map(OsStr::to_os_string)))
        .collect()
}

/// Every entrance of the harness that builds a child process, by name, each
/// with the overrides of a command that it builds. A new entrance goes here, so
/// that each test below checks it.
fn entrances() -> [(&'static str, Overrides); 3] {
    [
        (
            "git_command",
            env_overrides(&git_command(scratch(), &["status"])),
        ),
        ("swt_command", env_overrides(&swt_command(scratch()))),
        (
            "shell_command",
            env_overrides(&shell_command(scratch(), SHELL_SCRIPT)),
        ),
    ]
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

/// No entrance is more sandboxed than another. This is the assertion that keeps
/// them from drifting: a rule added to one and forgotten at another fails here
/// instead of quietly shrinking the suite's isolation.
#[test]
fn every_spawn_entrance_applies_the_same_isolation() {
    let [(reference_name, reference), others @ ..] = entrances();
    for (entrance, overrides) in others {
        assert_eq!(
            overrides, reference,
            "{entrance} and {reference_name} should scrub and pin the same environment"
        );
    }
}

/// Every entrance sheds every `GIT_` variable that the suite inherits, and not
/// only the names of a list.
///
/// Git reads the environment before it reads the working directory of a
/// command. Git also exports its own variables into every hook, and the
/// pre-commit hook of this repository runs `cargo test`. A list of location
/// names misses `GIT_OBJECT_DIRECTORY` and `GIT_CONFIG_PARAMETERS`, and it
/// misses every variable that git adds later.
///
/// The probes go into the environment of a re-executed child of this test
/// binary, not into this process. Cargo runs the tests of one binary as threads
/// of one process, so `std::env::set_var` changes the environment of every
/// sibling test. The child builds a command at each entrance and reads the
/// removals that each one schedules. It spawns nothing.
#[test]
fn every_spawn_entrance_sheds_every_inherited_git_variable() {
    if std::env::var_os(CHILD_MARKER).is_some() {
        assert_every_entrance_sheds_the_inherited_git_environment();
        // The child reaches this line only when the assertions above hold. The
        // parent reads it as the proof that this branch ran.
        child_ran();
        return;
    }

    let outcome = run_child_half(SHED_TEST, |child| {
        child.env(CHILD_MARKER, "1");
        for (name, _) in UNLISTED_GIT_VARIABLES {
            child.env(name, PROBE_VALUE);
        }
    });

    if let Err(report) = outcome {
        panic!("a spawn entrance kept an inherited `{GIT_PREFIX}` variable:\n{report}");
    }
}

/// The child half of [`every_spawn_entrance_sheds_every_inherited_git_variable`].
///
/// It checks the probes first, in their written order, so a failure names a
/// probe and quotes its cost. Then it checks every other `GIT_` variable that
/// this process holds, because a child of a hook holds more than the probes.
/// The two names that the harness pins are not in scope here.
/// [`spawning_the_binary_under_test_neutralizes_the_host_git_config`] checks
/// them.
///
/// # Panics
///
/// Panics when a probe is absent from this process, or when an entrance does not
/// schedule the removal of a `GIT_` variable that this process holds.
fn assert_every_entrance_sheds_the_inherited_git_environment() {
    for (name, _) in UNLISTED_GIT_VARIABLES {
        assert!(
            std::env::var_os(name).is_some(),
            "the child half did not get `{name}`, so this test checks nothing"
        );
    }

    for (entrance, overrides) in entrances() {
        for (name, consequence) in UNLISTED_GIT_VARIABLES {
            assert_eq!(
                overrides.get(OsStr::new(name)),
                Some(&None),
                "{entrance} does not shed the inherited `{name}`: {consequence}"
            );
        }

        for name in held_git_names() {
            assert_eq!(
                overrides.get(&name),
                Some(&None),
                "{entrance} does not shed the inherited `{}`, so the child reads a git \
                 setting that the harness did not choose",
                name.to_string_lossy()
            );
        }
    }
}

/// Every `GIT_` variable that this process holds, less the two names that the
/// harness pins to an empty file on purpose.
fn held_git_names() -> Vec<OsString> {
    std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| name.to_string_lossy().starts_with(GIT_PREFIX))
        .filter(|name| {
            !NEUTRALIZED_CONFIG
                .iter()
                .any(|(pinned, _)| name == OsStr::new(pinned))
        })
        .collect()
}

//! Pins the contract of the shared `nwt_command` builder (issue #283): every
//! integration test that spawns the real `nwt` binary does so through this
//! builder, and the builder must scrub the environment that would otherwise let
//! the spawned `nwt` reach out of its fixture and act on the developer's real
//! session — the terminal multiplexer (whose tab it would rename) and the whole
//! `GIT_*` family (which would point its `git worktree add`, its object writes
//! and its config reads at the developer's real repository).
//!
//! **The rule under test is the prefix, not a list of names.** An earlier
//! version of this file asserted a fixed five-name list, which merely mirrored
//! the builder's own five `env_remove` calls — so it could not catch the one
//! thing worth catching, a list that has gone stale because git grew another
//! variable. A stale list strips nothing new and still reports clean. This test
//! therefore sets a battery of `GIT_*` variables in its own process, including
//! names the builder never mentions by name, and demands that every one of them
//! is scheduled for removal.
//!
//! `ZELLIJ`/`TMUX` stay named explicitly because they are not a prefix family:
//! there is no `MULTIPLEXER_*` to sweep, and the tab-rename tests deliberately
//! re-add them on the returned command.
//!
//! This inspects the builder's configured environment directly via
//! `Command::get_envs`, so it is deterministic and cross-platform — no process
//! is spawned and it does not depend on whether the test runner itself happens
//! to be inside a multiplexer or a git hook.
//!
//! This file deliberately holds a SINGLE `#[test]`: it mutates the process-wide
//! environment, and cargo runs the tests within one test binary on parallel
//! threads. One test per binary means there is no sibling thread to race with,
//! while every other test file is a separate process with its own environment.

mod support;

use std::ffi::{OsStr, OsString};
use std::process::Command;

use support::nwt_command;

/// The value every probe variable is set to.
///
/// Its content is irrelevant — this test never spawns the command it builds, it
/// only inspects the removals the builder scheduled — but it must be non-empty
/// so the variable is genuinely present in the process environment and thus
/// visible to a scrub that enumerates `std::env::vars_os()`.
const PROBE_VALUE: &str = "nwt-builder-guard-probe";

/// Keys the builder must schedule for removal, each paired with the consequence
/// of leaving it in place — quoted back in the failure message so a regression
/// explains itself rather than just naming a missing key.
///
/// The `GIT_*` entries are a *sample* of the family, not its definition: the
/// contract is that every `GIT_*` variable present in the parent is removed, and
/// the ones below are simply the ones whose consequences are known to be
/// severe. `GIT_CONFIG_PARAMETERS` and `GIT_OBJECT_DIRECTORY` in particular were
/// verified to pass straight through a three-name scrub.
const REQUIRED_REMOVALS: &[(&str, &str)] = &[
    (
        "ZELLIJ",
        "the spawned nwt would believe it is inside zellij and rename the user's real tab",
    ),
    (
        "TMUX",
        "the spawned nwt would believe it is inside tmux and rename the user's real window",
    ),
    (
        "GIT_DIR",
        "git exports this into hooks and it overrides cwd-based discovery, so the spawned \
         nwt would create worktrees in the real repo instead of the fixture",
    ),
    (
        "GIT_WORK_TREE",
        "the spawned nwt's git would resolve paths against the real working tree",
    ),
    (
        "GIT_INDEX_FILE",
        "the spawned nwt's git would stage into the real repo's index",
    ),
    (
        "GIT_CONFIG_PARAMETERS",
        "git exports this to every pre-commit hook, and it injects arbitrary config \
         (user.email, core.bare, core.hooksPath) into the spawned nwt's git",
    ),
    (
        "GIT_OBJECT_DIRECTORY",
        "the spawned nwt's git would write its blobs and trees into the real repo's \
         object store — a write into another repository",
    ),
    (
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "the spawned nwt's git would resolve objects it never wrote, so a fixture could \
         appear to contain the real repo's history",
    ),
    (
        "GIT_COMMON_DIR",
        "refs, config and the object store would resolve into the real repo even with \
         GIT_DIR removed",
    ),
    (
        "GIT_CEILING_DIRECTORIES",
        "upward repo discovery from the fixture would be cut short or redirected, so the \
         spawned nwt could resolve to the wrong repository",
    ),
    (
        "GIT_AUTHOR_NAME",
        "git exports this to every pre-commit hook, so a commit the spawned nwt makes \
         would be attributed to whoever launched the suite rather than to the fixture",
    ),
];

/// The keys a [`Command`] will delete from the child's inherited environment.
///
/// `get_envs` yields `(key, Option<value>)`; a `None` value means the key is
/// scheduled for *removal* rather than being set to some value.
fn scheduled_removals(cmd: &Command) -> Vec<OsString> {
    cmd.get_envs()
        .filter(|(_, value)| value.is_none())
        .map(|(key, _)| key.to_owned())
        .collect()
}

#[test]
fn nwt_command_scrubs_the_environment_that_would_escape_the_fixture() {
    // A prefix scrub driven by `std::env::vars_os()` can only schedule a key it
    // can see, so the probe variables must be present in *this* process before
    // the builder runs.
    for (key, _) in REQUIRED_REMOVALS {
        std::env::set_var(key, PROBE_VALUE);
    }

    let cmd = nwt_command(&std::env::temp_dir());
    let removals = scheduled_removals(&cmd);

    // Put the environment back before asserting: a failing assertion panics, and
    // leaving probe variables set would make the failure infect anything that
    // ran afterwards in this process.
    for (key, _) in REQUIRED_REMOVALS {
        std::env::remove_var(key);
    }

    for (key, consequence) in REQUIRED_REMOVALS {
        assert!(
            removals.iter().any(|k| k == OsStr::new(key)),
            "nwt_command must remove {key} from the child env; otherwise {consequence}.\n\
             The rule is the GIT_ prefix, never a list of names — a list goes stale the \
             day git adds a variable, and then it strips nothing new while still \
             reporting clean.\nscheduled removals: {removals:?}"
        );
    }
}

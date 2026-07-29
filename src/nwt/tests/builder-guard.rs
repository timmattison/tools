//! Pins the contract of the shared `nwt_command` builder (issue #283): every
//! integration test that spawns the real `nwt` binary does so through this
//! builder, and the builder must scrub the environment that would otherwise let
//! the spawned `nwt` reach out of its fixture and act on the developer's real
//! session — the terminal multiplexer (whose tab it would rename) and the git
//! location vars (whose repo it would create worktrees in).
//!
//! This inspects the builder's configured environment directly via
//! `Command::get_envs`, so it is deterministic and cross-platform — no process
//! is spawned and it does not depend on whether the test runner itself happens
//! to be inside a multiplexer or a git hook.

mod support;

use std::ffi::{OsStr, OsString};
use std::process::Command;

use support::nwt_command;

/// Keys the builder must schedule for removal, each paired with the consequence
/// of leaving it in place — quoted back in the failure message so a regression
/// explains itself rather than just naming a missing key.
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
    let cmd = nwt_command(&std::env::temp_dir());
    let removals = scheduled_removals(&cmd);

    for (key, consequence) in REQUIRED_REMOVALS {
        assert!(
            removals.iter().any(|k| k == OsStr::new(key)),
            "nwt_command must remove {key} from the child env; otherwise {consequence}.\n\
             scheduled removals: {removals:?}"
        );
    }
}

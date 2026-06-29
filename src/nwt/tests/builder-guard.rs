//! Pins the contract of the shared `nwt_command` builder (issue #283): every
//! integration test that spawns the real `nwt` binary does so through this
//! builder, and the builder must scrub the terminal-multiplexer environment
//! from the child so a suite launched from inside zellij/tmux can never hijack
//! the user's real tab.
//!
//! This inspects the builder's configured environment directly via
//! `Command::get_envs`, so it is deterministic and cross-platform — no process
//! is spawned and it does not depend on whether the test runner itself happens
//! to be inside a multiplexer.

mod support;

use std::ffi::OsStr;

use support::nwt_command;

#[test]
fn nwt_command_scrubs_multiplexer_env() {
    let cmd = nwt_command(&std::env::temp_dir());

    // `get_envs` yields `(key, Option<value>)`; a `None` value means the key is
    // scheduled for *removal* from the child's inherited environment.
    let scheduled_for_removal: Vec<_> = cmd
        .get_envs()
        .filter(|(_, value)| value.is_none())
        .map(|(key, _)| key.to_owned())
        .collect();

    assert!(
        scheduled_for_removal
            .iter()
            .any(|k| k == OsStr::new("ZELLIJ")),
        "nwt_command must remove ZELLIJ from the child env so the spawned nwt \
         never believes it is inside zellij.\nscheduled removals: {scheduled_for_removal:?}"
    );
    assert!(
        scheduled_for_removal
            .iter()
            .any(|k| k == OsStr::new("TMUX")),
        "nwt_command must remove TMUX from the child env so the spawned nwt \
         never believes it is inside tmux.\nscheduled removals: {scheduled_for_removal:?}"
    );
}

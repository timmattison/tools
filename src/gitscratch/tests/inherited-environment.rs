//! What [`gitscratch::shed_inherited_git_environment`] takes off a command, and
//! why the rule has to be the `GIT_` prefix rather than a list of names.
//!
//! Git hands every hook a family of `GIT_*` variables describing the commit
//! being made, and whatever that hook launches inherits them — `cargo test`, a
//! tool built on this crate, a fixture building a throwaway repository. Each one
//! redirects some part of where git reads or writes, or says who wrote the
//! commit, and git resolves the environment ahead of both `-c` and `git config`,
//! so none of this crate's pinned settings can overrule one.
//!
//! **A named list is the bug, not the fix.** A list strips nothing new the day
//! git adds a variable, and from then on it returns the same clean-looking
//! answer as a list that works — which is, from the outside, indistinguishable
//! from a guard doing its job. This helper carried a fifteen-name list once, and
//! `GIT_CONFIG_PARAMETERS` walked straight through it. Git exports that variable
//! to every hook, and it injects arbitrary configuration — `user.email`,
//! `core.bare`, `core.hooksPath` — into every git this crate spawns. Enumerating
//! [`std::env::vars_os`] sweeps whatever git invents next, and nobody has to
//! edit this file for it.
//!
//! The probes below are a *sample* of the family and never its definition. One
//! of them is a name git has never defined, which is the point: no list can hold
//! a variable nobody has heard of yet, and the prefix holds it for free.
//!
//! This file holds a SINGLE `#[test]` on purpose. It mutates the process-wide
//! environment, and cargo runs the tests of one binary on parallel threads, so
//! one test per binary is what guarantees no sibling thread races with it. Cargo
//! gives every integration-test file a process of its own, which is exactly that
//! guarantee.

use std::ffi::{OsStr, OsString};
use std::process::Command;

/// The value every probe is set to.
///
/// The content is irrelevant — this test never spawns the command it builds, it
/// only reads back the removals the helper scheduled — but it must be non-empty
/// so the variable is genuinely present in the process environment, and so
/// visible to a scrub that enumerates [`std::env::vars_os`].
const PROBE_VALUE: &str = "gitscratch-inherited-environment-probe";

/// `GIT_*` variables the helper's fifteen-name list never mentioned, each paired
/// with what leaving it in place costs. The consequence is quoted back on
/// failure so a regression explains itself rather than only naming a key.
const UNLISTED_GIT_VARIABLES: &[(&str, &str)] = &[
    (
        "GIT_CONFIG_PARAMETERS",
        "every git this crate spawns would run under configuration the launching shell injected, \
         `user.email`, `core.bare` and `core.hooksPath` among it",
    ),
    (
        "GIT_CONFIG_COUNT",
        "git would read the numbered `GIT_CONFIG_KEY_*`/`GIT_CONFIG_VALUE_*` pairs beside it as \
         configuration, which is the same injection under a second spelling",
    ),
    (
        "GIT_CONFIG_GLOBAL",
        "git would read the caller's substitute for `~/.gitconfig` in place of the host's",
    ),
    (
        "GIT_CONFIG_SYSTEM",
        "git would read the caller's substitute for `/etc/gitconfig` in place of the host's",
    ),
    (
        "GIT_SSH_COMMAND",
        "a replay would reach any remote through a program the caller chose, which a local \
         dry-run has no reason to inherit",
    ),
    (
        "GIT_A_VARIABLE_GIT_HAS_NOT_INVENTED_YET",
        "the rule is the prefix, because no list can name a variable nobody has heard of yet",
    ),
];

/// Names that merely begin with the letters `GIT`, or merely contain a swept
/// name, and must survive. They pin the sweep as a prefix rule rather than a
/// substring one.
const MUST_SURVIVE: &[&str] = &["GITHUB_TOKEN", "GITALY_ADDRESS", "NOT_GIT_DIR"];

#[test]
fn sheds_every_inherited_git_variable_and_nothing_else() {
    for (key, _) in UNLISTED_GIT_VARIABLES {
        std::env::set_var(key, PROBE_VALUE);
    }
    for key in MUST_SURVIVE {
        std::env::set_var(key, PROBE_VALUE);
    }

    let mut command = Command::new("git");
    gitscratch::shed_inherited_git_environment(&mut command);

    // `env_remove` records a removal as a `None` value against the key, so the
    // scheduled removals can be read back without spawning anything.
    let removed: Vec<OsString> = command
        .get_envs()
        .filter(|(_, value)| value.is_none())
        .map(|(key, _)| key.to_os_string())
        .collect();

    for (key, consequence) in UNLISTED_GIT_VARIABLES {
        assert!(
            removed.iter().any(|removal| removal == OsStr::new(key)),
            "`{key}` survived the scrub: {consequence}"
        );
    }

    for key in MUST_SURVIVE {
        assert!(
            !removed.iter().any(|removal| removal == OsStr::new(key)),
            "`{key}` was removed, but the rule is the `GIT_` prefix and this name does not carry \
             it — a sweep that over-matches takes the caller's own environment with it"
        );
    }
}

//! What [`gitscratch::shed_inherited_git_environment_keeping_user_intent`]
//! takes off a production command, and what it deliberately leaves on.
//!
//! A fixture and a production tool differ in one way that decides the rule. A
//! fixture builds a throwaway repository and has no user whose intent to honor,
//! so it sheds the whole `GIT_` prefix. A production tool acts for the person
//! who started it. That person exports `GIT_SSH_COMMAND` to reach a host with a
//! key that is not the default one, and `GIT_TERMINAL_PROMPT=0` so that a
//! missing credential fails at once rather than waiting on a prompt nobody can
//! see. A tool that drops those two turns a working install into an
//! authentication failure, or into a stall on a stdout its shell wrapper
//! captures.
//!
//! **The split is not a split by git's families.** The prefix cannot tell
//! git's families apart, and a rule that tried to would be a list of names
//! again. The split is by whose intent a variable carries: git exports
//! `GIT_DIR` and `GIT_CONFIG_PARAMETERS` into every hook, so those carry the
//! launching hook's intent and leave; a person exports `GIT_SSH_COMMAND`, so
//! that carries the user's intent and stays.
//!
//! **A keep-list is safe in the way a strip-list is not.** Both go stale, and
//! the directions differ. A stale strip-list inherits a variable git added
//! after it was written, and reports the same clean-looking answer as a list
//! that works. A stale keep-list sheds a variable git added after it was
//! written, so the cost is one setting a user states again rather than one
//! repository a tool writes into by mistake.
//!
//! Every test here feeds synthetic keys to
//! [`gitscratch::shed_git_environment_from`] and never touches the process
//! environment. Cargo runs the tests of one binary on parallel threads, so a
//! test that set a real `GIT_*` variable would redirect the git children of
//! every sibling thread while it did so. That is why this file may hold more
//! than one `#[test]` where its sibling `tests/inherited-environment.rs`, which
//! does mutate the environment, holds exactly one.

use std::ffi::OsStr;
use std::process::Command;

use gitscratch::{
    shed_git_environment_from, InheritedGitEnvironment, HOOK_EXPORTED_GIT_ENVIRONMENT,
    USER_INTENT_GIT_ENVIRONMENT,
};

/// `GIT_` variables a production spawn must shed, each paired with what leaving
/// it in place costs. The consequence is quoted back on failure, so a
/// regression explains itself rather than only naming a key.
const MUST_SHED: &[(&str, &str)] = &[
    (
        "GIT_DIR",
        "points git at a foreign repository, and it beats both `current_dir` and `git -C`",
    ),
    (
        "GIT_WORK_TREE",
        "aims the checkout and every `git add` at a foreign working tree",
    ),
    (
        "GIT_INDEX_FILE",
        "stages files into a foreign repository's index, and git exports it to every pre-commit \
         hook",
    ),
    (
        "GIT_OBJECT_DIRECTORY",
        "writes every blob, tree and commit into a foreign repository's object store",
    ),
    (
        "GIT_CONFIG_PARAMETERS",
        "injects arbitrary configuration (`user.email`, `core.bare`, `core.hooksPath`) into the \
         child, and git hands it to every hook",
    ),
    (
        "GIT_AUTHOR_NAME",
        "authors the commits under whatever identity the launching environment carried",
    ),
];

/// A name no git has ever defined.
///
/// The rule is the `GIT_` prefix, so this leaves with the rest. A rewrite of
/// the sweep into a list of names strips nothing here and reports clean, which
/// is the failure this name exists to catch.
const INVENTED_LATER: &str = "GIT_SOMETHING_GIT_INVENTS_LATER";

/// Keys that are not git's, each paired with what removing it would cost. They
/// pin the sweep as a rule about the `GIT_` prefix and not as a rule about the
/// whole environment.
const NOT_GIT: &[(&str, &str)] = &[
    (
        "PATH",
        "is how the child finds `git` at all, so a spawn without it fails before it starts",
    ),
    (
        "HOME",
        "is where git looks `~/.gitconfig` up, so a child without it reads no user configuration",
    ),
];

/// The removals `command` holds, read back without spawning anything.
///
/// `env_remove` records a removal as a `None` value against the key, so
/// [`Command::get_envs`] reports the whole schedule.
fn scheduled_removals(command: &Command) -> Vec<String> {
    command
        .get_envs()
        .filter(|(_, value)| value.is_none())
        .map(|(key, _)| key.to_string_lossy().into_owned())
        .collect()
}

/// The production rule: shed the `GIT_` prefix, keep the six names the user
/// stated, and touch nothing outside the prefix.
///
/// The key list holds all three groups at once, because the rule is one rule
/// and a test that asked about each group separately could pass with three
/// rules that disagree.
#[test]
fn keeps_the_user_s_own_git_variables_and_sheds_the_rest_of_the_prefix() {
    let keys: Vec<&str> = MUST_SHED
        .iter()
        .map(|(key, _)| *key)
        .chain(USER_INTENT_GIT_ENVIRONMENT.iter().copied())
        .chain(NOT_GIT.iter().map(|(key, _)| *key))
        .collect();

    let mut command = Command::new("git");
    shed_git_environment_from(&mut command, &keys, InheritedGitEnvironment::KeepUserIntent);
    let removed = scheduled_removals(&command);

    for (key, consequence) in MUST_SHED {
        assert!(
            removed.iter().any(|removal| removal == key),
            "`{key}` survived a production sweep: left in place it {consequence}. The rule is the \
             `GIT_` prefix, and the only names it keeps are the ones a person states on purpose."
        );
    }

    for key in USER_INTENT_GIT_ENVIRONMENT {
        assert!(
            !removed.iter().any(|removal| removal == key),
            "`{key}` left a production sweep, and a person sets it on purpose. Dropping the \
             authentication family turns a working install into an authentication failure or \
             into a hang; dropping the configuration files answers a question about the user's \
             own git with a file the user replaced."
        );
    }

    for (key, cost) in NOT_GIT {
        assert!(
            !removed.iter().any(|removal| removal == key),
            "`{key}` left a production sweep, and it does not carry the `GIT_` prefix: it {cost}. \
             A sweep that over-matches takes the caller's own environment with it."
        );
    }
}

/// The keep-list and the names git exports into a hook share no name.
///
/// A name git hands every hook cannot be read as the user's intent, because the
/// value in the environment came from git and not from the person. A keep-list
/// that held one of those names would let a hook aim a production tool at the
/// hook's own repository, which is the whole defect the sweep exists to remove.
///
/// `GIT_EDITOR` is the entry that shows the rule has teeth. A person sets
/// `GIT_EDITOR` on purpose, exactly as they set `GIT_SSH_COMMAND`, and it is
/// still out of the keep-list — because git exports it into a hook as well, and
/// the hook's value is the one a tool would inherit.
#[test]
fn no_name_git_hands_a_hook_is_read_as_the_user_s_intent() {
    assert!(
        !HOOK_EXPORTED_GIT_ENVIRONMENT.is_empty() && !USER_INTENT_GIT_ENVIRONMENT.is_empty(),
        "one of the two lists is empty, so the disjointness below holds for a reason that has \
         nothing to do with the rule"
    );

    for name in HOOK_EXPORTED_GIT_ENVIRONMENT {
        assert!(
            !USER_INTENT_GIT_ENVIRONMENT.contains(name),
            "`{name}` is in both lists. Git exports it into the environment of every hook, so a \
             production tool that keeps it inherits the launching hook's value and acts on the \
             hook's repository."
        );
    }
}

/// A `GIT_` variable that does not exist today leaves on the production rule
/// too.
///
/// This is the test a rewrite of the sweep into a strip-list fails. Such a
/// rewrite strips the names somebody wrote down and inherits everything git
/// adds afterwards, and it reports the same clean-looking answer as a sweep
/// that works.
#[test]
fn sheds_a_git_variable_that_does_not_exist_yet() {
    let mut command = Command::new("git");
    shed_git_environment_from(
        &mut command,
        [INVENTED_LATER],
        InheritedGitEnvironment::KeepUserIntent,
    );

    assert!(
        scheduled_removals(&command)
            .iter()
            .any(|removal| removal == INVENTED_LATER),
        "`{INVENTED_LATER}` survived a production sweep. The rule is the `GIT_` prefix and the \
         keep-list is six names, so a name nobody has heard of yet leaves by default. A sweep \
         that keeps it is a strip-list, and a strip-list reports clean for whatever git invents \
         next."
    );
}

/// The fixture rule keeps nothing, the keep-list included.
///
/// Without this the two rules could quietly become one. A fixture has no user
/// whose intent to honor: it builds a throwaway repository, and a
/// `GIT_SSH_COMMAND` it inherited names a program the fixture has no reason to
/// run.
#[test]
fn the_fixture_rule_sheds_a_name_the_production_rule_keeps() {
    let kept_by_production = USER_INTENT_GIT_ENVIRONMENT
        .first()
        .expect("the keep-list names at least one variable");

    let mut command = Command::new("git");
    shed_git_environment_from(
        &mut command,
        [OsStr::new(kept_by_production)],
        InheritedGitEnvironment::ShedEverything,
    );

    assert!(
        scheduled_removals(&command)
            .iter()
            .any(|removal| removal == kept_by_production),
        "`{kept_by_production}` survived a fixture sweep. The fixture rule keeps nothing, and the \
         two rules have collapsed into one."
    );
}

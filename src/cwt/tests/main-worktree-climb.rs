//! What `--main` does when the user already stands in the main worktree.
//!
//! `family-main-worktree.rs` proves which repository answers the first press of
//! `wtm`: the one the user stands in. These tests prove what the next press
//! does. A user who is already in their main worktree has asked to go up, so
//! `--main` climbs to the repository that holds theirs and takes its main
//! worktree, and repeats that for as deep as the nest goes.
//!
//! The climb starts at the main worktree of the user's repository, never at the
//! worktree they stand in. That is what keeps the first press of `wtm` in a
//! child's feature worktree from skipping the child and landing on the parent.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use support::{code, combined, cwt, target_path, Nest};

/// The exit code `cwt` uses when it finds no worktree.
const WORKTREE_NOT_FOUND: i32 = 3;

/// Asserts that `cwt` with `args`, run in `from`, printed the path of `expected`.
fn assert_goes_to(nest: &Nest, from: &str, args: &[&str], expected: &str) {
    let output = cwt(&nest.at(from), args);

    assert_eq!(
        code(&output),
        0,
        "cwt {args:?} failed in {from}: {}",
        combined(&output)
    );
    assert_eq!(
        target_path(&output),
        nest.path_of(expected),
        "cwt {args:?} in {from} must go to {expected}"
    );
}

#[test]
fn a_feature_worktree_still_reaches_its_own_main_worktree() {
    // The first press of wtm. The climb must not fire here: the user is not in
    // the main worktree yet, and the repository above holds a main worktree of
    // its own that would swallow this one.
    let nest = Nest::build();

    assert_goes_to(
        &nest,
        "top/middle/leaf-worktrees/feature",
        &["--main"],
        "top/middle/leaf",
    );
}

#[test]
fn a_subdirectory_of_the_main_worktree_still_reaches_the_top_of_it() {
    // The first press of wtm, from where a developer normally stands: inside
    // the main worktree, below its root. That is a request to go to the top of
    // the worktree, not out of the repository. Only the root itself climbs.
    let nest = Nest::build();
    let inside = "top/middle/leaf/src/deep";
    nest.deepen(inside);

    assert_goes_to(&nest, inside, &["--main"], "top/middle/leaf");
}

#[test]
fn a_subdirectory_of_the_topmost_repository_still_reaches_the_top_of_it() {
    // The same press in the repository the climb has nothing above. Standing
    // below its root must take the user to it, never report that no repository
    // holds it: the user asked to go to the top of their worktree and there is
    // one to go to.
    let nest = Nest::build();
    let inside = "top/src/deep";
    nest.deepen(inside);

    assert_goes_to(&nest, inside, &["--main"], "top");
}

#[test]
fn a_subdirectory_of_a_feature_worktree_still_reaches_its_own_main_worktree() {
    // The case the fix must leave alone. A linked worktree already answered
    // with its repository's main worktree from its root, and standing below
    // that root changes nothing.
    let nest = Nest::build();
    let inside = "top/middle/leaf-worktrees/feature/src/deep";
    nest.deepen(inside);

    assert_goes_to(&nest, inside, &["--main"], "top/middle/leaf");
}

#[test]
fn the_main_worktree_climbs_to_the_repository_that_holds_it() {
    // The second press. leaf is a repository inside middle, so middle answers.
    // middle is on master, which proves the climb ranks branches the way the
    // first press does.
    let nest = Nest::build();

    assert_goes_to(&nest, "top/middle/leaf", &["--main"], "top/middle");
}

#[test]
fn the_climb_repeats_for_every_level_of_the_nest() {
    // The third press. The nest is three deep, so the climb has to happen more
    // than once to reach the top of it.
    let nest = Nest::build();

    assert_goes_to(&nest, "top/middle", &["--main"], "top");
}

#[test]
fn the_top_of_the_nest_reports_that_no_repository_holds_it() {
    let nest = Nest::build();
    let output = cwt(&nest.at("top"), &["--main"]);

    assert_eq!(
        code(&output),
        WORKTREE_NOT_FOUND,
        "cwt --main at the top must report not found, got: {}",
        combined(&output)
    );
    assert!(
        output.stdout.is_empty(),
        "cwt must print no path when nothing holds the repository, got: {}",
        combined(&output)
    );
}

#[test]
fn the_top_of_the_nest_names_the_repository_it_climbed_from() {
    // The message has to say which repository has nothing above it, or the user
    // cannot tell this apart from a repository with no main branch.
    let nest = Nest::build();
    let output = cwt(&nest.at("top"), &["--main"]);
    let message = combined(&output);

    assert!(
        message.contains("No repository above"),
        "the message must say that nothing holds this repository, got: {message}"
    );
    assert!(
        message.contains(&nest.path_of("top")),
        "the message must name the repository it climbed from, got: {message}"
    );
}

#[test]
fn a_repository_on_the_ladder_without_a_main_branch_is_stepped_over() {
    // hub is on trunk, so it has no main worktree to offer. It cannot be a
    // destination, and it must not stop the climb either: top is above it and
    // does have one.
    let nest = Nest::build();

    assert_goes_to(&nest, "top/hub/twig", &["--main"], "top");
}

#[test]
fn the_climb_reaches_the_main_branch_of_the_repository_above_not_its_directory() {
    // away is on trunk and keeps main in a worktree beside itself. The climb
    // finds the repository by the directory that holds sprig, and then has to
    // ask that repository for its main worktree like any other.
    let nest = Nest::build();

    assert_goes_to(
        &nest,
        "top/away/sprig",
        &["--main"],
        "top/away-worktrees/main",
    );
}

#[test]
fn opting_out_of_the_family_still_climbs() {
    // --no-family says which repositories the listing shows. The climb is not a
    // listing, and wtm has to behave the same either way.
    let nest = Nest::build();

    assert_goes_to(
        &nest,
        "top/middle/leaf",
        &["--no-family", "--main"],
        "top/middle",
    );
}

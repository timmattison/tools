//! Which worktree a name selects when the family offers more than one.
//!
//! The rule is nearest first: the repository the user stands in, then the
//! parent repository, then the rest. `wtm` has to keep meaning "my repository's
//! main branch" wherever the user is standing.

// These mirror the crate-root attributes in src/main.rs. A crate-root attribute
// reaches only its own target, so the binary raising them does nothing for this
// test target; repeating them here is what keeps the whole crate under one lint
// set now that they no longer live in a manifest `[lints]` table.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use support::{code, cwt, target_path, Family};

#[test]
fn a_name_prefers_the_repository_the_user_stands_in() {
    // `main` is a branch of the parent and of child-a. Standing in child-a, the
    // name has to select child-a.
    let family = Family::build();
    let output = cwt(&family.at("family/child-a"), &["main"]);

    assert_eq!(code(&output), 0, "cwt main failed in child-a");
    assert_eq!(
        target_path(&output),
        family.path_of("family/child-a"),
        "the repository the user stands in answers first"
    );
}

#[test]
fn a_name_the_home_repository_lacks_falls_back_to_the_parent() {
    // child-b has no `main` branch, so the parent's answers.
    let family = Family::build();
    let output = cwt(&family.at("family/child-b"), &["main"]);

    assert_eq!(code(&output), 0, "cwt main failed in child-b");
    assert_eq!(
        target_path(&output),
        family.path_of("family"),
        "the parent repository answers when the home repository cannot"
    );
}

#[test]
fn the_parent_answers_its_own_name_from_its_own_worktree() {
    let family = Family::build();
    let output = cwt(&family.at("family-worktrees/feature"), &["main"]);

    assert_eq!(
        target_path(&output),
        family.path_of("family"),
        "standing in a worktree of the parent, main is still the parent's"
    );
}

#[test]
fn a_child_repository_is_reachable_by_its_directory_name() {
    let family = Family::build();
    let output = cwt(&family.at("family"), &["child-b"]);

    assert_eq!(code(&output), 0, "cwt child-b failed");
    assert_eq!(target_path(&output), family.path_of("family/child-b"));
}

#[test]
fn a_child_worktree_is_reachable_by_its_branch() {
    let family = Family::build();
    let output = cwt(&family.at("family"), &["beta"]);

    assert_eq!(code(&output), 0, "cwt beta failed");
    assert_eq!(
        target_path(&output),
        family.path_of("family/child-b-worktrees/beta"),
        "a branch of a child repository is selectable from the parent"
    );
}

#[test]
fn a_child_worktree_is_reachable_from_a_sibling_repository() {
    let family = Family::build();
    let output = cwt(&family.at("family/child-a"), &["beta"]);

    assert_eq!(code(&output), 0, "cwt beta failed in child-a");
    assert_eq!(
        target_path(&output),
        family.path_of("family/child-b-worktrees/beta"),
        "siblings can reach each other without going through the parent"
    );
}

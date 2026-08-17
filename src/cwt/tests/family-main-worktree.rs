//! Which main worktree `--main` selects when the family offers more than one.
//!
//! Every repository of a family has a main worktree of its own, so the shortcut
//! has to choose between them. It chooses the repository the user stands in.
//!
//! This is where `--main` parts from a plain name. A name the home repository
//! cannot answer falls back to the parent repository, which is what
//! `family-navigation.rs` proves. `--main` does not fall back: a repository with
//! no `main` and no `master` has no main worktree, and sending the user to a
//! sibling's main worktree is not the shortcut they asked for.

// These mirror the crate-root attributes in src/main.rs. A crate-root attribute
// reaches only its own target, so the binary raising them does nothing for this
// test target; repeating them here is what keeps the whole crate under one lint
// set now that they no longer live in a manifest `[lints]` table.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use support::{code, combined, cwt, target_path, Family};

/// The exit code `cwt` uses when it finds no worktree.
const WORKTREE_NOT_FOUND: i32 = 3;

#[test]
fn the_main_worktree_of_a_child_is_that_child_not_the_parent() {
    // The parent and child-a are both on branch `main`, and the parent comes
    // first in the listing. Standing in child-a, the shortcut has to stay there.
    let family = Family::build();
    let output = cwt(&family.at("family/child-a-worktrees/shared"), &["--main"]);

    assert_eq!(code(&output), 0, "cwt --main failed in child-a");
    assert_eq!(
        target_path(&output),
        family.path_of("family/child-a"),
        "the shortcut must stay in the repository the user stands in"
    );
}

#[test]
fn the_main_worktree_of_the_parent_is_the_parent() {
    let family = Family::build();
    let output = cwt(&family.at("family-worktrees/feature"), &["--main"]);

    assert_eq!(code(&output), 0, "cwt --main failed in the parent");
    assert_eq!(
        target_path(&output),
        family.path_of("family"),
        "the parent's own worktree must reach the parent's main worktree"
    );
}

#[test]
fn a_repository_without_main_or_master_does_not_borrow_a_sibling() {
    // child-b is on `trunk`. The parent and child-a both have a worktree on
    // `main`, and neither of them is child-b's main worktree.
    let family = Family::build();
    let output = cwt(&family.at("family/child-b-worktrees/beta"), &["--main"]);

    assert_eq!(
        code(&output),
        WORKTREE_NOT_FOUND,
        "cwt --main must report not found, got: {}",
        combined(&output)
    );
    assert!(
        output.stdout.is_empty(),
        "cwt must print no path when the repository has no main worktree"
    );
}

#[test]
fn the_not_found_message_still_names_every_branch_that_was_searched() {
    // The message is built from the branch constant, and the family must not
    // have cost it that.
    let family = Family::build();
    let output = cwt(&family.at("family/child-b-worktrees/beta"), &["--main"]);
    let message = combined(&output);

    assert!(
        message.contains("'main' or 'master'"),
        "the not-found message must name every branch cwt searched for, got: {message}"
    );
}

#[test]
fn no_family_leaves_the_main_worktree_where_it_was() {
    // --main already stays inside one repository, so opting out of the family
    // must not move it.
    let family = Family::build();
    let output = cwt(
        &family.at("family/child-a-worktrees/shared"),
        &["--no-family", "--main"],
    );

    assert_eq!(code(&output), 0, "cwt --no-family --main failed in child-a");
    assert_eq!(
        target_path(&output),
        family.path_of("family/child-a"),
        "--no-family must not change which main worktree the shortcut selects"
    );
}

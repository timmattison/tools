//! Opting out of the family.
//!
//! `--no-family` confines `cwt` to the repository the user is standing in,
//! which is what `cwt` did before families existed. `CWT_NO_FAMILY` makes that
//! choice permanent without wrapping the command.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use support::{code, combined, cwt, cwt_with_env, stdout, target_path, Family};

#[test]
fn no_family_lists_only_the_repository_the_user_is_in() {
    let family = Family::build();
    let output = cwt(&family.at("family"), &["--no-family"]);

    assert_eq!(code(&output), 0, "cwt --no-family failed");
    assert_eq!(
        stdout(&output),
        format!(
            "> {} [main]\n  {} [inside]\n  {} [feature]\n",
            family.path_of("family"),
            family.path_of("family/inside-wt"),
            family.path_of("family-worktrees/feature")
        ),
        "one repository prints as a plain list, with no headings and no children"
    );
}

#[test]
fn no_family_confines_cycling_to_one_repository() {
    let family = Family::build();
    let output = cwt(
        &family.at("family-worktrees/feature"),
        &["-f", "--no-family"],
    );

    assert_eq!(code(&output), 0, "cwt -f --no-family failed");
    assert_eq!(
        target_path(&output),
        family.path_of("family"),
        "the last worktree of the repository wraps to its first, not into a child"
    );
}

#[test]
fn no_family_cannot_reach_a_child_repository() {
    let family = Family::build();
    let output = cwt(&family.at("family"), &["--no-family", "beta"]);

    assert_ne!(
        code(&output),
        0,
        "beta belongs to a child repository, which is out of scope: {}",
        combined(&output)
    );
    assert_eq!(
        stdout(&output),
        "",
        "a name that is not found prints no path"
    );
}

#[test]
fn the_environment_variable_opts_out_of_the_family() {
    let family = Family::build();
    let output = cwt_with_env(&family.at("family"), &[], &[("CWT_NO_FAMILY", "1")]);

    assert_eq!(code(&output), 0, "cwt failed: {}", combined(&output));
    assert_eq!(
        stdout(&output),
        format!(
            "> {} [main]\n  {} [inside]\n  {} [feature]\n",
            family.path_of("family"),
            family.path_of("family/inside-wt"),
            family.path_of("family-worktrees/feature")
        ),
        "CWT_NO_FAMILY=1 has the same effect as --no-family"
    );
}

#[test]
fn an_empty_environment_variable_keeps_the_family() {
    let family = Family::build();
    let output = cwt_with_env(&family.at("family"), &[], &[("CWT_NO_FAMILY", "")]);

    assert!(
        stdout(&output).contains(&family.path_of("family/child-a")),
        "an empty value is not a choice, so the family stays: {}",
        stdout(&output)
    );
}

#[test]
fn a_zero_environment_variable_keeps_the_family() {
    let family = Family::build();
    let output = cwt_with_env(&family.at("family"), &[], &[("CWT_NO_FAMILY", "0")]);

    assert!(
        stdout(&output).contains(&family.path_of("family/child-a")),
        "0 is the choice not to opt out, so the family stays: {}",
        stdout(&output)
    );
}

//! `cwt` treats a parent repository and the repositories checked out one level
//! below it as one family: the listing shows all of them, grouped by
//! repository, and the cycling flags walk the whole family.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use support::{code, cwt, headings, parse_listing, stdout, Family};

#[test]
fn list_includes_the_worktrees_of_every_child_repository() {
    let family = Family::build();
    let output = cwt(&family.at("family"), &[]);
    assert_eq!(code(&output), 0, "cwt failed: {}", stdout(&output));

    let listed = parse_listing(&stdout(&output));
    let paths: Vec<&str> = listed.iter().map(|l| l.path.as_str()).collect();

    for expected in [
        "aaa-worktree",
        "family",
        "family-worktrees/feature",
        "family/inside-wt",
        "family/child-a",
        "family/child-a-worktrees/shared",
        "family/child-b",
        "family/child-b-worktrees/beta",
        "family/child-b-worktrees/shared",
    ] {
        let wanted = family.path_of(expected);
        assert!(
            paths.contains(&wanted.as_str()),
            "cwt must list {wanted}, but listed {paths:#?}"
        );
    }
    assert_eq!(paths.len(), 9, "cwt listed unexpected extras: {paths:#?}");
}

#[test]
fn a_worktree_of_the_parent_inside_the_parent_does_not_become_a_repository() {
    // `inside-wt` is a worktree of the parent, sitting exactly where the scan
    // for child repositories looks. It belongs to the parent's group, once.
    let family = Family::build();
    let output = cwt(&family.at("family"), &[]);

    let listed = parse_listing(&stdout(&output));
    let wanted = family.path_of("family/inside-wt");
    let holding: Vec<&str> = listed
        .iter()
        .filter(|l| l.path == wanted)
        .map(|l| l.repo.as_str())
        .collect();

    assert_eq!(
        holding,
        vec!["family"],
        "the parent's own worktree is listed once, under the parent"
    );
    assert_eq!(
        headings(&listed),
        vec!["family", "child-a", "child-b"],
        "it must not open a repository group of its own"
    );
}

#[test]
fn a_repository_owns_its_worktrees_wherever_they_sit() {
    // `aaa-worktree` is a worktree of child-b that lives outside the family
    // directory altogether. It is still child-b's.
    let family = Family::build();
    let output = cwt(&family.at("family"), &[]);

    let listed = parse_listing(&stdout(&output));
    let wanted = family.path_of("aaa-worktree");
    let holding: Vec<&str> = listed
        .iter()
        .filter(|l| l.path == wanted)
        .map(|l| l.repo.as_str())
        .collect();

    assert_eq!(holding, vec!["child-b"]);
}

#[test]
fn list_groups_each_worktree_under_its_own_repository() {
    let family = Family::build();
    let output = cwt(&family.at("family"), &[]);

    let listed = parse_listing(&stdout(&output));
    assert_eq!(
        headings(&listed),
        vec!["family", "child-a", "child-b"],
        "the parent repository comes first, then the child repositories by name"
    );

    let child_b: Vec<&str> = listed
        .iter()
        .filter(|l| l.repo == "child-b")
        .map(|l| l.path.as_str())
        .collect();
    assert_eq!(
        child_b,
        vec![
            family.path_of("aaa-worktree"),
            family.path_of("family/child-b"),
            family.path_of("family/child-b-worktrees/beta"),
            family.path_of("family/child-b-worktrees/shared"),
        ],
        "every worktree of child-b belongs under the child-b heading, in path order"
    );
}

#[test]
fn list_ignores_a_directory_that_is_not_a_repository() {
    let family = Family::build();
    let output = cwt(&family.at("family"), &[]);

    assert!(
        !stdout(&output).contains("docs"),
        "the plain docs directory is not a repository and must not be listed: {}",
        stdout(&output)
    );
}

#[test]
fn list_from_a_child_repository_shows_the_whole_family() {
    let family = Family::build();
    let output = cwt(&family.at("family/child-a"), &[]);
    assert_eq!(code(&output), 0, "cwt failed: {}", stdout(&output));

    let listed = parse_listing(&stdout(&output));
    assert_eq!(
        headings(&listed),
        vec!["family", "child-a", "child-b"],
        "standing in a child repository shows the same family as standing in the parent"
    );

    let current: Vec<&str> = listed
        .iter()
        .filter(|l| l.current)
        .map(|l| l.path.as_str())
        .collect();
    assert_eq!(
        current,
        vec![family.path_of("family/child-a")],
        "the marker names the worktree the user stands in"
    );
}

#[test]
fn forward_leaves_one_repository_and_enters_the_next() {
    let family = Family::build();
    let output = cwt(&family.at("family-worktrees/feature"), &["-f"]);

    assert_eq!(code(&output), 0, "cwt -f failed: {}", stdout(&output));
    assert_eq!(
        stdout(&output).trim_end(),
        family.path_of("family/child-a"),
        "the last worktree of the parent is followed by the first child repository"
    );
}

#[test]
fn previous_leaves_one_repository_and_enters_the_one_before() {
    let family = Family::build();
    let output = cwt(&family.at("family/child-a"), &["-p"]);

    assert_eq!(code(&output), 0, "cwt -p failed: {}", stdout(&output));
    assert_eq!(
        stdout(&output).trim_end(),
        family.path_of("family-worktrees/feature"),
        "the first worktree of a child repository is preceded by the parent's last"
    );
}

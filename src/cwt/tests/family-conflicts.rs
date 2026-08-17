//! What `cwt` does when a name fits more than one repository in the family.
//!
//! It refuses to guess, and it names the candidates the way the user has to
//! type them: `repository:worktree`.

mod support;

use support::{code, combined, cwt, stdout, target_path, Family};

/// The exit code `cwt --help` publishes for an ambiguous name.
const MULTIPLE_MATCHES: i32 = 6;
/// The exit code `cwt --help` publishes for a name it cannot find.
const WORKTREE_NOT_FOUND: i32 = 3;

#[test]
fn a_name_two_repositories_share_is_refused() {
    // `shared` is a worktree of child-a and of child-b. Standing in the parent,
    // neither is nearer, so cwt must not pick one.
    let family = Family::build();
    let output = cwt(&family.at("family"), &["shared"]);

    assert_eq!(
        code(&output),
        MULTIPLE_MATCHES,
        "cwt must refuse an ambiguous name: {}",
        combined(&output)
    );
    assert_eq!(
        stdout(&output),
        "",
        "a refused name must print no path, or the shell function changes directory"
    );

    let message = combined(&output);
    assert!(
        message.contains("child-a:shared"),
        "the message must name child-a's candidate the way it has to be typed: {message}"
    );
    assert!(
        message.contains("child-b:shared"),
        "the message must name child-b's candidate the way it has to be typed: {message}"
    );
}

#[test]
fn a_repository_prefix_settles_the_conflict() {
    let family = Family::build();
    let output = cwt(&family.at("family"), &["child-b:shared"]);

    assert_eq!(
        code(&output),
        0,
        "cwt child-b:shared failed: {}",
        combined(&output)
    );
    assert_eq!(
        target_path(&output),
        family.path_of("family/child-b-worktrees/shared"),
        "the prefix picks the repository, the rest picks the worktree"
    );
}

#[test]
fn a_bare_repository_prefix_selects_its_main_worktree() {
    let family = Family::build();
    let output = cwt(&family.at("family"), &["child-b:"]);

    assert_eq!(code(&output), 0, "cwt child-b: failed: {}", combined(&output));
    assert_eq!(
        target_path(&output),
        family.path_of("family/child-b"),
        "a repository name with nothing after it means that repository's main worktree"
    );
}

#[test]
fn a_repository_prefix_can_name_the_parent() {
    let family = Family::build();
    let output = cwt(&family.at("family/child-a"), &["family:main"]);

    assert_eq!(
        code(&output),
        0,
        "cwt family:main failed: {}",
        combined(&output)
    );
    assert_eq!(
        target_path(&output),
        family.path_of("family"),
        "the parent is reachable by name from a child repository"
    );
}

#[test]
fn an_unknown_repository_prefix_is_not_found() {
    let family = Family::build();
    let output = cwt(&family.at("family"), &["nope:shared"]);

    assert_eq!(
        code(&output),
        WORKTREE_NOT_FOUND,
        "no repository is named nope: {}",
        combined(&output)
    );
    assert_eq!(stdout(&output), "", "a name that is not found prints no path");
}

#[test]
fn the_home_repository_answers_a_shared_name_without_a_prefix() {
    // Standing in child-a, `shared` is not ambiguous: the home repository has one.
    let family = Family::build();
    let output = cwt(&family.at("family/child-a"), &["shared"]);

    assert_eq!(code(&output), 0, "cwt shared failed: {}", combined(&output));
    assert_eq!(
        target_path(&output),
        family.path_of("family/child-a-worktrees/shared"),
        "a name the home repository can answer is never ambiguous"
    );
}

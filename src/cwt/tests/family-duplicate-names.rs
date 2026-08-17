//! Two repositories of one family can carry the same directory name: a parent
//! that holds a child named after itself. Each is still a repository of its
//! own, and every name `cwt` prints has to select the worktree it was printed
//! for.

// These mirror the crate-root attributes in src/main.rs. A crate-root attribute
// reaches only its own target, so the binary raising them does nothing for this
// test target; repeating them here is what keeps the whole crate under one lint
// set now that they no longer live in a manifest `[lints]` table.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use support::{
    add_worktree, code, combined, cwt, git, headings, make_repo, parse_listing, stdout, target_path,
};

/// The exit code `cwt --help` publishes for an ambiguous name.
const MULTIPLE_MATCHES: i32 = 6;
/// The exit code `cwt --help` publishes for a name it cannot find.
const WORKTREE_NOT_FOUND: i32 = 3;

/// A family whose parent repository holds a child of its own directory name.
///
/// ```text
/// root/
///   nest/                          repository (the anchor), branch anchor-main
///   nest-worktrees/spare           worktree of nest, branch anchor-spare
///   nest/nest/                     repository, branch child-main
///   nest/nest-worktrees/only       worktree of nest/nest, branch child-only
///   nest/nest-worktrees/shared     worktree of nest/nest, branch child-shared
///   nest/other/                    repository, branch other-main
///   nest/other-worktrees/shared    worktree of other, branch other-shared
/// ```
///
/// `nest` is the directory name of two repositories: the anchor and the child
/// checked out inside it. `other` shares its name with nothing, so the tests
/// can see that only the repeated name is treated differently.
///
/// Every branch name is unique, so the worktree a name reached can be named
/// from the outside: no two worktrees of one repository can have the same
/// branch checked out.
///
/// `shared` is the directory name of one worktree of `nest/nest` and one of
/// `other`. Standing in the anchor, neither of those repositories is nearer
/// than the other, so `cwt shared` is ambiguous and prints a candidate list.
struct Nest {
    /// Kept alive so the temp directory outlives the test.
    _tmp: TempDir,
    /// The canonical path of the temp directory. Canonical because git prints
    /// resolved paths, and on macOS the temp directory is reached through a
    /// symbolic link.
    root: PathBuf,
}

impl Nest {
    /// Build the family described in the type documentation.
    fn build() -> Self {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let root = tmp
            .path()
            .canonicalize()
            .expect("failed to canonicalize temp dir");

        make_repo(&root.join("nest"), "anchor-main");
        add_worktree(
            &root.join("nest"),
            "../nest-worktrees/spare",
            "anchor-spare",
        );

        // A repository one level below the anchor, carrying the anchor's name.
        make_repo(&root.join("nest/nest"), "child-main");
        add_worktree(
            &root.join("nest/nest"),
            "../nest-worktrees/only",
            "child-only",
        );
        add_worktree(
            &root.join("nest/nest"),
            "../nest-worktrees/shared",
            "child-shared",
        );

        make_repo(&root.join("nest/other"), "other-main");
        add_worktree(
            &root.join("nest/other"),
            "../other-worktrees/shared",
            "other-shared",
        );

        Self { _tmp: tmp, root }
    }

    /// Resolve a path inside the family, for example `nest/other`.
    fn at(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// The path of a worktree as a string, for comparison with `cwt` output.
    fn path_of(&self, relative: &str) -> String {
        self.at(relative).display().to_string()
    }

    /// The anchor repository, which is where most of these tests stand.
    fn anchor(&self) -> PathBuf {
        self.at("nest")
    }
}

/// The names `cwt` listed under a message, one per indented line.
fn listed_labels(message: &str) -> Vec<String> {
    message
        .lines()
        .filter(|line| line.starts_with("  "))
        .map(|line| line.trim().to_string())
        .collect()
}

/// Split a label of the form `repo:worktree [branch]` into the name the user
/// has to type and the branch that name promises to land on.
fn label_parts(label: &str) -> (String, String) {
    let (target, branch) = label
        .rsplit_once(" [")
        .unwrap_or_else(|| panic!("'{label}' is not a label of the form 'name [branch]'"));
    (
        target.trim().to_string(),
        branch.trim_end_matches(']').to_string(),
    )
}

/// The branch checked out in the worktree at `path`.
fn branch_of(path: &Path) -> String {
    assert!(
        path.is_dir(),
        "cwt named {} , which is not a directory",
        path.display()
    );
    let output = git(path, &["rev-parse", "--abbrev-ref", "HEAD"]);
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Every name `cwt` prints must select the worktree it was printed for.
///
/// That round trip is the whole point of naming a repository in a message: the
/// user reads a name out of the list and types it back. Each label is fed back
/// to `cwt` from the anchor, and the branch of the worktree it lands on has to
/// be the branch the label promised.
fn assert_labels_round_trip(nest: &Nest, labels: &[String]) {
    assert!(!labels.is_empty(), "there were no labels to check");
    for label in labels {
        let (target, branch) = label_parts(label);
        let output = cwt(&nest.anchor(), &[target.as_str()]);
        assert_eq!(
            code(&output),
            0,
            "cwt printed the name '{target}' and then refused it: {}",
            combined(&output)
        );
        assert_eq!(
            branch_of(Path::new(&target_path(&output))),
            branch,
            "the name '{target}' must select the worktree it was printed for"
        );
    }
}

#[test]
fn a_child_repository_named_after_its_parent_keeps_its_own_heading() {
    let nest = Nest::build();
    let output = cwt(&nest.anchor(), &[]);
    assert_eq!(code(&output), 0, "cwt failed: {}", combined(&output));

    let listed = parse_listing(&stdout(&output));
    assert_eq!(
        headings(&listed),
        vec!["nest", "nest/nest", "other"],
        "the child inside the anchor is a repository of its own, and the name it \
         shares with the anchor is qualified so each heading names one \
         repository: {}",
        stdout(&output)
    );

    let holding = |relative: &str| -> Vec<String> {
        let wanted = nest.path_of(relative);
        listed
            .iter()
            .filter(|line| line.path == wanted)
            .map(|line| line.repo.clone())
            .collect()
    };
    assert_eq!(
        holding("nest/nest-worktrees/only"),
        vec!["nest/nest".to_string()],
        "a worktree belongs under the repository that owns it"
    );
    assert_eq!(
        holding("nest-worktrees/spare"),
        vec!["nest".to_string()],
        "the anchor keeps its own worktrees"
    );
}

#[test]
fn each_repository_that_shares_a_name_has_a_name_of_its_own() {
    let nest = Nest::build();

    let inner = cwt(&nest.anchor(), &["nest/nest:"]);
    assert_eq!(
        code(&inner),
        0,
        "the repository inside the anchor must be reachable by name: {}",
        combined(&inner)
    );
    assert_eq!(target_path(&inner), nest.path_of("nest/nest"));

    let anchor = cwt(&nest.anchor(), &["nest:"]);
    assert_eq!(
        code(&anchor),
        0,
        "the anchor must still answer to its own name: {}",
        combined(&anchor)
    );
    assert_eq!(
        target_path(&anchor),
        nest.path_of("nest"),
        "the bare directory name belongs to the anchor"
    );

    let only = cwt(&nest.anchor(), &["nest/nest:only"]);
    assert_eq!(
        code(&only),
        0,
        "a worktree only the inner repository has must be reachable through it: {}",
        combined(&only)
    );
    assert_eq!(target_path(&only), nest.path_of("nest/nest-worktrees/only"));
}

#[test]
fn a_repository_keeps_its_name_wherever_the_user_stands() {
    // The family is the same from anywhere inside it, so the name that reaches
    // a repository cannot depend on which one the user is standing in.
    let nest = Nest::build();
    let inside = nest.at("nest/nest");

    let listing = cwt(&inside, &[]);
    let listed = parse_listing(&stdout(&listing));
    assert_eq!(headings(&listed), vec!["nest", "nest/nest", "other"]);

    let anchor = cwt(&inside, &["nest:"]);
    assert_eq!(code(&anchor), 0, "cwt nest: failed: {}", combined(&anchor));
    assert_eq!(target_path(&anchor), nest.path_of("nest"));

    let child = cwt(&inside, &["nest/nest:"]);
    assert_eq!(
        code(&child),
        0,
        "cwt nest/nest: failed: {}",
        combined(&child)
    );
    assert_eq!(target_path(&child), nest.path_of("nest/nest"));
}

#[test]
fn every_available_worktree_can_be_typed_back() {
    let nest = Nest::build();
    let missing = cwt(&nest.anchor(), &["no-worktree-answers-to-this"]);
    assert_eq!(
        code(&missing),
        WORKTREE_NOT_FOUND,
        "no worktree has that name: {}",
        combined(&missing)
    );

    let labels = listed_labels(&combined(&missing));
    assert_eq!(
        labels.len(),
        7,
        "the list must name every worktree of the family: {labels:#?}"
    );
    assert_labels_round_trip(&nest, &labels);
}

#[test]
fn every_candidate_of_an_ambiguous_name_can_be_typed_back() {
    let nest = Nest::build();
    let output = cwt(&nest.anchor(), &["shared"]);
    assert_eq!(
        code(&output),
        MULTIPLE_MATCHES,
        "two repositories hold a worktree named shared: {}",
        combined(&output)
    );

    let labels = listed_labels(&combined(&output));
    assert_eq!(
        labels.len(),
        2,
        "one candidate per repository that holds a shared worktree: {labels:#?}"
    );
    assert_labels_round_trip(&nest, &labels);
}

//! `swt list` end to end: which worktrees count as the children of a branch,
//! and what a caller reads on each stream.
//!
//! Every case drives the real binary against a fixture repository. Each child
//! comes from the real `swt create`, because the branch of a child is the
//! contract between the two commands. A test that made child branches by hand
//! would pin only its own copy of that format.
//!
//! Unix only: the fixtures are `sh` scripts dropped as executable `.swt-check`
//! overrides, which is precisely how the escape hatch is documented.
#![cfg(unix)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use support::{exiting_check, git, run_swt, unique, write_swt_check, TestRepo, SWT_CHECK};

/// The namespace of the local branches. A test removes it from the full ref,
/// so it compares the branch as a person spells it.
const LOCAL_BRANCH_NAMESPACE: &str = "refs/heads/";

/// The namespace of every branch that `swt create` makes.
const SWT_BRANCH_NAMESPACE: &str = "swt";

/// The branch of the parent in the tests that need only one parent. It has no
/// `/`, so no test here depends on how a nested branch is read.
const PARENT_BRANCH: &str = "issue-42";

/// The branch of a parent whose name is a prefix of [`NESTED_PARENT_BRANCH`].
const FLAT_PARENT_BRANCH: &str = "feat";

/// A branch that has [`FLAT_PARENT_BRANCH`] and a `/` as its prefix. Git cannot
/// hold this branch and [`FLAT_PARENT_BRANCH`] at the same time, so a fixture
/// deletes this one before it makes the other.
const NESTED_PARENT_BRANCH: &str = "feat/foo";

/// What the note for a branch with no children must say. The note must also
/// name the branch.
const NO_CHILDREN_PHRASE: &str = "No child worktrees";

/// A worktree that the real `swt create` made, as a test reads it back.
struct Child {
    /// The path that `swt create` printed on stdout.
    path: PathBuf,
    /// The branch that the child has checked out, without `refs/heads/`.
    branch: String,
}

/// Runs the real `swt create` in the worktree at `parent` with a check that
/// passes, and returns the child that it made.
///
/// The name comes from [`unique`], so a concurrent run of the same test cannot
/// meet this child. Panics when `swt create` fails, because every test here
/// needs the child to exist.
fn create_child(parent: &Path, label: &str) -> Child {
    // `swt` reads the override from the root of the worktree where it runs.
    write_swt_check(parent, &exiting_check(0));
    let output = run_swt(parent, &["create", &unique(label)]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "fixture precondition: swt create in {} must succeed: {}",
        parent.display(),
        support::stderr(&output)
    );
    let stdout = support::stdout(&output);
    let printed = stdout
        .strip_suffix('\n')
        .filter(|line| !line.contains('\n'))
        .unwrap_or_else(|| panic!("swt create must print exactly one path, got {stdout:?}"));
    let path = PathBuf::from(printed);
    let branch = checked_out_branch(&path);
    Child { path, branch }
}

/// The branch that the worktree at `path` has checked out.
///
/// It reads the full ref and removes `refs/heads/`. The short name is not safe:
/// when a tag has the same name, git spells it `heads/<branch>`.
fn checked_out_branch(path: &Path) -> String {
    let full_ref = git(path, &["symbolic-ref", "--quiet", "HEAD"]);
    full_ref
        .strip_prefix(LOCAL_BRANCH_NAMESPACE)
        .unwrap_or_else(|| {
            panic!(
                "the worktree at {} must have a local branch checked out, got {full_ref:?}",
                path.display()
            )
        })
        .to_string()
}

/// Asserts that the registry of `repo` holds a worktree at each of `paths`.
///
/// A mutation guard for the fixtures. A test that expects `swt list` to leave a
/// worktree out proves first that git knows the worktree, so an empty filter
/// cannot pass the test. `why` says what the fixture needs the worktrees for.
fn assert_registered(repo: &TestRepo, paths: &[&Path], why: &str) {
    let registry = repo.git(&["worktree", "list", "--porcelain"]);
    for path in paths {
        assert!(
            registry.contains(&format!("worktree {}\n", path.display())),
            "fixture precondition: the registry must hold {} ({why}): {registry}",
            path.display()
        );
    }
}

/// `path` as the one argument that a git command takes. Every fixture path is
/// UTF-8, because every name in it comes from [`unique`].
fn path_arg(path: &Path) -> &str {
    path.to_str().expect("utf-8 fixture path")
}

/// The line that `swt list` must print for `child`: the path, a tab, and the
/// branch.
fn line(child: &Child) -> String {
    format!("{}\t{}\n", child.path.display(), child.branch)
}

/// What `swt list` must print for `children`: the line of each child, in the
/// order of their paths.
///
/// The order comes from the paths and not from the order of the arguments, so
/// the expectation does not depend on the order of the registry of git.
fn listing(children: &[&Child]) -> String {
    let mut sorted = children.to_vec();
    sorted.sort_by(|left, right| left.path.cmp(&right.path));
    sorted.into_iter().map(line).collect()
}

// Issue #500. A parent sees exactly its own children, one line each. The path
// comes first and is the path that `swt create` printed, so a caller can give
// it directly to `swt merge`. Two worktrees that are not children of the parent
// share the registry: a child of `main`, and a worktree on a branch in the old
// format. The filter thus has something to leave out.
#[test]
fn a_parent_lists_exactly_its_children_as_a_path_a_tab_and_a_branch() {
    let repo = TestRepo::new();
    let parent = repo.add_worktree_on("parent", PARENT_BRANCH);
    let first = create_child(&parent.path, "first");
    let second = create_child(&parent.path, "second");
    let child_of_main = create_child(repo.path(), "elsewhere");
    let old_format = repo.add_worktree("bystander");
    assert_registered(
        &repo,
        &[&child_of_main.path, &old_format.path],
        "a worktree that is not a child of the parent",
    );

    let output = run_swt(&parent.path, &["list"]);
    let stderr = support::stderr(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "swt list in a parent with children must succeed: {stderr}"
    );
    assert_eq!(
        support::stdout(&output),
        listing(&[&first, &second]),
        "stdout must hold exactly the children of {PARENT_BRANCH}, one line each, \
         in the order of their paths: {stderr}"
    );
}

// Issue #500. A prefix is not sufficient: the children of `feat/foo` are not
// children of `feat`. Git refuses `feat` and `feat/foo` in one repository at
// the same time, so the fixture makes them one after the other. The child of
// `feat/foo` stays after its parent branch is gone, which is also the real
// case.
#[test]
fn a_child_of_a_longer_branch_is_not_a_child_of_its_prefix() {
    let repo = TestRepo::new();
    let nested_parent = repo.add_worktree_on("parent-nested", NESTED_PARENT_BRANCH);
    let nested_child = create_child(&nested_parent.path, "nested");
    // The override is untracked, so it goes first. The removal of the parent
    // then needs no force, and the safe delete proves that the branch held no
    // work.
    fs::remove_file(nested_parent.path.join(SWT_CHECK))
        .expect("the override of the nested parent should be removable");
    repo.git(&["worktree", "remove", path_arg(&nested_parent.path)]);
    repo.git(&["branch", "--delete", NESTED_PARENT_BRANCH]);
    let flat_parent = repo.add_worktree_on("parent-flat", FLAT_PARENT_BRANCH);
    let flat_child = create_child(&flat_parent.path, "flat");
    // Mutation guard. It proves that the fixture sets the trap: the child of
    // `feat/foo` is still a registered worktree, and its branch starts with the
    // text that a child of `feat` starts with.
    let nested_prefix = format!("{SWT_BRANCH_NAMESPACE}/{NESTED_PARENT_BRANCH}/");
    assert!(
        nested_child.branch.starts_with(&nested_prefix),
        "fixture precondition: the child of {NESTED_PARENT_BRANCH} must be on a branch under \
         {nested_prefix}, got {}",
        nested_child.branch
    );
    assert_registered(
        &repo,
        &[&nested_child.path],
        "the child of a parent branch that is gone",
    );

    let output = run_swt(&flat_parent.path, &["list"]);
    let stderr = support::stderr(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "swt list in {FLAT_PARENT_BRANCH} must succeed: {stderr}"
    );
    assert_eq!(
        support::stdout(&output),
        listing(&[&flat_child]),
        "{FLAT_PARENT_BRANCH} must show only its own child, and not the child of \
         {NESTED_PARENT_BRANCH}: {stderr}"
    );
}

// Issue #500. A branch with no children prints nothing on stdout, so
// `[ -n "$(swt list)" ]` is a complete check. A note on stderr tells a person
// why the output is empty. The status is 0, because no children is an answer
// and not a failure. The registry holds a child of `main` and a worktree in the
// old format, so the empty answer does not come from an empty registry.
#[test]
fn a_parent_with_no_children_prints_nothing_and_a_note_that_names_it() {
    let repo = TestRepo::new();
    let parent = repo.add_worktree_on("parent", PARENT_BRANCH);
    let child_of_main = create_child(repo.path(), "elsewhere");
    let old_format = repo.add_worktree("bystander");
    assert_registered(
        &repo,
        &[&child_of_main.path, &old_format.path],
        "a worktree that is not a child of the parent",
    );

    let output = run_swt(&parent.path, &["list"]);
    let stderr = support::stderr(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "no children is an answer, not a failure: {stderr}"
    );
    assert_eq!(
        support::stdout(&output),
        "",
        "a branch with no children prints nothing on stdout"
    );
    let note = stderr
        .strip_suffix('\n')
        .filter(|line| !line.contains('\n'))
        .unwrap_or_else(|| panic!("stderr must hold exactly one line, the note, got {stderr:?}"));
    assert!(
        note.contains(NO_CHILDREN_PHRASE) && note.contains(PARENT_BRANCH),
        "the note must say {NO_CHILDREN_PHRASE:?} and name {PARENT_BRANCH}, got {note:?}"
    );
}

// `list` starts from the branch that is checked out where it runs. Outside a
// repository there is no branch, and the user must see the explanation of git.
#[test]
fn list_outside_a_repository_fails_with_gits_own_complaint() {
    let output = support::run_swt_outside_a_repository(&["list"]);
    let stderr = support::stderr(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a list with no repository to read must fail: {stderr}"
    );
    assert_eq!(
        support::stdout(&output),
        "",
        "a failed list prints no line for a caller to read"
    );
    assert!(
        stderr.contains("not a git repository"),
        "the explanation of git must reach the user: {stderr}"
    );
}

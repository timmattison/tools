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

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use support::{
    exiting_check, git, git_allowing_failure, run_swt, shell_command, unique, write_swt_check,
    TestRepo, SWT_CHECK,
};
use tempfile::TempDir;

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

/// The word that git gives a worktree whose directory is gone. `swt list` adds
/// the same word as a third field to the line of such a child.
const PRUNABLE_FIELD: &str = "prunable";

/// What the refusal on a detached HEAD must say: the fact, and what the user
/// must do about it.
const DETACHED_REFUSAL_PHRASES: [&str; 2] = ["HEAD is detached", "Check out a branch"];

/// Separates the fields of a line that `swt list` prints.
const FIELD_SEPARATOR: char = '\t';

/// The plain-git command that prints the child branches of the branch that is
/// checked out where it runs. `README.md` quotes it byte for byte, and the
/// dotfiles document `SWT.md` quotes the same bytes.
///
/// Two choices make it correct. `git branch --show-current` prints the branch
/// even when a tag has the same name. In `git for-each-ref`, a `*` does not
/// match a `/`, so the pattern leaves out the children of a longer branch.
const PLAIN_GIT_CHILD_BRANCHES: &str = r#"git for-each-ref --format='%(refname:short)' "refs/heads/swt/$(git branch --show-current)/*""#;

/// The path of `README.md` at the root of the repository, from the manifest
/// directory of this crate.
const README_FROM_MANIFEST: &str = "../../README.md";

/// The text that every statement of the check of `swt list` holds: the command
/// substitution that reads what `swt list` prints.
const SWT_LIST_SUBSTITUTION: &str = "$(swt list)";

/// The source of the module `list`. Its module doc states the check of
/// `README.md` a second time.
const LIST_MODULE_SOURCE: &str = include_str!("../src/list.rs");

/// The status of the documented check when the branch has children.
const CHECK_STATUS_CHILDREN: i32 = 0;

/// The status of the documented check when the branch has no children.
const CHECK_STATUS_NO_CHILDREN: i32 = 1;

/// What a caller reads from the status of the documented check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    /// The branch has children.
    Children,
    /// The branch has no children.
    NoChildren,
    /// `swt list` failed, so the check knows nothing about the children.
    Failure,
}

impl Answer {
    /// The answer that `status` gives. Every status other than the two answers
    /// is a failure, a kill by a signal included.
    fn of(status: Option<i32>) -> Self {
        match status {
            Some(CHECK_STATUS_CHILDREN) => Self::Children,
            Some(CHECK_STATUS_NO_CHILDREN) => Self::NoChildren,
            _ => Self::Failure,
        }
    }
}

/// The text of `README.md` at the root of the repository.
fn readme() -> String {
    let readme_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(README_FROM_MANIFEST);
    fs::read_to_string(&readme_path)
        .unwrap_or_else(|err| panic!("{} must be readable: {err}", readme_path.display()))
}

/// Every inline code span of `text` that holds [`SWT_LIST_SUBSTITUTION`]. A
/// span is the text between a pair of backticks on one line.
fn swt_list_spans(text: &str) -> Vec<&str> {
    text.lines()
        .flat_map(|line| line.split('`').skip(1).step_by(2))
        .filter(|span| span.contains(SWT_LIST_SUBSTITUTION))
        .collect()
}

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

/// The line that `swt list` must print for `child` when git marks it prunable:
/// the path, a tab, the branch, a tab, and [`PRUNABLE_FIELD`].
fn prunable_line(child: &Child) -> String {
    format!(
        "{}\t{}\t{PRUNABLE_FIELD}\n",
        child.path.display(),
        child.branch
    )
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

/// Makes a child of [`NESTED_PARENT_BRANCH`] with the real `swt create`, then
/// removes the parent worktree and deletes that branch. Returns the child.
///
/// Git refuses [`FLAT_PARENT_BRANCH`] and [`NESTED_PARENT_BRANCH`] in one
/// repository at the same time. A test that needs a child of each calls this
/// function first, and then makes [`FLAT_PARENT_BRANCH`]. The child stays after
/// its parent branch is gone, which is also the real case.
///
/// Panics when the fixture does not set its trap: the child must stay a
/// registered worktree, on a branch whose text starts as the branch of a child
/// of [`FLAT_PARENT_BRANCH`] starts.
fn child_of_a_deleted_nested_parent(repo: &TestRepo) -> Child {
    let nested_parent = repo.add_worktree_on("parent-nested", NESTED_PARENT_BRANCH);
    let nested_child = create_child(&nested_parent.path, "nested");
    // The override is untracked, so it goes first. The removal of the parent
    // then needs no force, and the safe delete proves that the branch held no
    // work.
    fs::remove_file(nested_parent.path.join(SWT_CHECK))
        .expect("the override of the nested parent should be removable");
    repo.git(&["worktree", "remove", path_arg(&nested_parent.path)]);
    repo.git(&["branch", "--delete", NESTED_PARENT_BRANCH]);
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
        repo,
        &[&nested_child.path],
        "the child of a parent branch that is gone",
    );
    nested_child
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
// children of `feat`. The child of `feat/foo` comes first, because git refuses
// the two branches in one repository at the same time.
#[test]
fn a_child_of_a_longer_branch_is_not_a_child_of_its_prefix() {
    let repo = TestRepo::new();
    child_of_a_deleted_nested_parent(&repo);
    let flat_parent = repo.add_worktree_on("parent-flat", FLAT_PARENT_BRANCH);
    let flat_child = create_child(&flat_parent.path, "flat");

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

// Issue #500. A branch with no children prints nothing on stdout. A note on
// stderr tells a person why the output is empty. The status is 0, because no
// children is an answer and not a failure. The registry holds a child of `main`
// and a worktree in the old format, so the empty answer does not come from an
// empty registry.
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

// Issue #500. A child whose directory was deleted by hand stays in the registry
// of git, and git marks it prunable. `swt list` must still show it, with a
// third field that tells the user to prune it. The other child keeps two
// fields, so the third field marks one child and not the whole listing.
#[test]
fn a_child_whose_directory_is_gone_is_listed_as_prunable() {
    let repo = TestRepo::new();
    let parent = repo.add_worktree_on("parent", PARENT_BRANCH);
    let gone = create_child(&parent.path, "gone");
    let kept = create_child(&parent.path, "kept");
    fs::remove_dir_all(&gone.path).expect("the directory of a child should be removable");
    // Mutation guard. It proves that git still registers the child whose
    // directory is gone, and that git marks it prunable.
    let registry = repo.git(&["worktree", "list", "--porcelain"]);
    let record = registry
        .split("\n\n")
        .find(|record| record.starts_with(&format!("worktree {}\n", gone.path.display())))
        .unwrap_or_else(|| {
            panic!(
                "fixture precondition: the registry must still hold {}: {registry}",
                gone.path.display()
            )
        });
    assert!(
        record
            .lines()
            .any(|field| field.split_whitespace().next() == Some(PRUNABLE_FIELD)),
        "fixture precondition: git must mark {} {PRUNABLE_FIELD}: {record}",
        gone.path.display()
    );

    let output = run_swt(&parent.path, &["list"]);
    let stderr = support::stderr(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "swt list must succeed when a child directory is gone: {stderr}"
    );
    let mut expected = [
        (gone.path.clone(), prunable_line(&gone)),
        (kept.path.clone(), line(&kept)),
    ];
    expected.sort();
    let expected: String = expected.into_iter().map(|(_, text)| text).collect();
    assert_eq!(
        support::stdout(&output),
        expected,
        "the child whose directory is gone must carry the field {PRUNABLE_FIELD:?}, and only \
         that child: {stderr}"
    );
}

// Issue #500. A detached HEAD has no branch, so `swt list` has no branch whose
// children it can show. It must fail and say why, and it must print nothing
// that a caller can take for a child. The parent made a child before it
// detached, so the empty stdout does not come from an empty registry.
#[test]
fn list_on_a_detached_head_fails_and_says_why() {
    let repo = TestRepo::new();
    let parent = repo.add_worktree_on("parent", PARENT_BRANCH);
    let child = create_child(&parent.path, "before-detach");
    git(&parent.path, &["switch", "--quiet", "--detach"]);
    let (on_a_branch, _) = git_allowing_failure(&parent.path, &["symbolic-ref", "--quiet", "HEAD"]);
    assert!(
        !on_a_branch,
        "fixture precondition: HEAD of the parent must be detached"
    );
    assert_registered(
        &repo,
        &[&child.path],
        "a child of the branch that the parent held before it detached",
    );

    let output = run_swt(&parent.path, &["list"]);
    let stderr = support::stderr(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "swt list on a detached HEAD must fail: {stderr}"
    );
    for phrase in DETACHED_REFUSAL_PHRASES {
        assert!(
            stderr.contains(phrase),
            "the refusal must say {phrase:?}, got {stderr:?}"
        );
    }
    assert_eq!(
        support::stdout(&output),
        "",
        "a refused list prints no line for a caller to read"
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

// Issue #500. A hook or a skill finds the child branches of a branch with plain
// git, without `swt`. `README.md` quotes the command, and this test keeps the
// quote true. The fixture sets a trap for each spelling that the README warns
// against: a child of `feat/foo` beside the child of `feat`, and a tag `feat`
// beside the branch `feat`.
#[test]
fn the_documented_plain_git_command_finds_the_branches_that_swt_list_shows() {
    let repo = TestRepo::new();
    let nested_child = child_of_a_deleted_nested_parent(&repo);
    let flat_parent = repo.add_worktree_on("parent-flat", FLAT_PARENT_BRANCH);
    // A lightweight tag with the name of the branch. It comes before the child,
    // so `swt create` also reads the parent branch past the tag.
    git(&flat_parent.path, &["tag", FLAT_PARENT_BRANCH]);
    let flat_child = create_child(&flat_parent.path, "flat");
    // Mutation guard. It proves that the tag makes the short name of the branch
    // ambiguous, so a command that builds its pattern from that name finds
    // nothing.
    assert_eq!(
        git(&flat_parent.path, &["symbolic-ref", "--short", "HEAD"]),
        format!("heads/{FLAT_PARENT_BRANCH}"),
        "fixture precondition: the tag {FLAT_PARENT_BRANCH} must make the short name of the \
         branch ambiguous"
    );

    let plain = shell_command(&flat_parent.path, PLAIN_GIT_CHILD_BRANCHES)
        .output()
        .expect("failed to run the plain-git command through the shell");
    let listed = run_swt(&flat_parent.path, &["list"]);

    assert!(
        plain.status.success(),
        "the plain-git command must succeed: {}",
        support::stderr(&plain)
    );
    assert_eq!(
        listed.status.code(),
        Some(0),
        "swt list in {FLAT_PARENT_BRANCH} must succeed: {}",
        support::stderr(&listed)
    );
    let found: BTreeSet<String> = support::stdout(&plain)
        .lines()
        .map(str::to_string)
        .collect();
    let shown: BTreeSet<String> = support::stdout(&listed)
        .lines()
        .map(|line| {
            line.split(FIELD_SEPARATOR)
                .nth(1)
                .unwrap_or_else(|| panic!("each line of swt list must hold a branch, got {line:?}"))
                .to_string()
        })
        .collect();
    assert_eq!(
        found, shown,
        "the plain-git command must find exactly the branches that swt list shows"
    );
    assert_eq!(
        found,
        BTreeSet::from([flat_child.branch]),
        "the plain-git command must find exactly the one child of {FLAT_PARENT_BRANCH}"
    );
    // The README warns against `git branch --list`. There a `*` also matches a
    // `/`, so the same pattern also finds the child of `feat/foo`.
    let by_branch_list = repo.branches(&format!("{SWT_BRANCH_NAMESPACE}/{FLAT_PARENT_BRANCH}/*"));
    assert!(
        by_branch_list.contains(&nested_child.branch),
        "the warning in the README must stay true: git branch --list must also find {}, got \
         {by_branch_list:?}",
        nested_child.branch
    );

    assert!(
        readme().contains(PLAIN_GIT_CHILD_BRANCHES),
        "README.md must quote the plain-git command byte for byte: {PLAIN_GIT_CHILD_BRANCHES}"
    );
}

// Issue #500. A hook runs the check that `README.md` states, and its answer
// decides whether a merge or a deletion goes ahead. A failed `swt list` also
// leaves stdout empty, so a check that reads only stdout reports a failure as
// no children. The test runs the check from the README itself, not a copy. The
// module doc of `list` must state the same bytes, and the dotfiles document
// `SWT.md` quotes them too. Children, no children and a failure must each give
// a different status.
#[test]
fn the_documented_check_tells_children_no_children_and_a_failure_apart() {
    let readme = readme();
    let spans = swt_list_spans(&readme);
    assert_eq!(
        spans.len(),
        1,
        "README.md must state exactly one check of swt list, got {spans:?}"
    );
    let check = spans[0];
    assert!(
        LIST_MODULE_SOURCE.contains(check),
        "the module doc of src/swt/src/list.rs must state the check of README.md byte for \
         byte: {check}"
    );

    let repo = TestRepo::new();
    let with_child = repo.add_worktree_on("parent-with-child", "with-child");
    create_child(&with_child.path, "child");
    let without_children = repo.add_worktree_on("parent-without-children", "without-children");
    let detached = repo.add_worktree_on("parent-to-detach", "to-detach");
    let detached_child = create_child(&detached.path, "before-detach");
    git(&detached.path, &["switch", "--quiet", "--detach"]);
    let (on_a_branch, _) =
        git_allowing_failure(&detached.path, &["symbolic-ref", "--quiet", "HEAD"]);
    assert!(
        !on_a_branch,
        "fixture precondition: HEAD of the detached parent must be detached"
    );
    assert_registered(
        &repo,
        &[&detached_child.path],
        "a child of the branch that the parent held before it detached",
    );

    // The directory of the binary under test comes first on `PATH`, so an
    // installed `swt` cannot answer in its place. The empty directory holds no
    // `swt` at all. `[` and `exit` are builtins, so the check still runs there.
    let binary_dir = Path::new(env!("CARGO_BIN_EXE_swt"))
        .parent()
        .expect("the binary under test must sit in a directory");
    let inherited = std::env::var_os("PATH").expect("the test must run with PATH set");
    let with_swt = std::env::join_paths(
        std::iter::once(binary_dir.to_path_buf()).chain(std::env::split_paths(&inherited)),
    )
    .expect("the directory of the binary under test must fit in PATH");
    let empty = TempDir::new().expect("an empty directory for a PATH with no swt");
    let cases = [
        (
            "a parent with a child",
            &with_child.path,
            with_swt.as_os_str(),
            Answer::Children,
        ),
        (
            "a parent with no children",
            &without_children.path,
            with_swt.as_os_str(),
            Answer::NoChildren,
        ),
        (
            "a detached HEAD",
            &detached.path,
            with_swt.as_os_str(),
            Answer::Failure,
        ),
        (
            "a parent with a child and no swt on PATH",
            &with_child.path,
            empty.path().as_os_str(),
            Answer::Failure,
        ),
    ];

    let runs: Vec<(&str, Option<i32>, String)> = cases
        .iter()
        .map(|&(state, dir, path, _)| {
            let output = shell_command(dir, check)
                .env("PATH", path)
                .output()
                .expect("failed to run the documented check through the shell");
            (state, output.status.code(), support::stderr(&output))
        })
        .collect();
    let observed: Vec<(&str, Answer)> = runs
        .iter()
        .map(|&(state, status, _)| (state, Answer::of(status)))
        .collect();
    let expected: Vec<(&str, Answer)> = cases
        .iter()
        .map(|&(state, _, _, answer)| (state, answer))
        .collect();
    assert_eq!(
        observed, expected,
        "the check {check} must give 0 for children, 1 for no children, and another status \
         for a failure: {runs:#?}"
    );
}

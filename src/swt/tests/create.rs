//! `swt create` end to end: the green gate, where the check runs, where its
//! configuration comes from, and what is left behind when it says no.
//!
//! Every case here drives the real binary, because every guarantee `create`
//! makes is about the world outside the process — a directory that exists or
//! does not, a branch that survives or does not, a path on stdout that a caller
//! captures. The in-process tests can pin the plan; only a subprocess can pin
//! that the worktree is gone.
//!
//! Unix only: the fixtures are `sh` scripts dropped as executable `.swt-check`
//! overrides, which is precisely how the escape hatch is documented.
#![cfg(unix)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use support::{
    entry_names, exiting_check, git, recorded_calls, recording_function, run_swt, shell_command,
    swt_command, unique, write_swt_check, TestRepo, MAIN_BRANCH, SWT_CHECK, TRACKED_FILE,
    WORKTREE_SUFFIX,
};

/// A check that records the directory it ran in. `pwd -P` asks the kernel rather
/// than trusting an inherited `PWD`, so the answer is the cwd `swt` chose.
const RECORD_CWD_CHECK: &str = "#!/bin/sh\npwd -P > ran-in\n";

/// The file [`RECORD_CWD_CHECK`] writes, in whatever directory it ran in.
const CWD_MARKER: &str = "ran-in";

/// A check that passes only against an uncommitted edit in the parent worktree.
/// In a clean checkout of HEAD the tracked file still reads `original`.
const NEEDS_PARENT_EDIT_CHECK: &str = "#!/bin/sh\ngrep -q MODIFIED tracked.txt\n";

/// The uncommitted content [`NEEDS_PARENT_EDIT_CHECK`] looks for.
const PARENT_EDIT: &str = "MODIFIED\n";

/// A check that deletes the worktree's own `.git` link and then fails, so the
/// teardown that follows cannot succeed either: git refuses to remove a working
/// tree whose `.git` has vanished, and refuses to delete a branch a registered
/// worktree still claims. No permission games, so it behaves the same for an
/// unprivileged user and for root.
const SABOTAGE_CHECK: &str = "#!/bin/sh\nrm -f .git\nexit 1\n";

/// Parent branches that git accepts and that a shell does not read as one word.
/// A bare `it's` opens a quote that never closes. A bare `fix;id` ends one
/// command and starts a second. A bare `$(...)` runs a command that writes a
/// file.
const SHELL_HOSTILE_PARENT_BRANCHES: [&str; 3] = ["it's", "fix;id", "a$(touch${IFS}pwned)"];

/// The text in stderr immediately before the recovery line that a failed
/// teardown prints.
const RECOVERY_LINE_LABEL: &str = "Remove it by hand:\n  ";

/// Decodes a finished run's stdout.
fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Decodes a finished run's stderr.
fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A file a subagent commits, so a merge has work to bring back.
const SUBAGENT_FILE: &str = "subagent-only.txt";

/// How many concurrent runs the collision case fans out. Two is enough to make a
/// shared name a shared resource, which is the whole of the bug.
const CONCURRENT_RUNS: usize = 2;

/// The branch of one parent in the naming test. A lightweight tag of the same
/// name shadows it, so git spells its short name `heads/issue-42`. Only a read
/// of the full ref gives the branch back as it is.
const TAGGED_PARENT_BRANCH: &str = "issue-42";

/// The branch of the other parent in the naming test. It contains a `/`, and
/// the branch of a child must keep it as it is.
const NESTED_PARENT_BRANCH: &str = "feat/foo";

/// What the refusal of a parent on a detached HEAD must say: the fact, and what
/// the user must do about it.
const DETACHED_REFUSAL_PHRASES: [&str; 2] = ["HEAD is detached", "Check out a branch"];

/// The last component of `path` as text: the name of a worktree directory.
fn dir_name(path: &Path) -> String {
    path.file_name()
        .expect("a worktree path names a directory")
        .to_string_lossy()
        .into_owned()
}

/// The uniqueness token in the directory that `swt create <name>` built for
/// the parent worktree at `parent`.
///
/// Panics when the file name is not `<parent name>--<name>-<token>.swt`. A
/// name with no token is issue #284, and a name with no parent is issue #500.
/// The panic message shows the file name that the run built.
fn token_of_worktree(path: &Path, parent: &Path, name: &str) -> String {
    let file_name = dir_name(path);
    file_name
        .strip_prefix(&format!("{}--{name}-", dir_name(parent)))
        .and_then(|rest| rest.strip_suffix(WORKTREE_SUFFIX))
        .unwrap_or_else(|| {
            panic!(
                "the worktree directory must be <parent>--<name>-<token>{WORKTREE_SUFFIX}, \
                 got {file_name:?}"
            )
        })
        .to_string()
}

/// The branch that `swt create <name>` makes in a parent on `parent_branch`,
/// for the run whose token is `token`.
///
/// The one statement of the branch format in this file, so that each test
/// compares against the same spelling.
fn child_branch(parent_branch: &str, name: &str, token: &str) -> String {
    format!("swt/{parent_branch}/{name}-{token}")
}

/// Reads a uniqueness token out of `text`: whatever runs between the first
/// occurrence of `label` and the `terminator` after it.
///
/// The token is minted inside the child process, so a test can only ever read it
/// back out of what the run said — which is also the only way to check that two
/// *different* messages named the same one.
fn token_after(text: &str, label: &str, terminator: &str) -> String {
    let (_, rest) = text
        .split_once(label)
        .unwrap_or_else(|| panic!("no {label:?} in: {text}"));
    let (token, _) = rest
        .split_once(terminator)
        .unwrap_or_else(|| panic!("no {terminator:?} after {label:?} in: {text}"));
    token.to_string()
}

/// Starts a `swt create <name>` without waiting for it, with both streams
/// captured. Spawning is separated from waiting so the collision case can have
/// two runs genuinely overlap — waited on in turn, the second would simply find
/// the first's finished work and the race would never be run.
fn spawn_create(repo: &TestRepo, name: &str) -> Child {
    swt_command(repo.path())
        .args(["create", name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn swt create")
}

/// Sorted names of everything sitting beside the repository, so an orphaned
/// worktree cannot hide by being merely un-asserted-about.
fn beside_the_repo(repo: &TestRepo) -> Vec<String> {
    entry_names(repo.siblings())
}

// The whole point of the command: a worktree branched from a verified HEAD, and
// its path on stdout with nothing else beside it — callers capture stdout, so
// anything chatty there is a bug, not noise.
#[test]
fn a_green_check_yields_a_worktree_a_branch_and_only_the_path_on_stdout() {
    let repo = TestRepo::new();
    write_swt_check(repo.path(), &exiting_check(0));
    let name = unique("green");

    let output = run_swt(repo.path(), &["create", &name]);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "a green check must produce a worktree: {stderr}"
    );
    let created = repo.sole_created_worktree(&name);
    assert_eq!(
        stdout_of(&output),
        format!("{}\n", created.display()),
        "stdout carries the path and nothing else, so a caller can capture it"
    );
    assert!(
        created.is_dir(),
        "the verified worktree must still be there at {}",
        created.display()
    );
    let branches = repo.created_branches(&name);
    assert_eq!(
        branches.len(),
        1,
        "exactly one branch should have been created, got {branches:?}"
    );
    // The token is what makes a second run of the same name possible at all, and
    // it has to be the *same* token in both names — a directory and a branch
    // keyed differently would be two worktrees wearing one name.
    assert_eq!(
        branches[0],
        child_branch(
            MAIN_BRANCH,
            &name,
            &token_of_worktree(&created, repo.path(), &name)
        ),
        "the worktree directory and its branch must be keyed on one token"
    );
}

// The bug issue #284 names. `swt` exists to make *parallel* TDD safe, and the one
// resource two concurrent `swt create <same-name>` calls shared was the worktree
// directory — the only name that was not keyed for uniqueness. In parallel the
// second `git worktree add` either fails outright or, worse, two agents believe
// they own one directory. No merge lock applies to `create`, so this really is
// two runs at once.
#[test]
fn concurrent_creates_of_one_name_each_get_their_own_worktree_and_branch() {
    let repo = TestRepo::new();
    write_swt_check(repo.path(), &exiting_check(0));
    let name = unique("concurrent");

    // Every run started before any of them is waited on: that overlap is the
    // test.
    let running: Vec<Child> = (0..CONCURRENT_RUNS)
        .map(|_| spawn_create(&repo, &name))
        .collect();
    let finished: Vec<Output> = running
        .into_iter()
        .map(|run| run.wait_with_output().expect("swt create should finish"))
        .collect();

    let mut paths: Vec<PathBuf> = Vec::new();
    for (index, output) in finished.iter().enumerate() {
        assert_eq!(
            output.status.code(),
            Some(0),
            "concurrent run {index} of {CONCURRENT_RUNS} failed: {}",
            stderr_of(output)
        );
        paths.push(PathBuf::from(stdout_of(output).trim()));
    }
    paths.sort();
    paths.dedup();
    assert_eq!(
        paths.len(),
        CONCURRENT_RUNS,
        "concurrent runs of one name collided on the worktree directory: {paths:?}"
    );
    for path in &paths {
        assert!(
            path.is_dir(),
            "every concurrent run's worktree must survive, {} did not",
            path.display()
        );
    }
    assert_eq!(
        repo.created_worktrees(&name),
        paths,
        "the directories beside the repository must be exactly the ones reported"
    );

    let mut branches = repo.created_branches(&name);
    branches.dedup();
    assert_eq!(
        branches.len(),
        CONCURRENT_RUNS,
        "concurrent runs of one name collided on the branch: {branches:?}"
    );
    // Each run's two names still belong to each other, which is what makes a
    // stray directory attributable to a branch afterwards.
    for path in &paths {
        let branch = child_branch(
            MAIN_BRANCH,
            &name,
            &token_of_worktree(path, repo.path(), &name),
        );
        assert!(
            branches.contains(&branch),
            "no branch {branch} for worktree {}: {branches:?}",
            path.display()
        );
    }
}

// Issue #500. Each name of a child worktree names its parent, so a person or a
// tool can find the children of a parent again. The first parent proves that
// the branch comes from the full ref: a tag of the same name changes its short
// name. The second parent proves that a `/` in the parent branch stays as it is.
#[test]
fn a_child_names_its_parent_in_its_directory_and_in_its_branch() {
    let repo = TestRepo::new();
    let parents = [
        repo.add_worktree_on("parent-a", TAGGED_PARENT_BRANCH),
        repo.add_worktree_on("parent-b", NESTED_PARENT_BRANCH),
    ];
    repo.git(&["tag", TAGGED_PARENT_BRANCH]);
    // Mutation guard for the tag. It proves that the fixture sets the trap: git
    // now spells the short name of the branch with a `heads/` prefix.
    assert_eq!(
        git(&parents[0].path, &["symbolic-ref", "--short", "HEAD"]),
        format!("heads/{TAGGED_PARENT_BRANCH}"),
        "fixture precondition: the tag must make the short name of the branch ambiguous"
    );

    for parent in &parents {
        // `swt` reads the override from the root of the worktree where it runs.
        write_swt_check(&parent.path, &exiting_check(0));
        let name = unique("fix-parser");

        let output = run_swt(&parent.path, &["create", &name]);
        let stderr = stderr_of(&output);

        assert_eq!(
            output.status.code(),
            Some(0),
            "create in the parent on {} must succeed: {stderr}",
            parent.branch
        );
        let stdout = stdout_of(&output);
        let printed = stdout
            .strip_suffix('\n')
            .filter(|line| !line.contains('\n'))
            .unwrap_or_else(|| panic!("stdout must be exactly one line, the path, got {stdout:?}"));
        let created = PathBuf::from(printed);
        assert_eq!(
            created.parent(),
            parent.path.parent(),
            "the child must go in the directory that holds its parent"
        );
        assert_eq!(
            repo.sole_created_worktree(&name),
            created,
            "the printed path must be the one worktree that the run made"
        );
        let token = token_of_worktree(&created, &parent.path, &name);
        let branch = child_branch(&parent.branch, &name, &token);
        assert_eq!(
            repo.created_branches(&name),
            vec![branch.clone()],
            "the one branch of the run must name the parent branch and the token of the directory"
        );
        assert_eq!(
            git(&created, &["symbolic-ref", "--quiet", "HEAD"]),
            format!("refs/heads/{branch}"),
            "the child worktree must have its own branch checked out"
        );
    }
}

// Issue #500. A parent on a detached HEAD has no branch, so a child cannot name
// its parent. `swt create` must refuse it before it makes anything, and say why.
// The check passes, so the refusal cannot come from a red check.
#[test]
fn a_parent_on_a_detached_head_is_refused_before_anything_exists() {
    let repo = TestRepo::new();
    repo.git(&["switch", "--quiet", "--detach"]);
    let (on_a_branch, _) = repo.git_allowing_failure(&["symbolic-ref", "--quiet", "HEAD"]);
    assert!(!on_a_branch, "fixture precondition: HEAD must be detached");
    write_swt_check(repo.path(), &exiting_check(0));
    let name = unique("detached");

    let output = run_swt(repo.path(), &["create", &name]);
    let stderr = support::stderr(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a parent on a detached HEAD must be refused: {stderr}"
    );
    for phrase in DETACHED_REFUSAL_PHRASES {
        assert!(
            stderr.contains(phrase),
            "the refusal must say {phrase:?}, got {stderr:?}"
        );
    }
    assert_eq!(
        stdout_of(&output),
        "",
        "a refused create prints no path for a caller to capture"
    );
    assert_eq!(
        beside_the_repo(&repo),
        vec!["repo".to_string()],
        "a refused create must make no directory: {stderr}"
    );
    let registered = repo.git(&["worktree", "list", "--porcelain"]);
    assert_eq!(
        registered
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        1,
        "a refused create must register no worktree: {registered}"
    );
    assert_eq!(
        repo.branches("swt/*"),
        Vec::<String>::new(),
        "a refused create must make no branch"
    );
}

// The path format is `create`'s business and nobody else's: `merge` is handed the
// worktree path and reads the branch out of that worktree's own HEAD, so it never
// parses either name. Pinned rather than assumed, because "merge is unaffected"
// is the claim that lets the path format change at all.
#[test]
fn merge_takes_a_path_create_printed_and_removes_the_branch_it_names() {
    let repo = TestRepo::new();
    write_swt_check(repo.path(), &exiting_check(0));
    let name = unique("mergeable");

    let created = run_swt(repo.path(), &["create", &name]);
    assert_eq!(
        created.status.code(),
        Some(0),
        "fixture precondition: create must succeed: {}",
        stderr_of(&created)
    );
    let worktree = PathBuf::from(stdout_of(&created).trim());

    // Work to bring back, so the merge is a real fast-forward and not a no-op.
    fs::write(worktree.join(SUBAGENT_FILE), "subagent\n").expect("subagent fixture file");
    git(&worktree, &["add", "--", SUBAGENT_FILE]);
    git(&worktree, &["commit", "--quiet", "-m", "subagent work"]);

    let merged = run_swt(
        repo.path(),
        &["merge", worktree.to_str().expect("utf-8 fixture path")],
    );
    let stderr = stderr_of(&merged);

    assert_eq!(
        merged.status.code(),
        Some(0),
        "merge must accept the path create printed: {stderr}"
    );
    assert!(
        repo.path().join(SUBAGENT_FILE).exists(),
        "the subagent's commit should have landed in the parent: {stderr}"
    );
    assert!(
        !worktree.exists(),
        "a merged worktree must be removed: {stderr}"
    );
    assert!(
        repo.created_branches(&name).is_empty(),
        "a merged branch must be deleted: {stderr}"
    );
}

// The two halves of the same design decision. The check runs *inside* the fresh
// worktree — anything else verifies a tree nobody is branching from — while the
// `.swt-check` override is read from the *parent*, because it is an uncommitted
// per-developer file that a clean checkout of HEAD by definition does not have.
#[test]
fn the_check_runs_in_the_new_worktree_from_an_override_only_the_parent_has() {
    let repo = TestRepo::new();
    write_swt_check(repo.path(), RECORD_CWD_CHECK);
    let name = unique("inside");

    let output = run_swt(repo.path(), &["create", &name]);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "the parent's override should have been found and passed: {stderr}"
    );
    let worktree = repo.sole_created_worktree(&name);
    let recorded = fs::read_to_string(worktree.join(CWD_MARKER))
        .expect("the check should have recorded its own cwd inside the new worktree");
    assert_eq!(
        recorded.trim(),
        worktree.to_string_lossy(),
        "the check must run in the worktree being verified"
    );
    assert!(
        !repo.path().join(CWD_MARKER).exists(),
        "the parent supplies the check; it is never the directory it runs in"
    );
    assert!(
        !worktree.join(SWT_CHECK).exists(),
        "the fresh checkout has no override of its own — the plan came from the parent"
    );
}

// The inverse of the override case, and the failure mode that would make `swt`
// worthless: a repository where nothing can be detected is not green by default.
#[test]
fn a_repository_with_no_check_anywhere_fails_instead_of_reporting_a_vacuous_green() {
    let repo = TestRepo::new();
    let name = unique("nocheck");

    let output = run_swt(repo.path(), &["create", &name]);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "an undetectable check must fail the command: {stderr}"
    );
    assert!(
        stderr.contains("HEAD not green: No green-check defined"),
        "the user should be told no check applied, not handed a green: {stderr}"
    );
    assert!(
        stderr.contains(&repo.path().display().to_string()),
        "the override belongs at the parent root, so that is the path to name: {stderr}"
    );
    assert_eq!(
        repo.created_worktrees(&name),
        Vec::<PathBuf>::new(),
        "an unverified worktree must not survive"
    );
    assert!(
        repo.created_branches(&name).is_empty(),
        "an unverified branch must not survive either"
    );
}

// A red check leaves nothing behind — worktree, branch, or a directory beside
// the repo — and says so, because the user asked for a worktree and is getting
// none.
#[test]
fn a_red_check_tears_the_worktree_and_the_branch_down_and_says_so() {
    let repo = TestRepo::new();
    write_swt_check(repo.path(), &exiting_check(1));
    let name = unique("red");

    let output = run_swt(repo.path(), &["create", &name]);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a red check must fail the command: {stderr}"
    );
    assert!(
        stderr.contains("HEAD not green:"),
        "the red verdict should be reported: {stderr}"
    );
    assert_eq!(
        repo.created_worktrees(&name),
        Vec::<PathBuf>::new(),
        "a red check left an orphaned worktree: {stderr}"
    );
    assert!(
        repo.created_branches(&name).is_empty(),
        "a red check left an orphaned branch: {stderr}"
    );
    // The worktree is gone, so what it was called can only be read back out of
    // the report — which is also the only place the two names appear together,
    // and therefore the only place a run can be caught keying them differently.
    let parent_name = dir_name(repo.path());
    let reported_path = format!(
        "Cleaned up worktree {}/{parent_name}--{name}-",
        repo.siblings().display()
    );
    let path_token = token_after(&stderr, &reported_path, WORKTREE_SUFFIX);
    let branch_token = token_after(
        &stderr,
        &format!(" and branch swt/{MAIN_BRANCH}/{name}-"),
        ".",
    );
    assert_eq!(
        path_token, branch_token,
        "the cleaned-up directory and branch must have been keyed on one token: {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "Cleaned up worktree {}/{parent_name}--{name}-{path_token}{WORKTREE_SUFFIX} \
             and branch {}.",
            repo.siblings().display(),
            child_branch(MAIN_BRANCH, &name, &branch_token)
        )),
        "a cleanup that happened should be reported: {stderr}"
    );
    assert_eq!(
        beside_the_repo(&repo),
        vec!["repo".to_string()],
        "nothing at all may be left beside the repository: {stderr}"
    );
    assert_eq!(
        stdout_of(&output),
        "",
        "a failed create prints no path for a caller to capture"
    );
}

// Teardown is best-effort, so its success is reported rather than assumed.
// Claiming a cleanup that did not happen is worse than not cleaning up at all:
// it strands the user with an orphaned worktree *and* branch they were told did
// not exist.
#[test]
fn a_teardown_that_failed_is_never_reported_as_a_cleanup() {
    let repo = TestRepo::new();
    write_swt_check(repo.path(), SABOTAGE_CHECK);
    let name = unique("sabotaged");

    let output = run_swt(repo.path(), &["create", &name]);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a red check must fail the command: {stderr}"
    );
    // The claim would only be a lie if the orphans really are orphans.
    let worktree = repo.sole_created_worktree(&name);
    assert!(
        worktree.exists(),
        "fixture precondition: {} should have survived teardown",
        worktree.display()
    );
    let branches = repo.created_branches(&name);
    assert_eq!(
        branches.len(),
        1,
        "fixture precondition: the branch should have survived too, got {branches:?}"
    );
    assert!(
        !stderr.contains("Cleaned up"),
        "claimed a cleanup while {} and {branches:?} both survived: {stderr}",
        worktree.display()
    );
    assert!(
        stderr.contains("fatal:"),
        "git's own account of the failed teardown was swallowed: {stderr}"
    );
    assert!(
        stderr.contains(&format!("Could not clean up {}.", worktree.display())),
        "the user should be told the cleanup did not work: {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "  git worktree remove --force '{}' && git branch -D 'swt/{MAIN_BRANCH}/{name}-",
            worktree.display()
        )),
        "no copy-pasteable recovery command naming the quoted path and the quoted branch: {stderr}"
    );
}

// Review finding R-20260915T193927Z#I1. A person pastes the recovery line into
// a shell, so the line is a command line and not prose. Its branch holds the
// parent branch as git gave it, and git accepts parent branches that a shell
// does not read as one word. For each such parent, a shell must read the line
// as exactly the two git commands that it names. A shell function takes the
// place of git, so nothing real runs.
#[test]
fn a_shell_reads_the_recovery_line_as_the_two_commands_it_names() {
    for parent_branch in SHELL_HOSTILE_PARENT_BRANCHES {
        let repo = TestRepo::new();
        let parent = repo.add_worktree_on("parent", parent_branch);
        // `swt` reads the override from the root of the worktree where it runs.
        write_swt_check(&parent.path, SABOTAGE_CHECK);
        let name = unique("hostile");

        let output = run_swt(&parent.path, &["create", &name]);
        let stderr = stderr_of(&output);

        assert_eq!(
            output.status.code(),
            Some(1),
            "a red check must fail the command for the parent on {parent_branch:?}: {stderr}"
        );
        assert!(
            stderr.contains("Could not clean up"),
            "fixture precondition: the teardown for the parent on {parent_branch:?} must fail: \
             {stderr}"
        );
        // The worktree and its branch survive, so the line names real values.
        let worktree = repo.sole_created_worktree(&name);
        let branch = child_branch(
            parent_branch,
            &name,
            &token_of_worktree(&worktree, &parent.path, &name),
        );
        assert_eq!(
            repo.created_branches(&name),
            vec![branch.clone()],
            "fixture precondition: exactly one branch must survive, under the parent branch \
             {parent_branch:?}"
        );

        // The recovery line runs from its label to the end of that line.
        let recovery_line = token_after(&stderr, RECOVERY_LINE_LABEL, "\n");
        // An empty directory, so a file that the line makes is easy to see.
        let shell_dir = repo.sibling("recovery-shell");
        fs::create_dir(&shell_dir).expect("an empty directory for the shell");
        let script = format!("{}\n{recovery_line}", recording_function("git"));
        let pasted = shell_command(&shell_dir, &script)
            .output()
            .expect("the shell should run");

        assert_eq!(
            pasted.status.code(),
            Some(0),
            "a shell must read the recovery line for the parent on {parent_branch:?} without \
             an error: {recovery_line}\n{}",
            stderr_of(&pasted)
        );
        assert_eq!(
            recorded_calls(&stdout_of(&pasted)),
            vec![
                vec![
                    "worktree",
                    "remove",
                    "--force",
                    worktree.to_str().expect("utf-8 fixture path"),
                ],
                vec!["branch", "-D", branch.as_str()],
            ],
            "a shell must read each value in the recovery line as one argument: {recovery_line}"
        );
        assert_eq!(
            entry_names(&shell_dir),
            Vec::<String>::new(),
            "a shell must run no command that the parent branch {parent_branch:?} spells: \
             {recovery_line}"
        );
    }
}

// The reason the check runs in the fresh worktree at all. A check that passes
// against the parent's uncommitted edit must fail in a clean checkout of HEAD —
// otherwise `swt` would hand a subagent a worktree branched from a commit that
// was never green.
#[test]
fn uncommitted_parent_state_cannot_fake_a_green() {
    let repo = TestRepo::new();
    fs::write(repo.path().join(TRACKED_FILE), PARENT_EDIT).expect("uncommitted parent edit");
    let check = write_swt_check(repo.path(), NEEDS_PARENT_EDIT_CHECK);
    let name = unique("dirty");

    // Mutation guard: run the very same check against the parent, where it must
    // pass. Without this the test could be passing because the check is simply
    // broken everywhere.
    let in_parent = Command::new(&check)
        .current_dir(repo.path())
        .status()
        .expect("the fixture check should run");
    assert!(
        in_parent.success(),
        "fixture precondition: this check passes against the parent's uncommitted edit"
    );

    let output = run_swt(repo.path(), &["create", &name]);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a check that only passes on uncommitted parent state must not report green: {stderr}"
    );
    assert!(
        stderr.contains("HEAD not green:"),
        "the red verdict should be reported: {stderr}"
    );
    assert_eq!(
        repo.created_worktrees(&name),
        Vec::<PathBuf>::new(),
        "the unverified worktree must be gone: {stderr}"
    );
    assert!(
        repo.created_branches(&name).is_empty(),
        "the unverified branch must be gone: {stderr}"
    );
}

// Everything `create` does starts from `git rev-parse --show-toplevel`, so the
// one place there is no repository at all has to stop the command with git's own
// explanation rather than inventing a path beside nothing.
#[test]
fn create_outside_a_repository_fails_with_gits_own_complaint() {
    let scratch = tempfile::Builder::new()
        .prefix("swt-create-no-repo-")
        .tempdir()
        .expect("scratch temp dir");
    let dir = fs::canonicalize(scratch.path()).expect("canonical scratch dir");

    let output = run_swt(&dir, &["create", &unique("orphan")]);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a create with no repository to branch from must fail: {stderr}"
    );
    assert_eq!(
        stdout_of(&output),
        "",
        "a failed create prints no path for a caller to capture"
    );
    assert!(
        stderr.contains("not a git repository"),
        "git's own explanation should reach the user: {stderr}"
    );
}

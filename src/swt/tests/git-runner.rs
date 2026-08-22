//! The git subprocess runner, exercised against throwaway repositories.
//!
//! Three separable guarantees are pinned here, each of which is invisible from
//! the outside until it is already broken:
//!
//! - **There is no shell between `swt` and git.** Branch names and worktree
//!   paths are built from caller-supplied argv, so an argument containing a
//!   space, a `;` or a `$(…)` must reach git as one literal argument and execute
//!   nothing.
//! - **Teardown attempts both of its commands and reports both.** A caller shown
//!   only the first complaint cannot tell whether the branch is also still lying
//!   around, which is the difference between a usable recovery instruction and a
//!   wrong one.
//! - **The teardown shield really shields.** It rests on a single spawn option;
//!   without a test, a regression to an unshielded teardown is silent.
//!
//! These call the production functions rather than hand-rolled imitations of
//! them: a test that spawned its own git would only prove that the *platform*
//! still behaves, not that `swt` still asks it to.

mod support;

use support::{git_allowing_failure, TestRepo, SWT_CHECK, TRACKED_FILE};
use swt::git::{git, git_must, remove_worktree, run_git, worktree_dirt};

/// An argument every shell would mangle: a space word-splits it, `;` separates a
/// command, `$(…)` substitutes one, `&&` chains and `|` pipes. It survives
/// intact only if nothing ever parses it as shell.
const HOSTILE: &str = "weird ; $(touch pwned) && echo no | tee bad.txt";

/// The file [`HOSTILE`]'s command substitution would create if it ever ran.
const PWNED: &str = "pwned";

/// The file [`HOSTILE`]'s pipeline would create if it ever ran.
const BAD: &str = "bad.txt";

/// Config key the argv tests round-trip a hostile value through.
const CONFIG_KEY: &str = "swt.testvalue";

/// Asserts that nothing in [`HOSTILE`] was ever executed.
fn assert_nothing_was_executed(repo: &TestRepo) {
    assert!(
        !repo.path().join(PWNED).exists(),
        "command substitution was executed"
    );
    assert!(!repo.path().join(BAD).exists(), "the pipeline was executed");
}

#[test]
fn a_successful_command_reports_ok_with_gits_output() {
    let repo = TestRepo::new();
    let outcome = git(["rev-parse", "--abbrev-ref", "HEAD"], Some(repo.path()));

    assert!(outcome.ok, "git rev-parse failed: {}", outcome.out);
    assert_eq!(
        outcome.out.trim(),
        "main",
        "git's stdout must come back to the caller, not be swallowed"
    );
}

#[test]
fn a_failing_command_reports_not_ok_carrying_gits_own_message() {
    let repo = TestRepo::new();
    let outcome = git(["rev-parse", "definitely-not-a-ref"], Some(repo.path()));

    assert!(!outcome.ok, "a non-zero git exit is not ok");
    assert!(
        outcome.out.contains("definitely-not-a-ref") && outcome.out.contains("fatal:"),
        "git's own stderr must be captured, got: {:?}",
        outcome.out
    );
}

// The next four tests exist to prove there is no shell between swt and git.
#[test]
fn an_argument_containing_spaces_is_one_argument_not_several() {
    let repo = TestRepo::new();
    let set = git(
        ["config", "--local", CONFIG_KEY, "one two three"],
        Some(repo.path()),
    );
    assert!(set.ok, "git config failed: {}", set.out);

    // Under `sh -c` this would have reached git as four argv entries and failed.
    assert_eq!(
        git_must(
            ["config", "--local", "--get", CONFIG_KEY],
            Some(repo.path())
        ),
        "one two three"
    );
}

#[test]
fn shell_metacharacters_are_stored_verbatim_never_interpreted() {
    let repo = TestRepo::new();
    let set = git(
        ["config", "--local", CONFIG_KEY, HOSTILE],
        Some(repo.path()),
    );
    assert!(set.ok, "git config failed: {}", set.out);

    assert_eq!(
        git_must(
            ["config", "--local", "--get", CONFIG_KEY],
            Some(repo.path())
        ),
        HOSTILE,
        "the value must round-trip byte for byte"
    );
    assert_nothing_was_executed(&repo);
}

// The shape the brief calls for directly: a ref name full of metacharacters is
// merely a ref that does not exist. Anything else — a shell error, a created
// file — means something parsed it.
#[test]
fn a_hostile_ref_name_merely_fails_to_resolve() {
    let repo = TestRepo::new();
    let outcome = git(["rev-parse", HOSTILE], Some(repo.path()));

    assert!(!outcome.ok, "a nonexistent ref must not resolve");
    // git echoes the argument back verbatim in its complaint, which is the
    // strongest available evidence that it received one argument spelled exactly
    // this way — not a word-split sequence of them, and not the result of a
    // substitution.
    assert!(
        outcome.out.contains(HOSTILE),
        "git should complain about the ref it was actually given, got: {:?}",
        outcome.out
    );
    assert_nothing_was_executed(&repo);
}

#[test]
fn a_path_argument_full_of_metacharacters_round_trips_through_the_index() {
    let repo = TestRepo::new();
    let evil = format!("{HOSTILE}.txt");
    repo.write(&evil, "contents\n");

    let add = git(["add", "--", &evil], Some(repo.path()));
    assert!(add.ok, "git add failed: {}", add.out);

    // `ls-files -z` emits raw, unquoted paths, so this is an exact match.
    let listed = git(["ls-files", "-z"], Some(repo.path()));
    assert!(listed.ok, "git ls-files failed: {}", listed.out);
    let mut paths: Vec<&str> = listed
        .out
        .split('\0')
        .filter(|path| !path.is_empty())
        .collect();
    paths.sort_unstable();
    let mut expected = vec![evil.as_str(), TRACKED_FILE];
    expected.sort_unstable();
    assert_eq!(paths, expected);
    assert_nothing_was_executed(&repo);
}

#[test]
fn a_leading_dash_argument_is_not_swallowed_as_an_option() {
    let repo = TestRepo::new();
    let set = git(
        ["config", "--local", CONFIG_KEY, "--not-an-option"],
        Some(repo.path()),
    );
    assert!(set.ok, "git config failed: {}", set.out);

    assert_eq!(
        git_must(
            ["config", "--local", "--get", CONFIG_KEY],
            Some(repo.path())
        ),
        "--not-an-option"
    );
}

// `git_must`'s other half — writing git's output to stderr and exiting 1 — ends
// the process, so it is not observable from inside this test binary. It is
// covered end to end by the `swt create` and `swt merge` tests in later slices,
// which run the real binary as a child and can read its status and stderr.
#[test]
fn git_must_returns_trimmed_output_on_success() {
    let repo = TestRepo::new();

    assert_eq!(
        git_must(["rev-parse", "--abbrev-ref", "HEAD"], Some(repo.path())),
        "main",
        "the trailing newline git prints must be trimmed off"
    );
    assert_eq!(
        git_must(
            ["config", "--local", CONFIG_KEY, "written"],
            Some(repo.path())
        ),
        "",
        "a silent command yields an empty string, not whitespace"
    );
}

#[test]
fn remove_worktree_removes_the_directory_and_deletes_the_branch() {
    let repo = TestRepo::new();
    let worktree = repo.add_worktree("teardown");
    assert!(
        worktree.path.exists(),
        "fixture precondition: the worktree must exist first"
    );

    let torn = remove_worktree(repo.path(), &worktree.path, &worktree.branch);

    assert!(torn.ok, "teardown reported failure: {}", torn.out);
    assert!(
        !worktree.path.exists(),
        "{} survived a teardown that reported success",
        worktree.path.display()
    );
    assert!(
        !repo
            .git(&["worktree", "list"])
            .contains(worktree.path.to_str().expect("utf-8 fixture path")),
        "the worktree is still registered"
    );
    assert_eq!(
        repo.branches(&worktree.branch),
        Vec::<String>::new(),
        "the branch outlived the worktree it was created for"
    );
}

// The regression that matters most. Teardown is two commands, and the second is
// not conditional on the first: a caller told only that the directory could not
// be removed still would not know whether the branch is lying around.
#[test]
fn remove_worktree_deletes_the_branch_even_when_the_removal_fails() {
    let repo = TestRepo::new();
    let stranger = repo.sibling("not-a-worktree");
    std::fs::create_dir_all(&stranger).expect("stranger directory");
    let branch = support::unique("swt/orphan");
    repo.git(&["branch", &branch, "HEAD"]);

    let torn = remove_worktree(repo.path(), &stranger, &branch);

    assert!(
        !torn.ok,
        "the worktree removal failed, so teardown did not succeed: {torn:?}"
    );
    assert!(
        torn.out
            .contains(stranger.to_str().expect("utf-8 fixture path")),
        "git's worktree complaint is missing:\n{}",
        torn.out
    );
    assert!(
        torn.out.contains(&branch),
        "the branch delete was never attempted after the removal failed:\n{}",
        torn.out
    );
    assert_eq!(
        repo.branches(&branch),
        Vec::<String>::new(),
        "the branch delete ran but did not take effect"
    );
}

#[test]
fn remove_worktree_reports_both_failures_when_neither_command_can_succeed() {
    let repo = TestRepo::new();
    let stranger = repo.sibling("not-a-worktree");
    std::fs::create_dir_all(&stranger).expect("stranger directory");
    let branch = "swt/never-existed";

    let torn = remove_worktree(repo.path(), &stranger, branch);

    assert!(!torn.ok, "teardown claimed success: {torn:?}");
    assert!(
        torn.out
            .contains(stranger.to_str().expect("utf-8 fixture path")),
        "git's worktree complaint is missing:\n{}",
        torn.out
    );
    assert!(
        torn.out.contains(branch),
        "git's branch complaint is missing:\n{}",
        torn.out
    );
}

#[test]
fn a_freshly_committed_repo_is_clean_under_either_scope() {
    let repo = TestRepo::new();

    for include_untracked in [false, true] {
        assert_eq!(
            worktree_dirt(repo.path(), include_untracked).expect("git status should succeed"),
            "",
            "a committed repo is clean (include_untracked={include_untracked})"
        );
    }
}

#[test]
fn a_modified_tracked_file_is_dirt_under_either_scope() {
    let repo = TestRepo::new();
    repo.write(TRACKED_FILE, "changed\n");

    for include_untracked in [false, true] {
        let dirt =
            worktree_dirt(repo.path(), include_untracked).expect("git status should succeed");
        assert!(
            dirt.contains(TRACKED_FILE),
            "a modified tracked file is dirt however untracked files are treated \
             (include_untracked={include_untracked}), got: {dirt:?}"
        );
    }
}

// The asymmetry the two guards are built on, pinned hard. `merge` ignores
// untracked files in the parent — the `.swt-check` escape hatch is by definition
// an untracked file at the parent root, so an untracked-sensitive parent guard
// would hard-block every merge for anyone following the documented workflow —
// and counts them in the subagent worktree, where `git worktree remove` deletes
// the whole directory and everything untracked in it.
#[test]
fn an_untracked_file_is_dirt_only_when_untracked_files_are_included() {
    let repo = TestRepo::new();
    repo.write("scratch.txt", "scratch\n");

    assert_eq!(
        worktree_dirt(repo.path(), false).expect("git status should succeed"),
        "",
        "untracked files must not count when they are excluded"
    );
    let dirt = worktree_dirt(repo.path(), true).expect("git status should succeed");
    assert!(
        dirt.contains("scratch.txt"),
        "an untracked file must count when untracked files are included, got: {dirt:?}"
    );
}

#[test]
fn an_uncommitted_swt_check_escape_hatch_is_not_parent_dirt() {
    let repo = TestRepo::new();
    repo.write(SWT_CHECK, "#!/bin/sh\nexit 0\n");

    assert_eq!(
        worktree_dirt(repo.path(), false).expect("git status should succeed"),
        "",
        "the documented escape hatch must not block every merge"
    );
    assert!(
        worktree_dirt(repo.path(), true)
            .expect("git status should succeed")
            .contains(SWT_CHECK),
        "the subagent scope must still see it, since teardown would delete it"
    );
}

#[test]
fn a_staged_addition_is_dirt_even_when_untracked_files_are_excluded() {
    let repo = TestRepo::new();
    repo.write("added.txt", "added\n");
    repo.git(&["add", "--", "added.txt"]);

    let dirt = worktree_dirt(repo.path(), false).expect("git status should succeed");
    assert!(
        dirt.contains("added.txt"),
        "a staged file is tracked, so it is dirt in both scopes, got: {dirt:?}"
    );
}

#[test]
fn a_deleted_tracked_file_is_dirt_even_when_untracked_files_are_excluded() {
    let repo = TestRepo::new();
    std::fs::remove_file(repo.path().join(TRACKED_FILE)).expect("remove tracked file");

    let dirt = worktree_dirt(repo.path(), false).expect("git status should succeed");
    assert!(
        dirt.contains(TRACKED_FILE),
        "a deleted tracked file is dirt, got: {dirt:?}"
    );
}

// A git that failed is not "clean". Reporting an empty dirt listing when git
// never answered would wave a merge straight past the guard.
#[test]
fn worktree_dirt_surfaces_a_git_failure_as_an_error() {
    let outside = tempfile::tempdir().expect("temp dir outside any repository");
    let (ok, _) = git_allowing_failure(outside.path(), &["rev-parse", "--git-dir"]);
    assert!(
        !ok,
        "fixture precondition: the temp dir must not be inside a repository"
    );

    let err = worktree_dirt(outside.path(), false)
        .expect_err("git status outside a repository must not report a clean tree");
    assert!(
        err.to_string().contains("repository"),
        "the error must carry git's own complaint, got: {err}"
    );
}

// The shield that keeps interrupt teardown alive is a single spawn option. If it
// ever stopped being applied nothing would throw: teardown's git would simply
// slide back into swt's process group, where a second Ctrl-C — which a terminal
// sends to the whole foreground group — kills it mid-`worktree remove` and
// orphans both the worktree and the branch it still claims. These two tests are
// that alarm, and they call `run_git` itself: a hand-rolled spawn here would
// only prove that the platform still honors an option the production code might
// have stopped passing.
#[cfg(unix)]
mod process_group_shield {
    use super::{run_git, TestRepo};
    use std::path::Path;
    use std::process::Command;

    /// Flag that defines the alias for one invocation only, leaving the
    /// fixture's own config untouched and beating any same-named alias in the
    /// developer's global config.
    const GIT_CONFIG_FLAG: &str = "-c";

    /// A git alias that reports the process group git itself is running in.
    ///
    /// A `!`-prefixed alias body is handed to a shell git forks, so `$$` is that
    /// shell's pid and the group it reports is the one it inherited from git.
    const PGID_ALIAS: &str = "alias.swtpgid=!ps -o pgid= -p $$";

    /// Name the alias is invoked by.
    const PGID_ALIAS_NAME: &str = "swtpgid";

    /// Reports the process group a git launched through the production
    /// [`run_git`] ran in.
    fn git_process_group(cwd: &Path, shielded: bool) -> String {
        let outcome = run_git(
            [GIT_CONFIG_FLAG, PGID_ALIAS, PGID_ALIAS_NAME],
            Some(cwd),
            shielded,
        );
        assert!(
            outcome.ok,
            "the pgid alias failed to run: {:?}",
            outcome.out
        );
        let pgid = outcome.out.trim().to_string();
        assert!(
            !pgid.is_empty() && pgid.chars().all(|c| c.is_ascii_digit()),
            "expected a process group id, got {:?}",
            outcome.out
        );
        pgid
    }

    /// This process's own process group — the one a terminal Ctrl-C aims at, and
    /// therefore the group teardown has to stay out of.
    fn own_process_group() -> String {
        let out = Command::new("ps")
            .args(["-o", "pgid=", "-p", &std::process::id().to_string()])
            .output()
            .expect("ps should run");
        assert!(
            out.status.success(),
            "ps failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn a_shielded_git_runs_outside_swts_process_group() {
        let repo = TestRepo::new();
        assert_ne!(
            git_process_group(repo.path(), true),
            own_process_group(),
            "shielded git shared swt's process group, so a Ctrl-C aimed at swt \
             would kill teardown mid-flight and orphan the worktree it was removing"
        );
    }

    #[test]
    fn an_unshielded_git_runs_inside_swts_process_group() {
        let repo = TestRepo::new();
        assert_eq!(
            git_process_group(repo.path(), false),
            own_process_group(),
            "unshielded git escaped swt's process group; work the user is waiting \
             on must stay interruptible by the Ctrl-C that abandons it"
        );
    }
}

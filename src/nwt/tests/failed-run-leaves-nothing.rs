//! End-to-end coverage for what a failed `nwt` run leaves on disk.
//!
//! `nwt` puts every new worktree in a directory it names from the repository.
//! A run that made that directory before it asked git for the worktree left the
//! directory behind on every failure. The stray directory then sat beside the
//! repository forever, because nothing ever removed it. In a repository whose
//! git directory is detached from its work tree the stray directory was
//! `~/.local/share/yadm-worktrees`, which is issue #439.
//!
//! `git worktree add` makes the directories that lead to the new worktree
//! itself, and it makes none of them when it refuses the request. So `nwt` asks
//! git first, and a failed run leaves nothing.
//!
//! Two failure modes drive every test here, because one mode proves only itself.
//! A branch that already exists stops git before it touches the file system. A
//! ref that another worktree already holds stops it at a different check.

mod support;

use std::path::{Path, PathBuf};

use gitscratch::testing::DetachedGitDirRepo;
use support::{git_stdout, init_repo, nanos, nwt_command, run_git};

/// The suffix `nwt` adds to the repository name to name the directory that
/// holds every new worktree.
const WORKTREES_SUFFIX: &str = "-worktrees";

/// The git configuration key that names the worktrees directory.
const WORKTREES_DIR_KEY: &str = "nwt.worktreesDir";

/// Resolve a path before an assertion reads it.
///
/// Git prints resolved paths, and every fixture lives under a temporary
/// directory that macOS reaches through a symbolic link: `/var` resolves to
/// `/private/var`.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
}

/// A branch name that no concurrent copy of this suite can also hold.
///
/// Two `cargo test` runs share one machine, and a branch name is a shared
/// resource. The process id and a nanosecond clock reading keep them apart.
fn unique_branch(label: &str) -> String {
    format!("{label}-{}-{}", std::process::id(), nanos())
}

/// The directory `nwt` puts every new worktree in by default: the name of
/// `main_worktree` plus [`WORKTREES_SUFFIX`], beside `main_worktree`.
///
/// The answer is built from the resolved `main_worktree`, because the directory
/// itself is what these tests expect to be absent. A path that does not exist
/// cannot be resolved.
fn default_worktrees_dir(main_worktree: &Path) -> PathBuf {
    let main_worktree = canonical(main_worktree);
    let name = main_worktree
        .file_name()
        .expect("the main worktree has a name")
        .to_str()
        .expect("utf-8 main worktree name");

    main_worktree.with_file_name(format!("{name}{WORKTREES_SUFFIX}"))
}

/// A way to make `git worktree add` refuse the request.
///
/// Each one is a different check inside git, so a fix that satisfies one of
/// them says nothing about the other.
#[derive(Clone, Copy)]
enum Failure {
    /// `nwt -b <branch>` where a branch of that name is already there. Git
    /// answers "a branch named ... already exists".
    BranchExists,
    /// `nwt -c <branch>` where another worktree already holds that branch. Git
    /// answers "is already used by worktree at ...".
    RefInUse,
}

impl Failure {
    /// The name every message and assertion of this mode reads.
    fn label(self) -> &'static str {
        match self {
            Self::BranchExists => "a branch that already exists",
            Self::RefInUse => "a ref another worktree holds",
        }
    }

    /// The words git puts on standard error for this mode, in lower case.
    ///
    /// The run echoes what git said, so this proves the run stopped at the
    /// intended check. Git writes the first word of a sentence with a capital
    /// letter in some releases and a small letter in others, so the assertion
    /// reads a lower-case copy of the text.
    fn git_complaint(self) -> &'static str {
        match self {
            Self::BranchExists => "branch named",
            Self::RefInUse => "already used by worktree",
        }
    }
}

/// Run `nwt` in `from` and demand that it fails at git.
///
/// `arguments` carries the failure mode. The two flags after it keep the run
/// short and silent. `--no-copy-env` stops a walk of the repository for `.env`
/// files, and `--no-bootstrap-hooks` stops a package manager install.
///
/// The assertion reads the exit status and what git said, and not the exit
/// code. `nwt` sorts a git refusal into one of several codes by the words git
/// used, and this file is about what the run leaves on disk.
fn assert_nwt_fails(from: &Path, arguments: &[&str], mode: Failure) {
    let output = nwt_command(from)
        .args(arguments)
        .args(["--no-copy-env", "--no-bootstrap-hooks"])
        .output()
        .expect("run the nwt binary");

    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();

    assert!(
        !output.status.success(),
        "nwt must refuse {} in {}:\n{}\n{stderr}",
        mode.label(),
        from.display(),
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        stderr.contains(mode.git_complaint()),
        "the run must stop at git for {}, but git said:\n{stderr}",
        mode.label()
    );
}

/// Demand that `path` is not on disk.
fn assert_absent(path: &Path, mode: Failure) {
    assert!(
        !path.exists(),
        "a run that failed on {} left {} behind",
        mode.label(),
        path.display()
    );
}

/// The names of everything directly under `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .expect("read one directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// The branch the repository at `repo` has checked out.
fn checked_out_branch(repo: &Path) -> String {
    git_stdout(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
        .trim()
        .to_string()
}

/// Make a normal repository that fails `mode`, and hand back the temporary
/// directory that holds it, the repository, and the arguments that fail.
fn normal_repository_that_fails(mode: Failure) -> (tempfile::TempDir, PathBuf, Vec<String>) {
    let (temp, repo) = init_repo();

    let arguments = match mode {
        Failure::BranchExists => {
            let branch = unique_branch("already-there");
            assert!(
                run_git(&repo, &["branch", &branch]),
                "git branch failed in {}",
                repo.display()
            );
            vec!["-b".to_string(), branch]
        }
        // The main worktree already holds the checked-out branch, so a request
        // to check it out again is refused.
        Failure::RefInUse => vec!["-c".to_string(), checked_out_branch(&repo)],
    };

    (temp, repo, arguments)
}

/// Borrow every argument of `arguments` as a string slice, which is what
/// [`assert_nwt_fails`] takes.
fn slices(arguments: &[String]) -> Vec<&str> {
    arguments.iter().map(String::as_str).collect()
}

/// Run a failing `nwt` in a normal repository and prove it made nothing.
fn assert_a_normal_repository_keeps_its_shape(mode: Failure) {
    let (temp, repo, arguments) = normal_repository_that_fails(mode);
    let worktrees_dir = default_worktrees_dir(&repo);

    assert_nwt_fails(&repo, &slices(&arguments), mode);

    assert_absent(&worktrees_dir, mode);
    assert_eq!(
        entries(temp.path()),
        vec!["repo".to_string()],
        "a run that failed on {} must leave the repository alone beside it",
        mode.label()
    );
}

#[test]
fn a_branch_that_exists_makes_no_worktrees_directory() {
    assert_a_normal_repository_keeps_its_shape(Failure::BranchExists);
}

#[test]
fn a_ref_another_worktree_holds_makes_no_worktrees_directory() {
    assert_a_normal_repository_keeps_its_shape(Failure::RefInUse);
}

#[test]
fn a_failed_run_in_a_detached_repository_makes_no_worktrees_directory() {
    let repo = DetachedGitDirRepo::nested();
    let branch = unique_branch("detached-already-there");
    repo.git(&["branch", &branch]);
    let worktrees_dir = default_worktrees_dir(repo.git_dir());

    assert_nwt_fails(repo.git_dir(), &["-b", &branch], Failure::BranchExists);

    assert_absent(&worktrees_dir, Failure::BranchExists);
}

#[test]
fn a_failed_run_in_a_detached_repository_makes_no_worktrees_directory_for_a_held_ref() {
    let repo = DetachedGitDirRepo::nested();
    // The work tree holds no `.git` entry, so git names both the git directory
    // and the work tree on the command line. That is what `repo.git` does.
    let held = repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]);
    let worktrees_dir = default_worktrees_dir(repo.git_dir());

    assert_nwt_fails(repo.git_dir(), &["-c", &held], Failure::RefInUse);

    assert_absent(&worktrees_dir, Failure::RefInUse);
}

#[test]
fn a_failed_run_makes_no_stated_worktrees_directory() {
    let (temp, repo) = init_repo();
    let stated = temp.path().join("stated-worktrees");
    assert!(
        run_git(
            &repo,
            &[
                "config",
                WORKTREES_DIR_KEY,
                stated.to_str().expect("utf-8 override path"),
            ],
        ),
        "git config {WORKTREES_DIR_KEY} failed in {}",
        repo.display()
    );
    let held = checked_out_branch(&repo);

    assert_nwt_fails(&repo, &["-c", &held], Failure::RefInUse);

    assert_absent(&stated, Failure::RefInUse);
}

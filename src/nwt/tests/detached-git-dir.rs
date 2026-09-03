//! End-to-end coverage for a repository whose git directory is detached from
//! its work tree, which is how `yadm` keeps a directory of dotfiles.
//!
//! `yadm` puts the git directory at `~/.local/share/yadm/repo.git`, names
//! `$HOME` as the work tree, and leaves no `.git` entry anywhere. A search that
//! walks the file system upward for a `.git` entry therefore finds no
//! repository, so `nwt` has to ask git instead.
//!
//! Git names the main worktree of that layout as the git directory itself. It
//! builds the name from the common git directory with a trailing `/.git`
//! removed, and this layout carries no such suffix. So the new worktree lands
//! in `<git directory>-worktrees`, beside the git directory.
//!
//! The two failures this file pins are different. From the git directory `nwt`
//! found no repository at all and refused. From a linked worktree it found the
//! parent of the git directory, made a `-worktrees` directory beside that
//! parent, and then `git worktree add` failed there.
//!
//! A normal repository is here too, from its root and from a linked worktree of
//! it, because the correction must leave that path exactly as it was.

mod support;

use std::path::{Path, PathBuf};

use gitscratch::testing::DetachedGitDirRepo;
use support::{git_stdout, init_repo, nanos, nwt_command, run_git};

/// The suffix `nwt` adds to the repository name to name the directory that
/// holds every new worktree.
const WORKTREES_SUFFIX: &str = "-worktrees";

/// The prefix of the line that names a worktree in `git worktree list
/// --porcelain`.
const WORKTREE_LINE_PREFIX: &str = "worktree ";

/// The name each fixture gives its one linked worktree, and the branch that
/// worktree carries.
const LINKED_BRANCH: &str = "linked-branch";

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

/// The directory `nwt` must put every new worktree in: the name of
/// `main_worktree` plus [`WORKTREES_SUFFIX`], beside `main_worktree`.
fn expected_worktrees_dir(main_worktree: &Path) -> PathBuf {
    let main_worktree = canonical(main_worktree);
    let name = main_worktree
        .file_name()
        .expect("the main worktree has a name")
        .to_str()
        .expect("utf-8 main worktree name");

    main_worktree.with_file_name(format!("{name}{WORKTREES_SUFFIX}"))
}

/// Run `nwt -b <branch>` in `from`, and hand back the path it printed.
///
/// The two flags keep the run short and silent. `--no-copy-env` stops a walk of
/// the repository for `.env` files, and `--no-bootstrap-hooks` stops a package
/// manager install. Neither one has anything to do with where the worktree
/// lands.
fn new_worktree(from: &Path, branch: &str) -> PathBuf {
    let output = nwt_command(from)
        .args(["-b", branch, "--no-copy-env", "--no-bootstrap-hooks"])
        .output()
        .expect("run the nwt binary");

    assert!(
        output.status.success(),
        "nwt -b {branch} failed in {}:\n{}\n{}",
        from.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let printed = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string());
    assert!(
        printed.is_dir(),
        "nwt printed {}, which is no directory",
        printed.display()
    );

    printed
}

/// Every worktree the repository at `main_worktree` holds, resolved.
fn listed_worktrees(main_worktree: &Path) -> Vec<PathBuf> {
    git_stdout(main_worktree, &["worktree", "list", "--porcelain"])
        .lines()
        .filter_map(|line| line.strip_prefix(WORKTREE_LINE_PREFIX))
        .map(|path| canonical(Path::new(path)))
        .collect()
}

/// Run `nwt -b <branch>` in `from`, then prove the new worktree landed beside
/// `main_worktree` and that git counts it as a worktree of the repository.
fn assert_lands_beside_the_main_worktree(main_worktree: &Path, from: &Path, label: &str) {
    let branch = unique_branch(label);

    let created = new_worktree(from, &branch);

    let expected = expected_worktrees_dir(main_worktree).join(&branch);
    assert_eq!(
        canonical(&created),
        canonical(&expected),
        "nwt must put the worktree beside {}",
        main_worktree.display()
    );
    assert!(
        listed_worktrees(main_worktree).contains(&canonical(&created)),
        "git must count {} as a worktree of the repository at {}",
        created.display(),
        main_worktree.display()
    );
}

#[test]
fn a_nested_git_directory_gets_a_worktree_beside_itself() {
    let repo = DetachedGitDirRepo::nested();

    assert_lands_beside_the_main_worktree(repo.git_dir(), repo.git_dir(), "nested-from-git-dir");
}

#[test]
fn a_git_directory_beside_its_work_tree_gets_a_worktree_beside_itself() {
    let repo = DetachedGitDirRepo::beside();

    assert_lands_beside_the_main_worktree(repo.git_dir(), repo.git_dir(), "beside-from-git-dir");
}

#[test]
fn a_worktree_of_a_nested_repository_gets_a_worktree_beside_the_git_directory() {
    let repo = DetachedGitDirRepo::nested();
    let linked = repo.add_worktree(&repo.work_tree().join(LINKED_BRANCH), LINKED_BRANCH);

    assert_lands_beside_the_main_worktree(repo.git_dir(), &linked, "nested-from-linked");
}

#[test]
fn a_worktree_of_a_beside_repository_gets_a_worktree_beside_the_git_directory() {
    let repo = DetachedGitDirRepo::beside();
    let linked = repo.add_worktree(&repo.work_tree().join(LINKED_BRANCH), LINKED_BRANCH);

    assert_lands_beside_the_main_worktree(repo.git_dir(), &linked, "beside-from-linked");
}

#[test]
fn a_normal_repository_gets_a_worktree_beside_itself() {
    let (_temp, repo) = init_repo();

    assert_lands_beside_the_main_worktree(&repo, &repo, "normal-from-root");
}

#[test]
fn a_worktree_of_a_normal_repository_gets_a_worktree_beside_the_repository() {
    let (temp, repo) = init_repo();
    let linked = temp.path().join(LINKED_BRANCH);
    assert!(
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                linked.to_str().expect("utf-8 worktree path"),
                "-b",
                LINKED_BRANCH,
            ],
        ),
        "git worktree add failed"
    );

    assert_lands_beside_the_main_worktree(&repo, &linked, "normal-from-linked");
}

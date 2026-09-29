//! The links from a new worktree to the child repositories of the main
//! worktree (issue #537).
//!
//! A container repository tracks the map of a workspace. The real repositories
//! sit one level below it, and its `.gitignore` keeps them out of its history.
//! `git worktree add` writes only tracked files, so a new worktree of the
//! container holds no child. [`link_children`] makes the symlink
//! `<worktree>/<name>` -> `<main worktree>/<name>` for each child.
//!
//! A symlink gives the worktree the one real checkout of each child. Thus no
//! clone, no branch, and no remote is duplicated, and a branch exists only in
//! a repository that the work changes.
//!
//! [`repowalker::child_repositories`] finds the children, so `nwt` and `cwt`
//! use one definition of a child: a directory one level below the main
//! worktree that holds a `.git` entry.
//!
//! Before it makes a link, this module asks git in the **new** worktree
//! whether git ignores the path. The new worktree can hold a `.gitignore` that
//! differs from the main worktree, so the question goes to the new worktree. A
//! link that git does not ignore shows as untracked, and `git add -A` commits
//! it into the container. Git gives the same answer before the link exists, so
//! this module asks first and never makes a link that git does not ignore.
//!
//! The target of each link is absolute: the main worktree that git names,
//! joined with the name of the child. Git keeps worktree paths absolute too, so
//! a relative link gives no more safety.
//!
//! The shell wrapper reads the worktree path from stdout, so each line of this
//! module goes to stderr, and the git child writes into a captured buffer.

use std::ffi::OsStr;
use std::io;
use std::path::Path;

use crate::production_git_command;

/// The exit status of `git check-ignore -q` for a path that git ignores.
const CHECK_IGNORE_IGNORED: i32 = 0;

/// The exit status of `git check-ignore -q` for a path that git does not
/// ignore.
const CHECK_IGNORE_NOT_IGNORED: i32 = 1;

/// The first word of each line that reports a link.
const LINKED_WORD: &str = "Linked";

/// What happened to one child of the main worktree.
///
/// Each variant other than [`Outcome::Linked`] makes no link.
enum Outcome {
    /// The link exists now.
    Linked,
    /// Git does not ignore the path in the new worktree, so a link there shows
    /// as untracked.
    NotIgnored,
    /// Git gave no answer. It did not start, or it exited with a status that
    /// is not 0 or 1.
    NoAnswer,
    /// Git ignores the path, but the symlink could not be made.
    LinkFailed,
}

/// Link each child repository of `main_worktree` into `worktree`.
///
/// `main_worktree` is the main worktree that git names, and `worktree` is the
/// new worktree. The function writes one line to stderr for each link, and a
/// summary when it made at least one link. A main worktree without children
/// gives no link and no line.
pub(crate) fn link_children(main_worktree: &Path, worktree: &Path) {
    let mut linked = 0_usize;

    for child in repowalker::child_repositories(main_worktree) {
        let Some(name) = child.file_name() else {
            continue;
        };
        let target = main_worktree.join(name);

        let outcome = link_child(worktree, name, &target);
        if matches!(outcome, Outcome::Linked) {
            linked += 1;
        }
        report(name, &target, &outcome);
    }

    if linked > 0 {
        eprintln!("{}", summary_line(linked));
    }
}

/// Ask git whether it ignores `name` in `worktree`, and when it does, make the
/// symlink `<worktree>/<name>` -> `target`.
fn link_child(worktree: &Path, name: &OsStr, target: &Path) -> Outcome {
    match check_ignore_status(worktree, name) {
        Some(CHECK_IGNORE_IGNORED) => {}
        Some(CHECK_IGNORE_NOT_IGNORED) => return Outcome::NotIgnored,
        _ => return Outcome::NoAnswer,
    }

    match make_directory_link(target, &worktree.join(name)) {
        Ok(()) => Outcome::Linked,
        Err(_) => Outcome::LinkFailed,
    }
}

/// The exit status of `git check-ignore -q -- <name>` in `worktree`.
///
/// Returns `None` when git does not start, or when a signal stops it.
///
/// The command goes through [`production_git_command`], which sheds the
/// inherited `GIT_` environment. An inherited `GIT_DIR` or `GIT_INDEX_FILE`
/// otherwise aims the question at another repository. The output is captured,
/// so nothing that git writes reaches the stdout of `nwt`.
fn check_ignore_status(worktree: &Path, name: &OsStr) -> Option<i32> {
    let mut command = production_git_command(worktree);
    command.args(["check-ignore", "-q", "--"]).arg(name);
    command.output().ok()?.status.code()
}

/// Write the line for one child to stderr.
fn report(name: &OsStr, target: &Path, outcome: &Outcome) {
    match outcome {
        Outcome::Linked => eprintln!("{}", linked_line(name, target)),
        Outcome::NotIgnored | Outcome::NoAnswer | Outcome::LinkFailed => {}
    }
}

/// The line that reports the link of the child `name` to `target`.
fn linked_line(name: &OsStr, target: &Path) -> String {
    format!(
        "{LINKED_WORD} {} -> {}",
        name.to_string_lossy(),
        target.display()
    )
}

/// The line that counts the links of one run.
fn summary_line(linked: usize) -> String {
    let noun = if linked == 1 {
        "child repository"
    } else {
        "child repositories"
    };
    format!("{LINKED_WORD} {linked} {noun} from the main worktree")
}

/// Make the symlink `link` that points at the directory `target`.
#[cfg(unix)]
fn make_directory_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Make the symlink `link` that points at the directory `target`.
///
/// Windows keeps a link to a directory apart from a link to a file, and a
/// child repository is a directory.
#[cfg(windows)]
fn make_directory_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

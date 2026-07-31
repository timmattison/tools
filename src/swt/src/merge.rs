//! merge — `swt merge <worktree-path>`: the subagent's work comes back only
//! when both sides are clean and green, and nothing is destroyed until it has.
//!
//! The order the guards fire in is itself the contract, because each one exists
//! to stop the next from doing damage: refuse the parent worktree, refuse a path
//! that names nothing, refuse dirt, refuse red, and only then touch a ref. Every
//! refusal happens before anything has been created, moved or deleted, so a
//! failed merge leaves the two worktrees exactly as it found them.
//!
//! Two decisions in here are not obvious and are load-bearing.
//!
//! **Cleanliness is scoped per side.** The parent is judged on tracked changes
//! only: a fast-forward can only ever touch tracked files, and counting untracked
//! ones would hard-block every merge for anyone using the documented
//! `./.swt-check` escape hatch, which is by definition an uncommitted file at the
//! parent root. The subagent is judged including untracked files, because `git
//! worktree remove` deletes the whole directory and everything untracked in it.
//!
//! **Nothing inside the locked region may exit the process.** An exit skips the
//! release, and the failure most likely to tempt one — a rebase conflict — is the
//! very case the region exists to handle; a lock leaked there blocks every later
//! merge in the repository until the staleness reap an hour later. So the region
//! is [`merge_under_lock`], a separate function that *returns* an [`Outcome`] and
//! never exits, and the exit happens in [`merge`] once the lock is gone.
//! [`git_must`] is banned inside it for the same reason: it exits on failure.

use std::env;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use crate::git::{git, git_must, worktree_dirt};
use crate::green_check::{is_green, Outcome};
use crate::lock::with_parent_lock;

/// The git query that names the root of the worktree `swt` was invoked in.
const TOPLEVEL_ARGS: [&str; 2] = ["rev-parse", "--show-toplevel"];

/// The git query that names the branch checked out in a worktree.
const CURRENT_BRANCH_ARGS: [&str; 3] = ["rev-parse", "--abbrev-ref", "HEAD"];

/// What `swt` says when asked to merge the parent into itself.
const PARENT_REFUSAL: &str = "Refusing: that's the parent worktree.\n";

/// What `swt` says after listing a worktree's dirt.
const STASH_ADVICE: &str = "Commit or stash before merging.\n";

/// What `swt` says when the parent worktree is not green — the invariant is that
/// a merge never advances the parent past an in-progress red.
const RED_PARENT_ADVICE: &str =
    "Refusing to merge — finish your red→green cycle in the parent first.\n";

/// What `swt` says before rebasing, so a run that does more than a fast-forward
/// explains itself while it happens rather than afterwards.
const REBASING_NOTICE: &str = "Parent advanced; rebasing subagent onto parent…\n";

/// Which of the two worktrees a cleanliness check is about.
///
/// The side determines all three of what it is called, how much counts as dirt,
/// and how that dirt is described — kept together here so a change to the scope
/// cannot leave the message claiming the old one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// The worktree being merged *into*.
    Parent,
    /// The worktree being merged and then deleted.
    Subagent,
}

impl Side {
    /// How this side is named to the user.
    fn label(self) -> &'static str {
        match self {
            Self::Parent => "Parent worktree",
            Self::Subagent => "Subagent worktree",
        }
    }

    /// Whether untracked files count as dirt on this side. True only where
    /// untracked work is actually at risk — see the module docs.
    fn includes_untracked(self) -> bool {
        matches!(self, Self::Subagent)
    }

    /// How this side's dirt is described, derived from its scope so the two can
    /// never disagree.
    fn kind(self) -> &'static str {
        if self.includes_untracked() {
            "uncommitted/untracked"
        } else {
            "uncommitted"
        }
    }
}

/// Resolves a caller-supplied path to an absolute one without touching the
/// filesystem, so a path that does not exist yet still resolves.
///
/// `path` is the path as typed. Returns it made absolute against the current
/// directory and lexically normalized: `.` components drop out and `..`
/// components pop the one before them, with `..` at the root resolving to the
/// root, exactly as path resolution has it. A relative path survives as itself in
/// the one case where the current directory cannot be read, which is the honest
/// answer — inventing a root for it would name some entirely different file.
fn absolute(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };

    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            // `.` names the directory it sits in, so it contributes nothing.
            Component::CurDir => {}
            Component::ParentDir => {
                // `pop` reports "there was nothing above this" by returning
                // false, which at the root is not an error: the root's parent is
                // the root. Only a path that never became absolute has a `..`
                // worth keeping.
                if !normalized.pop() && !normalized.has_root() {
                    normalized.push(component);
                }
            }
            other => normalized.push(other),
        }
    }
    normalized
}

/// Checks that one side of the merge is clean at that side's scope, explaining
/// itself on stderr when it is not.
///
/// `cwd` is the worktree root to inspect and `side` decides both the scope and
/// the wording. Returns whether the merge may proceed; a git that could not
/// answer is reported as git said it and counts as "may not", because an
/// unanswered question is emphatically not a clean worktree.
fn is_clean(cwd: &Path, side: Side) -> bool {
    match worktree_dirt(cwd, side.includes_untracked()) {
        Err(failure) => {
            eprint!("{}", failure.output());
            false
        }
        Ok(dirt) if dirt.is_empty() => true,
        Ok(dirt) => {
            // git's own listing, so the user sees exactly which files stopped
            // the merge rather than being told to go and look.
            eprint!("{} has {} changes:\n{dirt}\n", side.label(), side.kind());
            eprint!("{STASH_ADVICE}");
            false
        }
    }
}

/// Brings a subagent worktree's branch into the parent and tears the worktree
/// down, all while the parent repository's merge lock is held.
///
/// # Nothing in here may exit the process
///
/// Every failure is *returned*, so the lock is released before `swt` acts on it.
/// A `process::exit` from in here would skip that release and block every later
/// merge in the repository until the staleness reap. That is also why every git
/// call below is [`git`] rather than [`git_must`], which exits on failure.
///
/// `root` is the parent worktree, `wt` the subagent worktree, `branch` the branch
/// checked out in `wt`, and `parent_branch` the one checked out in `root`.
/// Returns success with the line to print, or the failure to report.
fn merge_under_lock(root: &Path, wt: &Path, branch: &str, parent_branch: &str) -> Outcome {
    let ff = git(["merge", "--ff-only", branch], Some(root));
    if !ff.ok {
        // Not an error: the parent moving on during a subagent's work is the
        // normal case. What lands has to be green *as merged*, though, so the
        // subagent is replayed onto the parent and verified again before the
        // fast-forward is retried.
        eprint!("{REBASING_NOTICE}");
        let rebase = git(["rebase", parent_branch], Some(wt));
        if !rebase.ok {
            // The conflicted rebase is deliberately left in place: it is the
            // user's to finish, and the command to resume with is the same one
            // they just ran.
            return Outcome::failed(format!(
                "{}\nResolve conflicts in {}, then re-run: swt merge {}\n",
                rebase.out,
                wt.display(),
                wt.display()
            ));
        }
        let re_green = is_green(wt, Some(root));
        if !re_green.ok {
            return Outcome::failed(format!("Not green after rebase: {}", re_green.out));
        }
        let ff_after_rebase = git(["merge", "--ff-only", branch], Some(root));
        if !ff_after_rebase.ok {
            return Outcome::failed(ff_after_rebase.out);
        }
    }

    // Deliberately without `--force`: everything the user typed has already been
    // verified clean, so git's own dirty-worktree refusal is a backstop against
    // anything that appeared since, and refusing is the right answer there.
    let removed = git(
        [OsStr::new("worktree"), OsStr::new("remove"), wt.as_os_str()],
        Some(root),
    );
    if !removed.ok {
        return Outcome::failed(removed.out);
    }
    // Lowercase `-d`, which refuses to delete a branch that is not merged. The
    // fast-forward above is what makes it succeed; if anything went wrong with
    // it, this is the guard that keeps the work from being deleted anyway.
    let deleted = git(["branch", "-d", branch], Some(root));
    if !deleted.ok {
        return Outcome::failed(deleted.out);
    }

    Outcome::succeeded(format!("merged {branch}, removed {}\n", wt.display()))
}

/// Merges the subagent worktree at `worktree_path` back into the parent.
///
/// Both worktrees must be clean and green before anything moves; if the parent
/// advanced in the meantime the subagent is rebased onto it and re-verified, so
/// what lands is green as merged rather than merely green in isolation. On
/// success the worktree and its branch are removed and a one-line summary goes to
/// stdout; every refusal explains itself on stderr and leaves both worktrees
/// untouched.
///
/// `worktree_path` is the subagent worktree as typed on the command line,
/// absolute or relative. Returns the status `swt` should exit with.
pub fn merge(worktree_path: &Path) -> ExitCode {
    let wt = absolute(worktree_path);
    let root = PathBuf::from(git_must(TOPLEVEL_ARGS, None));

    // Merging the parent into itself would, at best, delete the worktree the
    // user is standing in.
    if absolute(&root) == wt {
        eprint!("{PARENT_REFUSAL}");
        return ExitCode::FAILURE;
    }
    if !wt.exists() {
        // The path is the only thing the user typed, so it is quoted back rather
        // than left for git to complain about some other directory.
        eprintln!("No such worktree: {}", wt.display());
        return ExitCode::FAILURE;
    }

    // Ordered parent-first, and short-circuiting: the first side that is dirty is
    // the one to report, and there is nothing to gain from listing both.
    if !is_clean(&root, Side::Parent) || !is_clean(&wt, Side::Subagent) {
        return ExitCode::FAILURE;
    }

    // The parent must be green: a merge must never advance it past an in-progress
    // red, which would bury the failure the user is currently looking at. This
    // mirrors the create-time invariant.
    let parent_green = is_green(&root, None);
    if !parent_green.ok {
        // The check's output already ends in a newline of its own.
        eprint!("Parent worktree not green: {}", parent_green.out);
        eprint!("{RED_PARENT_ADVICE}");
        return ExitCode::FAILURE;
    }

    // Checked in the subagent worktree, configured from the parent — the same
    // asymmetry `create` relies on, and for the same reason: the `.swt-check`
    // override is an uncommitted per-developer file that only exists in `root`.
    let green = is_green(&wt, Some(&root));
    if !green.ok {
        eprint!("Subagent worktree not green: {}", green.out);
        return ExitCode::FAILURE;
    }

    // Both branches are read out here, before the lock: `git_must` exits on
    // failure, and an exit inside the locked region would leak the lock.
    let branch = git_must(CURRENT_BRANCH_ARGS, Some(&wt));
    let parent_branch = git_must(CURRENT_BRANCH_ARGS, Some(&root));

    let outcome = with_parent_lock(&root, || {
        merge_under_lock(&root, &wt, &branch, &parent_branch)
    });

    // Out here, holding nothing: the lock is gone whichever way the region ended.
    if !outcome.ok {
        eprint!("{}", outcome.out);
        return ExitCode::FAILURE;
    }
    print!("{}", outcome.out);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::{absolute, Side};
    use std::path::{Path, PathBuf};

    #[test]
    fn the_two_sides_are_named_and_scoped_differently() {
        assert_eq!(Side::Parent.label(), "Parent worktree");
        assert_eq!(Side::Subagent.label(), "Subagent worktree");
        assert!(
            !Side::Parent.includes_untracked(),
            "the documented .swt-check escape hatch is an untracked file at the parent root"
        );
        assert!(
            Side::Subagent.includes_untracked(),
            "`git worktree remove` deletes the subagent directory, untracked files included"
        );
    }

    // The wording has to follow the scope, or a refusal will claim to have
    // considered files it never looked at.
    #[test]
    fn each_sides_wording_follows_its_scope() {
        assert_eq!(Side::Parent.kind(), "uncommitted");
        assert_eq!(Side::Subagent.kind(), "uncommitted/untracked");
    }

    #[test]
    fn an_absolute_path_is_normalized_without_touching_the_filesystem() {
        let cases: [(&str, &str); 6] = [
            ("/repos/tools", "/repos/tools"),
            ("/repos/./tools", "/repos/tools"),
            ("/repos/tools/", "/repos/tools"),
            ("/repos/tools/../fix-parser.swt", "/repos/fix-parser.swt"),
            ("/repos/tools/..", "/repos"),
            // A `..` with nothing above it resolves to the root, exactly as path
            // resolution has it.
            ("/..", "/"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                absolute(Path::new(input)),
                PathBuf::from(expected),
                "absolute({input:?})"
            );
        }
    }

    // The parent-worktree refusal compares the typed path against git's answer,
    // and git always answers with an absolute, already-normalized path — so the
    // comparison only holds if the typed side is brought to the same form.
    #[test]
    fn a_relative_path_is_resolved_against_the_current_directory() {
        let cwd = std::env::current_dir().expect("a readable current directory");
        assert_eq!(absolute(Path::new("sub.swt")), cwd.join("sub.swt"));
        assert_eq!(absolute(Path::new("./sub.swt")), cwd.join("sub.swt"));
        assert_eq!(
            absolute(Path::new("../sub.swt")),
            cwd.parent().unwrap_or(&cwd).join("sub.swt"),
            "a worktree is a sibling of the repo, so `../name.swt` is the common spelling"
        );
    }
}

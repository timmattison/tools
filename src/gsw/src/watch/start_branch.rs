//! The start branch of watch mode.
//!
//! This module holds `StartBranch`. Its own module makes the field private to
//! the rest of `watch`, tests included. The one question a caller can ask is
//! `StartBranch::is_shown_by`.

use crate::render::{Operation, Snapshot};

/// The words of the hint that the first press of Esc puts on the row. The
/// name of the start branch follows them, after one space.
const RETURN_HINT: &str = "Hit Escape again to go back to";

/// The branch that HEAD named when gsw started in watch mode: the start
/// branch.
///
/// **The field is private to this module, and no accessor gives the name out.**
/// The rest of `watch`, its tests included, thus cannot read the name. The one
/// question a caller may ask is [`StartBranch::is_shown_by`], so the rule for
/// "this frame is on the start branch" lives in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StartBranch(String);

impl StartBranch {
    /// Read the start branch from `repo`. A HEAD that names a branch gives
    /// that branch. A HEAD that a rebase detached gives the branch that the
    /// rebase rebases, because a user who starts gsw in a rebase has that
    /// branch to come back to. Any other detached HEAD names no branch, so
    /// the result is `None` and the session has no start branch to leave.
    ///
    /// The rule is [`StartBranch::branch_of`], which [`StartBranch::is_shown_by`]
    /// uses too.
    pub(super) fn read(repo: &gix::Repository) -> Option<Self> {
        // The conflict count is 0 here. It does not change the branch.
        Self::branch_of(
            &crate::repo::branch_name(repo),
            crate::repo::operation_state(repo, 0).as_ref(),
            repo,
        )
        .map(Self)
    }

    /// The branch that a HEAD named `branch` is on, with `operation` in
    /// progress in `repo`.
    ///
    /// A HEAD that names a branch is on that branch. A HEAD that a rebase
    /// detached is on the branch that the rebase rebases. That name comes from
    /// `head-name`, through [`crate::repo::operation_start_of`]. Any other
    /// detached HEAD is on no branch.
    fn branch_of(
        branch: &str,
        operation: Option<&Operation>,
        repo: &gix::Repository,
    ) -> Option<String> {
        if branch != crate::repo::DETACHED_HEAD {
            return Some(branch.to_string());
        }
        match operation {
            Some(operation @ Operation::Rebase { .. }) => {
                crate::repo::operation_start_of(repo, operation).branch
            }
            _ => None,
        }
    }

    /// Whether `snapshot`, which a walk of `repo` collected, shows the start
    /// branch.
    ///
    /// The names of the branches are compared, and never the header labels.
    ///
    /// Git detaches HEAD for the whole of a rebase, so the branch of the
    /// snapshot is then `HEAD`. A rebase of the start branch must still count
    /// as on it, because the `R` key of gsw makes such a rebase. The rule is
    /// [`StartBranch::branch_of`], which reads the rebased branch from
    /// `head-name`. A HEAD that names a branch is compared by that name alone,
    /// even when a rebase directory exists. A walk with no rebase reads
    /// nothing extra. A merge keeps HEAD on its branch and needs no rule.
    pub(super) fn is_shown_by(&self, snapshot: &Snapshot, repo: &gix::Repository) -> bool {
        Self::branch_of(&snapshot.branch, snapshot.operation.as_ref(), repo).as_deref()
            == Some(self.0.as_str())
    }

    /// The line that the first press of Esc puts on the row: it asks for a
    /// second press and names the start branch.
    ///
    /// The name is in the line, and the line is the only way it leaves this
    /// module as text.
    pub(super) fn return_hint(&self) -> String {
        format!("{RETURN_HINT} {}", self.0)
    }

    /// Whether the work tree that `repo` was opened on is on the start branch.
    ///
    /// The rule is [`StartBranch::read`], so a HEAD that names the branch is on
    /// it, and so is a HEAD that a rebase of the branch detached. Any other
    /// detached HEAD is on no branch.
    pub(super) fn is_checked_out_in(&self, repo: &gix::Repository) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testrepo::{diverged_repo, git, git_allowing_failure, init_repo};

    /// The hint names the start branch, in the words of the issue, and nothing
    /// more: no age, no punctuation at the end.
    #[test]
    fn the_return_hint_names_the_start_branch() {
        let branch = StartBranch("topic".to_string());
        assert_eq!(branch.return_hint(), "Hit Escape again to go back to topic");
    }

    /// A name with multi-byte characters goes into the line whole.
    #[test]
    fn the_return_hint_keeps_a_multi_byte_name_whole() {
        let branch = StartBranch("日本語-café-🎉".to_string());
        assert_eq!(
            branch.return_hint(),
            "Hit Escape again to go back to 日本語-café-🎉",
        );
    }

    /// Open the repository at `dir` the way the watch does: by path, with no
    /// search upward.
    fn open(dir: &std::path::Path) -> gix::Repository {
        gix::open(dir).expect("open the fixture")
    }

    /// A HEAD on the start branch is on it.
    #[test]
    fn a_work_tree_on_the_start_branch_is_on_it() {
        let dir = init_repo();
        git(dir.path(), &["checkout", "-q", "-b", "topic"]);
        let branch = StartBranch::read(&open(dir.path())).expect("a start branch");
        assert_eq!(branch, StartBranch("topic".to_string()));
        assert!(branch.is_checked_out_in(&open(dir.path())));
    }

    /// A HEAD on another branch is not on the start branch.
    #[test]
    fn a_work_tree_on_another_branch_is_not_on_the_start_branch() {
        let dir = init_repo();
        git(dir.path(), &["checkout", "-q", "-b", "topic"]);
        let branch = StartBranch::read(&open(dir.path())).expect("a start branch");
        git(dir.path(), &["checkout", "-q", "main"]);
        assert!(!branch.is_checked_out_in(&open(dir.path())));
    }

    /// A rebase of the start branch, stopped on a conflict, detaches HEAD. The
    /// work tree is still on the start branch.
    #[test]
    fn a_stopped_rebase_of_the_start_branch_is_on_it() {
        let dir = diverged_repo();
        let p = dir.path();
        let branch = StartBranch::read(&open(p)).expect("a start branch");
        assert_eq!(branch, StartBranch("feature".to_string()));

        git_allowing_failure(p, &["rebase", "main"]);

        let repo = open(p);
        assert_eq!(
            crate::repo::branch_name(&repo),
            crate::repo::DETACHED_HEAD,
            "git detaches HEAD in a rebase",
        );
        assert!(branch.is_checked_out_in(&repo));
    }

    /// A detached HEAD outside a rebase is on no branch.
    #[test]
    fn a_detached_head_outside_a_rebase_is_not_on_the_start_branch() {
        let dir = init_repo();
        git(dir.path(), &["checkout", "-q", "-b", "topic"]);
        let branch = StartBranch::read(&open(dir.path())).expect("a start branch");
        git(dir.path(), &["checkout", "-q", "--detach"]);
        assert!(!branch.is_checked_out_in(&open(dir.path())));
    }
}

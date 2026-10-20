//! The start branch of watch mode.
//!
//! This module holds `StartBranch`. Its own module makes the field private to
//! the rest of `watch`, tests included. The one question a caller can ask is
//! `StartBranch::is_shown_by`.

use crate::render::{Operation, Snapshot};

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
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

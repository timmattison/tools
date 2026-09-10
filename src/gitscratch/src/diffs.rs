//! The halt diff: the text `git diff` shows at one halt of a replay.
//!
//! A [`Conflicts`](crate::Conflicts) says how many hunks conflict, and in which
//! files. It does not show the hunks. To see them, a person must start the real
//! rebase or merge and read `git diff` at each halt, and a dry run exists to
//! prevent that work. So a replay can capture that text at each halt, as a
//! [`HaltDiff`], and give every one back in halt order, as [`HaltDiffs`].
//!
//! The capture is opt-in, and its result stays out of `Conflicts`. `grist`
//! replays each step of each ordering, prints no diff, and folds and ranks the
//! `Conflicts` values it gets. A diff inside that type puts a copy of each diff
//! into each ordering that `grist` keeps. So the entrance that captures returns
//! the halt diffs in a value beside the `Conflicts`, and the plain entrance
//! captures nothing.

use crate::git::Git;

/// The text `git diff` shows at one halt of a replay.
///
/// A rebase halt carries the name of its stopped commit. A merge halt carries
/// no name, because a merge has no stopped commit.
///
/// The diff is bytes, not text: the stdout of the diff call, byte for byte, not
/// trimmed and not decoded. File content can hold bytes that are not UTF-8, so
/// a caller that prints the diff converts it at print time, as a caller that
/// prints a conflicted name does. When git gives no diff, the halt diff holds
/// the error text in its place. The replay does not stop for that error,
/// because the counts must not depend on the diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HaltDiff {
    /// The name of the stopped commit, or `None` for a merge.
    stopped: Option<String>,
    /// The stdout of the diff call, or the error text when git gave no diff.
    diff: Result<Vec<u8>, String>,
}

impl HaltDiff {
    /// Record the halt that `git` stands on, with the name of its stopped
    /// commit.
    ///
    /// The diff is not read yet, so it is empty.
    pub(crate) fn capture(_git: &Git, stopped: Option<String>) -> Self {
        Self {
            stopped,
            diff: Ok(Vec::new()),
        }
    }

    /// The name of the commit the rebase stopped on, or `None` for a merge.
    ///
    /// The name is the text `git log -1 --format="%h %s"` prints for the
    /// stopped commit: its short id, a space, and its subject.
    #[must_use]
    pub fn stopped(&self) -> Option<&str> {
        self.stopped.as_deref()
    }

    /// The diff at this halt, as the bytes git wrote.
    ///
    /// # Errors
    ///
    /// Returns `Err(message)` when git gave no diff at this halt. The message
    /// is the error text, and it carries git's own stderr. The halt still
    /// counts: the replay does not stop for a diff it could not read.
    pub fn diff(&self) -> Result<&[u8], &str> {
        self.diff.as_deref().map_err(String::as_str)
    }
}

/// Every halt diff of one replay, in halt order.
///
/// Empty for a replay that did not halt. A replay that halted and captured has
/// one halt diff for each halt, so the length of this value is the stop count
/// of the [`Conflicts`](crate::Conflicts) beside it.
///
/// There is no `Default`, for the reason [`Conflicts`](crate::Conflicts) has
/// none. A released binary gets this value from a replay, and from nothing
/// else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HaltDiffs {
    halts: Vec<HaltDiff>,
}

impl HaltDiffs {
    /// A value that holds no halt diff yet: the start of a replay.
    #[must_use]
    pub(crate) fn nothing_captured() -> Self {
        Self { halts: Vec::new() }
    }

    /// Add the halt diff of the next halt.
    pub(crate) fn push(&mut self, halt: HaltDiff) {
        self.halts.push(halt);
    }

    /// Every halt diff, in halt order.
    pub fn iter(&self) -> std::slice::Iter<'_, HaltDiff> {
        self.halts.iter()
    }

    /// How many halt diffs the replay captured: one for each halt.
    #[must_use]
    pub fn len(&self) -> usize {
        self.halts.len()
    }

    /// Whether the replay captured no halt diff.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.halts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::HaltDiff;
    use crate::git::{Git, NoInheritedGitEnvironment};
    use crate::repo::PREFLIGHT_HOOKS_PATH;
    use crate::testing::not_a_repository;

    /// The name the test gives the capture, so that it can read the name back.
    const STOPPED: &str = "abc1234 a stopped commit";

    /// A diff call that fails leaves git's own words in the halt diff, and the
    /// capture still gives a halt diff.
    ///
    /// The capture must never fail the replay, because the counts must not
    /// depend on the diff. So a diff that git does not give is not an error of
    /// the replay. It is the content of that one halt diff, and the reader
    /// learns the cause from git. A directory that is not a repository is the
    /// plain way to make `git diff` fail.
    ///
    /// The control makes plain git fail the same way first, and reads the
    /// first line git writes about it. The assertion compares against that
    /// line and not against a sentence written here, so it holds in each
    /// language that git speaks.
    #[test]
    fn a_diff_call_that_fails_leaves_git_s_own_words_in_the_halt_diff() {
        let outside = not_a_repository();

        let refused = Command::new("git")
            .arg("diff")
            .current_dir(outside.path())
            .without_inherited_git_environment()
            .output()
            .expect("spawn git diff outside a repository");
        assert!(
            !refused.status.success(),
            "git gave a diff outside a repository, so nothing here makes the diff call fail and \
             the assertion below is measured against nothing"
        );
        let stderr = String::from_utf8_lossy(&refused.stderr);
        let first_words = stderr
            .lines()
            .next()
            .expect("git says why it refused the diff");

        let halt = HaltDiff::capture(
            &Git::new(outside.path(), PREFLIGHT_HOOKS_PATH),
            Some(STOPPED.to_owned()),
        );

        assert_eq!(
            halt.stopped(),
            Some(STOPPED),
            "the capture keeps the name it was given, whatever git said about the diff"
        );
        let message = halt.diff().expect_err(
            "git gave no diff outside a repository, so the halt diff has to hold the error in the \
             place of a diff",
        );
        assert!(
            message.contains(first_words),
            "the halt diff has to carry git's own account of the failure, `{first_words}`, got: \
             {message}"
        );
    }
}

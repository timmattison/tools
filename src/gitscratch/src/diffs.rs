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

/// The arguments of the diff call at a halt, after the subcommand `diff`.
///
/// The global configuration of the developer reaches the runner, and some of
/// its settings change the text of a diff. Each flag above the last one pins
/// such a setting, so one halt gives the same halt diff on each machine. A
/// test in `tests/diffs.rs` holds each pin against its setting.
///
/// `--diff-filter=U` makes the diff name the files that the counter reads, and
/// no other file. The counter reads `git diff --name-only --diff-filter=U`, so
/// with the same filter on the diff call, the halt diff and the breakdown
/// cannot name different files.
const DIFF_AT_HALT: &[&str] = &[
    // `color.ui=always` and `color.diff=always` put color codes into the text,
    // although git writes to a pipe. A renderer cannot tell such a code from
    // an ESC byte in the file, and the renderer paints the diff itself.
    "--no-color",
    "--diff-filter=U",
];

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
    /// Capture the halt that `git` stands on: the text `git diff` shows there,
    /// and the name of the stopped commit.
    ///
    /// A `HaltDiff` and never a `Result`, because the capture must never fail
    /// the replay. A diff call that fails gives `Err(message)` inside the halt
    /// diff, and the counts and the exit code stay the same. The message is
    /// the error text in its alternate form, so it carries git's own stderr.
    ///
    /// The diff comes through [`Git::verbatim`], because the halt diff is text
    /// that goes to a person verbatim, and each other reader trims or decodes.
    ///
    /// The caller decides where the capture happens. At a rebase stop, the
    /// capture must come before the replay stages the markers, because after
    /// that `git diff` shows nothing for the stop.
    pub(crate) fn capture(git: &Git, stopped: Option<String>) -> Self {
        Self {
            stopped,
            diff: git
                .verbatim("diff", DIFF_AT_HALT)
                .map_err(|err| format!("{err:#}")),
        }
    }

    /// Build a halt diff straight from its two parts.
    ///
    /// A fixture constructor for the tests of a renderer, gated the way
    /// [`Conflicts::from_files`](crate::Conflicts::from_files) is. Each call
    /// site is a fixture. A released binary cannot state a halt diff that
    /// nothing captured: it gets a `HaltDiff` from a replay, and from nothing
    /// else.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub fn from_parts(stopped: Option<&str>, diff: Result<&[u8], &str>) -> Self {
        Self {
            stopped: stopped.map(str::to_owned),
            diff: diff.map(<[u8]>::to_vec).map_err(str::to_owned),
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

    /// Build a set of halt diffs straight from `halts`, in the order given.
    ///
    /// A fixture constructor for the tests of a renderer, gated the way
    /// [`HaltDiff::from_parts`] is, and for the same reason.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub fn from_halts(halts: impl IntoIterator<Item = HaltDiff>) -> Self {
        Self {
            halts: halts.into_iter().collect(),
        }
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

    use super::{HaltDiff, HaltDiffs};
    use crate::git::{Git, NoInheritedGitEnvironment};
    use crate::repo::PREFLIGHT_HOOKS_PATH;
    use crate::testing::not_a_repository;

    /// The name a test gives a halt diff, so that it can read the name back.
    const STOPPED: &str = "abc1234 a stopped commit";

    /// A diff a test gives a hand-built halt diff.
    const DIFF_TEXT: &[u8] = b"diff --cc f.txt\n";

    /// An error a test gives a hand-built halt diff in the place of a diff.
    const NO_DIFF: &str = "fatal: unable to read files to diff";

    /// A hand-built halt diff reads back what it was given, and a hand-built
    /// set keeps the order it was given.
    ///
    /// The tests of a renderer build their halt diffs with these two
    /// constructors. A constructor that drops the name, puts the diff in the
    /// place of the error, or changes the order of the halts makes a broken
    /// renderer look correct. The test then compares the renderer against a
    /// fixture that is already wrong.
    #[test]
    fn a_hand_built_halt_diff_reads_back_what_it_was_given() {
        let rebase = HaltDiff::from_parts(Some(STOPPED), Ok(DIFF_TEXT));
        let merge = HaltDiff::from_parts(None, Err(NO_DIFF));

        assert_eq!(
            (rebase.stopped(), rebase.diff()),
            (Some(STOPPED), Ok(DIFF_TEXT)),
            "a halt diff built from a name and a diff has to read back that name and that diff"
        );
        assert_eq!(
            (merge.stopped(), merge.diff()),
            (None, Err(NO_DIFF)),
            "a halt diff built from no name and an error has to read back no name and that error"
        );

        let halts = HaltDiffs::from_halts([rebase.clone(), merge.clone()]);
        assert_eq!(
            halts.iter().collect::<Vec<_>>(),
            vec![&rebase, &merge],
            "a set built from two halt diffs has to hold both, in the order given"
        );
        assert_eq!(halts.len(), 2, "the length counts each halt diff given");
    }

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

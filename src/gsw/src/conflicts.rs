//! What one press of `m` measures, and the words that report it.
//!
//! `grind` measures a rebase of HEAD onto the default branch, and `grime`
//! measures a merge of the default branch into HEAD. This module makes the same
//! `gitscratch` calls in this process. It does not start the two binaries. So
//! gsw gets a typed [`Conflicts`] value, and it does not need either tool on the
//! `PATH`.
//!
//! The words name `grind` and `grime` all the same, because those are the names
//! the user knows.

use gitscratch::Conflicts;

use crate::lines::LineSplitter;

/// The two tools a notice names, spelled once for every sentence that names
/// them.
///
/// A macro and not a `const`, because [`WAITING_NOTICE`] is a `const` too, and
/// `concat!` takes only literals.
macro_rules! tools {
    () => {
        "grind and grime"
    };
}

/// The notice on the bottom row while gsw quits and a replay is in flight.
///
/// The quit waits for that replay. A replay that gsw abandons keeps a scratch
/// worktree registered in the repository of the user, so the wait is the price
/// of a clean repository.
pub(crate) const WAITING_NOTICE: &str = concat!("Waiting for ", tools!(), " to finish…");

/// What joins the parts of a measured line.
const SEPARATOR: &str = " · ";

/// The last part of a measured line when the work tree held uncommitted work.
///
/// `grind` and `grime` say the same thing on stderr. A replay starts from HEAD,
/// so a count never includes that work.
const DIRTY_NOTE: &str = "uncommitted work not included";

/// Whether the words of one half name the count of stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopWords {
    /// Name the stops. A rebase stops once for each commit that conflicts, so
    /// the count is a measurement.
    Named,
    /// Leave the stops out. A merge stops once or never, so the count is a
    /// constant. `grime` leaves it out for the same reason (see the comment on
    /// `without_stops` in `src/grime/src/main.rs`).
    Omitted,
}

/// What one press of `m` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConflictsOutcome {
    /// HEAD is the default branch itself. No replay ran.
    ///
    /// A rebase of a branch onto itself is clean by definition, so a replay
    /// costs a scratch worktree and tells the user nothing.
    OnDefault {
        /// The default branch, which HEAD is on.
        branch: String,
    },
    /// No measurement was possible: an empty repository, no default branch, or
    /// a directory that is not a repository.
    Refused {
        /// Why, as one row of text.
        reason: String,
    },
    /// Both replays were attempted. Each half is its own result.
    Measured {
        /// The branch that both replays measured against.
        branch: String,
        /// The rebase of HEAD onto `branch`, or why it failed.
        rebase: Result<Conflicts, String>,
        /// The merge of `branch` into HEAD, or why it failed.
        merge: Result<Conflicts, String>,
        /// Whether the work tree held uncommitted work. A replay starts from
        /// HEAD, so that work is not part of either result.
        dirty: bool,
    },
}

impl ConflictsOutcome {
    /// The one row that reports this outcome under the frame.
    ///
    /// The words for a count come only from the phrases of `gitscratch`, so
    /// gsw, `grind` and `grime` never give one number two names.
    ///
    /// The row is not cut to a width here. The overlay under the frame cuts
    /// every row it paints to the width of the pane.
    pub(crate) fn line(&self) -> String {
        match self {
            Self::OnDefault { branch } => format!("on {branch} — nothing to compare"),
            Self::Refused { reason } => {
                format!(concat!(tools!(), " failed: {}"), one_row(reason))
            }
            Self::Measured {
                branch,
                rebase,
                merge,
                dirty,
            } => {
                let mut parts = vec![
                    half("rebase", rebase, StopWords::Named),
                    half("merge", merge, StopWords::Omitted),
                ];
                if *dirty {
                    parts.push(DIRTY_NOTE.to_owned());
                }
                format!("{branch}: {}", parts.join(SEPARATOR))
            }
        }
    }
}

/// The notice on the bottom row while a run measures against `branch`.
///
/// The notice does not fade. It tells the user that a press of `m` does
/// nothing until the run ends.
pub(crate) fn running_notice(branch: &str) -> String {
    format!(concat!("Running ", tools!(), " against {}…"), branch)
}

/// The words for the result of one replay: `operation clean`, the counts, or
/// `operation failed: reason`.
///
/// A failed half shows its reason and no number. A number in the line must
/// never be a guess.
fn half(operation: &str, result: &Result<Conflicts, String>, stops: StopWords) -> String {
    match result {
        Err(reason) => format!("{operation} failed: {}", one_row(reason)),
        Ok(conflicts) if conflicts.is_clean() => format!("{operation} clean"),
        Ok(conflicts) => {
            let cost = format!(
                "{} in {}",
                conflicts.hunks().phrase(),
                conflicts.files().phrase()
            );
            match stops {
                StopWords::Named => format!("{operation} {cost}, {}", conflicts.stops().phrase()),
                StopWords::Omitted => format!("{operation} {cost}"),
            }
        }
    }
}

/// `text` as one row that is safe to paint: no escape sequence, no control
/// character, and each run of whitespace as one space.
///
/// An error from `gitscratch` carries the stdout and the stderr of git, so it
/// can span many lines. The overlay under the frame measures a row in display
/// columns, and a newline or an escape sequence draws a different number of
/// columns than it measures. [`LineSplitter`] is the one place in gsw that makes
/// the text of a child process safe to paint, so this function uses it and does
/// not keep a second copy of those rules.
fn one_row(text: &str) -> String {
    let mut splitter = LineSplitter::new();
    let mut lines = splitter.feed(text.as_bytes());
    lines.extend(splitter.finish());
    lines
        .iter()
        .flat_map(|line| line.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::path::PathBuf;

    use gitscratch::{Conflicts, Stops};

    use super::{running_notice, ConflictsOutcome, WAITING_NOTICE};

    /// A replay that hit no conflict.
    fn clean() -> Conflicts {
        Conflicts::nothing_replayed()
    }

    /// A replay that conflicted in `files`, each with its count of hunks, and
    /// that stopped `stops` times.
    fn conflicted(files: &[(&str, usize)], stops: usize) -> Conflicts {
        Conflicts::from_files(
            files.iter().map(|(name, hunks)| {
                (
                    PathBuf::from(name),
                    NonZeroUsize::new(*hunks).expect("a conflicted file has at least one hunk"),
                )
            }),
            Stops::new(stops),
        )
    }

    /// An outcome of both replays against `main`.
    fn measured(
        rebase: Result<Conflicts, String>,
        merge: Result<Conflicts, String>,
        dirty: bool,
    ) -> ConflictsOutcome {
        ConflictsOutcome::Measured {
            branch: "main".to_owned(),
            rebase,
            merge,
            dirty,
        }
    }

    #[test]
    fn two_clean_replays_say_clean_twice() {
        assert_eq!(
            measured(Ok(clean()), Ok(clean()), false).line(),
            "main: rebase clean · merge clean",
        );
    }

    #[test]
    fn two_conflicted_replays_name_their_counts_in_the_plural() {
        let rebase = conflicted(&[("a.txt", 2), ("b.txt", 1)], 2);
        let merge = conflicted(&[("a.txt", 1)], 1);
        assert_eq!(
            measured(Ok(rebase), Ok(merge), false).line(),
            "main: rebase 3 hunks in 2 files, 2 stops · merge 1 hunk in 1 file",
        );
    }

    #[test]
    fn a_count_of_one_takes_the_singular() {
        let rebase = conflicted(&[("a.txt", 1)], 1);
        assert_eq!(
            measured(Ok(rebase), Ok(clean()), false).line(),
            "main: rebase 1 hunk in 1 file, 1 stop · merge clean",
        );
    }

    /// A merge stops once or never, so its count of stops is a constant and
    /// not a measurement. `grime` leaves it out for that reason. This test
    /// gives the merge a count of stops that a real merge never has, so a line
    /// that names it cannot pass by accident.
    #[test]
    fn the_merge_half_never_names_its_stops() {
        let merge = conflicted(&[("a.txt", 2), ("b.txt", 2)], 2);
        let line = measured(Ok(clean()), Ok(merge), false).line();
        assert_eq!(line, "main: rebase clean · merge 4 hunks in 2 files");
        assert!(
            !line.contains("stop"),
            "the merge half named its stops: {line}"
        );
    }

    #[test]
    fn a_failed_half_shows_its_reason_and_the_other_half_shows_its_result() {
        assert_eq!(
            measured(Ok(clean()), Err("the merge failed".to_owned()), false).line(),
            "main: rebase clean · merge failed: the merge failed",
        );
        assert_eq!(
            measured(
                Err("the rebase failed".to_owned()),
                Ok(conflicted(&[("a.txt", 1)], 1)),
                false,
            )
            .line(),
            "main: rebase failed: the rebase failed · merge 1 hunk in 1 file",
        );
    }

    #[test]
    fn a_dirty_tree_adds_that_uncommitted_work_is_not_included() {
        let rebase = conflicted(&[("a.txt", 2), ("b.txt", 1)], 2);
        assert_eq!(
            measured(Ok(rebase), Ok(clean()), true).line(),
            "main: rebase 3 hunks in 2 files, 2 stops · merge clean · uncommitted work not included",
        );
    }

    #[test]
    fn head_on_the_default_branch_has_nothing_to_compare() {
        let outcome = ConflictsOutcome::OnDefault {
            branch: "main".to_owned(),
        };
        assert_eq!(outcome.line(), "on main — nothing to compare");
    }

    #[test]
    fn a_refusal_names_both_tools_and_the_reason() {
        let outcome = ConflictsOutcome::Refused {
            reason: "no default branch resolves here".to_owned(),
        };
        assert_eq!(
            outcome.line(),
            "grind and grime failed: no default branch resolves here",
        );
    }

    /// The line is one row under the frame. A newline in it pushes the bottom
    /// row of the frame off the screen, and an error from git carries the
    /// streams of git, newlines and all.
    #[test]
    fn a_reason_that_spans_lines_becomes_one_row() {
        let refused = ConflictsOutcome::Refused {
            reason: "no branch was named,\n  and no default\tbranch resolves\n".to_owned(),
        };
        assert_eq!(
            refused.line(),
            "grind and grime failed: no branch was named, and no default branch resolves",
        );

        let failed_half = measured(
            Ok(clean()),
            Err("the merge failed and left nothing to resolve:\n\nfatal: refusing\n".to_owned()),
            false,
        )
        .line();
        assert_eq!(
            failed_half,
            "main: rebase clean · merge failed: the merge failed and left nothing to resolve: \
             fatal: refusing",
        );
        assert!(
            !failed_half.contains('\n'),
            "the line spans rows: {failed_half:?}"
        );
    }

    /// The overlay under the frame cuts a row to the width of the pane, and it
    /// measures that row in display columns. An escape sequence draws in no
    /// column and repaints the frame, so it must not reach the row.
    #[test]
    fn a_reason_holds_no_escape_sequence_or_control_character() {
        let outcome = ConflictsOutcome::Refused {
            reason: "\u{1b}[31mfatal\u{1b}[0m: bad\u{7} revision".to_owned(),
        };
        assert_eq!(
            outcome.line(),
            "grind and grime failed: fatal: bad revision"
        );
    }

    #[test]
    fn the_running_notice_names_both_tools_and_the_branch() {
        assert_eq!(
            running_notice("main"),
            "Running grind and grime against main…"
        );
    }

    #[test]
    fn the_waiting_notice_names_both_tools() {
        assert_eq!(WAITING_NOTICE, "Waiting for grind and grime to finish…");
    }
}

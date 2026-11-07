//! What `bibibi` writes: the progress line and the result.

use crate::step::{SkipReason, Verdict};

/// The progress line of one tested commit, without the line end.
///
/// The line is `bibibi: <short> <verdict> <subject> - about <n> step(s) left`.
/// The verdict is padded to four columns. A skip adds its reason after the
/// subject: `bibibi: <short> skip <subject> - <reason> - about <n> steps left`.
pub fn progress_line(short: &str, verdict: Verdict, subject: &str, steps_left: u32) -> String {
    let (word, reason) = match verdict {
        Verdict::Good => ("good", String::new()),
        Verdict::Bad => ("bad ", String::new()),
        Verdict::Skip(reason) => ("skip", format!(" - {reason}")),
    };
    let unit = if steps_left == 1 { "step" } else { "steps" };
    format!("bibibi: {short} {word} {subject}{reason} - about {steps_left} {unit} left")
}

/// The result line: the full id and the subject of the first bad commit.
pub fn result_line(id: &str, subject: &str) -> String {
    format!("{id} {subject}")
}

/// The result line of one candidate, when skips leave more than one.
///
/// `skipped` is the reason for a skipped commit, or `None` for the bad commit.
pub fn candidate_line(id: &str, subject: &str, skipped: Option<SkipReason>) -> String {
    match skipped {
        Some(reason) => format!("{id} {subject} - skipped: {reason}"),
        None => format!("{id} {subject} - bad"),
    }
}

/// The line on standard error that says how many candidates the skips leave.
pub fn skips_leave_line(candidates: usize) -> String {
    format!("bibibi: skips leave {candidates} candidates for the first bad commit")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_good_line_names_the_commit_the_verdict_and_the_steps() {
        assert_eq!(
            progress_line("abc1234", Verdict::Good, "commit 3", 4),
            "bibibi: abc1234 good commit 3 - about 4 steps left"
        );
    }

    #[test]
    fn a_bad_line_pads_the_verdict_to_four_columns() {
        assert_eq!(
            progress_line("abc1234", Verdict::Bad, "commit 8", 3),
            "bibibi: abc1234 bad  commit 8 - about 3 steps left"
        );
    }

    #[test]
    fn one_step_left_is_singular() {
        assert_eq!(
            progress_line("abc1234", Verdict::Good, "commit 5", 1),
            "bibibi: abc1234 good commit 5 - about 1 step left"
        );
    }

    #[test]
    fn two_steps_left_is_plural() {
        assert_eq!(
            progress_line("abc1234", Verdict::Bad, "commit 6", 2),
            "bibibi: abc1234 bad  commit 6 - about 2 steps left"
        );
    }

    #[test]
    fn zero_steps_left_is_plural() {
        assert_eq!(
            progress_line("abc1234", Verdict::Bad, "commit 6", 0),
            "bibibi: abc1234 bad  commit 6 - about 0 steps left"
        );
    }

    #[test]
    fn a_skip_line_names_the_reason() {
        assert_eq!(
            progress_line(
                "abc1234",
                Verdict::Skip(SkipReason::BuildExited(1)),
                "commit 4",
                2
            ),
            "bibibi: abc1234 skip commit 4 - the build exited 1 - about 2 steps left"
        );
    }

    #[test]
    fn each_skip_reason_has_its_text() {
        assert_eq!(
            SkipReason::PatchDoesNotApply.to_string(),
            "the patch does not apply"
        );
        assert_eq!(SkipReason::BuildExited(3).to_string(), "the build exited 3");
        assert_eq!(
            SkipReason::BuildKilled(9).to_string(),
            "a signal (9) stopped the build"
        );
        assert_eq!(SkipReason::TestExited125.to_string(), "the test exited 125");
    }

    #[test]
    fn a_skipped_candidate_line_names_the_reason() {
        assert_eq!(
            candidate_line("abc", "commit 5", Some(SkipReason::BuildExited(1))),
            "abc commit 5 - skipped: the build exited 1"
        );
    }

    #[test]
    fn the_bad_candidate_line_ends_in_bad() {
        assert_eq!(
            candidate_line("abc", "commit 7", None),
            "abc commit 7 - bad"
        );
    }

    #[test]
    fn the_skips_leave_line_counts_the_candidates() {
        assert_eq!(
            skips_leave_line(3),
            "bibibi: skips leave 3 candidates for the first bad commit"
        );
    }
}

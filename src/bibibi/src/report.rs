//! What `bibibi` writes: the progress line and the result.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::step::{SkipReason, Verdict};

/// The progress line of one tested commit, without the line end.
///
/// The line is `bibibi: <short> <verdict> <subject> - about <n> step(s) left`.
/// Its display width is at most `width` columns when the parts other than the
/// subject fit. Only the subject gets shorter, see [`truncate_to_width`].
/// The verdict is padded to four columns. A skip adds its reason after the
/// subject: `bibibi: <short> skip <subject> - <reason> - about <n> steps left`.
pub fn progress_line(
    width: usize,
    short: &str,
    verdict: Verdict,
    subject: &str,
    steps_left: u32,
) -> String {
    let (word, reason) = verdict_parts(verdict);
    let unit = if steps_left == 1 { "step" } else { "steps" };
    let head = format!("bibibi: {short} {word} ");
    let tail = format!("{reason} - about {steps_left} {unit} left");
    let subject = fit_subject(width, &[&head, &tail], subject);
    format!("{head}{subject}{tail}")
}

/// The `…` that ends a shortened subject. It is one column wide.
const ELLIPSIS: char = '…';

/// Give the subject the columns that the fixed parts of a line leave over.
/// The fixed parts alone can be wider than `width`. Then the subject is empty.
fn fit_subject(width: usize, fixed_parts: &[&str], subject: &str) -> String {
    let fixed: usize = fixed_parts.iter().map(|part| part.width()).sum();
    truncate_to_width(subject, width.saturating_sub(fixed))
}

/// The verdict word, padded to four columns, and the skip reason with its
/// leading separator (empty when the commit is not a skip).
fn verdict_parts(verdict: Verdict) -> (&'static str, String) {
    match verdict {
        Verdict::Good => ("good", String::new()),
        Verdict::Bad(_) => ("bad ", String::new()),
        Verdict::Skip(reason) => ("skip", format!(" - {reason}")),
    }
}

/// The pre-flight line of one tested commit, without the line end.
///
/// The line is `bibibi: <short> <verdict> <subject> - pre-flight`. A skip adds
/// its reason after the subject. The width rule is the one of [`progress_line`].
pub fn preflight_line(width: usize, short: &str, verdict: Verdict, subject: &str) -> String {
    let (word, reason) = verdict_parts(verdict);
    let head = format!("bibibi: {short} {word} ");
    let tail = format!("{reason} - pre-flight");
    let subject = fit_subject(width, &[&head, &tail], subject);
    format!("{head}{subject}{tail}")
}

/// Shorten `subject` so that its display width is at most `budget` columns.
///
/// A subject that fits stays as it is. A longer subject loses characters from
/// its end, and the last column holds `…` (one column wide). A character that
/// does not fit completely is dropped, so a two-column character never gets
/// cut in half. A budget of 0 gives an empty string.
pub fn truncate_to_width(subject: &str, budget: usize) -> String {
    if subject.width() <= budget {
        return subject.to_string();
    }
    // The ellipsis takes one column. A budget of 0 leaves no room for it.
    let Some(room) = budget.checked_sub(1) else {
        return String::new();
    };
    let mut kept = String::new();
    let mut used = 0;
    for character in subject.chars() {
        let columns = character.width().unwrap_or(0);
        if used + columns > room {
            break;
        }
        used += columns;
        kept.push(character);
    }
    kept.push(ELLIPSIS);
    kept
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
    use crate::step::TestFailure;
    use unicode_width::UnicodeWidthStr;

    /// A width at which no line needs a cut.
    const WIDE: usize = usize::MAX;

    #[test]
    fn a_good_line_names_the_commit_the_verdict_and_the_steps() {
        assert_eq!(
            progress_line(WIDE, "abc1234", Verdict::Good, "commit 3", 4),
            "bibibi: abc1234 good commit 3 - about 4 steps left"
        );
    }

    #[test]
    fn a_bad_line_pads_the_verdict_to_four_columns() {
        assert_eq!(
            progress_line(
                WIDE,
                "abc1234",
                Verdict::Bad(TestFailure::Exited(1)),
                "commit 8",
                3
            ),
            "bibibi: abc1234 bad  commit 8 - about 3 steps left"
        );
    }

    #[test]
    fn one_step_left_is_singular() {
        assert_eq!(
            progress_line(WIDE, "abc1234", Verdict::Good, "commit 5", 1),
            "bibibi: abc1234 good commit 5 - about 1 step left"
        );
    }

    #[test]
    fn two_steps_left_is_plural() {
        assert_eq!(
            progress_line(
                WIDE,
                "abc1234",
                Verdict::Bad(TestFailure::Exited(1)),
                "commit 6",
                2
            ),
            "bibibi: abc1234 bad  commit 6 - about 2 steps left"
        );
    }

    #[test]
    fn zero_steps_left_is_plural() {
        assert_eq!(
            progress_line(
                WIDE,
                "abc1234",
                Verdict::Bad(TestFailure::Exited(1)),
                "commit 6",
                0
            ),
            "bibibi: abc1234 bad  commit 6 - about 0 steps left"
        );
    }

    #[test]
    fn a_skip_line_names_the_reason() {
        assert_eq!(
            progress_line(
                WIDE,
                "abc1234",
                Verdict::Skip(SkipReason::BuildExited(1)),
                "commit 4",
                2
            ),
            "bibibi: abc1234 skip commit 4 - the build exited 1 - about 2 steps left"
        );
    }

    #[test]
    fn a_preflight_line_for_a_good_commit_ends_in_pre_flight() {
        assert_eq!(
            preflight_line(WIDE, "abc1234", Verdict::Good, "commit 1"),
            "bibibi: abc1234 good commit 1 - pre-flight"
        );
    }

    #[test]
    fn a_preflight_line_for_a_bad_commit_pads_the_verdict() {
        assert_eq!(
            preflight_line(
                WIDE,
                "abc1234",
                Verdict::Bad(TestFailure::Exited(1)),
                "commit 10"
            ),
            "bibibi: abc1234 bad  commit 10 - pre-flight"
        );
    }

    #[test]
    fn a_preflight_line_for_a_skip_names_the_reason() {
        assert_eq!(
            preflight_line(
                WIDE,
                "abc1234",
                Verdict::Skip(SkipReason::PatchDoesNotApply),
                "commit 4"
            ),
            "bibibi: abc1234 skip commit 4 - the patch does not apply - pre-flight"
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

    #[test]
    fn a_subject_that_fits_stays_the_same() {
        assert_eq!(truncate_to_width("commit 3", 8), "commit 3");
        assert_eq!(truncate_to_width("commit 3", 20), "commit 3");
        assert_eq!(truncate_to_width("日本語", 6), "日本語");
    }

    #[test]
    fn japanese_characters_count_two_columns_each() {
        assert_eq!(truncate_to_width("日本語テスト", 5), "日本…");
    }

    #[test]
    fn a_wide_character_that_does_not_fit_is_dropped() {
        assert_eq!(truncate_to_width("日本語", 4), "日…");
    }

    #[test]
    fn an_emoji_counts_two_columns() {
        assert_eq!(truncate_to_width("🎉🎊🎁", 4), "🎉…");
    }

    #[test]
    fn accented_text_cuts_between_characters() {
        assert_eq!(truncate_to_width("café au lait", 6), "café …");
    }

    #[test]
    fn a_budget_of_zero_gives_an_empty_subject() {
        assert_eq!(truncate_to_width("commit 3", 0), "");
    }

    #[test]
    fn a_budget_of_one_gives_only_the_ellipsis() {
        assert_eq!(truncate_to_width("commit 3", 1), "…");
        assert_eq!(truncate_to_width("日本語", 1), "…");
    }

    #[test]
    fn a_progress_line_fits_the_width() {
        let line = progress_line(
            60,
            "abc1234",
            Verdict::Good,
            "commit 3 日本語のテストの長い件名 🎉🎊🎁",
            2,
        );
        assert!(line.width() <= 60, "{line} is {} columns", line.width());
        assert!(line.contains('…'), "{line}");
        assert!(line.ends_with(" - about 2 steps left"), "{line}");
    }

    #[test]
    fn a_preflight_line_fits_the_width() {
        let line = preflight_line(
            40,
            "abc1234",
            Verdict::Good,
            "commit 3 日本語のテストの長い件名 🎉🎊🎁",
        );
        assert!(line.width() <= 40, "{line} is {} columns", line.width());
        assert!(line.contains('…'), "{line}");
        assert!(line.ends_with(" - pre-flight"), "{line}");
    }
}

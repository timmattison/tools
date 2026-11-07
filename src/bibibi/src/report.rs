//! What `bibibi` writes: the progress line and the result.

use crate::step::Verdict;

/// The progress line of one tested commit, without the line end.
///
/// The line is `bibibi: <short> <verdict> <subject> - about <n> step(s) left`.
/// The verdict is padded to four columns.
pub fn progress_line(_short: &str, _verdict: Verdict, _subject: &str, _steps_left: u32) -> String {
    String::new()
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
}

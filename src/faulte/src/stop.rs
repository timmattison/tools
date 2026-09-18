//! The stop of `faulte kill`: the question that the person answers, the check
//! immediately before each signal, and the sequence of the two signals.
//!
//! A stop is not reversible, and the plan can be minutes old when the person
//! answers. Thus the question and the check are pure functions over plain
//! values, and the sequence reads this Mac through one trait. No unit test
//! signals a real process.

/// The one short answer that confirms.
const SHORT_YES: &str = "y";

/// The one long answer that confirms.
const LONG_YES: &str = "yes";

/// Tells whether `answer` confirms the question `Stop N sessions? [y/N]`.
///
/// Only `y` and `yes` confirm, in any case, after the spaces come off. The
/// issue states that rule, and no flag skips the question. `None` is the end
/// of the input, which is no answer at all.
///
/// The comparison reads ASCII alone. Text of another script that looks like
/// `yes` is not `yes`, and a stop is not reversible.
#[must_use]
pub fn confirms(answer: Option<&str>) -> bool {
    answer.is_some_and(|text| {
        let answer = text.trim();
        answer.eq_ignore_ascii_case(SHORT_YES) || answer.eq_ignore_ascii_case(LONG_YES)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only `y` and `yes` confirm, in any case, after the spaces come off.
    ///
    /// Every other answer stops nothing: the end of the input, no text, a
    /// word that holds `yes` inside it, the letters of `yes` apart, and text
    /// of another script. The issue states this rule, and no flag skips the
    /// question.
    #[test]
    fn only_y_and_yes_confirm() {
        let cases = [
            (Some("y"), true),
            (Some("Y"), true),
            (Some("yes"), true),
            (Some("YES"), true),
            (Some("YeS"), true),
            (Some("  yes  "), true),
            (Some("\ty\n"), true),
            (Some(""), false),
            (Some("   "), false),
            (Some("n"), false),
            (Some("no"), false),
            (Some("N"), false),
            (Some("Y E S"), false),
            (Some("yes please"), false),
            (Some("yep"), false),
            (Some("ok"), false),
            (Some("1"), false),
            (Some("ja"), false),
            (Some("ｙｅｓ"), false),
            (Some("はい"), false),
            (None, false),
        ];

        for (answer, confirmed) in cases {
            assert_eq!(confirms(answer), confirmed, "the answer {answer:?}");
        }
    }
}

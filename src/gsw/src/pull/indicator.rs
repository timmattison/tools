//! The pull-request indicator that ends the header row of watch mode.

use colored::Colorize;
use ghrest::PullNumber;

use super::PullTarget;

/// The text of the indicator when the branch has no open pull request.
pub(crate) const NO_PULL_TEXT: &str = "no PR";

/// The text of the indicator when no answer is known: no lookup finished yet,
/// or the last lookup failed.
pub(crate) const UNKNOWN_TEXT: &str = "PR ?";

/// What the header says about the pull requests of the branch.
///
/// A draft pull request is open, so it counts as open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PullIndicator {
    /// One or more open pull requests. `lowest` is the lowest number, and
    /// `others` counts the other open pull requests.
    Open { lowest: PullNumber, others: usize },
    /// The lookup found no open pull request.
    NoPull,
    /// No answer yet, or the last lookup failed.
    Unknown,
}

/// Builds the indicator from the numbers of the open pull requests.
///
/// The input has any order and may repeat a number. No numbers give
/// [`PullIndicator::NoPull`]. Otherwise the result holds the lowest number and
/// the count of the other distinct numbers.
pub(crate) fn from_numbers(numbers: &[PullNumber]) -> PullIndicator {
    let _ = numbers;
    PullIndicator::Unknown
}

impl PullIndicator {
    /// The plain text of the indicator: `PR #690`, `PR #690 +1`, `no PR`, or `PR ?`.
    pub(crate) fn text(&self) -> String {
        String::new()
    }

    /// The text of the indicator with its style: green and bold for an open
    /// pull request, dim for the other two states.
    pub(crate) fn paint(&self) -> String {
        self.text().normal().to_string()
    }
}

/// What the header of the watch frame shows about the pull request.
///
/// The watch loop sets it. A walk in watch mode gives the target, and the loop
/// puts in the indicator from its cache before each render. `None` on the
/// snapshot means that the indicator does not apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PullHeader {
    /// The pull request that the indicator asks about.
    pub(crate) target: PullTarget,
    /// What the cache says about that target.
    pub(crate) indicator: PullIndicator,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers(raw: &[u64]) -> Vec<PullNumber> {
        raw.iter().map(|&n| PullNumber::new(n)).collect()
    }

    #[test]
    fn no_numbers_give_no_pull() {
        assert_eq!(from_numbers(&[]), PullIndicator::NoPull);
    }

    #[test]
    fn one_number_gives_an_open_indicator_with_no_others() {
        assert_eq!(
            from_numbers(&numbers(&[690])),
            PullIndicator::Open {
                lowest: PullNumber::new(690),
                others: 0
            }
        );
    }

    #[test]
    fn unsorted_numbers_with_a_duplicate_give_the_lowest_and_the_distinct_others() {
        assert_eq!(
            from_numbers(&numbers(&[700, 690, 700])),
            PullIndicator::Open {
                lowest: PullNumber::new(690),
                others: 1
            }
        );
    }

    #[test]
    fn thirteen_numbers_give_twelve_others() {
        let raw: Vec<u64> = (0..13).rev().map(|n| 690 + n).collect();
        assert_eq!(
            from_numbers(&numbers(&raw)),
            PullIndicator::Open {
                lowest: PullNumber::new(690),
                others: 12
            }
        );
    }

    #[test]
    fn the_text_of_each_state_is_fixed() {
        let open = |others| PullIndicator::Open {
            lowest: PullNumber::new(690),
            others,
        };
        assert_eq!(open(0).text(), "PR #690");
        assert_eq!(open(1).text(), "PR #690 +1");
        assert_eq!(open(12).text(), "PR #690 +12");
        assert_eq!(PullIndicator::NoPull.text(), "no PR");
        assert_eq!(PullIndicator::Unknown.text(), "PR ?");
    }

    #[test]
    fn the_paint_of_each_state_is_its_text_in_its_style() {
        let open = PullIndicator::Open {
            lowest: PullNumber::new(690),
            others: 1,
        };
        testcolor::with_forced_ansi(|| {
            assert_eq!(open.paint(), "PR #690 +1".green().bold().to_string());
            assert_eq!(PullIndicator::NoPull.paint(), "no PR".dimmed().to_string());
            assert_eq!(PullIndicator::Unknown.paint(), "PR ?".dimmed().to_string());
        });
    }
}

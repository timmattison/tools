//! Reading a chain of issue numbers out of one line of text.
//!
//! A chain is written by hand, and it is written in more than one way. The
//! arrow is `→` one day and `->` the next, the number carries a `#` or it does
//! not, and a fork in the plan is written `∥`. The chain says one thing
//! whichever way it is written: do these issues in this order.
//!
//! So this module reads the numbers and it drops the punctuation between them.
//! It does not read `∥` as "at the same time", because the plan the tool is
//! given is a plan the reader already decided to walk in order.
//!
//! # What it refuses
//!
//! A forgiving reader is not a silent one. A token that is not a number is an
//! error that names the token, rather than a number this module quietly did
//! not find. `#277 an #278` is a typo, and a reader that answers it with the
//! two numbers hides the word in the middle.

use std::fmt;

use thiserror::Error;

/// The characters that stand between two issues of a chain.
///
/// Each of them means the same thing here: the issue on the left comes before
/// the issue on the right. The list holds every arrow and every bar the plans
/// of this repository are written with, plus the comma and the semicolon that
/// a hand-typed list falls back on. Whitespace separates as well, and it is
/// not in the list because [`char::is_whitespace`] already names it.
const SEPARATORS: &[char] = &[
    '\u{2192}', // → RIGHTWARDS ARROW
    '\u{27f6}', // ⟶ LONG RIGHTWARDS ARROW
    '\u{21d2}', // ⇒ RIGHTWARDS DOUBLE ARROW
    '\u{279c}', // ➜ HEAVY ROUND-TIPPED RIGHTWARDS ARROW
    '\u{2794}', // ➔ HEAVY WIDE-HEADED RIGHTWARDS ARROW
    '\u{2225}', // ∥ PARALLEL TO
    '\u{2016}', // ‖ DOUBLE VERTICAL LINE
    '|',        // the ASCII spelling of the bar, doubled or not
    '>',        // the head of the ASCII arrow `->`
    '-',        // the tail of the ASCII arrow `->`
    ',', ';',
];

/// The character that marks a number as an issue number.
const HASH: char = '#';

/// The number of an issue or a pull request, as GitHub numbers them.
///
/// A newtype rather than a `u64`, so a number that reached the GitHub API can
/// never be a width, a count, or an index by accident. It also holds the one
/// rule GitHub states about the value: numbering starts at one, so zero is not
/// a number [`new`](IssueNumber::new) gives back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IssueNumber(u64);

impl IssueNumber {
    /// The issue number `value` names, or `None` when `value` is zero.
    #[must_use]
    pub fn new(value: u64) -> Option<Self> {
        (value != 0).then_some(Self(value))
    }

    /// The bare number, without the `#`. This is what a command line wants:
    /// `si 278` takes the number and not the mark.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for IssueNumber {
    /// Writes the number the way a plan writes it, with the `#`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{HASH}{}", self.0)
    }
}

/// Why a line of text is not a chain.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ChainError {
    /// The text holds no issue number at all.
    #[error("no issue number found in {0:?}")]
    NoIssues(String),
    /// The text holds a token that is not an issue number.
    #[error("{0:?} is not an issue number")]
    NotAnIssue(String),
}

/// Read the issue numbers of `input`, in the order they are written.
///
/// A number that appears more than once is kept at its first position and
/// dropped after that, so a chain that names one issue twice checks it once.
///
/// # Errors
///
/// Gives [`ChainError::NotAnIssue`] for a token that is not `#` followed by
/// digits, and [`ChainError::NoIssues`] for text that holds no token at all.
pub fn parse_chain(input: &str) -> Result<Vec<IssueNumber>, ChainError> {
    let mut numbers: Vec<IssueNumber> = Vec::new();
    for token in tokens(input) {
        let number = read_number(&token).ok_or(ChainError::NotAnIssue(token))?;
        if !numbers.contains(&number) {
            numbers.push(number);
        }
    }
    if numbers.is_empty() {
        return Err(ChainError::NoIssues(input.to_string()));
    }
    Ok(numbers)
}

/// Cut `input` into the pieces that each name one issue.
///
/// A separator ends a token, and so does a `#` that arrives while a token is
/// already open: `#1#2` is a chain of two written with no separator at all.
/// Every other character stays in the token it arrived in, which is what makes
/// `v2` a token the caller refuses rather than a `2` it silently reads.
fn tokens(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in input.chars() {
        if c.is_whitespace() || SEPARATORS.contains(&c) {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            if c == HASH && !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            current.push(c);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// The issue number `token` names, or `None` when it names none.
fn read_number(token: &str) -> Option<IssueNumber> {
    let digits = token.strip_prefix(HASH).unwrap_or(token);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    IssueNumber::new(digits.parse().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers(input: &str) -> Vec<u64> {
        parse_chain(input)
            .expect("the test input is a chain")
            .iter()
            .map(|n| n.get())
            .collect()
    }

    #[test]
    fn reads_the_arrows_and_the_bars_of_a_written_plan() {
        assert_eq!(
            numbers(" #277 → #278 ∥ #279 → #280 → #281 → #282"),
            vec![277, 278, 279, 280, 281, 282]
        );
        assert_eq!(
            numbers("#230 → #315 → #316 → #317"),
            vec![230, 315, 316, 317]
        );
    }

    #[test]
    fn a_double_bar_is_an_arrow() {
        // The tool is given a plan the reader walks in order, so the bar that
        // means "these can run at the same time" means "then" here.
        assert_eq!(numbers("#1 ∥ #2"), numbers("#1 → #2"));
        assert_eq!(numbers("#1 ‖ #2"), vec![1, 2]);
        assert_eq!(numbers("#1 || #2"), vec![1, 2]);
    }

    #[test]
    fn reads_the_ascii_spellings_and_the_bare_numbers() {
        assert_eq!(numbers("#1 -> #2 -> #3"), vec![1, 2, 3]);
        assert_eq!(numbers("1, 2; 3"), vec![1, 2, 3]);
        assert_eq!(numbers("#1#2"), vec![1, 2]);
        assert_eq!(numbers("277→278"), vec![277, 278]);
    }

    #[test]
    fn keeps_a_repeated_issue_at_its_first_place() {
        assert_eq!(numbers("#5 → #6 → #5 → #7"), vec![5, 6, 7]);
    }

    #[test]
    fn refuses_a_token_that_is_not_a_number() {
        assert_eq!(
            parse_chain("#277 an #278"),
            Err(ChainError::NotAnIssue("an".to_string()))
        );
        assert_eq!(
            parse_chain("#277 v2"),
            Err(ChainError::NotAnIssue("v2".to_string()))
        );
    }

    #[test]
    fn refuses_a_number_that_names_no_issue() {
        // GitHub numbers from one, and a number too large for a u64 is a typo
        // rather than an issue.
        assert_eq!(
            parse_chain("#0"),
            Err(ChainError::NotAnIssue("#0".to_string()))
        );
        assert_eq!(
            parse_chain("#99999999999999999999999"),
            Err(ChainError::NotAnIssue(
                "#99999999999999999999999".to_string()
            ))
        );
    }

    #[test]
    fn refuses_text_that_holds_no_number() {
        assert_eq!(parse_chain(""), Err(ChainError::NoIssues(String::new())));
        assert_eq!(
            parse_chain("   →  "),
            Err(ChainError::NoIssues("   →  ".to_string()))
        );
    }

    #[test]
    fn an_issue_number_writes_itself_with_the_hash() {
        let number = IssueNumber::new(278).expect("278 is an issue number");
        assert_eq!(number.to_string(), "#278");
        assert_eq!(number.get(), 278);
        assert_eq!(IssueNumber::new(0), None);
    }
}

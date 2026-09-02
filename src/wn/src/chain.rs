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

/// The characters of a text an error message repeats back.
///
/// A clipboard holds a page of prose as easily as it holds a chain, and an
/// error that repeats the whole page hides its own last line. Sixty characters
/// is enough for the reader to recognize what was copied.
pub const SNIPPET_CHARS: usize = 60;

/// Text that an error message repeats back.
#[derive(Clone, PartialEq, Eq)]
pub struct Snippet(String);

impl Snippet {
    /// The snippet of `text`, with the space around it dropped.
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self(text.trim().to_string())
    }
}

impl fmt::Display for Snippet {
    /// Writes the text, with nothing around it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Snippet {
    /// Writes the text as a quoted string, the way a [`String`] writes itself.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

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
    /// the command that starts the work takes the number and not the mark.
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
    NoIssues(Snippet),
    /// The text holds a token that is not an issue number.
    #[error("{0:?} is not an issue number")]
    NotAnIssue(Snippet),
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
        let number =
            read_number(&token).ok_or_else(|| ChainError::NotAnIssue(Snippet::new(&token)))?;
        if !numbers.contains(&number) {
            numbers.push(number);
        }
    }
    if numbers.is_empty() {
        return Err(ChainError::NoIssues(Snippet::new(input)));
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
            Err(ChainError::NotAnIssue(Snippet::new("an")))
        );
        assert_eq!(
            parse_chain("#277 v2"),
            Err(ChainError::NotAnIssue(Snippet::new("v2")))
        );
    }

    #[test]
    fn refuses_a_number_that_names_no_issue() {
        // GitHub numbers from one, and a number too large for a u64 is a typo
        // rather than an issue.
        assert_eq!(
            parse_chain("#0"),
            Err(ChainError::NotAnIssue(Snippet::new("#0")))
        );
        assert_eq!(
            parse_chain("#99999999999999999999999"),
            Err(ChainError::NotAnIssue(Snippet::new(
                "#99999999999999999999999"
            )))
        );
    }

    #[test]
    fn refuses_text_that_holds_no_number() {
        assert_eq!(parse_chain(""), Err(ChainError::NoIssues(Snippet::new(""))));
        assert_eq!(
            parse_chain("   →  "),
            Err(ChainError::NoIssues(Snippet::new("   →  ")))
        );
    }

    #[test]
    fn a_token_with_no_separator_in_it_is_cut_in_the_message() {
        // The clipboard of a reader holds a URL, a token, or a password as
        // easily as it holds prose, and none of those hold a separator. A
        // message that repeats the whole of one hides its own last line.
        let token = "a".repeat(200);
        let message = parse_chain(&token)
            .expect_err("one long word is not a chain")
            .to_string();
        let cut: String = token.chars().take(SNIPPET_CHARS).collect();
        assert!(!message.contains(&token), "{message}");
        assert!(message.contains(&format!("\"{cut}…\"")), "{message}");
    }

    #[test]
    fn text_that_holds_no_number_at_all_is_cut_in_the_message() {
        let arrows = "→ ".repeat(100);
        let message = parse_chain(&arrows)
            .expect_err("arrows alone are not a chain")
            .to_string();
        let cut: String = arrows.trim().chars().take(SNIPPET_CHARS).collect();
        assert!(!message.contains(arrows.trim()), "{message}");
        assert!(message.contains(&format!("\"{cut}…\"")), "{message}");
    }

    #[test]
    fn a_token_of_multi_byte_characters_is_cut_by_characters() {
        // A cut through the middle of a multi-byte character panics, and the
        // clipboard of a person who reads Japanese holds Japanese.
        let token = "日本語🎉café".repeat(20);
        let message = parse_chain(&token)
            .expect_err("one long word is not a chain")
            .to_string();
        let cut: String = token.chars().take(SNIPPET_CHARS).collect();
        assert_eq!(cut.chars().count(), SNIPPET_CHARS);
        assert!(message.contains(&format!("\"{cut}…\"")), "{message}");
    }

    #[test]
    fn a_short_token_arrives_whole_and_carries_no_mark() {
        assert_eq!(
            parse_chain("#277 an #278")
                .expect_err("the word is not an issue number")
                .to_string(),
            "\"an\" is not an issue number"
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

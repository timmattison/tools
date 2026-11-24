//! The words of a search and the first match in a session.
//!
//! Matching is literal and not fuzzy. A fuzzy subsequence match over 175 MB of
//! text matches almost every session, so it does not help a person find one.
//!
//! Smart case applies to each word on its own. A word with an uppercase
//! character matches its exact case. Each other word matches any case.

use std::ops::Range;

use regex::{Regex, RegexBuilder};

use crate::session::Session;

/// The words of a search. Each word must occur in a session.
#[derive(Debug, Clone)]
pub struct Query {
    words: Vec<Regex>,
}

/// One match inside one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// The index of the message.
    pub message: usize,
    /// The byte range of the match inside the text of that message.
    pub range: Range<usize>,
}

/// The result of a search in one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// The query is empty, so each session matches.
    Any,
    /// The earliest match of any word.
    At(Match),
}

impl Query {
    /// Split `text` on whitespace into words.
    ///
    /// A word is literal. A word with an uppercase character matches its exact
    /// case. Each other word matches any case. A blank text gives an empty query.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        Query {
            words: text.split_whitespace().filter_map(word_regex).collect(),
        }
    }

    /// True when the query has no word.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Find the earliest match of any word, when each word occurs.
    ///
    /// An empty query gives `Found::Any`. Each word may occur in a different
    /// message. The range is relative to the start of the message, so
    /// `&session.message_text(m.message)[m.range]` is the matched text.
    #[must_use]
    pub fn find(&self, session: &Session) -> Option<Found> {
        if self.words.is_empty() {
            return Some(Found::Any);
        }
        let mut earliest: Option<Range<usize>> = None;
        for word in &self.words {
            let hit = word.find(&session.text)?;
            if earliest
                .as_ref()
                .is_none_or(|best| hit.start() < best.start)
            {
                earliest = Some(hit.range());
            }
        }
        let range = earliest?;
        let after = session
            .messages
            .partition_point(|span| span.start <= range.start);
        let message = after.checked_sub(1)?;
        let span = session.messages.get(message)?;
        Some(Found::At(Match {
            message,
            range: range.start - span.start..range.end - span.start,
        }))
    }

    /// Each occurrence of each word in `text`, sorted by start.
    ///
    /// Ranges that overlap or touch become one range.
    #[must_use]
    pub fn ranges_in(&self, text: &str) -> Vec<Range<usize>> {
        let mut all: Vec<Range<usize>> = self
            .words
            .iter()
            .flat_map(|word| word.find_iter(text).map(|hit| hit.range()))
            .collect();
        all.sort_by_key(|range| (range.start, range.end));
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(all.len());
        for range in all {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        merged
    }
}

/// Regex that matches nothing. A word that fails to build uses it.
const NEVER: &str = r"\b\B";

/// Build the regex of one word. A build error is not possible for an escaped
/// literal. If one occurs, the word matches nothing.
fn word_regex(word: &str) -> Option<Regex> {
    let exact = word.chars().any(char::is_uppercase);
    RegexBuilder::new(&regex::escape(word))
        .case_insensitive(!exact)
        .build()
        .ok()
        .or_else(|| Regex::new(NEVER).ok())
}

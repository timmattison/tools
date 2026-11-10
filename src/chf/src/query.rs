//! The words of a search and the first match in a session.
//!
//! Matching is literal and not fuzzy. A fuzzy subsequence match over 175 MB of
//! text matches almost every session, so it does not help a person find one.
//!
//! Smart case applies to each word on its own. A word with an uppercase
//! character matches its exact case. Each other word matches any case.

use std::ops::Range;

use regex::Regex;

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
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let _ = text;
        Query { words: Vec::new() }
    }

    /// True when the query has no word.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Find the earliest match of any word, when each word occurs.
    #[must_use]
    pub fn find(&self, session: &Session) -> Option<Found> {
        let _ = session;
        None
    }

    /// Each occurrence of each word in `text`, sorted, with overlaps merged.
    #[must_use]
    pub fn ranges_in(&self, text: &str) -> Vec<Range<usize>> {
        let _ = text;
        Vec::new()
    }
}

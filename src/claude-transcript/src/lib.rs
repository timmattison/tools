//! The one definition of the facts that `crap` and `chf` share about Claude
//! Code transcripts.
//!
//! Both tools must agree on what a session id is, on where the store lives,
//! and on what a compaction is. This crate holds each rule once, so that the
//! two definitions cannot drift apart.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// The rule for a session id: a canonical UUID, 8-4-4-4-12 hex digits, either case.
#[must_use]
pub fn is_valid_session_id(_id: &str) -> bool {
    false
}

/// A session id that obeys the rule. The field is private.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(String);

impl SessionId {
    /// Check `id` against the rule. Give `None` when it fails.
    #[must_use]
    pub fn parse(_id: &str) -> Option<Self> {
        None
    }

    /// `<uuid>.jsonl` gives the id. Each other name gives `None`.
    #[must_use]
    pub fn from_file_name(_name: &OsStr) -> Option<Self> {
        None
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        ""
    }
}

impl TryFrom<String> for SessionId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Err(value)
    }
}

impl From<SessionId> for String {
    fn from(_id: SessionId) -> Self {
        String::new()
    }
}

/// `<home>/.claude/projects`. The one store location of crap and chf.
#[must_use]
pub fn projects_dir(_home: &Path) -> PathBuf {
    PathBuf::new()
}

/// One line of a transcript that parses as a JSON object. The pairs keep file order.
#[derive(Debug)]
pub struct Record<'a> {
    pairs: Vec<(String, &'a RawValue)>,
}

impl<'a> Record<'a> {
    /// Parse one line. A trailing `\n` or `\r\n` is removed first.
    /// A line that is not UTF-8 or not a JSON object gives `None`.
    #[must_use]
    pub fn parse(_line: &'a [u8]) -> Option<Self> {
        None
    }

    /// The top-level pairs, in file order.
    #[must_use]
    pub fn pairs(&self) -> &[(String, &'a RawValue)] {
        &self.pairs
    }

    /// The raw value of the first pair named `key`.
    #[must_use]
    pub fn raw(&self, _key: &str) -> Option<&'a RawValue> {
        None
    }

    /// The value of `key`, read as `T`. A missing key or a wrong type gives `None`.
    #[must_use]
    pub fn get<T: DeserializeOwned>(&self, _key: &str) -> Option<T> {
        None
    }

    /// Whether the line has a pair named `key`.
    #[must_use]
    pub fn has_key(&self, _key: &str) -> bool {
        false
    }

    /// `subtype` is the string `compact_boundary`.
    #[must_use]
    pub fn is_compact_boundary(&self) -> bool {
        false
    }

    /// `isCompactSummary` is `true`.
    #[must_use]
    pub fn is_compact_summary(&self) -> bool {
        false
    }

    /// `compactMetadata.preTokens`, when it is a u64.
    #[must_use]
    pub fn pre_tokens(&self) -> Option<u64> {
        None
    }

    /// A non-empty string `cwd`. The rule of crap: the first such value of a transcript wins.
    #[must_use]
    pub fn cwd(&self) -> Option<String> {
        None
    }
}

/// What a transcript says about its compactions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionFacts {
    /// At least one compact boundary or one compact summary.
    pub compacted: bool,
    /// The count of distinct uuids of the compact boundaries.
    pub compactions: usize,
    /// `preTokens` of the LAST boundary line in file order. `None` when that line has none.
    pub last_pre_tokens: Option<u64>,
}

/// Collects the facts one record at a time, for a caller that reads each line for its own reason.
#[derive(Debug, Default)]
pub struct CompactionTally {
    facts: CompactionFacts,
}

impl CompactionTally {
    /// Add one record to the tally.
    pub fn observe(&mut self, _record: &Record<'_>) {}

    /// The facts of the records seen so far.
    #[must_use]
    pub fn facts(&self) -> CompactionFacts {
        self.facts
    }
}

/// The facts of a whole transcript. Lines that do not parse are skipped.
#[must_use]
pub fn compaction_facts(_transcript: &[u8]) -> CompactionFacts {
    CompactionFacts::default()
}

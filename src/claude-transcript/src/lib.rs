//! The one definition of the facts that `crap` and `chf` share about Claude
//! Code transcripts.
//!
//! Both tools must agree on what a session id is, on where the store lives,
//! and on what a compaction is. This crate holds each rule once, so that the
//! two definitions cannot drift apart.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::{DeserializeOwned, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;

/// Hyphen positions in a canonical UUID.
const HYPHEN_POSITIONS: [usize; 4] = [8, 13, 18, 23];
/// The length of a canonical UUID.
const UUID_LEN: usize = 36;
/// The extension of a transcript file.
const TRANSCRIPT_EXTENSION: &str = ".jsonl";
/// The folder of the store, below `<home>`.
const CLAUDE_DIR: &str = ".claude";
/// The folder of the transcripts, below the store.
const PROJECTS_DIR: &str = "projects";

const KEY_SUBTYPE: &str = "subtype";
const KEY_IS_COMPACT_SUMMARY: &str = "isCompactSummary";
const KEY_COMPACT_METADATA: &str = "compactMetadata";
const KEY_PRE_TOKENS: &str = "preTokens";
const KEY_UUID: &str = "uuid";
const KEY_CWD: &str = "cwd";
const SUBTYPE_COMPACT_BOUNDARY: &str = "compact_boundary";

/// The rule for a session id: a canonical UUID, 8-4-4-4-12 hex digits, either case.
#[must_use]
pub fn is_valid_session_id(id: &str) -> bool {
    if id.len() != UUID_LEN {
        return false;
    }
    id.bytes().enumerate().all(|(index, byte)| {
        if HYPHEN_POSITIONS.contains(&index) {
            byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

/// A session id that obeys the rule. The field is private.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(String);

impl SessionId {
    /// Check `id` against the rule. Give `None` when it fails.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        is_valid_session_id(id).then(|| Self(id.to_string()))
    }

    /// `<uuid>.jsonl` gives the id. Each other name gives `None`.
    #[must_use]
    pub fn from_file_name(name: &OsStr) -> Option<Self> {
        Self::parse(name.to_str()?.strip_suffix(TRANSCRIPT_EXTENSION)?)
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SessionId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if is_valid_session_id(&value) {
            Ok(Self(value))
        } else {
            Err(format!("{value:?} is not a canonical UUID"))
        }
    }
}

impl From<SessionId> for String {
    fn from(id: SessionId) -> Self {
        id.0
    }
}

/// `<home>/.claude/projects`. The one store location of crap and chf.
#[must_use]
pub fn projects_dir(home: &Path) -> PathBuf {
    home.join(CLAUDE_DIR).join(PROJECTS_DIR)
}

/// The top-level pairs of a JSON object, in file order.
struct Pairs<'a>(Vec<(String, &'a RawValue)>);

impl<'de> Deserialize<'de> for Pairs<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PairsVisitor;

        impl<'de> Visitor<'de> for PairsVisitor {
            type Value = Pairs<'de>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut pairs = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, &'de RawValue>()? {
                    pairs.push((key, value));
                }
                Ok(Pairs(pairs))
            }
        }

        deserializer.deserialize_map(PairsVisitor)
    }
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
    pub fn parse(line: &'a [u8]) -> Option<Self> {
        let text = std::str::from_utf8(line).ok()?;
        let body = text.strip_suffix('\n').unwrap_or(text);
        let body = body.strip_suffix('\r').unwrap_or(body);
        let pairs = serde_json::from_str::<Pairs<'a>>(body).ok()?;
        Some(Self { pairs: pairs.0 })
    }

    /// The top-level pairs, in file order.
    #[must_use]
    pub fn pairs(&self) -> &[(String, &'a RawValue)] {
        &self.pairs
    }

    /// The raw value of the first pair named `key`.
    #[must_use]
    pub fn raw(&self, key: &str) -> Option<&'a RawValue> {
        self.pairs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| *value)
    }

    /// The value of `key`, read as `T`. A missing key or a wrong type gives `None`.
    #[must_use]
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        serde_json::from_str(self.raw(key)?.get()).ok()
    }

    /// Whether the line has a pair named `key`.
    #[must_use]
    pub fn has_key(&self, key: &str) -> bool {
        self.pairs.iter().any(|(name, _)| name == key)
    }

    /// `subtype` is the string `compact_boundary`.
    #[must_use]
    pub fn is_compact_boundary(&self) -> bool {
        self.get::<String>(KEY_SUBTYPE).as_deref() == Some(SUBTYPE_COMPACT_BOUNDARY)
    }

    /// `isCompactSummary` is `true`.
    #[must_use]
    pub fn is_compact_summary(&self) -> bool {
        self.get::<bool>(KEY_IS_COMPACT_SUMMARY) == Some(true)
    }

    /// `compactMetadata.preTokens`, when it is a u64.
    #[must_use]
    pub fn pre_tokens(&self) -> Option<u64> {
        let metadata = self.get::<serde_json::Value>(KEY_COMPACT_METADATA)?;
        metadata.get(KEY_PRE_TOKENS)?.as_u64()
    }

    /// A non-empty string `cwd`. The rule of crap: the first such value of a transcript wins.
    #[must_use]
    pub fn cwd(&self) -> Option<String> {
        self.get::<String>(KEY_CWD).filter(|cwd| !cwd.is_empty())
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
    boundary_uuids: HashSet<String>,
}

impl CompactionTally {
    /// Add one record to the tally.
    pub fn observe(&mut self, record: &Record<'_>) {
        if record.is_compact_boundary() {
            self.facts.compacted = true;
            if let Some(uuid) = record.get::<String>(KEY_UUID) {
                self.boundary_uuids.insert(uuid);
                self.facts.compactions = self.boundary_uuids.len();
            }
            self.facts.last_pre_tokens = record.pre_tokens();
        }
        if record.is_compact_summary() {
            self.facts.compacted = true;
        }
    }

    /// The facts of the records seen so far.
    #[must_use]
    pub fn facts(&self) -> CompactionFacts {
        self.facts
    }
}

/// The facts of a whole transcript. Lines that do not parse are skipped.
#[must_use]
pub fn compaction_facts(transcript: &[u8]) -> CompactionFacts {
    let mut tally = CompactionTally::default();
    for line in transcript.split_inclusive(|byte| *byte == b'\n') {
        if let Some(record) = Record::parse(line) {
            tally.observe(&record);
        }
    }
    tally.facts()
}

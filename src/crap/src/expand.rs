//! Undo the compactions of a Claude Code transcript.
//!
//! `expand_transcript` takes the bytes of a transcript and gives back the
//! bytes of a copy in which each compaction is undone. The module does no file
//! access and reads no environment.

use std::fmt;

/// What `expand_transcript` made of a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpandOutcome {
    /// The transcript has no compact boundary and no compact summary.
    NoCompaction,
    /// The transcript had at least one compaction, and each one is undone.
    Expanded(Expansion),
}

/// A transcript with each compaction undone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    /// The bytes of the expanded transcript.
    pub transcript: Vec<u8>,
    /// The count of distinct compact boundary uuids.
    pub compactions: usize,
    /// `compactMetadata.preTokens` of the LAST boundary line in file order, if it is a u64.
    pub last_pre_tokens: Option<u64>,
}

/// Why a transcript cannot be expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpandError {
    /// A boundary has no string uuid. `line` is 1-based.
    MissingUuid { line: usize },
    /// A boundary has no `logicalParentUuid`.
    MissingLogicalParent { boundary: String },
    /// The `logicalParentUuid` of a boundary names no line.
    UnknownLogicalParent {
        boundary: String,
        logical_parent: String,
    },
    /// A boundary owns no summary.
    NoSummary { boundary: String },
    /// A summary descends from no boundary.
    OrphanSummary { summary: String },
    /// The messages from before a boundary are not in the transcript.
    HistoryMissing { boundary: String },
}

impl fmt::Display for ExpandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingUuid { line } => {
                write!(f, "the compact boundary on line {line} has no uuid")
            }
            Self::MissingLogicalParent { boundary } => {
                write!(f, "compact boundary {boundary} has no logicalParentUuid")
            }
            Self::UnknownLogicalParent {
                boundary,
                logical_parent,
            } => write!(
                f,
                "compact boundary {boundary} names logicalParentUuid {logical_parent}, but no line of the transcript has that uuid"
            ),
            Self::NoSummary { boundary } => {
                write!(f, "compact boundary {boundary} has no compact summary")
            }
            Self::OrphanSummary { summary } => {
                write!(f, "compact summary {summary} has no compact boundary")
            }
            Self::HistoryMissing { boundary } => write!(
                f,
                "the transcript does not hold the messages from before compact boundary {boundary}"
            ),
        }
    }
}

impl std::error::Error for ExpandError {}

/// Undo each compaction of `transcript`. Every `sessionId` of the output is `import_id`.
pub fn expand_transcript(
    _transcript: &[u8],
    _import_id: &str,
) -> Result<ExpandOutcome, ExpandError> {
    Ok(ExpandOutcome::Expanded(Expansion {
        transcript: Vec::new(),
        compactions: 0,
        last_pre_tokens: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const IMPORT_ID: &str = "11111111-2222-4333-8444-555555555555";
    const OLD_SESSION_ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

    fn line(value: &serde_json::Value) -> String {
        format!("{}\n", serde_json::to_string(value).unwrap())
    }

    fn message(uuid: &str, parent: Option<&str>) -> serde_json::Value {
        json!({
            "parentUuid": parent,
            "isSidechain": false,
            "type": "user",
            "message": {"role": "user", "content": uuid},
            "uuid": uuid,
            "timestamp": "2026-11-01T10:00:00.000Z",
            "sessionId": OLD_SESSION_ID,
        })
    }

    #[test]
    fn a_transcript_without_compaction_is_reported_as_such() {
        let transcript = [
            line(&message("m1", None)),
            line(&message("m2", Some("m1"))),
        ]
        .concat();

        let outcome = expand_transcript(transcript.as_bytes(), IMPORT_ID);

        assert_eq!(outcome, Ok(ExpandOutcome::NoCompaction));
    }
}

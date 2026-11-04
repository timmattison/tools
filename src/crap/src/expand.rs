//! Undo the compactions of a Claude Code transcript.
//!
//! `expand_transcript` takes the bytes of a transcript and gives back the
//! bytes of a copy in which each compaction is undone. The module does no file
//! access and reads no environment.

use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

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

const KEY_SUBTYPE: &str = "subtype";
const KEY_IS_COMPACT_SUMMARY: &str = "isCompactSummary";
const SUBTYPE_COMPACT_BOUNDARY: &str = "compact_boundary";

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

/// One line of a transcript: its bytes, and its pairs when it is a JSON object.
struct Line<'a> {
    /// The whole line, terminator included.
    raw: &'a [u8],
    /// The pairs of the line. `None` marks an opaque line.
    pairs: Option<Vec<(String, &'a RawValue)>>,
}

impl<'a> Line<'a> {
    fn parse(raw: &'a [u8]) -> Self {
        let pairs = std::str::from_utf8(raw)
            .ok()
            .and_then(|text| {
                let body = text.strip_suffix('\n').unwrap_or(text);
                let body = body.strip_suffix('\r').unwrap_or(body);
                serde_json::from_str::<Pairs<'_>>(body).ok()
            })
            .map(|pairs| pairs.0);
        Self { raw, pairs }
    }

    fn value<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        let (_, raw) = self.pairs.as_ref()?.iter().find(|(k, _)| k == key)?;
        serde_json::from_str(raw.get()).ok()
    }

    fn is_boundary(&self) -> bool {
        self.value::<String>(KEY_SUBTYPE).as_deref() == Some(SUBTYPE_COMPACT_BOUNDARY)
    }

    fn is_summary(&self) -> bool {
        self.value::<bool>(KEY_IS_COMPACT_SUMMARY) == Some(true)
    }
}

/// Split `transcript` on `\n`. Each line keeps its terminator. An empty last segment is no line.
fn split_lines(transcript: &[u8]) -> Vec<&[u8]> {
    transcript.split_inclusive(|byte| *byte == b'\n').collect()
}

/// Undo each compaction of `transcript`. Every `sessionId` of the output is `import_id`.
pub fn expand_transcript(
    transcript: &[u8],
    _import_id: &str,
) -> Result<ExpandOutcome, ExpandError> {
    let lines: Vec<Line<'_>> = split_lines(transcript)
        .into_iter()
        .map(Line::parse)
        .collect();
    if !lines.iter().any(|l| l.is_boundary() || l.is_summary()) {
        return Ok(ExpandOutcome::NoCompaction);
    }
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
        let transcript = [line(&message("m1", None)), line(&message("m2", Some("m1")))].concat();

        let outcome = expand_transcript(transcript.as_bytes(), IMPORT_ID);

        assert_eq!(outcome, Ok(ExpandOutcome::NoCompaction));
    }

    fn boundary(
        uuid: &str,
        logical_parent: &str,
        pre_tokens: Option<u64>,
        anchor: &str,
    ) -> serde_json::Value {
        let mut metadata = json!({
            "trigger": "manual",
            "postTokens": 21332,
            "preservedSegment": {"headUuid": "m4", "anchorUuid": anchor, "tailUuid": logical_parent},
        });
        if let Some(tokens) = pre_tokens {
            metadata["preTokens"] = json!(tokens);
        }
        json!({
            "parentUuid": null,
            "logicalParentUuid": logical_parent,
            "isSidechain": false,
            "type": "system",
            "subtype": "compact_boundary",
            "content": "Conversation compacted",
            "level": "info",
            "compactMetadata": metadata,
            "uuid": uuid,
            "timestamp": "2026-11-01T11:00:00.000Z",
            "sessionId": OLD_SESSION_ID,
            "version": "2.1.318",
        })
    }

    fn summary(uuid: &str, parent: &str) -> serde_json::Value {
        json!({
            "parentUuid": parent,
            "isSidechain": false,
            "type": "user",
            "message": {"role": "user", "content": "This session is being continued from a previous conversation"},
            "isVisibleInTranscriptOnly": true,
            "isCompactSummary": true,
            "uuid": uuid,
            "timestamp": "2026-11-01T11:00:01.000Z",
            "session_id": OLD_SESSION_ID,
            "sessionId": OLD_SESSION_ID,
            "version": "2.1.318",
        })
    }

    /// Walk the way Claude loads a session: from the last message up to the first.
    fn walk(transcript: &[u8]) -> Vec<String> {
        let values: Vec<serde_json::Value> = std::str::from_utf8(transcript)
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let mut parents = std::collections::HashMap::new();
        let mut start = None;
        for v in &values {
            let Some(uuid) = v["uuid"].as_str() else {
                continue;
            };
            parents.insert(
                uuid.to_string(),
                v["parentUuid"].as_str().map(str::to_string),
            );
            let kind = v["type"].as_str().unwrap_or_default();
            if (kind == "user" || kind == "assistant") && v["isSidechain"] != json!(true) {
                start = Some(uuid.to_string());
            }
        }
        let mut path = Vec::new();
        let mut next = start;
        while let Some(uuid) = next {
            assert!(!path.contains(&uuid), "cycle at {uuid}");
            let parent = parents
                .get(&uuid)
                .unwrap_or_else(|| panic!("unknown uuid {uuid}"));
            next = parent.clone();
            path.push(uuid);
        }
        path
    }

    fn chain(uuids: &[&str]) -> Vec<serde_json::Value> {
        let mut parent = None;
        uuids
            .iter()
            .map(|uuid| {
                let value = message(uuid, parent);
                parent = Some(*uuid);
                value
            })
            .collect()
    }

    fn join(values: &[serde_json::Value]) -> String {
        values.iter().map(line).collect()
    }

    fn expanded(transcript: &str) -> Expansion {
        match expand_transcript(transcript.as_bytes(), IMPORT_ID) {
            Ok(ExpandOutcome::Expanded(expansion)) => expansion,
            other => panic!("expected an expansion, got {other:?}"),
        }
    }

    fn texts(uuids: &[&str]) -> Vec<String> {
        uuids.iter().map(|u| (*u).to_string()).collect()
    }

    #[test]
    fn one_partial_compaction_is_undone() {
        let mut values = chain(&["m1", "m2", "m3", "m4", "m5", "m6"]);
        values.push(boundary("b1", "m6", Some(1000), "s1"));
        values.push(summary("s1", "b1"));
        values.push(message("p1", Some("s1")));
        values.push(message("p2", Some("p1")));

        let output = expanded(&join(&values)).transcript;

        let text = String::from_utf8(output.clone()).unwrap();
        assert!(!text.contains("compact_boundary"));
        assert!(!text.contains("isCompactSummary"));
        assert_eq!(
            walk(&output),
            texts(&["p2", "p1", "m6", "m5", "m4", "m3", "m2", "m1"])
        );
    }
}

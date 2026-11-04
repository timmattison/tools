//! Undo the compactions of a Claude Code transcript.
//!
//! `expand_transcript` takes the bytes of a transcript and gives back the
//! bytes of a copy in which each compaction is undone. The module does no file
//! access and reads no environment.

use std::collections::{HashMap, HashSet};
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

const KEY_UUID: &str = "uuid";
const KEY_PARENT_UUID: &str = "parentUuid";
const KEY_LOGICAL_PARENT_UUID: &str = "logicalParentUuid";
const KEY_TYPE: &str = "type";
const DROPPED_TYPES: [&str; 2] = ["bridge-session", "queue-operation"];
const KEY_SESSION_ID: &str = "sessionId";
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

    fn has_key(&self, key: &str) -> bool {
        self.pairs
            .as_ref()
            .is_some_and(|pairs| pairs.iter().any(|(k, _)| k == key))
    }

    /// A line type that an import does not keep.
    fn is_dropped_type(&self) -> bool {
        self.value::<String>(KEY_TYPE)
            .is_some_and(|kind| DROPPED_TYPES.contains(&kind.as_str()))
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

/// The terminator of a line: `\r\n`, `\n`, or nothing for a torn last line.
fn terminator(raw: &[u8]) -> &[u8] {
    if raw.ends_with(b"\r\n") {
        b"\r\n"
    } else if raw.ends_with(b"\n") {
        b"\n"
    } else {
        b""
    }
}

/// A uuid that compaction removes from the chain, and what replaces it.
struct Removed {
    /// The uuid that a child of the removed uuid walks to next.
    effective_parent: String,
    /// The boundary that owns the removed uuid.
    owner: String,
}

/// Walk from `start` through the removed uuids to the first kept uuid.
fn resolve(start: &str, removed: &HashMap<String, Removed>) -> Result<String, ExpandError> {
    let mut seen = HashSet::new();
    let mut current = start;
    while let Some(entry) = removed.get(current) {
        if !seen.insert(current) {
            let first = &removed[start];
            return Err(ExpandError::HistoryMissing {
                boundary: first.owner.clone(),
            });
        }
        current = &entry.effective_parent;
    }
    Ok(current.to_string())
}

/// The new parent of a kept line, and the boundary to blame if that parent makes a cycle.
struct Relink {
    new_parent: String,
    blame: String,
}

/// Find the boundary that owns a summary: the first boundary on the old parent chain.
fn owner_of_summary(
    summary_parent: Option<&str>,
    parent_of: &HashMap<String, Option<String>>,
    boundary_uuids: &HashSet<String>,
    summary: &str,
) -> Result<String, ExpandError> {
    let orphan = || ExpandError::OrphanSummary {
        summary: summary.to_string(),
    };
    let mut seen = HashSet::new();
    let mut current = summary_parent.ok_or_else(orphan)?;
    loop {
        if boundary_uuids.contains(current) {
            return Ok(current.to_string());
        }
        let next = parent_of.get(current).ok_or_else(orphan)?;
        if !seen.insert(current) {
            return Err(orphan());
        }
        current = next.as_deref().ok_or_else(orphan)?;
    }
}

/// Fail when a relinked line now leads back to itself. The walk is iterative.
fn check_no_cycle(kept: &[(&Line<'_>, Option<Relink>)]) -> Result<(), ExpandError> {
    let mut new_parent: HashMap<String, Option<String>> = HashMap::new();
    for (line, relink) in kept {
        if let Some(uuid) = line.value::<String>(KEY_UUID) {
            let parent = match relink {
                Some(r) => Some(r.new_parent.clone()),
                None => line.value::<String>(KEY_PARENT_UUID),
            };
            new_parent.insert(uuid, parent);
        }
    }
    let mut finished: HashSet<&str> = HashSet::new();
    for (line, relink) in kept {
        let (Some(relink), Some(uuid)) = (relink, line.value::<String>(KEY_UUID)) else {
            continue;
        };
        let mut on_walk: HashSet<&str> = HashSet::new();
        let mut current = new_parent
            .get_key_value(uuid.as_str())
            .map(|(k, _)| k.as_str());
        while let Some(node) = current {
            if finished.contains(node) {
                break;
            }
            if !on_walk.insert(node) {
                return Err(ExpandError::HistoryMissing {
                    boundary: relink.blame.clone(),
                });
            }
            current = new_parent
                .get(node)
                .and_then(Option::as_deref)
                .and_then(|p| new_parent.get_key_value(p))
                .map(|(k, _)| k.as_str());
        }
        finished.extend(on_walk);
    }
    Ok(())
}

/// Re-emit a line. Each `sessionId` becomes `import_id`, and the `parentUuid`
/// becomes the new parent when the line has a relink.
fn rewrite(line: &Line<'_>, relink: Option<&Relink>, import_id: &str, out: &mut Vec<u8>) {
    let Some(pairs) = line.pairs.as_ref() else {
        out.extend_from_slice(line.raw);
        return;
    };
    out.push(b'{');
    for (index, (key, raw_value)) in pairs.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        out.extend_from_slice(json_string(key).as_bytes());
        out.push(b':');
        if key == KEY_SESSION_ID {
            out.extend_from_slice(json_string(import_id).as_bytes());
        } else if let (KEY_PARENT_UUID, Some(relink)) = (key.as_str(), relink) {
            out.extend_from_slice(json_string(&relink.new_parent).as_bytes());
        } else {
            out.extend_from_slice(raw_value.get().as_bytes());
        }
    }
    out.push(b'}');
    out.extend_from_slice(terminator(line.raw));
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| String::from("\"\""))
}

/// Undo each compaction of `transcript`. Every `sessionId` of the output is `import_id`.
pub fn expand_transcript(transcript: &[u8], import_id: &str) -> Result<ExpandOutcome, ExpandError> {
    let lines: Vec<Line<'_>> = split_lines(transcript)
        .into_iter()
        .map(Line::parse)
        .collect();
    if !lines.iter().any(|l| l.is_boundary() || l.is_summary()) {
        return Ok(ExpandOutcome::NoCompaction);
    }

    // Step 2: every uuid, and the old parent of each uuid (the last line wins).
    let mut all_uuids = HashSet::new();
    let mut parent_of: HashMap<String, Option<String>> = HashMap::new();
    for line in &lines {
        if let Some(uuid) = line.value::<String>(KEY_UUID) {
            all_uuids.insert(uuid.clone());
            parent_of.insert(uuid, line.value::<String>(KEY_PARENT_UUID));
        }
    }

    // Step 3: each boundary has a uuid and a known logical parent.
    let mut boundary_uuids = HashSet::new();
    let mut boundary_order = Vec::new();
    let mut logical_of: HashMap<String, String> = HashMap::new();
    for (index, line) in lines.iter().enumerate().filter(|(_, l)| l.is_boundary()) {
        let uuid = line
            .value::<String>(KEY_UUID)
            .ok_or(ExpandError::MissingUuid { line: index + 1 })?;
        let logical = line
            .value::<String>(KEY_LOGICAL_PARENT_UUID)
            .ok_or_else(|| ExpandError::MissingLogicalParent {
                boundary: uuid.clone(),
            })?;
        if !all_uuids.contains(&logical) {
            return Err(ExpandError::UnknownLogicalParent {
                boundary: uuid,
                logical_parent: logical,
            });
        }
        if boundary_uuids.insert(uuid.clone()) {
            boundary_order.push(uuid.clone());
        }
        logical_of.insert(uuid, logical);
    }

    // Step 4: each summary has an owner.
    let mut removed: HashMap<String, Removed> = HashMap::new();
    let mut owned_boundaries = HashSet::new();
    for line in lines.iter().filter(|l| l.is_summary()) {
        let uuid = line.value::<String>(KEY_UUID);
        let parent = line.value::<String>(KEY_PARENT_UUID);
        let name = uuid.clone().unwrap_or_default();
        let owner = owner_of_summary(parent.as_deref(), &parent_of, &boundary_uuids, &name)?;
        owned_boundaries.insert(owner.clone());
        if let (Some(uuid), Some(parent)) = (uuid, parent) {
            removed.insert(
                uuid,
                Removed {
                    effective_parent: parent,
                    owner,
                },
            );
        }
    }

    // Step 5: each boundary owns a summary.
    for boundary in &boundary_order {
        if !owned_boundaries.contains(boundary) {
            return Err(ExpandError::NoSummary {
                boundary: boundary.clone(),
            });
        }
    }

    // Step 6: the boundaries are removed too.
    for (boundary, logical) in &logical_of {
        removed.insert(
            boundary.clone(),
            Removed {
                effective_parent: logical.clone(),
                owner: boundary.clone(),
            },
        );
    }

    // Step 7: relink each kept line whose parent is removed.
    let mut kept: Vec<(&Line<'_>, Option<Relink>)> = Vec::new();
    for line in lines
        .iter()
        .filter(|l| !l.is_boundary() && !l.is_summary() && !l.is_dropped_type())
    {
        let relink = match line.value::<String>(KEY_PARENT_UUID) {
            Some(parent) if removed.contains_key(&parent) => Some(Relink {
                new_parent: resolve(&parent, &removed)?,
                blame: removed[&parent].owner.clone(),
            }),
            _ => None,
        };
        kept.push((line, relink));
    }

    // Step 8: the relink makes no cycle.
    check_no_cycle(&kept)?;

    // Step 9: write the kept lines.
    let mut output = Vec::with_capacity(transcript.len());
    for (line, relink) in &kept {
        if relink.is_some() || line.has_key(KEY_SESSION_ID) {
            rewrite(line, relink.as_ref(), import_id, &mut output);
        } else {
            output.extend_from_slice(line.raw);
        }
    }

    Ok(ExpandOutcome::Expanded(Expansion {
        transcript: output,
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

    #[test]
    fn two_compactions_are_undone() {
        let mut values = chain(&["m1", "m2", "m3"]);
        values.push(boundary("b1", "m3", Some(1000), "s1"));
        values.push(summary("s1", "b1"));
        values.push(message("p1", Some("s1")));
        values.push(message("p2", Some("p1")));
        values.push(boundary("b2", "p2", Some(2000), "s2"));
        values.push(summary("s2", "b2"));
        values.push(message("q1", Some("s2")));

        let output = expanded(&join(&values)).transcript;

        assert_eq!(walk(&output), texts(&["q1", "p2", "p1", "m3", "m2", "m1"]));
    }

    #[test]
    fn a_compaction_right_after_a_compaction_skips_both_summaries() {
        // The second boundary names the first summary as its logical parent.
        // There are no messages between the two summaries, so the walk from q1
        // goes q1 -> s2 -> b2 -> s1 -> b1 -> m3 -> m2 -> m1.
        let mut values = chain(&["m1", "m2", "m3"]);
        values.push(boundary("b1", "m3", Some(1000), "s1"));
        values.push(summary("s1", "b1"));
        values.push(boundary("b2", "s1", Some(2000), "s2"));
        values.push(summary("s2", "b2"));
        values.push(message("q1", Some("s2")));

        let output = expanded(&join(&values)).transcript;

        assert_eq!(walk(&output), texts(&["q1", "m3", "m2", "m1"]));
    }

    #[test]
    fn only_session_id_changes_and_the_key_order_stays() {
        // The first line is written by hand, with keys in an order that is not
        // alphabetical, and with a snake case `session_id` beside `sessionId`.
        let first = format!(
            "{{\"uuid\":\"m1\",\"sessionId\":\"{OLD_SESSION_ID}\",\"parentUuid\":null,\"type\":\"user\",\"session_id\":\"{OLD_SESSION_ID}\",\"cwd\":\"/work/project\"}}\n"
        );
        let mut values = chain(&["m1", "m2"]);
        values.push(boundary("b1", "m2", Some(1000), "s1"));
        values.push(summary("s1", "b1"));
        values.push(message("p1", Some("s1")));
        let mut input = first.clone();
        input.push_str(&join(&values[1..]));

        let output = String::from_utf8(expanded(&input).transcript).unwrap();

        let old_pair = format!("\"sessionId\":\"{OLD_SESSION_ID}\"");
        let new_pair = format!("\"sessionId\":\"{IMPORT_ID}\"");
        let kept_lines: Vec<String> = output.split_inclusive('\n').map(str::to_string).collect();
        assert_eq!(kept_lines.len(), 3);
        // m1: only the sessionId value differs, byte for byte. session_id stays.
        assert_eq!(kept_lines[0], first.replace(&old_pair, &new_pair));
        assert!(kept_lines[0].contains(&format!("\"session_id\":\"{OLD_SESSION_ID}\"")));
        // p1: the sessionId and the parentUuid differ, and nothing else.
        let expected_p1 = line(&values[4])
            .replace(&old_pair, &new_pair)
            .replace("\"parentUuid\":\"s1\"", "\"parentUuid\":\"m2\"");
        assert_eq!(kept_lines[2], expected_p1);
    }

    #[test]
    fn bridge_session_and_queue_operation_lines_are_dropped() {
        let bridge = json!({
            "type": "bridge-session",
            "sessionId": OLD_SESSION_ID,
            "bridgeSessionId": "cse_0123456789",
            "lastSequenceNum": 0,
        });
        let queue = json!({
            "type": "queue-operation",
            "operation": "enqueue",
            "timestamp": "2026-11-01T10:00:02.000Z",
            "sessionId": OLD_SESSION_ID,
        });
        let mut values = chain(&["m1", "m2"]);
        values.push(bridge);
        values.push(boundary("b1", "m2", Some(1000), "s1"));
        values.push(queue);
        values.push(summary("s1", "b1"));
        values.push(message("p1", Some("s1")));

        let output = String::from_utf8(expanded(&join(&values)).transcript).unwrap();

        assert!(!output.contains("bridge-session"));
        assert!(!output.contains("queue-operation"));
        assert_eq!(output.lines().count(), 3);
    }

    #[test]
    fn lines_the_rules_do_not_change_stay_byte_identical() {
        let snapshot = "{\"type\":\"file-history-snapshot\",\"messageId\":\"m1\",\"snapshot\":{ \"trackedFileBackups\": {} },\"isSnapshotUpdate\":false}\n";
        let not_json = "this is not json\n";
        let torn = "{\"type\":\"user\",\"uuid\":\"t1\",\"parentUuid\":\"p1\",\"mess";
        let mut values = chain(&["m1", "m2"]);
        values.push(boundary("b1", "m2", Some(1000), "s1"));
        values.push(summary("s1", "b1"));
        // A rewritten line that ends in \r\n keeps its \r\n.
        let crlf = line(&message("p1", Some("s1"))).replace('\n', "\r\n");
        let mut input = String::new();
        input.push_str(snapshot);
        input.push_str(&join(&values));
        input.push_str(not_json);
        input.push_str(&crlf);
        input.push_str(torn);

        let output = String::from_utf8(expanded(&input).transcript).unwrap();

        assert!(output.starts_with(snapshot));
        assert!(output.contains(&format!("\n{not_json}")));
        assert!(output.ends_with(torn));
        let expected_p1 = crlf
            .replace(OLD_SESSION_ID, IMPORT_ID)
            .replace("\"parentUuid\":\"s1\"", "\"parentUuid\":\"m2\"");
        assert!(output.contains(&expected_p1));
    }
}

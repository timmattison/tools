//! Behavior tests for the public API of `claude-transcript`.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error"
)]

use std::ffi::OsStr;
use std::path::Path;

use claude_transcript::{
    compaction_facts, is_valid_session_id, projects_dir, CompactionFacts, CompactionTally, Record,
    SessionId,
};
use serde_json::{json, Value};

const UUID: &str = "4733ee2a-1ad6-4619-a01a-11840b8e1901";

fn line(value: &Value) -> String {
    format!("{}\n", serde_json::to_string(value).unwrap())
}

fn boundary(uuid: &str, pre_tokens: Option<u64>) -> String {
    let mut value = json!({
        "type": "system",
        "subtype": "compact_boundary",
        "uuid": uuid,
    });
    if let Some(tokens) = pre_tokens {
        value["compactMetadata"] = json!({"trigger": "auto", "preTokens": tokens});
    }
    line(&value)
}

fn summary(uuid: &str) -> String {
    line(&json!({
        "type": "user",
        "isCompactSummary": true,
        "uuid": uuid,
        "message": {"role": "user", "content": "summary"},
    }))
}

fn message(text: &str) -> String {
    line(&json!({"type": "user", "message": {"role": "user", "content": text}}))
}

// ---- compaction facts ----

#[test]
fn a_transcript_with_only_a_boundary_is_compacted() {
    let transcript = [message("hi"), boundary("b1", Some(100))].concat();

    let facts = compaction_facts(transcript.as_bytes());

    assert_eq!(
        facts,
        CompactionFacts {
            compacted: true,
            compactions: 1,
            last_pre_tokens: Some(100),
        }
    );
}

#[test]
fn a_transcript_with_only_a_summary_is_compacted_with_zero_compactions() {
    let transcript = [message("hi"), summary("s1")].concat();

    let facts = compaction_facts(transcript.as_bytes());

    assert_eq!(
        facts,
        CompactionFacts {
            compacted: true,
            compactions: 0,
            last_pre_tokens: None,
        }
    );
}

#[test]
fn a_transcript_with_neither_is_not_compacted() {
    let facts = compaction_facts(message("hi").as_bytes());

    assert_eq!(facts, CompactionFacts::default());
    assert!(!facts.compacted);
}

#[test]
fn a_session_that_only_talks_about_compaction_is_not_compacted() {
    let text = r#"look: {"subtype":"compact_boundary"} and "isCompactSummary":true café 日本語 🎉"#;
    let transcript = message(text);
    assert!(transcript.contains("compact_boundary"));

    let facts = compaction_facts(transcript.as_bytes());

    assert!(!facts.compacted);
    assert_eq!(facts.compactions, 0);
}

#[test]
fn a_line_that_does_not_parse_in_the_middle_is_skipped() {
    let transcript = [
        boundary("b1", Some(1)),
        "this is not json\n".to_string(),
        boundary("b2", Some(2)),
    ]
    .concat();

    let facts = compaction_facts(transcript.as_bytes());

    assert_eq!(facts.compactions, 2);
    assert_eq!(facts.last_pre_tokens, Some(2));
}

#[test]
fn a_torn_last_line_leaves_the_earlier_lines_counted() {
    let mut transcript = boundary("b1", Some(7));
    transcript.push_str(r#"{"type":"system","subtype":"compact_bound"#);

    let facts = compaction_facts(transcript.as_bytes());

    assert!(facts.compacted);
    assert_eq!(facts.compactions, 1);
    assert_eq!(facts.last_pre_tokens, Some(7));
}

#[test]
fn two_boundaries_with_the_same_uuid_count_once() {
    let transcript = [boundary("b1", Some(1)), boundary("b1", Some(2))].concat();

    assert_eq!(compaction_facts(transcript.as_bytes()).compactions, 1);
}

#[test]
fn two_boundaries_with_different_uuids_count_twice() {
    let transcript = [boundary("b1", Some(1)), boundary("b2", Some(2))].concat();

    assert_eq!(compaction_facts(transcript.as_bytes()).compactions, 2);
}

#[test]
fn last_pre_tokens_is_the_value_of_the_last_boundary() {
    let transcript = [
        boundary("b1", Some(10)),
        boundary("b2", Some(20)),
        message("later"),
    ]
    .concat();

    assert_eq!(
        compaction_facts(transcript.as_bytes()).last_pre_tokens,
        Some(20)
    );
}

#[test]
fn last_pre_tokens_is_none_when_the_last_boundary_has_none() {
    let transcript = [boundary("b1", Some(10)), boundary("b2", None)].concat();

    let facts = compaction_facts(transcript.as_bytes());

    assert_eq!(facts.compactions, 2);
    assert_eq!(facts.last_pre_tokens, None);
}

#[test]
fn the_tally_matches_the_whole_transcript_function() {
    let transcript = [
        message("café"),
        boundary("b1", Some(5)),
        summary("s1"),
        boundary("b1", Some(6)),
    ]
    .concat();
    let mut tally = CompactionTally::default();
    for raw in transcript.as_bytes().split_inclusive(|byte| *byte == b'\n') {
        if let Some(record) = Record::parse(raw) {
            tally.observe(&record);
        }
    }

    assert_eq!(tally.facts(), compaction_facts(transcript.as_bytes()));
    assert_eq!(tally.facts().compactions, 1);
    assert_eq!(tally.facts().last_pre_tokens, Some(6));
}

// ---- session id ----

#[test]
fn valid_session_id_accepts_uuid() {
    assert!(is_valid_session_id(UUID));
}

#[test]
fn valid_session_id_rejects_traversal_and_separators() {
    assert!(!is_valid_session_id(""));
    assert!(!is_valid_session_id(".."));
    assert!(!is_valid_session_id("../etc/passwd"));
    assert!(!is_valid_session_id("a/b"));
    assert!(!is_valid_session_id("a\\b"));
}

#[test]
fn valid_session_id_requires_uuid_shape() {
    assert!(!is_valid_session_id("not-a-uuid"));
    assert!(!is_valid_session_id("4733ee2a-1ad6-4619-a01a-11840b8e190")); // too short
    assert!(!is_valid_session_id(
        "4733ee2a-1ad6-4619-a01a-11840b8e19012"
    )); // too long
    assert!(!is_valid_session_id("4733ee2a1ad64619a01a11840b8e1901")); // no hyphens
    assert!(!is_valid_session_id("4733ee2g-1ad6-4619-a01a-11840b8e1901")); // 'g' not hex
    assert!(!is_valid_session_id("4733ee2a 1ad6 4619 a01a 11840b8e1901")); // spaces
    assert!(!is_valid_session_id("4733ee2a-1ad6-4619-a01a-11840b8e190é")); // multi-byte
}

#[test]
fn valid_session_id_accepts_uppercase_hex() {
    assert!(is_valid_session_id("4733EE2A-1AD6-4619-A01A-11840B8E1901"));
}

#[test]
fn parse_agrees_with_the_rule() {
    assert_eq!(SessionId::parse(UUID).unwrap().as_str(), UUID);
    assert!(SessionId::parse("4733EE2A-1AD6-4619-A01A-11840B8E1901").is_some());
    assert!(SessionId::parse("not-a-uuid").is_none());
    assert!(SessionId::parse("").is_none());
}

#[test]
fn from_file_name_accepts_only_uuid_dot_jsonl() {
    let ok = format!("{UUID}.jsonl");
    assert_eq!(
        SessionId::from_file_name(OsStr::new(&ok)).unwrap().as_str(),
        UUID
    );
    for bad in [
        format!("{UUID}.json"),
        UUID.to_string(),
        "not-a-uuid.jsonl".to_string(),
        format!("{UUID}.jsonl.bak"),
        ".jsonl".to_string(),
    ] {
        assert!(
            SessionId::from_file_name(OsStr::new(&bad)).is_none(),
            "{bad} must give None"
        );
    }
}

#[test]
fn a_session_id_serializes_as_a_plain_json_string() {
    let id = SessionId::parse(UUID).unwrap();

    assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{UUID}\""));
    let back: SessionId = serde_json::from_str(&format!("\"{UUID}\"")).unwrap();
    assert_eq!(back, id);
}

#[test]
fn deserializing_an_invalid_session_id_fails() {
    assert!(serde_json::from_str::<SessionId>("\"not-a-uuid\"").is_err());
    assert!(serde_json::from_str::<SessionId>("42").is_err());
}

// ---- store location ----

#[test]
fn projects_dir_is_home_dot_claude_projects() {
    assert_eq!(
        projects_dir(Path::new("/h")),
        Path::new("/h/.claude/projects")
    );
}

// ---- record ----

#[test]
fn record_parse_strips_lf_and_crlf() {
    assert!(Record::parse(b"{\"a\":1}\n").is_some());
    assert!(Record::parse(b"{\"a\":1}\r\n").is_some());
    assert!(Record::parse(b"{\"a\":1}").is_some());
}

#[test]
fn record_parse_rejects_what_is_not_an_object() {
    assert!(Record::parse(b"[1,2]\n").is_none());
    assert!(Record::parse(b"\"text\"\n").is_none());
    assert!(Record::parse(b"{\"a\":\xff}\n").is_none());
    assert!(Record::parse(b"").is_none());
    assert!(Record::parse(b"\n").is_none());
}

#[test]
fn record_pairs_keep_file_order() {
    let record = Record::parse(r#"{"z":1,"a":"é","m":null}"#.as_bytes()).unwrap();

    let keys: Vec<&str> = record.pairs().iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["z", "a", "m"]);
    assert_eq!(record.pairs()[1].1.get(), "\"é\"");
}

#[test]
fn record_accessors_read_keys() {
    let record = Record::parse(r#"{"n":5,"s":"日本語","o":{"x":1}}"#.as_bytes()).unwrap();

    assert!(record.has_key("n"));
    assert!(!record.has_key("missing"));
    assert_eq!(record.raw("o").unwrap().get(), r#"{"x":1}"#);
    assert!(record.raw("missing").is_none());
    assert_eq!(record.get::<u64>("n"), Some(5));
    assert_eq!(record.get::<String>("s").as_deref(), Some("日本語"));
    assert_eq!(record.get::<String>("n"), None);
}

#[test]
fn record_predicates_and_pre_tokens() {
    let b = boundary("b1", Some(42));
    let record = Record::parse(b.as_bytes()).unwrap();
    assert!(record.is_compact_boundary());
    assert!(!record.is_compact_summary());
    assert_eq!(record.pre_tokens(), Some(42));

    let s = summary("s1");
    let record = Record::parse(s.as_bytes()).unwrap();
    assert!(record.is_compact_summary());
    assert!(!record.is_compact_boundary());
    assert_eq!(record.pre_tokens(), None);

    let record = Record::parse(br#"{"isCompactSummary":"true"}"#).unwrap();
    assert!(!record.is_compact_summary());
}

#[test]
fn record_cwd_gives_a_non_empty_string_only() {
    let cwd = |text: &str| Record::parse(text.as_bytes()).unwrap().cwd();

    assert_eq!(
        cwd(r#"{"cwd":"/tmp/café 日本語 🎉"}"#).as_deref(),
        Some("/tmp/café 日本語 🎉")
    );
    assert_eq!(cwd(r#"{"cwd":null}"#), None);
    assert_eq!(cwd(r#"{"cwd":""}"#), None);
    assert_eq!(cwd(r#"{"cwd":5}"#), None);
    assert_eq!(cwd(r#"{"other":"/x"}"#), None);
}

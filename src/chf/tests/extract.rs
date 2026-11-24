//! Behavior tests for `chf::extract`.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error"
)]

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use chf::extract::extract;
use chf::session::{CompactionPoint, Role, Session};
use claude_transcript::{compaction_facts, SessionId};
use serde_json::{json, Value};

const UUID: &str = "4733ee2a-1ad6-4619-a01a-11840b8e1901";

fn line(value: &Value) -> String {
    format!("{}\n", serde_json::to_string(value).unwrap())
}

fn user(content: &Value) -> String {
    line(
        &json!({"type": "user", "timestamp": "2026-01-01T00:00:00Z", "message": {"role": "user", "content": content}}),
    )
}

fn assistant(content: &Value) -> String {
    line(
        &json!({"type": "assistant", "timestamp": "2026-01-01T00:00:01Z", "message": {"role": "assistant", "content": content}}),
    )
}

fn boundary(uuid: &str, pre_tokens: u64) -> String {
    line(
        &json!({"type": "system", "subtype": "compact_boundary", "uuid": uuid,
        "timestamp": "2030-01-01T00:00:00Z", "compactMetadata": {"preTokens": pre_tokens}}),
    )
}

fn mtime() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

fn run(transcript: &str) -> Session {
    extract(
        SessionId::parse(UUID).unwrap(),
        PathBuf::from("/x/session.jsonl"),
        transcript.as_bytes(),
        mtime(),
    )
}

fn texts(session: &Session) -> Vec<&str> {
    (0..session.messages.len())
        .map(|i| session.message_text(i))
        .collect()
}

#[test]
fn user_string_and_assistant_blocks() {
    let t = format!(
        "{}{}",
        user(&json!("hello there")),
        assistant(&json!([
            {"type": "text", "text": "first"},
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "SECRETTOOL"}},
            {"type": "thinking", "thinking": "SECRETTHINK", "signature": "s"},
            {"type": "text", "text": "second"}
        ]))
    );
    let s = run(&t);
    assert_eq!(s.messages.len(), 2);
    assert_eq!(s.messages[0].role, Role::User);
    assert_eq!(s.messages[1].role, Role::Assistant);
    assert_eq!(texts(&s), vec!["hello there", "first\nsecond"]);
    assert!(!s.text.contains("SECRETTOOL"));
    assert!(!s.text.contains("SECRETTHINK"));
    assert_eq!(s.text, "hello there\0first\nsecond");
}

#[test]
fn tool_result_only_gives_no_message() {
    let t = user(&json!([{"type": "tool_result", "tool_use_id": "t1", "content": "big output"}]));
    assert!(run(&t).messages.is_empty());
}

#[test]
fn sidechain_meta_and_summary_give_no_message() {
    let mut t = String::new();
    for key in ["isSidechain", "isMeta", "isCompactSummary"] {
        t.push_str(&line(&json!({"type": "user", key: true,
            "message": {"role": "user", "content": "hidden"}})));
    }
    assert!(run(&t).messages.is_empty());
}

#[test]
fn whitespace_only_text_is_not_kept() {
    let t = format!(
        "{}{}",
        user(&json!("  \n\t ")),
        assistant(&json!([{"type": "text", "text": " "}]))
    );
    assert!(run(&t).messages.is_empty());
}

#[test]
fn other_record_types_give_no_message() {
    let mut t = String::new();
    for kind in ["system", "attachment", "file-history-snapshot"] {
        t.push_str(&line(
            &json!({"type": kind, "message": {"content": "nope"}, "content": "nope"}),
        ));
    }
    assert!(run(&t).messages.is_empty());
}

#[test]
fn titles() {
    let t = format!(
        "{}{}{}",
        line(&json!({"type": "ai-title", "aiTitle": "First title"})),
        user(&json!("x")),
        line(&json!({"type": "ai-title", "aiTitle": "Second title"}))
    );
    let s = run(&t);
    assert_eq!(s.ai_title.as_deref(), Some("Second title"));
    assert_eq!(s.title(), Some("Second title"));

    let t = format!(
        "{}{}",
        user(&json!("  \n")),
        user(&json!("\n  Fix the bug  \nmore lines"))
    );
    assert_eq!(run(&t).title(), Some("Fix the bug"));

    let t = format!("{}{}", assistant(&json!("only assistant")), "");
    assert_eq!(run(&t).title(), None);
    assert_eq!(run("").title(), None);
}

#[test]
fn cwd_first_and_branch_last() {
    let t = [
        json!({"type": "user", "cwd": null, "gitBranch": "one", "message": {"content": "a"}}),
        json!({"type": "user", "cwd": "", "gitBranch": "", "message": {"content": "b"}}),
        json!({"type": "user", "cwd": "/first", "gitBranch": "two", "message": {"content": "c"}}),
        json!({"type": "user", "cwd": "/second", "gitBranch": "", "message": {"content": "d"}}),
    ]
    .iter()
    .map(line)
    .collect::<String>();
    let s = run(&t);
    assert_eq!(s.cwd.as_deref(), Some("/first"));
    assert_eq!(s.git_branch.as_deref(), Some("two"));
}

#[test]
fn last_activity_rules() {
    let t = format!(
        "{}{}",
        user(&json!("a")),
        line(&json!({"type": "system", "timestamp": "2031-01-01T00:00:00Z"}))
    );
    let s = run(&t);
    assert_eq!(s.last_activity.to_rfc3339(), "2026-01-01T00:00:00+00:00");
    let s = run(&assistant(&json!("b")));
    assert_eq!(s.last_activity.to_rfc3339(), "2026-01-01T00:00:01+00:00");

    let s = run(&line(
        &json!({"type": "system", "timestamp": "2031-01-01T00:00:00Z"}),
    ));
    assert_eq!(
        s.last_activity,
        chrono::DateTime::<chrono::Utc>::from(mtime())
    );
    let s = run(&line(
        &json!({"type": "user", "timestamp": "garbage", "message": {"content": "a"}}),
    ));
    assert_eq!(
        s.last_activity,
        chrono::DateTime::<chrono::Utc>::from(mtime())
    );
}

#[test]
fn compaction_facts_and_points() {
    let t = format!(
        "{}{}{}{}{}{}",
        user(&json!("one")),
        assistant(&json!("two")),
        boundary("b1", 1000),
        user(&json!("three")),
        boundary("b2", 2000),
        user(&json!("four")),
    );
    let s = run(&t);
    assert_eq!(s.compaction, compaction_facts(t.as_bytes()));
    assert_eq!(
        s.compaction_points,
        vec![
            CompactionPoint {
                before_message: 2,
                pre_tokens: Some(1000)
            },
            CompactionPoint {
                before_message: 3,
                pre_tokens: Some(2000)
            },
        ]
    );
    assert_eq!(s.last_compaction_before(), Some(3));
    assert_eq!(run(&user(&json!("a"))).last_compaction_before(), None);
}

#[test]
fn torn_last_line_and_garbage_line() {
    let good = format!("{}{}", user(&json!("one")), assistant(&json!("two")));
    let t = format!("{good}this is not json\n{}", user(&json!("three")));
    assert_eq!(texts(&run(&t)), vec!["one", "two", "three"]);
    let torn = format!("{good}{{\"type\":\"user\",\"message\":{{\"content\":\"par");
    assert_eq!(texts(&run(&torn)), vec!["one", "two"]);
}

#[test]
fn multibyte_text_and_spans() {
    let t = format!(
        "{}{}{}",
        user(&json!("café")),
        assistant(&json!("日本語")),
        user(&json!("🎉"))
    );
    let s = run(&t);
    assert_eq!(texts(&s), vec!["café", "日本語", "🎉"]);
    for (i, span) in s.messages.iter().enumerate() {
        assert_eq!(s.text.get(span.start..span.end), Some(s.message_text(i)));
    }
    assert_eq!(s.messages[0].start, 0);
    assert_eq!(s.messages[1].start, "café".len() + 1);
}

#[test]
fn nul_becomes_replacement_character() {
    let s = run(&format!("{}{}", user(&json!("a\u{0}b")), user(&json!("c"))));
    assert_eq!(texts(&s), vec!["a\u{FFFD}b", "c"]);
    assert_eq!(s.text.matches('\0').count(), 1);
}

#[test]
fn serde_round_trip() {
    let t = format!(
        "{}{}{}",
        user(&json!("café")),
        boundary("b1", 5),
        assistant(&json!("x"))
    );
    let s = run(&t);
    let back: Session = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back, s);
}

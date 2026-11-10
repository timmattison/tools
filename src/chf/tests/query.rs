//! Behavior tests for `chf::query`.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error"
)]

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use chf::extract::extract;
use chf::query::{Found, Match, Query};
use chf::session::{MessageSpan, Role, Session};
use claude_transcript::SessionId;

const UUID: &str = "4733ee2a-1ad6-4619-a01a-11840b8e1901";

/// Build a session whose messages are `messages`.
fn session(messages: &[&str]) -> Session {
    let mut base = extract(
        SessionId::parse(UUID).unwrap(),
        PathBuf::from("/x/session.jsonl"),
        b"",
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    let mut text = String::new();
    let mut spans = Vec::new();
    for (i, message) in messages.iter().enumerate() {
        if i > 0 {
            text.push('\0');
        }
        let start = text.len();
        text.push_str(message);
        spans.push(MessageSpan {
            role: Role::User,
            start,
            end: text.len(),
        });
    }
    base.text = text;
    base.messages = spans;
    base
}

fn at(query: &str, messages: &[&str]) -> Option<Found> {
    Query::parse(query).find(&session(messages))
}

fn matched<'a>(query: &str, messages: &'a [&str]) -> &'a str {
    let s = session(messages);
    match Query::parse(query).find(&s) {
        Some(Found::At(m)) => {
            let text = messages[m.message];
            text.get(m.range.clone()).unwrap()
        }
        other => panic!("expected a match, got {other:?}"),
    }
}

#[test]
fn all_words_must_match_in_any_message() {
    assert!(at("alpha beta", &["has alpha", "has beta"]).is_some());
    assert_eq!(at("alpha gamma", &["has alpha", "has beta"]), None);
}

#[test]
fn smart_case_folds_a_lowercase_word() {
    assert!(at("café", &["CAFÉ"]).is_some());
    assert!(at("café", &["Café"]).is_some());
}

#[test]
fn smart_case_keeps_the_case_of_an_uppercase_word() {
    assert!(at("Café", &["Café"]).is_some());
    assert_eq!(at("Café", &["café"]), None);
}

#[test]
fn smart_case_applies_to_each_word() {
    assert_eq!(at("Foo bar", &["foo BAR"]), None);
    assert!(at("Foo bar", &["Foo BAR"]).is_some());
}

#[test]
fn multi_byte_ranges_give_back_the_word() {
    assert_eq!(matched("日本語", &["これは日本語です"]), "日本語");
    assert_eq!(matched("🎉", &["party 🎉 time"]), "🎉");
}

#[test]
fn words_are_literal() {
    assert_eq!(at("a.b", &["axb"]), None);
    assert!(at("a.b", &["a.b"]).is_some());
    assert!(at("(x", &["f(x)"]).is_some());
}

#[test]
fn first_match_is_the_earliest_of_any_word() {
    let messages = ["one two", "three alpha", "beta alpha"];
    let found = at("beta alpha", &messages);
    assert_eq!(
        found,
        Some(Found::At(Match {
            message: 1,
            range: 6..11
        }))
    );
    assert_eq!(matched("beta alpha", &messages), "alpha");
}

#[test]
fn no_match_crosses_two_messages() {
    assert_eq!(at("foo", &["fo", "o"]), None);
}

#[test]
fn empty_and_blank_queries_match_any_session() {
    assert_eq!(at("", &["x"]), Some(Found::Any));
    assert_eq!(at("   ", &["x"]), Some(Found::Any));
    assert_eq!(at("", &[]), Some(Found::Any));
    assert_eq!(at("  \t ", &[]), Some(Found::Any));
    assert!(Query::parse("  ").is_empty());
    assert!(!Query::parse("a").is_empty());
}

#[test]
fn ranges_in_merges_overlaps_and_sorts() {
    let query = Query::parse("ab bc");
    assert_eq!(query.ranges_in("xabcx ab"), vec![1..4, 6..8]);
    let reversed = Query::parse("bc ab");
    assert_eq!(reversed.ranges_in("xabcx ab"), vec![1..4, 6..8]);
}

#[test]
fn ranges_in_of_an_empty_query_is_empty() {
    assert!(Query::parse("").ranges_in("abc").is_empty());
}

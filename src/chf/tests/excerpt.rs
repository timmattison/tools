//! Behavior tests for `chf::excerpt`.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error"
)]

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use chf::excerpt::{preview, thousands, Preview, PreviewLine, Tone};
use chf::extract::extract;
use chf::query::{Found, Match, Query};
use chf::session::{CompactionPoint, MessageSpan, Role, Session};
use claude_transcript::SessionId;

const UUID: &str = "4733ee2a-1ad6-4619-a01a-11840b8e1901";
const FFFD: &str = "\u{FFFD}";
/// A height so large that the view holds the whole session.
const WHOLE: u16 = 60_000;

/// Build a session whose messages are `messages`. Roles alternate from user.
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
            role: if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            },
            start,
            end: text.len(),
        });
    }
    base.text = text;
    base.messages = spans;
    base
}

/// The whole sequence of lines of a session at `width`.
fn all(s: &Session, query: &str, width: u16) -> Vec<PreviewLine> {
    let p = preview(
        s,
        &Query::parse(query),
        &Found::Any,
        width,
        WHOLE,
        -1_000_000,
    );
    for line in &p.lines {
        assert!(line.width() <= usize::from(width), "too wide: {line:?}");
    }
    p.lines
}

fn texts(lines: &[PreviewLine]) -> Vec<String> {
    lines.iter().map(PreviewLine::text).collect()
}

/// The text lines of a one-message session, without the author line.
fn body(message: &str, width: u16) -> Vec<String> {
    let lines = all(&session(&[message]), "", width);
    texts(&lines[1..])
}

fn view(s: &Session, query: &str, found: &Found, height: u16, scroll: i64) -> Preview {
    let p = preview(s, &Query::parse(query), found, 40, height, scroll);
    for line in &p.lines {
        assert!(line.width() <= 40);
    }
    p
}

fn hit(message: usize, start: usize) -> Found {
    Found::At(Match {
        message,
        range: start..start + 1,
    })
}

#[test]
fn author_lines_come_before_each_message() {
    let lines = all(&session(&["hello", "world"]), "", 20);
    assert_eq!(texts(&lines), ["user", "hello", "", "assistant", "world"]);
    assert_eq!(lines[0].0[0].tone, Tone::Author);
    assert_eq!(lines[3].0[0].tone, Tone::Author);
    assert_eq!(lines[1].0[0].tone, Tone::Text);
}

#[test]
fn wraps_after_spaces() {
    let message = "aaaa bbbb cccc dddd eeee";
    let lines = body(message, 10);
    assert!(lines.len() > 1);
    for line in &lines {
        assert!(line.chars().count() <= 10);
    }
    for line in &lines[..lines.len() - 1] {
        assert!(line.ends_with(' '), "{line:?}");
    }
    assert_eq!(lines.concat(), message);
    assert_eq!(lines[0], "aaaa bbbb ");
}

#[test]
fn wraps_before_the_char_when_no_space() {
    assert_eq!(body("abcdefghij", 4), ["abcd", "efgh", "ij"]);
}

#[test]
fn multi_byte_text_wraps_by_columns() {
    let lines = body("日本語テスト", 5);
    assert_eq!(lines.concat(), "日本語テスト");
    assert_eq!(lines, ["日本", "語テ", "スト"]);
    assert_eq!(body("🎉🎉🎉", 3), ["🎉", "🎉", "🎉"]);
    let combined = body("cafe\u{301}", 4);
    assert_eq!(combined, ["cafe\u{301}"]);
    assert_eq!(body("cafe\u{301}", 3), ["caf", "e\u{301}"]);
}

#[test]
fn no_panic_and_lossless_at_each_small_width() {
    for message in ["日本語テスト", "🎉🎉🎉", "cafe\u{301} au lait", "a b  c\td"] {
        for width in 1..=8_u16 {
            let lines = all(&session(&[message]), "au", width);
            assert!(!lines.is_empty());
        }
    }
}

#[test]
fn a_char_wider_than_the_width_becomes_a_replacement() {
    assert_eq!(body("日", 1), [FFFD]);
    assert_eq!(body("a日b", 1), ["a", FFFD, "b"]);
}

#[test]
fn control_characters_are_sanitized() {
    assert_eq!(body("a\tb", 20), ["a    b"]);
    assert_eq!(body("a\r\nb", 20), ["a", "b"]);
    assert_eq!(body("\u{7}", 20), [FFFD]);
    assert_eq!(body("x\ny\n\nz", 20), ["x", "y", "", "z"]);
}

#[test]
fn query_words_are_highlighted() {
    let lines = all(&session(&["say ALPHA now"]), "alpha", 40);
    let pieces: Vec<(String, Tone)> = lines[1]
        .0
        .iter()
        .map(|p| (p.text.clone(), p.tone))
        .collect();
    assert_eq!(
        pieces,
        [
            ("say ".to_owned(), Tone::Text),
            ("ALPHA".to_owned(), Tone::Highlight),
            (" now".to_owned(), Tone::Text),
        ]
    );
}

#[test]
fn a_highlight_survives_a_wrap() {
    let lines = all(&session(&["abcdefgh"]), "cdef", 4);
    assert_eq!(texts(&lines[1..]), ["abcd", "efgh"]);
    let tones = |i: usize| -> Vec<(String, Tone)> {
        lines[i]
            .0
            .iter()
            .map(|p| (p.text.clone(), p.tone))
            .collect()
    };
    assert_eq!(
        tones(1),
        [
            ("ab".to_owned(), Tone::Text),
            ("cd".to_owned(), Tone::Highlight)
        ]
    );
    assert_eq!(
        tones(2),
        [
            ("ef".to_owned(), Tone::Highlight),
            ("gh".to_owned(), Tone::Text)
        ]
    );
}

#[test]
fn a_compaction_point_gives_a_line_between_messages() {
    let mut s = session(&["one", "two"]);
    s.compaction_points = vec![CompactionPoint {
        before_message: 1,
        pre_tokens: Some(259_822),
    }];
    let lines = all(&s, "", 60);
    let t = texts(&lines);
    assert_eq!(t[0], "user");
    assert_eq!(t[1], "one");
    assert_eq!(t[2], "");
    assert!(
        t[3].contains("compaction") && t[3].contains("259,822"),
        "{t:?}"
    );
    assert_eq!(lines[3].0[0].tone, Tone::Compaction);
    assert_eq!(t[4], "assistant");
}

#[test]
fn a_compaction_point_without_tokens_has_no_digit() {
    let mut s = session(&["one", "two"]);
    s.compaction_points = vec![CompactionPoint {
        before_message: 1,
        pre_tokens: None,
    }];
    let t = texts(&all(&s, "", 60));
    assert!(t[3].contains("compaction"));
    assert!(!t[3].chars().any(|c| c.is_ascii_digit()));
}

#[test]
fn a_compaction_point_after_the_last_message_comes_last() {
    let mut s = session(&["one"]);
    s.compaction_points = vec![CompactionPoint {
        before_message: 1,
        pre_tokens: Some(5),
    }];
    let lines = all(&s, "", 60);
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[2].0[0].tone, Tone::Compaction);
}

#[test]
fn a_session_with_no_message_says_so() {
    let lines = all(&session(&[]), "", 40);
    assert_eq!(texts(&lines), ["(no messages)"]);
    assert_eq!(lines[0].0[0].tone, Tone::Muted);
}

fn twenty() -> Session {
    let names: Vec<String> = (0..20).map(|i| format!("m{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    session(&refs)
}

#[test]
fn found_at_puts_the_match_line_third_from_the_top() {
    let s = twenty();
    let p = view(&s, "m10", &hit(10, 0), 10, 0);
    assert_eq!(p.lines.len(), 10);
    assert_eq!(p.lines[3].text(), "m10");
    assert_eq!(p.scroll, 0);
}

#[test]
fn found_at_the_first_message_starts_at_line_zero() {
    let s = twenty();
    let p = view(&s, "m0", &hit(0, 0), 10, 0);
    assert_eq!(p.lines.len(), 10);
    assert_eq!(p.lines[0].text(), "user");
    assert_eq!(p.lines[1].text(), "m0");
}

#[test]
fn found_any_ends_at_the_last_line() {
    let s = twenty();
    let p = view(&s, "", &Found::Any, 5, 0);
    assert_eq!(p.lines.len(), 5);
    assert_eq!(p.lines[4].text(), "m19");
    assert_eq!(p.lines[3].text(), "assistant");
}

#[test]
fn scroll_moves_the_view() {
    let s = twenty();
    let base = view(&s, "m10", &hit(10, 0), 10, 0);
    let down = view(&s, "m10", &hit(10, 0), 10, 2);
    assert_eq!(down.scroll, 2);
    assert_eq!(down.lines[0], base.lines[2]);
    let top = view(&s, "m10", &hit(10, 0), 10, -1000);
    assert_eq!(top.lines[0].text(), "user");
    assert!(top.scroll < 0);
    let all_lines = all(&s, "", 40);
    let end = view(&s, "m10", &hit(10, 0), 10, 1000);
    assert_eq!(end.lines.last(), all_lines.last());
    assert_eq!(end.lines.len(), 10);
    assert!(end.scroll > 0);
}

#[test]
fn scroll_reports_the_clamped_value() {
    let s = twenty();
    let top = view(&s, "m10", &hit(10, 0), 10, -1000);
    // Message 10 text sits at line 31, so the default top is line 28.
    assert_eq!(top.scroll, -28);
}

#[test]
fn a_long_session_finds_the_right_anchor() {
    let names: Vec<String> = (0..10_000).map(|i| format!("m{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let s = session(&refs);
    let p = view(&s, "m5000", &hit(5000, 0), 30, 0);
    assert_eq!(p.lines.len(), 30);
    assert_eq!(p.lines[3].text(), "m5000");
    let end = view(&s, "", &Found::Any, 30, 0);
    assert_eq!(end.lines[29].text(), "m9999");
}

#[test]
fn a_zero_size_gives_an_empty_preview() {
    let s = twenty();
    let q = Query::parse("");
    for (w, h) in [(0, 5), (5, 0)] {
        let p = preview(&s, &q, &Found::Any, w, h, 7);
        assert!(p.lines.is_empty());
        assert_eq!(p.scroll, 0);
    }
}

#[test]
fn thousands_groups_digits() {
    assert_eq!(thousands(0), "0");
    assert_eq!(thousands(999), "999");
    assert_eq!(thousands(1000), "1,000");
    assert_eq!(thousands(259_822), "259,822");
    assert_eq!(thousands(1_234_567), "1,234,567");
    assert_eq!(thousands(u64::MAX), "18,446,744,073,709,551,615");
}

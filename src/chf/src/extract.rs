//! One transcript into one `Session`.

use std::path::PathBuf;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use claude_transcript::{CompactionTally, Record, SessionId};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::session::{CompactionPoint, MessageSpan, Role, Session};

/// The character that divides two messages in `Session::text`.
const MESSAGE_SEPARATOR: char = '\0';
/// The character that takes the place of a `'\0'` inside a message.
const SEPARATOR_REPLACEMENT: char = '\u{FFFD}';
/// The `type` of a record that holds the title of the session.
const TYPE_AI_TITLE: &str = "ai-title";
/// The `type` of a record of the person.
const TYPE_USER: &str = "user";
/// The `type` of a record of the model.
const TYPE_ASSISTANT: &str = "assistant";
/// The `type` of a content block that holds text.
const BLOCK_TEXT: &str = "text";

/// The part of `message` that chf reads. Serde skips every other field.
#[derive(Deserialize)]
struct MessageBody<'a> {
    #[serde(borrow)]
    content: Option<&'a RawValue>,
}

/// One block of an array content. Serde skips the unknown fields.
#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

/// The text of a `message` value, or `None` when it has none.
///
/// This function never builds a `serde_json::Value`, because a tool result can
/// hold megabytes.
fn message_content_text(message: &RawValue) -> Option<String> {
    let body: MessageBody<'_> = serde_json::from_str(message.get()).ok()?;
    let content = body.content?.get();
    match content.trim_start().as_bytes().first()? {
        b'"' => serde_json::from_str::<String>(content).ok(),
        b'[' => {
            let blocks: Vec<Block> = serde_json::from_str(content).ok()?;
            let texts: Vec<String> = blocks
                .into_iter()
                .filter(|block| block.kind == BLOCK_TEXT)
                .filter_map(|block| block.text)
                .collect();
            Some(texts.join("\n"))
        }
        _ => None,
    }
}

/// Build a `Session` from the bytes of a transcript. This function reads no file.
///
/// Lines that do not parse are skipped. A torn last line is such a line.
#[must_use]
pub fn extract(id: SessionId, path: PathBuf, bytes: &[u8], mtime: SystemTime) -> Session {
    let mut tally = CompactionTally::default();
    let mut cwd: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut ai_title: Option<String> = None;
    let mut last_activity: Option<DateTime<Utc>> = None;
    let mut text = String::new();
    let mut messages: Vec<MessageSpan> = Vec::new();
    let mut compaction_points: Vec<CompactionPoint> = Vec::new();

    for line in bytes.split(|byte| *byte == b'\n') {
        let Some(record) = Record::parse(line) else {
            continue;
        };
        tally.observe(&record);
        if record.is_compact_boundary() {
            compaction_points.push(CompactionPoint {
                before_message: messages.len(),
                pre_tokens: record.pre_tokens(),
            });
        }
        if cwd.is_none() {
            cwd = record.cwd();
        }
        if let Some(branch) = record.get::<String>("gitBranch").filter(|b| !b.is_empty()) {
            git_branch = Some(branch);
        }
        let kind = record.get::<String>("type");
        let kind = kind.as_deref();
        if kind == Some(TYPE_AI_TITLE) {
            if let Some(title) = record.get::<String>("aiTitle") {
                ai_title = Some(title);
            }
        }
        let role = match kind {
            Some(TYPE_USER) => Role::User,
            Some(TYPE_ASSISTANT) => Role::Assistant,
            _ => continue,
        };
        if record.get::<bool>("isSidechain") == Some(true) {
            continue;
        }
        if let Some(time) = record
            .get::<String>("timestamp")
            .and_then(|stamp| DateTime::parse_from_rfc3339(&stamp).ok())
        {
            last_activity = Some(time.with_timezone(&Utc));
        }
        if record.get::<bool>("isMeta") == Some(true) || record.is_compact_summary() {
            continue;
        }
        let Some(body) = record.raw("message").and_then(message_content_text) else {
            continue;
        };
        if body.trim().is_empty() {
            continue;
        }
        if !messages.is_empty() {
            text.push(MESSAGE_SEPARATOR);
        }
        let start = text.len();
        text.extend(body.chars().map(|c| {
            if c == MESSAGE_SEPARATOR {
                SEPARATOR_REPLACEMENT
            } else {
                c
            }
        }));
        messages.push(MessageSpan {
            role,
            start,
            end: text.len(),
        });
    }

    Session {
        id,
        path,
        cwd,
        git_branch,
        ai_title,
        last_activity: last_activity.unwrap_or_else(|| DateTime::<Utc>::from(mtime)),
        compaction: tally.facts(),
        text,
        messages,
        compaction_points,
    }
}

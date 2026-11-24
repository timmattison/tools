//! The data of one indexed session.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use claude_transcript::{CompactionFacts, SessionId};
use serde::{Deserialize, Serialize};

/// Who wrote a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// The person.
    User,
    /// The model.
    Assistant,
}

/// The place of one message inside `Session::text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSpan {
    /// Who wrote the message.
    pub role: Role,
    /// The byte offset of the first byte of the message.
    pub start: usize,
    /// The byte offset after the last byte of the message.
    pub end: usize,
}

/// One compaction of the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionPoint {
    /// The count of messages kept before the compact boundary.
    pub before_message: usize,
    /// The token count before the compaction, when the line states it.
    pub pre_tokens: Option<u64>,
}

/// Everything chf keeps about one session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// The id of the session.
    pub id: SessionId,
    /// The path of the transcript file.
    pub path: PathBuf,
    /// The first non-empty working directory of the transcript.
    pub cwd: Option<String>,
    /// The last non-empty git branch of the transcript.
    pub git_branch: Option<String>,
    /// The title from the last `ai-title` record.
    pub ai_title: Option<String>,
    /// The time of the last conversation record, else the file mtime.
    pub last_activity: DateTime<Utc>,
    /// What the transcript says about its compactions.
    pub compaction: CompactionFacts,
    /// The text of each message, joined with `'\0'`.
    pub text: String,
    /// The place of each message in `text`.
    pub messages: Vec<MessageSpan>,
    /// One entry for each compact boundary line, in file order.
    pub compaction_points: Vec<CompactionPoint>,
}

impl Session {
    /// The text of the message at `index`. A bad index gives an empty string.
    ///
    /// The spans sit on char boundaries by construction, so `get` does not fail
    /// for a `Session` that `extract` made.
    #[must_use]
    pub fn message_text(&self, index: usize) -> &str {
        self.messages
            .get(index)
            .and_then(|span| self.text.get(span.start..span.end))
            .unwrap_or("")
    }

    /// `ai_title`, else the first non-empty line of the first user message, else `None`.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        if let Some(title) = self.ai_title.as_deref() {
            return Some(title);
        }
        let index = self.messages.iter().position(|span| {
            span.role == Role::User && !self.message_text_of(span).trim().is_empty()
        })?;
        self.message_text(index)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
    }

    /// `before_message` of the last compaction point.
    #[must_use]
    pub fn last_compaction_before(&self) -> Option<usize> {
        self.compaction_points
            .last()
            .map(|point| point.before_message)
    }

    fn message_text_of(&self, span: &MessageSpan) -> &str {
        self.text.get(span.start..span.end).unwrap_or("")
    }
}

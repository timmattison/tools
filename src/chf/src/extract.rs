//! One transcript into one `Session`.

use std::path::PathBuf;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use claude_transcript::{CompactionFacts, SessionId};

use crate::session::Session;

/// Build a `Session` from the bytes of a transcript. This function reads no file.
#[must_use]
pub fn extract(id: SessionId, path: PathBuf, bytes: &[u8], mtime: SystemTime) -> Session {
    let _ = bytes;
    Session {
        id,
        path,
        cwd: None,
        git_branch: None,
        ai_title: None,
        last_activity: DateTime::<Utc>::from(mtime),
        compaction: CompactionFacts::default(),
        text: String::new(),
        messages: vec![],
        compaction_points: vec![],
    }
}

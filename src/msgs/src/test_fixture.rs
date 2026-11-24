//! Test-only fixture: a throwaway `chat.db` with the subset of the Messages
//! schema that msgs reads. Each fixture owns its own temporary directory, so
//! tests that use it can run in parallel.

use std::path::Path;

use rusqlite::{params, Connection};
use tempfile::TempDir;

use crate::db::MessageDb;

/// Seconds between the Unix epoch and the Apple epoch (2001-01-01 UTC).
const APPLE_EPOCH_OFFSET_SECONDS: i64 = 978_307_200;
const NANOS_PER_SECOND: i64 = 1_000_000_000;

const SCHEMA: &str = "
CREATE TABLE handle (ROWID INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL);
CREATE TABLE chat (
    ROWID INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_identifier TEXT,
    display_name TEXT,
    style INTEGER
);
CREATE TABLE message (
    ROWID INTEGER PRIMARY KEY AUTOINCREMENT,
    guid TEXT,
    text TEXT,
    attributedBody BLOB,
    is_from_me INTEGER DEFAULT 0,
    handle_id INTEGER DEFAULT 0,
    date INTEGER,
    date_read INTEGER,
    date_edited INTEGER DEFAULT 0,
    is_audio_message INTEGER DEFAULT 0,
    reply_to_guid TEXT,
    associated_message_emoji TEXT,
    cache_has_attachments INTEGER DEFAULT 0
);
CREATE TABLE chat_message_join (
    chat_id INTEGER,
    message_id INTEGER,
    message_date INTEGER DEFAULT 0,
    PRIMARY KEY (chat_id, message_id)
);
CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER);
CREATE TABLE attachment (
    ROWID INTEGER PRIMARY KEY AUTOINCREMENT,
    filename TEXT,
    mime_type TEXT,
    total_bytes INTEGER DEFAULT 0,
    transfer_name TEXT,
    is_sticker INTEGER DEFAULT 0
);
CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER);
";

/// Convert a Unix time to the Apple date that chat.db stores
/// (nanoseconds since 2001-01-01 UTC).
pub fn apple_ns(unix_seconds: i64, nanos: i64) -> i64 {
    (unix_seconds - APPLE_EPOCH_OFFSET_SECONDS) * NANOS_PER_SECOND + nanos
}

/// Build an `attributedBody` blob with the real typedstream length encoding.
///
/// The length is one byte below 128, else `0x81` and an i16 little-endian up
/// to 32,767, else `0x82` and an i32 little-endian.
pub fn attributed_body(text: &str) -> Vec<u8> {
    let mut blob = Vec::new();
    blob.extend_from_slice(b"\x04\x0bstreamtyped\x81\xe8\x03\x84\x01@\x84\x84\x84\x12NSAttributedString\x00\x84\x84\x08NSObject\x00\x85\x92\x84\x84\x84\x08");
    blob.extend_from_slice(b"NSString");
    blob.extend_from_slice(&[0x01, 0x94, 0x84, 0x01, 0x2b]);
    let len = text.len();
    if len < 0x80 {
        blob.push(u8::try_from(len).expect("below 128"));
    } else if let Ok(short) = i16::try_from(len) {
        blob.push(0x81);
        blob.extend_from_slice(&short.to_le_bytes());
    } else {
        blob.push(0x82);
        blob.extend_from_slice(&i32::try_from(len).expect("text below 2 GiB").to_le_bytes());
    }
    blob.extend_from_slice(text.as_bytes());
    blob.extend_from_slice(b"\x86\x84\x02iI");
    blob
}

/// The columns of one `message` row, plus the chats it belongs to.
#[derive(Debug, Default, Clone)]
pub struct MessageSpec {
    pub text: Option<String>,
    pub attributed_body: Option<Vec<u8>>,
    pub is_from_me: bool,
    /// ROWID of the sender handle; 0 means no handle.
    pub handle: i64,
    /// Apple date (nanoseconds since 2001-01-01); see [`apple_ns`].
    pub date: i64,
    /// ROWIDs of the chats that hold the message.
    pub chats: Vec<i64>,
    pub reply_to_guid: Option<String>,
    pub associated_emoji: Option<String>,
}

/// A temporary `chat.db` with the msgs schema.
pub struct ChatDbFixture {
    dir: TempDir,
    path: std::path::PathBuf,
    writer: Connection,
}

impl ChatDbFixture {
    /// Create an empty `chat.db` in a new temporary directory.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("chat.db");
        let writer = Connection::open(&path).expect("create fixture chat.db");
        writer.execute_batch(SCHEMA).expect("create fixture schema");
        Self { dir, path, writer }
    }

    /// The temporary directory that holds the fixture (removed on drop).
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    /// Path of the fixture `chat.db` file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Insert a handle and return its ROWID.
    pub fn add_handle(&self, id: &str) -> i64 {
        self.writer
            .execute("INSERT INTO handle (id) VALUES (?1)", [id])
            .expect("insert handle");
        self.writer.last_insert_rowid()
    }

    /// Insert a chat and return its ROWID.
    pub fn add_chat(&self, identifier: &str, display_name: Option<&str>, style: i64) -> i64 {
        self.writer
            .execute(
                "INSERT INTO chat (chat_identifier, display_name, style) VALUES (?1, ?2, ?3)",
                params![identifier, display_name, style],
            )
            .expect("insert chat");
        self.writer.last_insert_rowid()
    }

    /// Make `handle` a participant of `chat`.
    pub fn link_handle(&self, chat: i64, handle: i64) {
        self.writer
            .execute(
                "INSERT INTO chat_handle_join (chat_id, handle_id) VALUES (?1, ?2)",
                [chat, handle],
            )
            .expect("insert chat_handle_join");
    }

    /// Insert a message and one `chat_message_join` row for each chat in
    /// `spec.chats`. Returns the message ROWID.
    pub fn add_message(&self, spec: MessageSpec) -> i64 {
        self.writer
            .execute(
                "INSERT INTO message
                    (guid, text, attributedBody, is_from_me, handle_id, date,
                     reply_to_guid, associated_message_emoji)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    format!("guid-{}", spec.date),
                    spec.text,
                    spec.attributed_body,
                    spec.is_from_me,
                    spec.handle,
                    spec.date,
                    spec.reply_to_guid,
                    spec.associated_emoji,
                ],
            )
            .expect("insert message");
        let message_id = self.writer.last_insert_rowid();
        for chat in &spec.chats {
            self.writer
                .execute(
                    "INSERT INTO chat_message_join (chat_id, message_id, message_date)
                     VALUES (?1, ?2, ?3)",
                    [*chat, message_id, spec.date],
                )
                .expect("insert chat_message_join");
        }
        message_id
    }

    /// Insert an attachment for `message_id` and flag the message as having
    /// attachments. Returns the attachment ROWID.
    pub fn add_attachment(
        &self,
        message_id: i64,
        filename: &str,
        transfer_name: Option<&str>,
    ) -> i64 {
        self.writer
            .execute(
                "INSERT INTO attachment (filename, mime_type, total_bytes, transfer_name)
                 VALUES (?1, 'image/jpeg', 2048, ?2)",
                params![filename, transfer_name],
            )
            .expect("insert attachment");
        let attachment_id = self.writer.last_insert_rowid();
        self.writer
            .execute(
                "INSERT INTO message_attachment_join (message_id, attachment_id) VALUES (?1, ?2)",
                [message_id, attachment_id],
            )
            .expect("insert message_attachment_join");
        self.writer
            .execute(
                "UPDATE message SET cache_has_attachments = 1 WHERE ROWID = ?1",
                [message_id],
            )
            .expect("flag message attachments");
        attachment_id
    }

    /// Run one write statement against the fixture and return the number of
    /// changed rows. For a test that edits or deletes after the first open.
    pub fn execute(&self, sql: &str, params: impl rusqlite::Params) -> usize {
        self.writer
            .execute(sql, params)
            .expect("execute fixture sql")
    }

    /// Open the fixture through `MessageDb::open_at`.
    pub fn open_db(&self) -> MessageDb {
        MessageDb::open_at(self.path()).expect("open fixture db")
    }
}

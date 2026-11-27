use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::error::MsgsError;
use crate::parser::extract_text_from_attributed_body;
use crate::types::{
    apple_timestamp_to_datetime, truncate_preview, AttachmentInfo, Conversation, DbStatus, Message,
    SearchResult,
};
use crate::util::page_bounds;

const PREVIEW_MAX_CHARS: usize = 100;
/// Messages that a search hit shows before and after itself.
const CONTEXT_BEFORE_COUNT: i64 = 3;
const CONTEXT_AFTER_COUNT: i64 = 3;

/// Separates the handle ids that the participants subquery joins (ASCII unit separator).
const PARTICIPANT_SEPARATOR: char = '\u{1f}';

/// The preview of a last message: its text, else the text of its
/// attributedBody, cut to `PREVIEW_MAX_CHARS`.
fn preview_of(text: Option<String>, body: Option<&[u8]>) -> String {
    let text = text
        .or_else(|| body.and_then(extract_text_from_attributed_body))
        .unwrap_or_default();
    truncate_preview(&text, PREVIEW_MAX_CHARS)
}

/// The `FROM` and `JOIN` part and the column list of each message query.
/// A query adds its own `WHERE` and `ORDER BY`. `MessageRow::from_row` reads
/// the columns by the aliases that this list gives.
const MESSAGE_SELECT: &str = "SELECT
        m.ROWID AS message_id,
        m.text AS text,
        m.attributedBody AS attributed_body,
        m.is_from_me AS is_from_me,
        COALESCE(h.id, 'Unknown') AS sender_id,
        m.date AS date,
        m.date_read AS date_read,
        m.is_audio_message AS is_audio,
        m.reply_to_guid AS reply_to_guid,
        m.associated_message_emoji AS associated_emoji,
        m.cache_has_attachments AS has_attachments
    FROM message m
    JOIN chat_message_join cmj ON m.ROWID = cmj.message_id
    LEFT JOIN handle h ON m.handle_id = h.ROWID";

/// One row of a query that starts with [`MESSAGE_SELECT`].
struct MessageRow {
    message_id: i64,
    text: Option<String>,
    attributed_body: Option<Vec<u8>>,
    is_from_me: bool,
    sender_id: String,
    date: i64,
    date_read: Option<i64>,
    is_audio: bool,
    reply_to_guid: Option<String>,
    associated_emoji: Option<String>,
    has_attachments: bool,
}

impl MessageRow {
    /// Read one row of [`MESSAGE_SELECT`] by column name.
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            message_id: row.get("message_id")?,
            text: row.get("text")?,
            attributed_body: row.get("attributed_body")?,
            is_from_me: row.get("is_from_me")?,
            sender_id: row.get("sender_id")?,
            date: row.get("date")?,
            date_read: row.get("date_read")?,
            is_audio: row.get("is_audio")?,
            reply_to_guid: row.get("reply_to_guid")?,
            associated_emoji: row.get("associated_emoji")?,
            has_attachments: row.get("has_attachments")?,
        })
    }
}

pub struct MessageDb {
    conn: Connection,
}

/// Map a `SQLite` error from the open or the probe query to a `MsgsError`.
///
/// A refused open or a denied access means Full Disk Access is missing. The
/// result code decides, not the text of the message.
fn classify_open_error(e: rusqlite::Error) -> MsgsError {
    match &e {
        rusqlite::Error::SqliteFailure(err, _)
            if matches!(
                err.code,
                rusqlite::ErrorCode::CannotOpen
                    | rusqlite::ErrorCode::PermissionDenied
                    | rusqlite::ErrorCode::AuthorizationForStatementDenied
            ) =>
        {
            MsgsError::NoFullDiskAccess
        }
        _ => MsgsError::DatabaseError(e.to_string()),
    }
}

impl MessageDb {
    /// Open a chat.db file at `db_path` in read-only mode.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseNotFound` if the file doesn't exist.
    /// Returns `MsgsError::NoFullDiskAccess` if permission is denied.
    /// Returns `MsgsError::DatabaseError` for any other failure.
    pub fn open_at(db_path: &Path) -> Result<Self, MsgsError> {
        match std::fs::metadata(db_path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(MsgsError::DatabaseNotFound);
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(MsgsError::NoFullDiskAccess);
            }
            Err(e) => return Err(MsgsError::DatabaseError(e.to_string())),
        }

        let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(classify_open_error)?;

        // Verify we can actually query (use query_row, not execute_batch, for SELECT)
        conn.query_row("SELECT 1 FROM message LIMIT 1", [], |_| Ok(()))
            .map_err(classify_open_error)?;

        Ok(Self { conn })
    }

    /// Check if the database is accessible and return status.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError` variants for various failure modes.
    pub fn check_access(&self) -> Result<DbStatus, MsgsError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM message", [], |row| row.get(0))?;
        Ok(DbStatus {
            accessible: true,
            message_count: Some(count),
            error: None,
        })
    }

    /// List conversations, paginated, sorted by most recent message.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseError` on query failure.
    pub fn list_conversations(
        &self,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<Conversation>, MsgsError> {
        let (offset, limit) = page_bounds(offset, limit);
        let mut stmt = self.conn.prepare(
            "SELECT
                c.ROWID,
                c.chat_identifier,
                c.display_name,
                c.style,
                MAX(cmj.message_date) as last_date,
                COUNT(cmj.message_id) as msg_count,
                (SELECT GROUP_CONCAT(h.id, char(31))
                   FROM chat_handle_join chj
                   JOIN handle h ON h.ROWID = chj.handle_id
                  WHERE chj.chat_id = c.ROWID) as participants,
                lm.text,
                lm.attributedBody
            FROM chat c
            JOIN chat_message_join cmj ON c.ROWID = cmj.chat_id
            LEFT JOIN message lm ON lm.ROWID = (
                SELECT j.message_id
                  FROM chat_message_join j
                  JOIN message m2 ON m2.ROWID = j.message_id
                 WHERE j.chat_id = c.ROWID
                 ORDER BY j.message_date DESC, j.message_id DESC
                 LIMIT 1)
            GROUP BY c.ROWID
            ORDER BY last_date DESC, c.ROWID DESC
            LIMIT ?1 OFFSET ?2",
        )?;

        let rows = stmt.query_map([limit, offset], |row| {
            Ok((
                row.get::<_, i64>(0)?,             // chat_id
                row.get::<_, String>(1)?,          // chat_identifier
                row.get::<_, Option<String>>(2)?,  // display_name
                row.get::<_, i64>(3)?,             // style
                row.get::<_, i64>(4)?,             // last_date
                row.get::<_, i64>(5)?,             // msg_count
                row.get::<_, Option<String>>(6)?,  // participants, joined
                row.get::<_, Option<String>>(7)?,  // last message text
                row.get::<_, Option<Vec<u8>>>(8)?, // last message attributedBody
            ))
        })?;

        let mut conversations = Vec::new();
        for row in rows {
            let (
                chat_id,
                chat_identifier,
                display_name,
                style,
                last_date,
                msg_count,
                joined_participants,
                preview_text,
                preview_body,
            ) = row?;
            let participants = joined_participants
                .map(|joined| {
                    joined
                        .split(PARTICIPANT_SEPARATOR)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();

            let last_message_date = apple_timestamp_to_datetime(last_date);

            conversations.push(Conversation {
                chat_id,
                chat_identifier,
                display_name: if display_name.as_deref() == Some("") {
                    None
                } else {
                    display_name
                },
                is_group: style == 43,
                participants,
                last_message_date,
                last_message_preview: preview_of(preview_text, preview_body.as_deref()),
                message_count: msg_count,
            });
        }

        Ok(conversations)
    }

    fn get_participants(&self, chat_id: i64) -> Result<Vec<String>, MsgsError> {
        let mut stmt = self.conn.prepare(
            "SELECT h.id FROM handle h
             JOIN chat_handle_join chj ON h.ROWID = chj.handle_id
             WHERE chj.chat_id = ?1",
        )?;
        let rows = stmt.query_map([chat_id], |row| row.get::<_, String>(0))?;
        let mut participants = Vec::new();
        for row in rows {
            participants.push(row?);
        }
        Ok(participants)
    }

    fn get_last_message_preview(&self, chat_id: i64) -> Result<String, MsgsError> {
        let result: Option<(Option<String>, Option<Vec<u8>>)> = self
            .conn
            .query_row(
                "SELECT m.text, m.attributedBody
                 FROM message m
                 JOIN chat_message_join cmj ON m.ROWID = cmj.message_id
                 WHERE cmj.chat_id = ?1
                 ORDER BY cmj.message_date DESC, m.ROWID DESC
                 LIMIT 1",
                [chat_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        Ok(result.map_or_else(String::new, |(text, body)| {
            preview_of(text, body.as_deref())
        }))
    }

    /// Build a `Message` from raw query row fields.
    fn build_message(&self, row: MessageRow) -> Result<Message, MsgsError> {
        let MessageRow {
            message_id,
            text,
            attributed_body,
            is_from_me,
            sender_id,
            date,
            date_read,
            is_audio,
            reply_to_guid,
            associated_emoji,
            has_attachments,
        } = row;

        let extracted_text = text.or_else(|| {
            attributed_body
                .as_deref()
                .and_then(extract_text_from_attributed_body)
        });

        let attachments = if has_attachments {
            self.get_attachments_for_message(message_id)?
        } else {
            Vec::new()
        };

        Ok(Message {
            message_id,
            text: extracted_text,
            sender: if is_from_me {
                "Me".to_string()
            } else {
                sender_id
            },
            is_from_me,
            date: apple_timestamp_to_datetime(date),
            date_read: date_read.and_then(apple_timestamp_to_datetime),
            is_audio,
            attachments,
            reply_to_guid,
            associated_emoji,
        })
    }

    /// Run one message query and build each row into a `Message`.
    /// `sql` starts with [`MESSAGE_SELECT`].
    fn query_messages(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<Message>, MsgsError> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, MessageRow::from_row)?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(self.build_message(row?)?);
        }
        Ok(messages)
    }

    /// Get paginated messages for a conversation.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseError` on query failure.
    pub fn get_messages(
        &self,
        chat_id: i64,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<Message>, MsgsError> {
        let (offset, limit) = page_bounds(offset, limit);
        self.query_messages(
            &format!(
                "{MESSAGE_SELECT}
                WHERE cmj.chat_id = ?1
                ORDER BY cmj.message_date DESC, m.ROWID DESC
                LIMIT ?2 OFFSET ?3"
            ),
            [chat_id, limit, offset],
        )
    }

    /// The count of messages of the chat that come before `message_id` in the
    /// newest-first order of `get_messages`. `get_messages(chat_id, position, 1)`
    /// returns the message.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseError` when the message is not in the chat
    /// or the query fails.
    pub fn message_position(&self, chat_id: i64, message_id: i64) -> Result<i64, MsgsError> {
        let (date, rowid): (i64, i64) = self
            .conn
            .query_row(
                "SELECT message_date, message_id FROM chat_message_join
                 WHERE chat_id = ?1 AND message_id = ?2",
                [chat_id, message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => MsgsError::DatabaseError(format!(
                    "Message {message_id} is not in chat {chat_id}"
                )),
                other => MsgsError::from(other),
            })?;
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM chat_message_join
             WHERE chat_id = ?1 AND (message_date, message_id) > (?2, ?3)",
            [chat_id, date, rowid],
            |row| row.get(0),
        )?)
    }

    fn get_attachments_for_message(
        &self,
        message_id: i64,
    ) -> Result<Vec<AttachmentInfo>, MsgsError> {
        let mut stmt = self.conn.prepare(
            "SELECT a.ROWID, a.filename, a.mime_type, a.total_bytes, a.transfer_name, a.is_sticker
             FROM attachment a
             JOIN message_attachment_join maj ON a.ROWID = maj.attachment_id
             WHERE maj.message_id = ?1",
        )?;

        let rows = stmt.query_map([message_id], |row| {
            Ok(AttachmentInfo {
                attachment_id: row.get(0)?,
                filename: row.get(1)?,
                mime_type: row.get(2)?,
                total_bytes: row.get(3)?,
                transfer_name: row.get(4)?,
                is_sticker: row.get(5)?,
            })
        })?;

        let mut attachments = Vec::new();
        for row in rows {
            attachments.push(row?);
        }
        Ok(attachments)
    }

    /// Get the underlying connection (for cache building).
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Get a single conversation's metadata by chat_id.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseError` on query failure.
    pub fn get_conversation(&self, chat_id: i64) -> Result<Conversation, MsgsError> {
        let (chat_identifier, display_name, style, last_date, msg_count): (
            String,
            Option<String>,
            i64,
            i64,
            i64,
        ) = self.conn.query_row(
            "SELECT
                c.chat_identifier,
                c.display_name,
                c.style,
                MAX(cmj.message_date) as last_date,
                COUNT(cmj.message_id) as msg_count
            FROM chat c
            JOIN chat_message_join cmj ON c.ROWID = cmj.chat_id
            WHERE c.ROWID = ?1
            GROUP BY c.ROWID",
            [chat_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;

        let participants = self.get_participants(chat_id)?;
        let preview = self.get_last_message_preview(chat_id)?;

        Ok(Conversation {
            chat_id,
            chat_identifier,
            display_name: if display_name.as_deref() == Some("") {
                None
            } else {
                display_name
            },
            is_group: style == 43,
            participants,
            last_message_date: apple_timestamp_to_datetime(last_date),
            last_message_preview: preview,
            message_count: msg_count,
        })
    }

    /// Get the messages of a conversation from one message to another, both
    /// included, oldest first (for export). The order of the two ids does not
    /// matter. The bounds are the position of each message, its date then its
    /// ROWID, so a sub-second date cannot drop a message.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::ExportError` when an id is not in the chat, and
    /// `MsgsError::DatabaseError` on query failure.
    pub fn get_messages_between(
        &self,
        chat_id: i64,
        first_message_id: i64,
        second_message_id: i64,
    ) -> Result<Vec<Message>, MsgsError> {
        let position = |message_id: i64| -> Result<(i64, i64), MsgsError> {
            self.conn
                .query_row(
                    "SELECT message_date, message_id FROM chat_message_join
                     WHERE chat_id = ?1 AND message_id = ?2",
                    [chat_id, message_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => MsgsError::ExportError(format!(
                        "Message {message_id} is not in chat {chat_id}"
                    )),
                    other => MsgsError::from(other),
                })
        };
        let first = position(first_message_id)?;
        let second = position(second_message_id)?;
        let (low, high) = if first <= second {
            (first, second)
        } else {
            (second, first)
        };
        self.query_messages(
            &format!(
                "{MESSAGE_SELECT}
                WHERE cmj.chat_id = ?1
                  AND (cmj.message_date, m.ROWID) BETWEEN (?2, ?3) AND (?4, ?5)
                ORDER BY cmj.message_date ASC, m.ROWID ASC"
            ),
            [chat_id, low.0, low.1, high.0, high.1],
        )
    }

    /// Get a message with surrounding context for search results. Gives
    /// `None` when the message is not in the chat, for example when chat.db
    /// lost the message after the search index was built.
    ///
    /// All reads see the snapshot of the first read. Thus a message or a chat
    /// that Messages deletes during the reads cannot fail the hit.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseError` on query failure.
    pub fn get_message_with_context(
        &self,
        message_id: i64,
        chat_id: i64,
    ) -> Result<Option<SearchResult>, MsgsError> {
        // A deferred transaction takes its snapshot at the first read. A drop
        // without a commit ends it too.
        let snapshot = self.conn.unchecked_transaction()?;
        let target_row = match self.conn.query_row(
            &format!("{MESSAGE_SELECT} WHERE cmj.chat_id = ?1 AND m.ROWID = ?2"),
            [chat_id, message_id],
            MessageRow::from_row,
        ) {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(other) => return Err(MsgsError::from(other)),
        };
        let target = self.build_message(target_row)?;
        // The context orders by the date of the join row, as the list does.
        let target_date: i64 = self.conn.query_row(
            "SELECT message_date FROM chat_message_join WHERE chat_id = ?1 AND message_id = ?2",
            [chat_id, message_id],
            |row| row.get(0),
        )?;

        // Row-value comparison orders by date, then by ROWID, so messages
        // that share one date keep a stable order around the hit.
        let mut context_before = self.query_messages(
            &format!(
                "{MESSAGE_SELECT}
                WHERE cmj.chat_id = ?1 AND (cmj.message_date, m.ROWID) < (?2, ?3)
                ORDER BY cmj.message_date DESC, m.ROWID DESC
                LIMIT ?4"
            ),
            [chat_id, target_date, message_id, CONTEXT_BEFORE_COUNT],
        )?;
        context_before.reverse();
        let context_after = self.query_messages(
            &format!(
                "{MESSAGE_SELECT}
                WHERE cmj.chat_id = ?1 AND (cmj.message_date, m.ROWID) > (?2, ?3)
                ORDER BY cmj.message_date ASC, m.ROWID ASC
                LIMIT ?4"
            ),
            [chat_id, target_date, message_id, CONTEXT_AFTER_COUNT],
        )?;

        // Get conversation info
        let conversation = self.get_conversation(chat_id)?;
        snapshot.commit()?;

        Ok(Some(SearchResult {
            message: target,
            conversation,
            context_before,
            context_after,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixture::{apple_ns, attributed_body, ChatDbFixture, MessageSpec};

    /// A whole-second Unix time used as the base of the fixture dates.
    const BASE: i64 = 1_700_000_000;

    fn text_message(text: &str, secs: i64, handle: i64, chats: &[i64]) -> MessageSpec {
        MessageSpec {
            text: Some(text.to_string()),
            handle,
            date: apple_ns(BASE + secs, 0),
            chats: chats.to_vec(),
            ..MessageSpec::default()
        }
    }

    #[test]
    fn open_at_missing_file_is_database_not_found() {
        let dir = tempfile::tempdir().expect("temp dir");
        let result = MessageDb::open_at(&dir.path().join("absent.db"));
        assert!(matches!(result, Err(MsgsError::DatabaseNotFound)));
    }

    /// Set the Unix mode of `path`.
    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("set mode");
    }

    /// Sets a mode on drop, so a `TempDir` can delete the path when a test fails.
    #[cfg(unix)]
    struct RestoreMode<'a>(&'a Path);

    #[cfg(unix)]
    impl Drop for RestoreMode<'_> {
        fn drop(&mut self) {
            set_mode(self.0, 0o755);
        }
    }

    #[cfg(unix)]
    #[test]
    fn open_at_inside_unreadable_directory_is_no_full_disk_access() {
        let fx = ChatDbFixture::new();
        let dir = tempfile::tempdir().expect("temp dir");
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).expect("make subdir");
        let db_path = sub.join("chat.db");
        std::fs::copy(fx.path(), &db_path).expect("copy fixture");

        let _restore = RestoreMode(&sub);
        set_mode(&sub, 0o000);
        if std::fs::metadata(&db_path).is_ok() {
            eprintln!("skipped: the mode does not block this user (root)");
            return;
        }
        let result = MessageDb::open_at(&db_path);
        set_mode(&sub, 0o755);
        assert!(matches!(result, Err(MsgsError::NoFullDiskAccess)));
    }

    #[cfg(unix)]
    #[test]
    fn open_at_unreadable_file_is_no_full_disk_access() {
        let fx = ChatDbFixture::new();
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("chat.db");
        std::fs::copy(fx.path(), &db_path).expect("copy fixture");

        let _restore = RestoreMode(&db_path);
        set_mode(&db_path, 0o000);
        if std::fs::File::open(&db_path).is_ok() {
            eprintln!("skipped: the mode does not block this user (root)");
            return;
        }
        let result = MessageDb::open_at(&db_path);
        set_mode(&db_path, 0o755);
        assert!(matches!(result, Err(MsgsError::NoFullDiskAccess)));
    }

    #[test]
    fn open_at_non_sqlite_file_is_database_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("chat.db");
        std::fs::write(
            &db_path,
            "this is plain text, not a sqlite database. ".repeat(3),
        )
        .expect("write file");
        let result = MessageDb::open_at(&db_path);
        assert!(matches!(result, Err(MsgsError::DatabaseError(_))));
    }

    #[test]
    fn list_conversations_sorts_newest_first_and_counts_messages() {
        let fx = ChatDbFixture::new();
        let old = fx.add_chat("old@example.com", None, 45);
        let new = fx.add_chat("new@example.com", None, 45);
        fx.add_message(text_message("a", 10, 0, &[old]));
        fx.add_message(text_message("b", 20, 0, &[old]));
        fx.add_message(text_message("c", 30, 0, &[old]));
        fx.add_message(text_message("d", 40, 0, &[new]));
        // Newest message sits in the chat with the lower ROWID.
        fx.add_message(text_message("e", 50, 0, &[old]));

        let list = fx.open_db().list_conversations(0, 10).expect("list");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].chat_id, old);
        assert_eq!(list[0].message_count, 4);
        assert_eq!(list[1].chat_id, new);
        assert_eq!(list[1].message_count, 1);
        assert_eq!(
            list[0].last_message_date.map(|d| d.timestamp()),
            Some(BASE + 50)
        );
    }

    #[test]
    fn list_conversations_pages_with_offset_and_limit() {
        let fx = ChatDbFixture::new();
        let chats: Vec<i64> = (0..3)
            .map(|i| fx.add_chat(&format!("chat{i}"), None, 45))
            .collect();
        for (i, chat) in chats.iter().enumerate() {
            let secs = i64::try_from(i).expect("small index") * 10 + 10;
            fx.add_message(text_message("x", secs, 0, &[*chat]));
        }
        let db = fx.open_db();

        let first = db.list_conversations(0, 1).expect("page 1");
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].chat_id, chats[2]);
        let second = db.list_conversations(1, 1).expect("page 2");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].chat_id, chats[1]);
        let rest = db.list_conversations(1, 10).expect("rest");
        let ids: Vec<i64> = rest.iter().map(|c| c.chat_id).collect();
        assert_eq!(ids, vec![chats[1], chats[0]]);
    }

    #[test]
    fn list_conversations_maps_style_name_and_participants() {
        let fx = ChatDbFixture::new();
        let alice = fx.add_handle("+15550001");
        let bob = fx.add_handle("bob@example.com");
        let group = fx.add_chat("chat-group", Some("Team"), 43);
        let direct = fx.add_chat("+15550001", Some(""), 45);
        fx.link_handle(group, alice);
        fx.link_handle(group, bob);
        fx.link_handle(direct, alice);
        fx.add_message(text_message("g", 20, alice, &[group]));
        fx.add_message(text_message("d", 10, alice, &[direct]));

        let list = fx.open_db().list_conversations(0, 10).expect("list");
        let group_row = list.iter().find(|c| c.chat_id == group).expect("group");
        assert!(group_row.is_group);
        assert_eq!(group_row.display_name.as_deref(), Some("Team"));
        let mut people = group_row.participants.clone();
        people.sort();
        assert_eq!(people, vec!["+15550001", "bob@example.com"]);

        let direct_row = list.iter().find(|c| c.chat_id == direct).expect("direct");
        assert!(!direct_row.is_group);
        assert_eq!(direct_row.display_name, None);
        assert_eq!(direct_row.participants, vec!["+15550001"]);
    }

    #[test]
    fn list_conversations_preview_uses_text_then_attributed_body() {
        let fx = ChatDbFixture::new();
        let with_text = fx.add_chat("text-chat", None, 45);
        let with_body = fx.add_chat("body-chat", None, 45);
        fx.add_message(text_message("old", 5, 0, &[with_text]));
        fx.add_message(text_message("newest text", 10, 0, &[with_text]));
        fx.add_message(MessageSpec {
            attributed_body: Some(attributed_body("from the blob")),
            date: apple_ns(BASE + 20, 0),
            chats: vec![with_body],
            ..MessageSpec::default()
        });

        let list = fx.open_db().list_conversations(0, 10).expect("list");
        let text_row = list.iter().find(|c| c.chat_id == with_text).expect("text");
        assert_eq!(text_row.last_message_preview, "newest text");
        let body_row = list.iter().find(|c| c.chat_id == with_body).expect("body");
        assert_eq!(body_row.last_message_preview, "from the blob");
    }

    #[test]
    fn get_conversation_returns_metadata_and_participants() {
        let fx = ChatDbFixture::new();
        let alice = fx.add_handle("alice@example.com");
        let chat = fx.add_chat("chat-42", Some("Book club"), 43);
        let other = fx.add_chat("other", None, 45);
        fx.link_handle(chat, alice);
        fx.add_message(text_message("one", 10, alice, &[chat]));
        fx.add_message(text_message("two", 20, alice, &[chat]));
        fx.add_message(text_message("elsewhere", 30, alice, &[other]));

        let conversation = fx.open_db().get_conversation(chat).expect("conversation");
        assert_eq!(conversation.chat_id, chat);
        assert_eq!(conversation.chat_identifier, "chat-42");
        assert_eq!(conversation.display_name.as_deref(), Some("Book club"));
        assert!(conversation.is_group);
        assert_eq!(conversation.participants, vec!["alice@example.com"]);
        assert_eq!(conversation.message_count, 2);
        assert_eq!(conversation.last_message_preview, "two");
        assert_eq!(
            conversation.last_message_date.map(|d| d.timestamp()),
            Some(BASE + 20)
        );
    }

    #[test]
    fn get_messages_orders_newest_first_and_pages() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("c", None, 45);
        for (i, secs) in [10, 20, 30, 40].into_iter().enumerate() {
            fx.add_message(text_message(&format!("m{i}"), secs, 0, &[chat]));
        }
        let db = fx.open_db();

        let all = db.get_messages(chat, 0, 10).expect("all");
        let texts: Vec<_> = all.iter().filter_map(|m| m.text.as_deref()).collect();
        assert_eq!(texts, vec!["m3", "m2", "m1", "m0"]);

        let page = db.get_messages(chat, 1, 2).expect("page");
        let texts: Vec<_> = page.iter().filter_map(|m| m.text.as_deref()).collect();
        assert_eq!(texts, vec!["m2", "m1"]);
    }

    #[test]
    fn get_messages_resolves_sender_text_and_date() {
        let fx = ChatDbFixture::new();
        let alice = fx.add_handle("alice@example.com");
        let chat = fx.add_chat("c", None, 45);
        fx.add_message(MessageSpec {
            is_from_me: true,
            ..text_message("mine", 10, alice, &[chat])
        });
        fx.add_message(text_message("theirs", 20, alice, &[chat]));
        fx.add_message(text_message("nobody", 30, 0, &[chat]));
        fx.add_message(MessageSpec {
            attributed_body: Some(attributed_body("blob text")),
            handle: alice,
            date: apple_ns(BASE + 40, 0),
            chats: vec![chat],
            reply_to_guid: Some("parent-guid".to_string()),
            associated_emoji: Some("👍".to_string()),
            ..MessageSpec::default()
        });

        let messages = fx.open_db().get_messages(chat, 0, 10).expect("messages");
        assert_eq!(messages.len(), 4);
        // Newest first: blob, nobody, theirs, mine.
        assert_eq!(messages[0].text.as_deref(), Some("blob text"));
        assert_eq!(messages[0].reply_to_guid.as_deref(), Some("parent-guid"));
        assert_eq!(messages[0].associated_emoji.as_deref(), Some("👍"));
        assert_eq!(messages[1].sender, "Unknown");
        assert_eq!(messages[2].sender, "alice@example.com");
        assert!(!messages[2].is_from_me);
        assert_eq!(messages[3].sender, "Me");
        assert!(messages[3].is_from_me);
        let utc = messages[3].date.expect("date").with_timezone(&chrono::Utc);
        assert_eq!(utc.timestamp(), BASE + 10);
        assert_eq!(utc.to_rfc3339(), "2023-11-14T22:13:30+00:00");
    }

    #[test]
    fn get_messages_lists_attachments() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("c", None, 45);
        let with = fx.add_message(text_message("photo", 10, 0, &[chat]));
        fx.add_message(text_message("plain", 20, 0, &[chat]));
        let attachment =
            fx.add_attachment(with, "~/Library/Messages/Attachments/a.jpg", Some("a.jpg"));

        let messages = fx.open_db().get_messages(chat, 0, 10).expect("messages");
        assert!(messages[0].attachments.is_empty());
        assert_eq!(messages[1].attachments.len(), 1);
        let info = &messages[1].attachments[0];
        assert_eq!(info.attachment_id, attachment);
        assert_eq!(
            info.filename.as_deref(),
            Some("~/Library/Messages/Attachments/a.jpg")
        );
        assert_eq!(info.transfer_name.as_deref(), Some("a.jpg"));
        assert_eq!(info.mime_type.as_deref(), Some("image/jpeg"));
        assert_eq!(info.total_bytes, 2048);
        assert!(!info.is_sticker);
    }

    #[test]
    fn get_message_with_context_returns_three_before_and_three_after() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("c", Some("Ctx"), 45);
        let ids: Vec<i64> = (1..=7)
            .map(|n| fx.add_message(text_message(&format!("m{n}"), n * 10, 0, &[chat])))
            .collect();

        let result = fx
            .open_db()
            .get_message_with_context(ids[3], chat)
            .expect("context")
            .expect("the message is in the chat");
        assert_eq!(result.message.message_id, ids[3]);
        let before: Vec<i64> = result.context_before.iter().map(|m| m.message_id).collect();
        let after: Vec<i64> = result.context_after.iter().map(|m| m.message_id).collect();
        assert_eq!(before, ids[0..3].to_vec());
        assert_eq!(after, ids[4..7].to_vec());
        assert_eq!(result.conversation.chat_id, chat);
    }

    #[test]
    fn get_messages_between_is_inclusive_ascending_and_per_chat() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("c", None, 45);
        let other = fx.add_chat("other", None, 45);
        let ids: Vec<i64> = [
            ("before", 5),
            ("start", 10),
            ("middle", 15),
            ("end", 20),
            ("after", 25),
        ]
        .into_iter()
        .map(|(text, secs)| fx.add_message(text_message(text, secs, 0, &[chat])))
        .collect();
        fx.add_message(text_message("foreign", 15, 0, &[other]));

        let messages = fx
            .open_db()
            .get_messages_between(chat, ids[1], ids[3])
            .expect("range");
        let texts: Vec<_> = messages.iter().filter_map(|m| m.text.as_deref()).collect();
        assert_eq!(texts, vec!["start", "middle", "end"]);
    }

    #[test]
    fn get_message_with_context_breaks_date_ties_by_rowid() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("c", None, 45);
        // Nine messages with one whole-second date, as old chat.db rows have.
        let ids: Vec<i64> = (0..9)
            .map(|n| fx.add_message(text_message(&format!("m{n}"), 10, 0, &[chat])))
            .collect();

        let result = fx
            .open_db()
            .get_message_with_context(ids[4], chat)
            .expect("context")
            .expect("the message is in the chat");
        assert_eq!(result.message.message_id, ids[4]);
        let before: Vec<i64> = result.context_before.iter().map(|m| m.message_id).collect();
        let after: Vec<i64> = result.context_after.iter().map(|m| m.message_id).collect();
        assert_eq!(before, ids[1..4].to_vec());
        assert_eq!(after, ids[5..8].to_vec());
    }

    #[test]
    fn get_message_with_context_gives_none_when_the_message_is_not_in_the_chat() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("c", None, 45);
        let other = fx.add_chat("other", None, 45);
        fx.add_message(text_message("here", 10, 0, &[chat]));
        let elsewhere = fx.add_message(text_message("there", 20, 0, &[other]));

        let result = fx.open_db().get_message_with_context(elsewhere, chat);
        assert!(matches!(result, Ok(None)), "{result:?}");
        let missing = fx.open_db().get_message_with_context(9_999, chat);
        assert!(matches!(missing, Ok(None)), "{missing:?}");
    }

    #[test]
    fn get_message_with_context_keeps_its_snapshot_when_the_chat_is_deleted_during_the_reads() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        use rusqlite::trace::{TraceEvent, TraceEventCodes};

        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("c", None, 45);
        let ids: Vec<i64> = (1..=7)
            .map(|n| fx.add_message(text_message(&format!("m{n}"), n * 10, 0, &[chat])))
            .collect();
        // Messages writes chat.db in WAL mode, where a reader keeps its
        // snapshot while a writer commits.
        let deleter = Connection::open(fx.path()).expect("open the deleting connection");
        let mode: String = deleter
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .expect("set WAL mode");
        assert_eq!(mode, "wal");

        let mut db = fx.open_db();
        let deleted_rows = Arc::new(AtomicUsize::new(0));
        let deleted_rows_seen = Arc::clone(&deleted_rows);
        let mut fired = false;
        // The first row comes from the first read, after that read took its
        // snapshot. Messages deletes the chat at that time.
        db.conn
            .trace_v2(
                TraceEventCodes::SQLITE_TRACE_ROW,
                Some(move |_: TraceEvent<'_>| {
                    if !fired {
                        fired = true;
                        let joins = deleter
                            .execute("DELETE FROM chat_message_join WHERE chat_id = ?1", [chat])
                            .unwrap_or(0);
                        let chats = deleter
                            .execute("DELETE FROM chat WHERE ROWID = ?1", [chat])
                            .unwrap_or(0);
                        deleted_rows_seen.store(joins + chats, Ordering::SeqCst);
                    }
                }),
            )
            .expect("set the row trace");

        let result = db.get_message_with_context(ids[3], chat);
        assert_eq!(
            deleted_rows.load(Ordering::SeqCst),
            ids.len() + 1,
            "the deletion did not commit during the reads"
        );
        let hit = result
            .expect("a chat deleted during the reads failed the hit")
            .expect("the first read found the message");
        assert_eq!(hit.message.message_id, ids[3]);
        let before: Vec<i64> = hit.context_before.iter().map(|m| m.message_id).collect();
        let after: Vec<i64> = hit.context_after.iter().map(|m| m.message_id).collect();
        assert_eq!(before, ids[0..3].to_vec());
        assert_eq!(after, ids[4..7].to_vec());
        assert_eq!(hit.conversation.chat_id, chat);
        assert_eq!(hit.conversation.message_count, 7);

        // The next call reads a new snapshot, thus no transaction stays open.
        let later = db.get_message_with_context(ids[3], chat);
        assert!(matches!(later, Ok(None)), "{later:?}");
    }

    #[test]
    fn list_conversations_clamps_a_negative_limit_to_one() {
        let fx = ChatDbFixture::new();
        for i in 0..3 {
            let chat = fx.add_chat(&format!("chat{i}"), None, 45);
            fx.add_message(text_message("x", 10 + i, 0, &[chat]));
        }
        let list = fx.open_db().list_conversations(0, -1).expect("list");
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn get_messages_clamps_a_negative_limit_to_one() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("c", None, 45);
        for i in 0..3 {
            fx.add_message(text_message("x", 10 + i, 0, &[chat]));
        }
        let messages = fx.open_db().get_messages(chat, 0, -1).expect("messages");
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn list_conversations_pages_chats_with_one_last_date_without_a_repeat() {
        let fx = ChatDbFixture::new();
        let chats: Vec<i64> = (0..3)
            .map(|i| fx.add_chat(&format!("chat{i}"), None, 45))
            .collect();
        for chat in &chats {
            fx.add_message(text_message("x", 10, 0, &[*chat]));
        }
        let db = fx.open_db();
        let ids: Vec<i64> = (0..3)
            .map(|offset| db.list_conversations(offset, 1).expect("page")[0].chat_id)
            .collect();
        assert_eq!(ids, vec![chats[2], chats[1], chats[0]]);
    }

    #[test]
    fn get_messages_pages_messages_with_one_date_without_a_repeat() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("c", None, 45);
        let ids: Vec<i64> = (0..4)
            .map(|i| fx.add_message(text_message(&format!("m{i}"), 10, 0, &[chat])))
            .collect();
        let db = fx.open_db();
        let paged: Vec<i64> = (0..4)
            .map(|offset| db.get_messages(chat, offset, 1).expect("page")[0].message_id)
            .collect();
        let expected: Vec<i64> = ids.iter().rev().copied().collect();
        assert_eq!(paged, expected);
    }

    #[test]
    fn list_conversations_treats_a_negative_offset_as_zero() {
        let fx = ChatDbFixture::new();
        let chats: Vec<i64> = (0..3)
            .map(|i| fx.add_chat(&format!("chat{i}"), None, 45))
            .collect();
        for (i, chat) in chats.iter().enumerate() {
            let secs = i64::try_from(i).expect("small index") + 10;
            fx.add_message(text_message("x", secs, 0, &[*chat]));
        }
        let list = fx.open_db().list_conversations(-5, 2).expect("list");
        let ids: Vec<i64> = list.iter().map(|c| c.chat_id).collect();
        assert_eq!(ids, vec![chats[2], chats[1]]);
    }

    #[test]
    fn get_messages_between_orders_one_date_by_rowid() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("c", None, 45);
        let ids: Vec<i64> = (0..4)
            .map(|i| fx.add_message(text_message(&format!("m{i}"), 10, 0, &[chat])))
            .collect();
        let found: Vec<i64> = fx
            .open_db()
            .get_messages_between(chat, ids[0], ids[3])
            .expect("range")
            .iter()
            .map(|m| m.message_id)
            .collect();
        assert_eq!(found, ids);
    }

    #[test]
    fn a_message_with_date_zero_has_a_null_date() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("undated", None, 45);
        fx.add_message(MessageSpec {
            text: Some("no date".to_string()),
            date: 0,
            chats: vec![chat],
            ..MessageSpec::default()
        });

        let messages = fx.open_db().get_messages(chat, 0, 10).expect("messages");
        let json = serde_json::to_value(&messages[0]).expect("serialize");
        assert_eq!(json["date"], serde_json::Value::Null);
    }

    #[test]
    fn a_chat_whose_only_message_has_date_zero_has_a_null_last_message_date() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("undated-chat", None, 45);
        fx.add_message(MessageSpec {
            text: Some("no date".to_string()),
            date: 0,
            chats: vec![chat],
            ..MessageSpec::default()
        });

        let list = fx.open_db().list_conversations(0, 10).expect("list");
        let json = serde_json::to_value(&list[0]).expect("serialize");
        assert_eq!(json["last_message_date"], serde_json::Value::Null);
    }

    #[test]
    fn the_extreme_dates_do_not_panic_and_keep_their_own_value() {
        // No i64 nanosecond count is out of chrono's range, so only 0 gives null.
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("extremes", None, 45);
        for date in [i64::MAX, i64::MIN] {
            fx.add_message(MessageSpec {
                text: Some("extreme".to_string()),
                date,
                chats: vec![chat],
                ..MessageSpec::default()
            });
        }

        let messages = fx.open_db().get_messages(chat, 0, 10).expect("messages");
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|m| m.date.is_some()));
    }

    #[test]
    fn message_position_locates_each_message_in_the_newest_first_order() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("c", None, 45);
        let other = fx.add_chat("other", None, 45);
        // Seven messages, some on one date, added out of date order.
        let secs = [30, 10, 10, 20, 20, 20, 40];
        let ids: Vec<i64> = secs
            .iter()
            .enumerate()
            .map(|(i, s)| fx.add_message(text_message(&format!("m{i}"), *s, 0, &[chat])))
            .collect();
        let foreign = fx.add_message(text_message("x", 50, 0, &[other]));
        let db = fx.open_db();
        for id in &ids {
            let position = db.message_position(chat, *id).expect("position");
            let found = db.get_messages(chat, position, 1).expect("page");
            assert_eq!(found.len(), 1, "id {id} at {position}");
            assert_eq!(found[0].message_id, *id, "position {position}");
        }
        assert!(matches!(
            db.message_position(chat, foreign),
            Err(MsgsError::DatabaseError(_))
        ));
    }
}

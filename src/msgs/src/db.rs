use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::error::MsgsError;
use crate::parser::extract_text_from_attributed_body;
use crate::types::{
    apple_timestamp_to_datetime, truncate_preview, AttachmentInfo, Conversation, DbStatus, Message,
    SearchResult,
};

const CHAT_DB_PATH: &str = "Library/Messages/chat.db";
const PREVIEW_MAX_CHARS: usize = 100;

/// Row tuple returned by message queries.
type MessageRow = (
    i64,
    Option<String>,
    Option<Vec<u8>>,
    bool,
    String,
    i64,
    Option<i64>,
    bool,
    Option<String>,
    Option<String>,
    bool,
);

pub struct MessageDb {
    conn: Connection,
}

impl MessageDb {
    /// Open chat.db in read-only mode.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseNotFound` if the file doesn't exist.
    /// Returns `MsgsError::NoFullDiskAccess` if permission is denied.
    pub fn open() -> Result<Self, MsgsError> {
        let home = dirs::home_dir().ok_or(MsgsError::DatabaseError(
            "Cannot determine home directory".to_string(),
        ))?;
        Self::open_at(&home.join(CHAT_DB_PATH))
    }

    /// Open a chat.db file at `db_path` in read-only mode.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseNotFound` if the file doesn't exist.
    /// Returns `MsgsError::NoFullDiskAccess` if permission is denied.
    /// Returns `MsgsError::DatabaseError` for any other failure.
    pub fn open_at(db_path: &Path) -> Result<Self, MsgsError> {
        if !db_path.exists() {
            return Err(MsgsError::DatabaseNotFound);
        }

        let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(
            |e| {
                let msg = e.to_string();
                if msg.contains("unable to open") || msg.contains("permission") {
                    MsgsError::NoFullDiskAccess
                } else {
                    MsgsError::DatabaseError(msg)
                }
            },
        )?;

        // Verify we can actually query (use query_row, not execute_batch, for SELECT)
        conn.query_row("SELECT 1 FROM message LIMIT 1", [], |_| Ok(()))
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("authorization denied")
                    || msg.contains("unable to open")
                    || msg.contains("not authorized")
                {
                    MsgsError::NoFullDiskAccess
                } else {
                    MsgsError::DatabaseError(msg)
                }
            })?;

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
        let mut stmt = self.conn.prepare(
            "SELECT
                c.ROWID,
                c.chat_identifier,
                c.display_name,
                c.style,
                MAX(cmj.message_date) as last_date,
                COUNT(cmj.message_id) as msg_count
            FROM chat c
            JOIN chat_message_join cmj ON c.ROWID = cmj.chat_id
            GROUP BY c.ROWID
            ORDER BY last_date DESC
            LIMIT ?1 OFFSET ?2",
        )?;

        let rows = stmt.query_map([limit, offset], |row| {
            Ok((
                row.get::<_, i64>(0)?,            // chat_id
                row.get::<_, String>(1)?,         // chat_identifier
                row.get::<_, Option<String>>(2)?, // display_name
                row.get::<_, i64>(3)?,            // style
                row.get::<_, i64>(4)?,            // last_date
                row.get::<_, i64>(5)?,            // msg_count
            ))
        })?;

        let mut conversations = Vec::new();
        for row in rows {
            let (chat_id, chat_identifier, display_name, style, last_date, msg_count) = row?;
            let is_group = style == 43;

            // Get participants
            let participants = self.get_participants(chat_id)?;

            // Get last message preview
            let preview = self.get_last_message_preview(chat_id)?;

            let last_message_date = apple_timestamp_to_datetime(last_date)
                .unwrap_or_else(|| chrono::Utc::now().fixed_offset());

            conversations.push(Conversation {
                chat_id,
                chat_identifier,
                display_name: if display_name.as_deref() == Some("") {
                    None
                } else {
                    display_name
                },
                is_group,
                participants,
                last_message_date,
                last_message_preview: preview,
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
                 ORDER BY cmj.message_date DESC
                 LIMIT 1",
                [chat_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        let text = result
            .and_then(|(text, body)| {
                text.or_else(|| body.as_deref().and_then(extract_text_from_attributed_body))
            })
            .unwrap_or_default();

        Ok(truncate_preview(&text, PREVIEW_MAX_CHARS))
    }

    /// Build a `Message` from raw query row fields.
    fn build_message(&self, row: MessageRow) -> Result<Message, MsgsError> {
        let (
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
        ) = row;

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
            date: apple_timestamp_to_datetime(date)
                .unwrap_or_else(|| chrono::Utc::now().fixed_offset()),
            date_read: date_read.and_then(apple_timestamp_to_datetime),
            is_audio,
            attachments,
            reply_to_guid,
            associated_emoji,
        })
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
        let mut stmt = self.conn.prepare(
            "SELECT
                m.ROWID,
                m.text,
                m.attributedBody,
                m.is_from_me,
                COALESCE(h.id, 'Unknown') as sender_id,
                m.date,
                m.date_read,
                m.is_audio_message,
                m.reply_to_guid,
                m.associated_message_emoji,
                m.cache_has_attachments
            FROM message m
            JOIN chat_message_join cmj ON m.ROWID = cmj.message_id
            LEFT JOIN handle h ON m.handle_id = h.ROWID
            WHERE cmj.chat_id = ?1
            ORDER BY cmj.message_date DESC
            LIMIT ?2 OFFSET ?3",
        )?;

        let rows = stmt.query_map([chat_id, limit, offset], |row| {
            Ok((
                row.get::<_, i64>(0)?,             // message_id
                row.get::<_, Option<String>>(1)?,  // text
                row.get::<_, Option<Vec<u8>>>(2)?, // attributedBody
                row.get::<_, bool>(3)?,            // is_from_me
                row.get::<_, String>(4)?,          // sender_id
                row.get::<_, i64>(5)?,             // date
                row.get::<_, Option<i64>>(6)?,     // date_read
                row.get::<_, bool>(7)?,            // is_audio
                row.get::<_, Option<String>>(8)?,  // reply_to_guid
                row.get::<_, Option<String>>(9)?,  // associated_emoji
                row.get::<_, bool>(10)?,           // cache_has_attachments
            ))
        })?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(self.build_message(row?)?);
        }

        Ok(messages)
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

    /// Get an attachment's file path by ROWID, validated to be within Messages/Attachments.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseError` on query failure.
    pub fn get_attachment_path(&self, attachment_id: i64) -> Result<Option<String>, MsgsError> {
        let path: Option<String> = self
            .conn
            .query_row(
                "SELECT filename FROM attachment WHERE ROWID = ?1",
                [attachment_id],
                |row| row.get(0),
            )
            .ok()
            .flatten();

        // Validate path is within Messages/Attachments
        if let Some(ref p) = path {
            let expanded = crate::util::expand_tilde(p);
            let allowed = dirs::home_dir()
                .map(|h| h.join("Library/Messages/Attachments"))
                .unwrap_or_default();
            if !expanded.starts_with(&allowed) {
                return Ok(None);
            }
        }
        Ok(path)
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
            last_message_date: apple_timestamp_to_datetime(last_date)
                .unwrap_or_else(|| chrono::Utc::now().fixed_offset()),
            last_message_preview: preview,
            message_count: msg_count,
        })
    }

    /// Get messages around a specific date in a conversation (for search context).
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseError` on query failure.
    pub fn get_messages_around(
        &self,
        chat_id: i64,
        target_date: i64,
        before_count: i64,
        after_count: i64,
    ) -> Result<Vec<Message>, MsgsError> {
        // Get `before_count` messages before the target date + the target + `after_count` after
        let mut before = self.get_messages_by_date_query(
            chat_id,
            "cmj.message_date <= ?2 ORDER BY cmj.message_date DESC LIMIT ?3",
            target_date,
            before_count + 1, // +1 to include the target itself
        )?;
        before.reverse(); // Put in chronological order

        let after = self.get_messages_by_date_query(
            chat_id,
            "cmj.message_date > ?2 ORDER BY cmj.message_date ASC LIMIT ?3",
            target_date,
            after_count,
        )?;

        before.extend(after);
        Ok(before)
    }

    /// Get all messages in a date range for a conversation (for export).
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseError` on query failure.
    pub fn get_messages_in_date_range(
        &self,
        chat_id: i64,
        start_date: i64,
        end_date: i64,
    ) -> Result<Vec<Message>, MsgsError> {
        let mut stmt = self.conn.prepare(
            "SELECT
                m.ROWID, m.text, m.attributedBody, m.is_from_me,
                COALESCE(h.id, 'Unknown') as sender_id,
                m.date, m.date_read, m.is_audio_message,
                m.reply_to_guid, m.associated_message_emoji, m.cache_has_attachments
            FROM message m
            JOIN chat_message_join cmj ON m.ROWID = cmj.message_id
            LEFT JOIN handle h ON m.handle_id = h.ROWID
            WHERE cmj.chat_id = ?1 AND cmj.message_date BETWEEN ?2 AND ?3
            ORDER BY cmj.message_date ASC",
        )?;

        let rows = stmt.query_map([chat_id, start_date, end_date], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, bool>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, bool>(10)?,
            ))
        })?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(self.build_message(row?)?);
        }

        Ok(messages)
    }

    /// Helper for date-based message queries.
    fn get_messages_by_date_query(
        &self,
        chat_id: i64,
        where_order: &str,
        date_param: i64,
        limit: i64,
    ) -> Result<Vec<Message>, MsgsError> {
        let sql = format!(
            "SELECT
                m.ROWID, m.text, m.attributedBody, m.is_from_me,
                COALESCE(h.id, 'Unknown'), m.date, m.date_read,
                m.is_audio_message, m.reply_to_guid, m.associated_message_emoji,
                m.cache_has_attachments
            FROM message m
            JOIN chat_message_join cmj ON m.ROWID = cmj.message_id
            LEFT JOIN handle h ON m.handle_id = h.ROWID
            WHERE cmj.chat_id = ?1 AND {where_order}"
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([chat_id, date_param, limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, bool>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, bool>(10)?,
            ))
        })?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(self.build_message(row?)?);
        }

        Ok(messages)
    }

    /// Get a message with surrounding context for search results.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseError` on query failure.
    pub fn get_message_with_context(
        &self,
        message_id: i64,
        chat_id: i64,
    ) -> Result<SearchResult, MsgsError> {
        // Get the target message's date
        let msg_date: i64 = self.conn.query_row(
            "SELECT cmj.message_date FROM chat_message_join cmj
             WHERE cmj.message_id = ?1 AND cmj.chat_id = ?2",
            [message_id, chat_id],
            |row| row.get(0),
        )?;

        // Get the target message with surrounding context
        let messages = self.get_messages_around(chat_id, msg_date, 3, 3)?;

        // Find the target in results
        let target_idx = messages
            .iter()
            .position(|m| m.message_id == message_id)
            .unwrap_or(0);

        let context_before = messages[..target_idx].to_vec();
        let target = messages[target_idx].clone();
        let context_after = if target_idx + 1 < messages.len() {
            messages[target_idx + 1..].to_vec()
        } else {
            Vec::new()
        };

        // Get conversation info
        let conversation = self.get_conversation(chat_id)?;

        Ok(SearchResult {
            message: target,
            conversation,
            context_before,
            context_after,
        })
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
        assert_eq!(list[0].last_message_date.timestamp(), BASE + 50);
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
        assert_eq!(conversation.last_message_date.timestamp(), BASE + 20);
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
        let utc = messages[3].date.with_timezone(&chrono::Utc);
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
            .expect("context");
        assert_eq!(result.message.message_id, ids[3]);
        let before: Vec<i64> = result.context_before.iter().map(|m| m.message_id).collect();
        let after: Vec<i64> = result.context_after.iter().map(|m| m.message_id).collect();
        assert_eq!(before, ids[0..3].to_vec());
        assert_eq!(after, ids[4..7].to_vec());
        assert_eq!(result.conversation.chat_id, chat);
    }

    #[test]
    fn get_messages_in_date_range_is_inclusive_ascending_and_per_chat() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("c", None, 45);
        let other = fx.add_chat("other", None, 45);
        fx.add_message(text_message("before", 5, 0, &[chat]));
        fx.add_message(text_message("start", 10, 0, &[chat]));
        fx.add_message(text_message("middle", 15, 0, &[chat]));
        fx.add_message(text_message("end", 20, 0, &[chat]));
        fx.add_message(text_message("after", 25, 0, &[chat]));
        fx.add_message(text_message("foreign", 15, 0, &[other]));

        let messages = fx
            .open_db()
            .get_messages_in_date_range(chat, apple_ns(BASE + 10, 0), apple_ns(BASE + 20, 0))
            .expect("range");
        let texts: Vec<_> = messages.iter().filter_map(|m| m.text.as_deref()).collect();
        assert_eq!(texts, vec!["start", "middle", "end"]);
    }
}

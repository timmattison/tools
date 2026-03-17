use rusqlite::{Connection, OpenFlags};

use crate::error::MsgsError;
use crate::parser::extract_text_from_attributed_body;
use crate::types::{
    apple_timestamp_to_datetime, truncate_preview, AttachmentInfo, Conversation, DbStatus, Message,
    SearchResult,
};

const CHAT_DB_PATH: &str = "Library/Messages/chat.db";
const PREVIEW_MAX_CHARS: usize = 100;

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
        let db_path = home.join(CHAT_DB_PATH);

        if !db_path.exists() {
            return Err(MsgsError::DatabaseNotFound);
        }

        let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("unable to open") || msg.contains("permission") {
                    MsgsError::NoFullDiskAccess
                } else {
                    MsgsError::DatabaseError(msg)
                }
            })?;

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
                row.get::<_, i64>(0)?,           // chat_id
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
                row.get::<_, i64>(0)?,            // message_id
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
            ) = row?;

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

            messages.push(Message {
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
            });
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
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
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
            ) = row?;

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

            messages.push(Message {
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
            });
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
            ) = row?;

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

            messages.push(Message {
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
            });
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

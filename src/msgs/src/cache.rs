use rusqlite::Connection;

use crate::error::MsgsError;
use crate::parser::extract_text_from_attributed_body;

const CACHE_DB_NAME: &str = "text-cache.db";
const CACHE_DIR_NAME: &str = "msgs";

/// FTS5 text cache for searchable message text.
pub struct TextCache {
    conn: Connection,
}

impl TextCache {
    /// Open or create the text cache database.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::CacheError` on filesystem or database errors.
    pub fn open() -> Result<Self, MsgsError> {
        let cache_dir = dirs::data_local_dir()
            .ok_or_else(|| MsgsError::CacheError("Cannot determine local data dir".to_string()))?
            .join(CACHE_DIR_NAME);

        std::fs::create_dir_all(&cache_dir).map_err(|e| {
            MsgsError::CacheError(format!("Cannot create cache dir {}: {e}", cache_dir.display()))
        })?;

        let db_path = cache_dir.join(CACHE_DB_NAME);
        let conn = Connection::open(&db_path)
            .map_err(|e| MsgsError::CacheError(format!("Cannot open cache db: {e}")))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT);
             CREATE VIRTUAL TABLE IF NOT EXISTS message_text USING fts5(
                 message_id UNINDEXED,
                 chat_id UNINDEXED,
                 sender UNINDEXED,
                 text,
                 content=''
             );",
        )
        .map_err(|e| MsgsError::CacheError(format!("Cannot create cache tables: {e}")))?;

        Ok(Self { conn })
    }

    /// Check if the cache needs rebuilding by comparing message counts.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::CacheError` on query failure.
    pub fn needs_rebuild(&self, source_count: i64) -> Result<bool, MsgsError> {
        let cached_count: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM metadata WHERE key = 'message_count'",
                [],
                |row| row.get(0),
            )
            .ok();

        match cached_count {
            Some(count_str) => {
                let cached: i64 = count_str.parse().unwrap_or(0);
                Ok(cached != source_count)
            }
            None => Ok(true),
        }
    }

    /// Rebuild the cache from chat.db.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::CacheError` or `MsgsError::DatabaseError` on failure.
    pub fn rebuild(&self, chat_db: &Connection) -> Result<(), MsgsError> {
        log::info!("Rebuilding text cache...");

        // Start transaction first so DELETE is inside it (rollback restores data on failure)
        self.conn
            .execute_batch("BEGIN; DELETE FROM message_text;")
            .map_err(|e| MsgsError::CacheError(format!("Cannot begin rebuild: {e}")))?;

        match self.rebuild_inner(chat_db) {
            Ok(count) => {
                // Store the count for staleness check (after commit, separate concern)
                self.conn
                    .execute(
                        "INSERT OR REPLACE INTO metadata (key, value) VALUES ('message_count', ?1)",
                        [count.to_string()],
                    )
                    .map_err(|e| MsgsError::CacheError(format!("Cannot update metadata: {e}")))?;

                log::info!("Text cache rebuilt with {count} messages");
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Inner rebuild logic that runs inside a transaction.
    /// Returns the total message count on success (after committing).
    fn rebuild_inner(&self, chat_db: &Connection) -> Result<i64, MsgsError> {
        // Query all messages with their chat_id
        let mut stmt = chat_db
            .prepare(
                "SELECT m.ROWID, cmj.chat_id, m.is_from_me, COALESCE(h.id, 'Unknown'),
                        m.text, m.attributedBody
                 FROM message m
                 JOIN chat_message_join cmj ON m.ROWID = cmj.message_id
                 LEFT JOIN handle h ON m.handle_id = h.ROWID",
            )
            .map_err(|e| MsgsError::DatabaseError(e.to_string()))?;

        let mut insert = self
            .conn
            .prepare("INSERT INTO message_text (message_id, chat_id, sender, text) VALUES (?1, ?2, ?3, ?4)")
            .map_err(|e| MsgsError::CacheError(format!("Cannot prepare insert: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<Vec<u8>>>(5)?,
                ))
            })
            .map_err(|e| MsgsError::DatabaseError(e.to_string()))?;

        let mut count: i64 = 0;
        for row in rows {
            let (message_id, chat_id, is_from_me, sender_id, text, body) =
                row.map_err(|e| MsgsError::DatabaseError(e.to_string()))?;

            let extracted = text.or_else(|| {
                body.as_deref()
                    .and_then(extract_text_from_attributed_body)
            });

            if let Some(ref text_content) = extracted {
                if !text_content.is_empty() {
                    let sender = if is_from_me {
                        "Me".to_string()
                    } else {
                        sender_id
                    };
                    insert
                        .execute(rusqlite::params![message_id, chat_id, sender, text_content])
                        .map_err(|e| {
                            MsgsError::CacheError(format!("Cannot insert into cache: {e}"))
                        })?;
                }
            }
            count += 1;
        }

        // Commit the transaction
        self.conn
            .execute_batch("COMMIT;")
            .map_err(|e| MsgsError::CacheError(format!("Cannot commit cache: {e}")))?;

        Ok(count)
    }

    /// Search messages by text query, optionally filtered to a chat.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::CacheError` on query failure.
    pub fn search(
        &self,
        query: &str,
        chat_id: Option<i64>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<(i64, i64)>, MsgsError> {
        // FTS5 match syntax -- quote the query to handle special chars
        let fts_query = format!("\"{}\"", query.replace('"', "\"\""));

        let sql = if chat_id.is_some() {
            "SELECT message_id, chat_id FROM message_text
             WHERE message_text MATCH ?1 AND chat_id = ?2
             ORDER BY rank
             LIMIT ?3 OFFSET ?4"
        } else {
            "SELECT message_id, chat_id FROM message_text
             WHERE message_text MATCH ?1
             ORDER BY rank
             LIMIT ?2 OFFSET ?3"
        };

        let results = if let Some(cid) = chat_id {
            let mut stmt = self
                .conn
                .prepare(sql)
                .map_err(|e| MsgsError::CacheError(format!("Search query failed: {e}")))?;
            let collected = stmt
                .query_map(rusqlite::params![fts_query, cid, limit, offset], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|e| MsgsError::CacheError(format!("Search execution failed: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| MsgsError::CacheError(format!("Search result error: {e}")))?;
            collected
        } else {
            let mut stmt = self
                .conn
                .prepare(sql)
                .map_err(|e| MsgsError::CacheError(format!("Search query failed: {e}")))?;
            let collected = stmt
                .query_map(rusqlite::params![fts_query, limit, offset], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|e| MsgsError::CacheError(format!("Search execution failed: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| MsgsError::CacheError(format!("Search result error: {e}")))?;
            collected
        };

        Ok(results)
    }
}

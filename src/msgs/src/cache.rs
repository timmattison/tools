use rusqlite::Connection;

use crate::error::MsgsError;
use crate::parser::extract_text_from_attributed_body;
use crate::util::page_bounds;

/// How many rows the build reads between two progress reports.
pub const PROGRESS_INTERVAL: i64 = 5_000;

/// In-memory FTS5 index of the searchable message text.
pub struct TextCache {
    conn: Connection,
}

impl TextCache {
    /// Build the search index from `chat_db`.
    ///
    /// The index lives in memory only. It has no file, and it keeps temporary
    /// data in memory, thus it writes no copy of the message text to disk.
    /// The build runs in one transaction for the speed of the inserts.
    ///
    /// `progress` gets `(indexed, total)` once at the start, after each
    /// `report_every` rows, and once at the end. `total` is the count of
    /// joined rows. A report never repeats the one before it.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::CacheError` or `MsgsError::DatabaseError` on failure.
    pub fn build(
        chat_db: &Connection,
        report_every: i64,
        progress: &mut dyn FnMut(i64, i64),
    ) -> Result<Self, MsgsError> {
        log::info!("Building the search index in memory...");
        let conn = Connection::open_in_memory()
            .map_err(|e| MsgsError::CacheError(format!("Cannot open the index: {e}")))?;
        // Without this, SQLite can spill statement journals and sort data,
        // which hold page content, to temporary files on disk.
        conn.execute_batch(
            "PRAGMA temp_store = MEMORY;
             CREATE VIRTUAL TABLE message_text USING fts5(
                 message_id UNINDEXED,
                 chat_id UNINDEXED,
                 sender UNINDEXED,
                 text
             );",
        )
        .map_err(|e| MsgsError::CacheError(format!("Cannot create index tables: {e}")))?;

        // A transaction that drops without a commit rolls back.
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| MsgsError::CacheError(format!("Cannot begin build: {e}")))?;
        let count = Self::fill(&tx, chat_db, report_every.max(1), progress)?;
        tx.commit()
            .map_err(|e| MsgsError::CacheError(format!("Cannot commit index: {e}")))?;

        log::info!("Search index built with {count} messages");
        Ok(Self { conn })
    }

    /// Insert the text of each message in `chat_db` into `cache`. The caller
    /// holds the transaction.
    fn fill(
        cache: &Connection,
        chat_db: &Connection,
        report_every: i64,
        progress: &mut dyn FnMut(i64, i64),
    ) -> Result<i64, MsgsError> {
        let total: i64 = chat_db
            .query_row(
                "SELECT COUNT(*) FROM message m
                 JOIN chat_message_join cmj ON m.ROWID = cmj.message_id",
                [],
                |row| row.get(0),
            )
            .map_err(|e| MsgsError::DatabaseError(e.to_string()))?;
        progress(0, total);
        let mut last_reported: i64 = 0;

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

        let mut insert = cache
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

            let extracted =
                text.or_else(|| body.as_deref().and_then(extract_text_from_attributed_body));

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
            if count % report_every == 0 {
                progress(count, total);
                last_reported = count;
            }
        }
        if count != last_reported {
            progress(count, total);
        }

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
        let (offset, limit) = page_bounds(offset, limit);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixture::{apple_ns, ChatDbFixture, MessageSpec};

    fn built_cache(db: &crate::db::MessageDb) -> TextCache {
        TextCache::build(db.connection(), PROGRESS_INTERVAL, &mut |_, _| {}).expect("build")
    }

    /// Builds a fixture with two chats: "lunch" once in each chat, and
    /// "dinner" in chat A only. Returns the fixture, the two chat ids, and
    /// the ids of the "lunch" messages.
    fn two_chat_fixture() -> (ChatDbFixture, i64, i64, i64, i64) {
        let fx = ChatDbFixture::new();
        let chat_a = fx.add_chat("a", None, 45);
        let chat_b = fx.add_chat("b", None, 45);
        let lunch_a = fx.add_message(MessageSpec {
            text: Some("lunch at noon".to_string()),
            date: apple_ns(1_700_000_010, 0),
            chats: vec![chat_a],
            ..MessageSpec::default()
        });
        let lunch_b = fx.add_message(MessageSpec {
            text: Some("lunch tomorrow".to_string()),
            date: apple_ns(1_700_000_020, 0),
            chats: vec![chat_b],
            ..MessageSpec::default()
        });
        fx.add_message(MessageSpec {
            text: Some("dinner".to_string()),
            date: apple_ns(1_700_000_030, 0),
            chats: vec![chat_a],
            ..MessageSpec::default()
        });
        (fx, chat_a, chat_b, lunch_a, lunch_b)
    }

    #[test]
    fn search_returns_the_message_and_chat_of_each_hit() {
        let (fx, chat_a, chat_b, lunch_a, lunch_b) = two_chat_fixture();
        let db = fx.open_db();
        let cache = built_cache(&db);

        let mut hits = cache.search("lunch", None, 50, 0).expect("search");
        hits.sort_unstable();
        assert_eq!(hits, vec![(lunch_a, chat_a), (lunch_b, chat_b)]);
    }

    #[test]
    fn search_clamps_a_negative_limit_to_one() {
        let (fx, _chat_a, _chat_b, _lunch_a, _lunch_b) = two_chat_fixture();
        let db = fx.open_db();
        let cache = built_cache(&db);

        let hits = cache.search("lunch", None, -1, 0).expect("search");
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn search_with_a_chat_id_returns_only_the_hits_of_that_chat() {
        let (fx, _chat_a, chat_b, _lunch_a, lunch_b) = two_chat_fixture();
        let db = fx.open_db();
        let cache = built_cache(&db);

        let hits = cache.search("lunch", Some(chat_b), 50, 0).expect("search");
        assert_eq!(hits, vec![(lunch_b, chat_b)]);
    }

    #[test]
    fn a_build_from_a_source_without_a_message_table_fails() {
        let empty_source = Connection::open_in_memory().expect("open empty source");
        assert!(TextCache::build(&empty_source, PROGRESS_INTERVAL, &mut |_, _| {}).is_err());
    }

    #[test]
    fn the_index_has_no_file_and_keeps_temporary_data_in_memory() {
        let (fx, ..) = two_chat_fixture();
        let db = fx.open_db();
        let cache = built_cache(&db);
        let file: String = cache
            .conn
            .query_row(
                "SELECT file FROM pragma_database_list WHERE name = 'main'",
                [],
                |row| row.get(0),
            )
            .expect("database list");
        assert_eq!(file, "");
        let temp_store: i64 = cache
            .conn
            .query_row("PRAGMA temp_store", [], |row| row.get(0))
            .expect("temp_store");
        assert_eq!(temp_store, 2);
    }

    #[test]
    fn build_reports_progress_up_to_the_total() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("a", None, 45);
        for n in 0..3 {
            fx.add_message(MessageSpec {
                text: Some(format!("m{n}")),
                date: apple_ns(1_700_000_010 + n, 0),
                chats: vec![chat],
                ..MessageSpec::default()
            });
        }
        let db = fx.open_db();
        let mut reports = Vec::new();
        TextCache::build(db.connection(), 1, &mut |indexed, total| {
            reports.push((indexed, total));
        })
        .expect("build");
        assert_eq!(reports, vec![(0, 3), (1, 3), (2, 3), (3, 3)]);
    }
}

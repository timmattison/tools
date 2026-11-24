use std::path::Path;

use rusqlite::Connection;

use crate::error::MsgsError;
use crate::parser::extract_text_from_attributed_body;
use crate::util::page_bounds;

/// The version of the cache schema, kept in `PRAGMA user_version`. A cache
/// file with a different version is dropped and made again.
const CACHE_SCHEMA_VERSION: i64 = 1;

/// How many rows the rebuild reads between two progress reports.
pub const PROGRESS_INTERVAL: i64 = 5_000;

/// The metadata key that holds the source fingerprint of the last rebuild.
const FINGERPRINT_KEY: &str = "source_fingerprint";

/// A fingerprint of the rows that the rebuild reads from `chat_db`:
/// `"{joined}:{max_rowid}:{max_edited}"`. `joined` is the count of
/// `message JOIN chat_message_join` rows, `max_rowid` is the highest message
/// ROWID of that join, and `max_edited` is the latest `date_edited` (0 when the
/// column is absent in an old chat.db). An insert, a delete, and an edit each
/// change it.
fn source_fingerprint(chat_db: &Connection) -> Result<String, MsgsError> {
    let db_error = |e: rusqlite::Error| MsgsError::DatabaseError(e.to_string());
    let (joined, max_rowid): (i64, i64) = chat_db
        .query_row(
            "SELECT COUNT(*), COALESCE(MAX(m.ROWID), 0)
             FROM message m
             JOIN chat_message_join cmj ON m.ROWID = cmj.message_id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error)?;
    let has_edited: i64 = chat_db
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('message') WHERE name = 'date_edited'",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    let max_edited: i64 = if has_edited > 0 {
        chat_db
            .query_row(
                "SELECT COALESCE(MAX(date_edited), 0) FROM message",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?
    } else {
        0
    };
    Ok(format!("{joined}:{max_rowid}:{max_edited}"))
}

/// FTS5 text cache for searchable message text.
pub struct TextCache {
    conn: Connection,
}

impl TextCache {
    /// Open or create the text cache database file at `db_path`.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::CacheError` on database errors.
    pub fn open_at(db_path: &Path) -> Result<Self, MsgsError> {
        let conn = Connection::open(db_path)
            .map_err(|e| MsgsError::CacheError(format!("Cannot open cache db: {e}")))?;

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| MsgsError::CacheError(format!("Cannot read cache version: {e}")))?;
        if version != CACHE_SCHEMA_VERSION {
            // A cache of a different schema holds nothing that the rebuild cannot
            // make again. The first release wrote a contentless table, which
            // returns NULL for each column and refuses each DELETE.
            conn.execute_batch(
                "DROP TABLE IF EXISTS message_text;
                 DROP TABLE IF EXISTS metadata;",
            )
            .map_err(|e| MsgsError::CacheError(format!("Cannot drop old cache tables: {e}")))?;
        }

        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT);
             CREATE VIRTUAL TABLE IF NOT EXISTS message_text USING fts5(
                 message_id UNINDEXED,
                 chat_id UNINDEXED,
                 sender UNINDEXED,
                 text
             );
             PRAGMA user_version = {CACHE_SCHEMA_VERSION};"
        ))
        .map_err(|e| MsgsError::CacheError(format!("Cannot create cache tables: {e}")))?;

        Ok(Self { conn })
    }

    /// Check if the cache needs rebuilding by comparing the stored source
    /// fingerprint of `chat_db` with the current one. A cache with no stored
    /// fingerprint needs a rebuild.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseError` when the fingerprint query fails.
    pub fn needs_rebuild(&self, chat_db: &Connection) -> Result<bool, MsgsError> {
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM metadata WHERE key = ?1",
                [FINGERPRINT_KEY],
                |row| row.get(0),
            )
            .ok();

        match stored {
            Some(stored) => Ok(stored != source_fingerprint(chat_db)?),
            None => Ok(true),
        }
    }

    /// Rebuild the cache from chat.db.
    ///
    /// The rebuild runs in one transaction. A failure rolls it back, thus the
    /// cache keeps its earlier rows and no transaction stays open.
    ///
    /// `progress` gets `(indexed, total)` once at the start, after each
    /// `report_every` rows, and once at the end. `total` is the count of
    /// joined rows. A report never repeats the one before it.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::CacheError` or `MsgsError::DatabaseError` on failure.
    pub fn rebuild(
        &self,
        chat_db: &Connection,
        report_every: i64,
        progress: &mut dyn FnMut(i64, i64),
    ) -> Result<(), MsgsError> {
        log::info!("Rebuilding text cache...");

        // A transaction that drops without a commit rolls back.
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| MsgsError::CacheError(format!("Cannot begin rebuild: {e}")))?;
        let count = Self::fill(&tx, chat_db, report_every.max(1), progress)?;
        tx.commit()
            .map_err(|e| MsgsError::CacheError(format!("Cannot commit cache: {e}")))?;

        log::info!("Text cache rebuilt with {count} messages");
        Ok(())
    }

    /// Replace the rows of the cache with the text of each message in
    /// `chat_db`, and store the source fingerprint for the staleness check. The caller
    /// holds the transaction.
    fn fill(
        cache: &Connection,
        chat_db: &Connection,
        report_every: i64,
        progress: &mut dyn FnMut(i64, i64),
    ) -> Result<i64, MsgsError> {
        cache
            .execute_batch("DELETE FROM message_text;")
            .map_err(|e| MsgsError::CacheError(format!("Cannot clear cache: {e}")))?;

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

        cache
            .execute(
                "INSERT OR REPLACE INTO metadata (key, value) VALUES (?1, ?2)",
                [FINGERPRINT_KEY, &source_fingerprint(chat_db)?],
            )
            .map_err(|e| MsgsError::CacheError(format!("Cannot update metadata: {e}")))?;

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

    #[test]
    fn rebuild_stores_the_source_fingerprint() {
        let fx = ChatDbFixture::new();
        let chat_a = fx.add_chat("a", None, 45);
        fx.add_message(MessageSpec {
            text: Some("one".to_string()),
            date: apple_ns(1_700_000_010, 0),
            chats: vec![chat_a],
            ..MessageSpec::default()
        });
        let db = fx.open_db();
        let cache = TextCache::open_at(&fx.dir().join("text-cache.db")).expect("open cache");

        assert!(cache.needs_rebuild(db.connection()).expect("needs before"));
        cache
            .rebuild(db.connection(), PROGRESS_INTERVAL, &mut |_, _| {})
            .expect("rebuild");
        assert!(!cache.needs_rebuild(db.connection()).expect("needs after"));
    }

    #[test]
    fn a_rebuild_after_an_edit_stores_the_new_fingerprint() {
        let (fx, _chat_a, _chat_b, lunch_a, _lunch_b) = two_chat_fixture();
        let db = fx.open_db();
        let cache = rebuilt_cache(&fx, &db);
        fx.execute(
            "UPDATE message SET text = 'lunch at one', date_edited = ?1 WHERE ROWID = ?2",
            rusqlite::params![apple_ns(1_700_000_100, 0), lunch_a],
        );
        assert!(needs_rebuild_as_the_command_does(&cache, &db));
        cache
            .rebuild(db.connection(), PROGRESS_INTERVAL, &mut |_, _| {})
            .expect("second rebuild");
        assert!(!needs_rebuild_as_the_command_does(&cache, &db));
    }

    #[test]
    fn a_chat_db_without_date_edited_still_has_a_fingerprint() {
        let old = Connection::open_in_memory().expect("open old chat.db");
        old.execute_batch(
            "CREATE TABLE message (ROWID INTEGER PRIMARY KEY, text TEXT);
             CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
             INSERT INTO message (ROWID, text) VALUES (1, 'x');
             INSERT INTO chat_message_join VALUES (1, 1);",
        )
        .expect("create old schema");
        assert_eq!(source_fingerprint(&old).expect("fingerprint"), "1:1:0");
    }

    #[test]
    fn a_message_that_arrives_after_the_rows_are_read_needs_a_rebuild() {
        let (fx, chat_a, ..) = two_chat_fixture();
        let db = fx.open_db();
        let cache = TextCache::open_at(&fx.dir().join("text-cache.db")).expect("open cache");
        let mut arrived = false;
        cache
            .rebuild(db.connection(), PROGRESS_INTERVAL, &mut |indexed, total| {
                // The last report comes after the stream of rows ends.
                if indexed == total && !arrived {
                    arrived = true;
                    fx.add_message(MessageSpec {
                        text: Some("a late message".to_string()),
                        date: apple_ns(1_700_000_050, 0),
                        chats: vec![chat_a],
                        ..MessageSpec::default()
                    });
                }
            })
            .expect("rebuild");
        assert!(arrived, "the rebuild gave no last report");
        assert!(needs_rebuild_as_the_command_does(&cache, &db));
    }

    /// The staleness check as `check_db_access` runs it.
    fn needs_rebuild_as_the_command_does(cache: &TextCache, db: &crate::db::MessageDb) -> bool {
        cache.needs_rebuild(db.connection()).expect("needs rebuild")
    }

    fn rebuilt_cache(fx: &ChatDbFixture, db: &crate::db::MessageDb) -> TextCache {
        let cache = TextCache::open_at(&fx.dir().join("text-cache.db")).expect("open cache");
        cache
            .rebuild(db.connection(), PROGRESS_INTERVAL, &mut |_, _| {})
            .expect("rebuild");
        cache
    }

    #[test]
    fn a_message_in_two_chats_needs_no_rebuild_after_a_rebuild() {
        let fx = ChatDbFixture::new();
        let chat_a = fx.add_chat("a", None, 45);
        let chat_b = fx.add_chat("b", None, 45);
        fx.add_message(MessageSpec {
            text: Some("shared".to_string()),
            date: apple_ns(1_700_000_010, 0),
            chats: vec![chat_a, chat_b],
            ..MessageSpec::default()
        });
        let db = fx.open_db();
        let cache = rebuilt_cache(&fx, &db);
        assert!(!needs_rebuild_as_the_command_does(&cache, &db));
    }

    #[test]
    fn a_message_in_no_chat_needs_no_rebuild_after_a_rebuild() {
        let fx = ChatDbFixture::new();
        let chat_a = fx.add_chat("a", None, 45);
        fx.add_message(MessageSpec {
            text: Some("in a chat".to_string()),
            date: apple_ns(1_700_000_010, 0),
            chats: vec![chat_a],
            ..MessageSpec::default()
        });
        fx.add_message(MessageSpec {
            text: Some("orphan".to_string()),
            date: apple_ns(1_700_000_020, 0),
            chats: vec![],
            ..MessageSpec::default()
        });
        let db = fx.open_db();
        let cache = rebuilt_cache(&fx, &db);
        assert!(!needs_rebuild_as_the_command_does(&cache, &db));
    }

    #[test]
    fn an_edit_that_keeps_the_count_needs_a_rebuild() {
        let (fx, _chat_a, _chat_b, lunch_a, _lunch_b) = two_chat_fixture();
        let db = fx.open_db();
        let cache = rebuilt_cache(&fx, &db);
        fx.execute(
            "UPDATE message SET text = 'lunch at one', date_edited = ?1 WHERE ROWID = ?2",
            rusqlite::params![apple_ns(1_700_000_100, 0), lunch_a],
        );
        assert!(needs_rebuild_as_the_command_does(&cache, &db));
    }

    #[test]
    fn a_delete_and_an_insert_that_keep_the_count_need_a_rebuild() {
        let (fx, chat_a, _chat_b, _lunch_a, lunch_b) = two_chat_fixture();
        let db = fx.open_db();
        let cache = rebuilt_cache(&fx, &db);
        fx.execute(
            "DELETE FROM chat_message_join WHERE message_id = ?1",
            [lunch_b],
        );
        fx.execute("DELETE FROM message WHERE ROWID = ?1", [lunch_b]);
        fx.add_message(MessageSpec {
            text: Some("a new message".to_string()),
            date: apple_ns(1_700_000_040, 0),
            chats: vec![chat_a],
            ..MessageSpec::default()
        });
        assert!(needs_rebuild_as_the_command_does(&cache, &db));
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
        let cache = TextCache::open_at(&fx.dir().join("text-cache.db")).expect("open cache");
        cache
            .rebuild(db.connection(), PROGRESS_INTERVAL, &mut |_, _| {})
            .expect("rebuild");

        let mut hits = cache.search("lunch", None, 50, 0).expect("search");
        hits.sort_unstable();
        assert_eq!(hits, vec![(lunch_a, chat_a), (lunch_b, chat_b)]);
    }

    #[test]
    fn search_clamps_a_negative_limit_to_one() {
        let (fx, _chat_a, _chat_b, _lunch_a, _lunch_b) = two_chat_fixture();
        let db = fx.open_db();
        let cache = rebuilt_cache(&fx, &db);

        let hits = cache.search("lunch", None, -1, 0).expect("search");
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn search_with_a_chat_id_returns_only_the_hits_of_that_chat() {
        let (fx, _chat_a, chat_b, _lunch_a, lunch_b) = two_chat_fixture();
        let db = fx.open_db();
        let cache = TextCache::open_at(&fx.dir().join("text-cache.db")).expect("open cache");
        cache
            .rebuild(db.connection(), PROGRESS_INTERVAL, &mut |_, _| {})
            .expect("rebuild");

        let hits = cache.search("lunch", Some(chat_b), 50, 0).expect("search");
        assert_eq!(hits, vec![(lunch_b, chat_b)]);
    }

    #[test]
    fn a_second_rebuild_replaces_the_rows_of_the_first() {
        let (fx, chat_a, chat_b, lunch_a, lunch_b) = two_chat_fixture();
        let db = fx.open_db();
        let cache = TextCache::open_at(&fx.dir().join("text-cache.db")).expect("open cache");
        cache
            .rebuild(db.connection(), PROGRESS_INTERVAL, &mut |_, _| {})
            .expect("first rebuild");
        cache
            .rebuild(db.connection(), PROGRESS_INTERVAL, &mut |_, _| {})
            .expect("second rebuild");

        let mut hits = cache.search("lunch", None, 50, 0).expect("search");
        hits.sort_unstable();
        assert_eq!(hits, vec![(lunch_a, chat_a), (lunch_b, chat_b)]);
    }

    #[test]
    fn a_cache_file_with_the_old_contentless_table_rebuilds_and_searches() {
        let (fx, chat_a, chat_b, lunch_a, lunch_b) = two_chat_fixture();
        let cache_path = fx.dir().join("text-cache.db");
        write_old_contentless_cache(&cache_path);
        let db = fx.open_db();
        let cache = TextCache::open_at(&cache_path).expect("open cache");
        cache
            .rebuild(db.connection(), PROGRESS_INTERVAL, &mut |_, _| {})
            .expect("rebuild");

        let mut hits = cache.search("lunch", None, 50, 0).expect("search");
        hits.sort_unstable();
        assert_eq!(hits, vec![(lunch_a, chat_a), (lunch_b, chat_b)]);
    }

    /// Writes a cache file with the contentless table of the first release
    /// of msgs, and one row in it.
    fn write_old_contentless_cache(cache_path: &Path) {
        let old = Connection::open(cache_path).expect("open old cache");
        old.execute_batch(
            "CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT);
             CREATE VIRTUAL TABLE message_text USING fts5(
                 message_id UNINDEXED, chat_id UNINDEXED, sender UNINDEXED, text,
                 content=''
             );
             INSERT INTO message_text (message_id, chat_id, sender, text)
                 VALUES (1, 1, 'Me', 'stale lunch');
             INSERT INTO metadata (key, value) VALUES ('message_count', '3');",
        )
        .expect("create old schema");
    }

    #[test]
    fn a_rebuild_leaves_no_open_transaction_when_it_succeeds_or_fails() {
        let (fx, ..) = two_chat_fixture();
        let db = fx.open_db();
        let cache_path = fx.dir().join("text-cache.db");
        write_old_contentless_cache(&cache_path);
        let cache = TextCache::open_at(&cache_path).expect("open cache");
        let _ = cache.rebuild(db.connection(), PROGRESS_INTERVAL, &mut |_, _| {});
        assert!(
            cache.conn.is_autocommit(),
            "a rebuild of the old cache left a transaction open"
        );

        // A source with no message table makes the rebuild fail.
        let empty_source = Connection::open_in_memory().expect("open empty source");
        assert!(cache
            .rebuild(&empty_source, PROGRESS_INTERVAL, &mut |_, _| {})
            .is_err());
        assert!(
            cache.conn.is_autocommit(),
            "a failed rebuild left a transaction open"
        );
    }

    #[test]
    fn rebuild_reports_progress_up_to_the_total() {
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
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = TextCache::open_at(&dir.path().join("cache.db")).expect("open cache");
        let mut reports = Vec::new();
        cache
            .rebuild(db.connection(), 1, &mut |indexed, total| {
                reports.push((indexed, total));
            })
            .expect("rebuild");
        assert_eq!(reports, vec![(0, 3), (1, 3), (2, 3), (3, 3)]);
    }
}

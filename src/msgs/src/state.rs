//! The state of the app and the logic of the commands, without Tauri types,
//! so tests can drive it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;

use crate::cache::TextCache;
use crate::db::MessageDb;
use crate::error::MsgsError;
use crate::export::export_conversation_range;
use crate::types::{Conversation, DbStatus, ExportResult, Message, SearchResult};

/// Where chat.db lives, relative to the home directory.
const CHAT_DB_PATH: &str = "Library/Messages/chat.db";
const CACHE_DB_NAME: &str = "text-cache.db";
const CACHE_DIR_NAME: &str = "msgs";

/// The two files that the app reads and writes.
#[derive(Debug, Clone)]
pub struct DataPaths {
    pub chat_db: PathBuf,
    pub cache_db: PathBuf,
}

impl DataPaths {
    /// The paths of a real install. Creates the folder of the cache.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseError` when the home directory is unknown.
    /// Returns `MsgsError::CacheError` when the local data directory is
    /// unknown or the cache folder cannot be created.
    pub fn default_paths() -> Result<Self, MsgsError> {
        let home = dirs::home_dir().ok_or(MsgsError::DatabaseError(
            "Cannot determine home directory".to_string(),
        ))?;
        let cache_dir = dirs::data_local_dir()
            .ok_or_else(|| MsgsError::CacheError("Cannot determine local data dir".to_string()))?
            .join(CACHE_DIR_NAME);
        std::fs::create_dir_all(&cache_dir).map_err(|e| {
            MsgsError::CacheError(format!(
                "Cannot create cache dir {}: {e}",
                cache_dir.display()
            ))
        })?;
        Ok(Self {
            chat_db: home.join(CHAT_DB_PATH),
            cache_db: cache_dir.join(CACHE_DB_NAME),
        })
    }
}

/// The state of the search index, as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CacheStatus {
    Building { indexed: i64, total: i64 },
    Ready,
}

/// The search index and its state.
enum CacheSlot {
    NotStarted,
    Ready(TextCache),
}

/// The app state: the open chat.db and the search index.
pub struct AppState {
    paths: Result<DataPaths, String>,
    db: Mutex<Option<MessageDb>>,
    cache: Arc<Mutex<CacheSlot>>,
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, MsgsError> {
    mutex
        .lock()
        .map_err(|e| MsgsError::DatabaseError(e.to_string()))
}

fn not_initialized() -> MsgsError {
    MsgsError::DatabaseError("Database not initialized".to_string())
}

impl AppState {
    /// Make the state. `paths` is an error text when the home directory is
    /// unknown, and the first `open` reports it.
    pub fn new(paths: Result<DataPaths, String>) -> Self {
        Self {
            paths,
            db: Mutex::new(None),
            cache: Arc::new(Mutex::new(CacheSlot::NotStarted)),
        }
    }

    /// Open chat.db and the search index. Calls `on_status` with each change
    /// of the index state.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError` when the access check, the cache or its rebuild
    /// fails. A chat.db that cannot open gives `Ok` with `accessible: false`.
    pub fn open(
        &self,
        on_status: impl Fn(CacheStatus) + Send + 'static,
    ) -> Result<DbStatus, MsgsError> {
        let mut db_slot = lock(&self.db)?;
        if let Some(db) = db_slot.as_ref() {
            return db.check_access();
        }
        let inaccessible = |error: String| DbStatus {
            accessible: false,
            message_count: None,
            error: Some(error),
        };
        let paths = match &self.paths {
            Ok(paths) => paths,
            Err(error) => return Ok(inaccessible(error.clone())),
        };
        match MessageDb::open_at(&paths.chat_db) {
            Ok(db) => {
                let status = db.check_access()?;

                // Auto-initialize the text cache so search works immediately
                let cache = TextCache::open_at(&paths.cache_db)?;
                if cache.needs_rebuild(db.connection())? {
                    on_status(CacheStatus::Building {
                        indexed: 0,
                        total: status.message_count.unwrap_or(0),
                    });
                    cache.rebuild(db.connection())?;
                }
                *lock(&self.cache)? = CacheSlot::Ready(cache);
                on_status(CacheStatus::Ready);
                *db_slot = Some(db);
                Ok(status)
            }
            Err(e) => Ok(inaccessible(e.to_string())),
        }
    }

    /// List conversations, paginated.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError` when the database is not open or the query fails.
    pub fn list_conversations(
        &self,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<Conversation>, MsgsError> {
        let guard = lock(&self.db)?;
        guard
            .as_ref()
            .ok_or_else(not_initialized)?
            .list_conversations(offset, limit)
    }

    /// Get the messages of a chat, paginated.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError` when the database is not open or the query fails.
    pub fn get_messages(
        &self,
        chat_id: i64,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<Message>, MsgsError> {
        let guard = lock(&self.db)?;
        guard
            .as_ref()
            .ok_or_else(not_initialized)?
            .get_messages(chat_id, offset, limit)
    }

    /// Search the text of the messages.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::CacheError` when the index is not ready, and
    /// `MsgsError` when a query fails.
    pub fn search(
        &self,
        query: &str,
        chat_id: Option<i64>,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<SearchResult>, MsgsError> {
        let hits = {
            let slot = lock(&self.cache)?;
            match &*slot {
                CacheSlot::Ready(cache) => cache.search(query, chat_id, limit, offset)?,
                CacheSlot::NotStarted => {
                    return Err(MsgsError::CacheError(
                        "Text cache not initialized".to_string(),
                    ));
                }
            }
        };
        let guard = lock(&self.db)?;
        let db = guard.as_ref().ok_or_else(not_initialized)?;
        let mut results = Vec::new();
        for (message_id, hit_chat_id) in hits {
            if let Ok(context) = db.get_message_with_context(message_id, hit_chat_id) {
                results.push(context);
            }
        }
        Ok(results)
    }

    /// Export a range of messages to `export_path`.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError` when the database is not open or the export fails.
    pub fn export(
        &self,
        chat_id: i64,
        start_message_id: i64,
        end_message_id: i64,
        include_attachments: bool,
        export_path: &str,
    ) -> Result<ExportResult, MsgsError> {
        let guard = lock(&self.db)?;
        let db = guard.as_ref().ok_or_else(not_initialized)?;
        // Without a home directory there is no root, and the export copies no attachment.
        let attachments_root = crate::util::default_attachments_root().unwrap_or_default();
        export_conversation_range(
            db,
            chat_id,
            start_message_id,
            end_message_id,
            include_attachments,
            std::path::Path::new(export_path),
            &attachments_root,
        )
    }
}

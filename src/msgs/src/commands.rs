use std::sync::Mutex;

use tauri::State;

use crate::cache::TextCache;
use crate::db::MessageDb;
use crate::error::MsgsError;
use crate::export::export_conversation_range;
use crate::types::{Conversation, DbStatus, ExportResult, Message, SearchResult};

pub struct AppStateInner {
    pub db: Option<MessageDb>,
    pub cache: Option<TextCache>,
}

pub struct AppState {
    pub inner: Mutex<AppStateInner>,
}

#[tauri::command]
#[specta::specta]
pub fn check_db_access(state: State<'_, AppState>) -> Result<DbStatus, MsgsError> {
    let mut lock = state
        .inner
        .lock()
        .map_err(|e| MsgsError::DatabaseError(e.to_string()))?;
    match lock.db.as_ref() {
        Some(db) => db.check_access(),
        None => match MessageDb::open() {
            Ok(db) => {
                let status = db.check_access()?;

                // Auto-initialize the text cache so search works immediately
                let cache = TextCache::open()?;
                if cache.needs_rebuild(status.message_count.unwrap_or(0))? {
                    cache.rebuild(db.connection())?;
                }

                lock.cache = Some(cache);
                lock.db = Some(db);
                Ok(status)
            }
            Err(e) => Ok(DbStatus {
                accessible: false,
                message_count: None,
                error: Some(e.to_string()),
            }),
        },
    }
}

#[tauri::command]
#[specta::specta]
pub fn list_conversations(
    state: State<'_, AppState>,
    offset: i64,
    limit: i64,
) -> Result<Vec<Conversation>, MsgsError> {
    let lock = state
        .inner
        .lock()
        .map_err(|e| MsgsError::DatabaseError(e.to_string()))?;
    let db = lock
        .db
        .as_ref()
        .ok_or(MsgsError::DatabaseError("Database not initialized".to_string()))?;
    db.list_conversations(offset, limit)
}

#[tauri::command]
#[specta::specta]
pub fn get_messages(
    state: State<'_, AppState>,
    chat_id: i64,
    offset: i64,
    limit: i64,
) -> Result<Vec<Message>, MsgsError> {
    let lock = state
        .inner
        .lock()
        .map_err(|e| MsgsError::DatabaseError(e.to_string()))?;
    let db = lock
        .db
        .as_ref()
        .ok_or(MsgsError::DatabaseError("Database not initialized".to_string()))?;
    db.get_messages(chat_id, offset, limit)
}

#[tauri::command]
#[specta::specta]
pub fn get_version() -> String {
    buildinfo::version_string!().to_string()
}

#[tauri::command]
#[specta::specta]
pub fn search_messages(
    state: State<'_, AppState>,
    query: String,
    chat_id: Option<i64>,
    offset: i64,
    limit: i64,
) -> Result<Vec<SearchResult>, MsgsError> {
    let lock = state
        .inner
        .lock()
        .map_err(|e| MsgsError::DatabaseError(e.to_string()))?;
    let cache = lock
        .cache
        .as_ref()
        .ok_or(MsgsError::CacheError("Text cache not initialized".to_string()))?;
    let db = lock
        .db
        .as_ref()
        .ok_or(MsgsError::DatabaseError("Database not initialized".to_string()))?;

    let hits = cache.search(&query, chat_id, limit, offset)?;

    let mut results = Vec::new();
    for (message_id, hit_chat_id) in hits {
        if let Ok(context) = db.get_message_with_context(message_id, hit_chat_id) {
            results.push(context);
        }
    }

    Ok(results)
}

#[tauri::command]
#[specta::specta]
pub fn rebuild_text_cache(state: State<'_, AppState>) -> Result<(), MsgsError> {
    let mut lock = state
        .inner
        .lock()
        .map_err(|e| MsgsError::DatabaseError(e.to_string()))?;
    let db = lock
        .db
        .as_ref()
        .ok_or(MsgsError::DatabaseError("Database not initialized".to_string()))?;

    let cache = TextCache::open()?;
    cache.rebuild(db.connection())?;
    lock.cache = Some(cache);

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn get_attachment(
    state: State<'_, AppState>,
    attachment_id: i64,
) -> Result<Option<String>, MsgsError> {
    let lock = state
        .inner
        .lock()
        .map_err(|e| MsgsError::DatabaseError(e.to_string()))?;
    let db = lock
        .db
        .as_ref()
        .ok_or(MsgsError::DatabaseError("Database not initialized".to_string()))?;
    db.get_attachment_path(attachment_id)
}

#[tauri::command]
#[specta::specta]
pub fn export_messages(
    state: State<'_, AppState>,
    chat_id: i64,
    start_message_date: i64,
    end_message_date: i64,
    include_attachments: bool,
    export_path: String,
) -> Result<ExportResult, MsgsError> {
    let lock = state
        .inner
        .lock()
        .map_err(|e| MsgsError::DatabaseError(e.to_string()))?;
    let db = lock
        .db
        .as_ref()
        .ok_or(MsgsError::DatabaseError("Database not initialized".to_string()))?;

    let export_dir = std::path::Path::new(&export_path);
    export_conversation_range(
        db,
        chat_id,
        start_message_date,
        end_message_date,
        include_attachments,
        export_dir,
    )
}

use std::sync::Mutex;

use tauri::State;

use crate::db::MessageDb;
use crate::error::MsgsError;
use crate::types::{Conversation, DbStatus, Message};

pub struct AppState {
    pub db: Mutex<Option<MessageDb>>,
}

#[tauri::command]
#[specta::specta]
pub fn check_db_access(state: State<'_, AppState>) -> Result<DbStatus, MsgsError> {
    let mut db_lock = state
        .db
        .lock()
        .map_err(|e| MsgsError::DatabaseError(e.to_string()))?;
    if db_lock.is_none() {
        match MessageDb::open() {
            Ok(db) => {
                let status = db.check_access()?;
                *db_lock = Some(db);
                Ok(status)
            }
            Err(e) => Ok(DbStatus {
                accessible: false,
                message_count: None,
                error: Some(e.to_string()),
            }),
        }
    } else {
        db_lock.as_ref().unwrap().check_access()
    }
}

#[tauri::command]
#[specta::specta]
pub fn list_conversations(
    state: State<'_, AppState>,
    offset: i64,
    limit: i64,
) -> Result<Vec<Conversation>, MsgsError> {
    let db_lock = state
        .db
        .lock()
        .map_err(|e| MsgsError::DatabaseError(e.to_string()))?;
    let db = db_lock
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
    let db_lock = state
        .db
        .lock()
        .map_err(|e| MsgsError::DatabaseError(e.to_string()))?;
    let db = db_lock
        .as_ref()
        .ok_or(MsgsError::DatabaseError("Database not initialized".to_string()))?;
    db.get_messages(chat_id, offset, limit)
}

#[tauri::command]
#[specta::specta]
pub fn get_version() -> String {
    buildinfo::version_string!().to_string()
}

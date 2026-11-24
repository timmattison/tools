use tauri::{Emitter, State};

use crate::error::MsgsError;
use crate::state::{AppState, CacheStatus};
use crate::types::{Conversation, DbStatus, ExportResult, Message, SearchResult};

/// The event that carries each change of the search index state.
const CACHE_STATUS_EVENT: &str = "cache-status";

#[tauri::command(async)]
#[specta::specta]
pub fn check_db_access(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<DbStatus, MsgsError> {
    state.open(move |status| {
        let _ = app.emit(CACHE_STATUS_EVENT, status);
    })
}

#[tauri::command(async)]
#[specta::specta]
pub fn get_cache_status(state: State<'_, AppState>) -> Result<CacheStatus, MsgsError> {
    state.cache_status()
}

#[tauri::command(async)]
#[specta::specta]
pub fn list_conversations(
    state: State<'_, AppState>,
    offset: i64,
    limit: i64,
) -> Result<Vec<Conversation>, MsgsError> {
    state.list_conversations(offset, limit)
}

#[tauri::command(async)]
#[specta::specta]
pub fn get_messages(
    state: State<'_, AppState>,
    chat_id: i64,
    offset: i64,
    limit: i64,
) -> Result<Vec<Message>, MsgsError> {
    state.get_messages(chat_id, offset, limit)
}

#[tauri::command(async)]
#[specta::specta]
pub fn get_version() -> String {
    buildinfo::version_string!().to_string()
}

#[tauri::command(async)]
#[specta::specta]
pub fn search_messages(
    state: State<'_, AppState>,
    query: String,
    chat_id: Option<i64>,
    offset: i64,
    limit: i64,
) -> Result<Vec<SearchResult>, MsgsError> {
    state.search(&query, chat_id, offset, limit)
}

#[tauri::command(async)]
#[specta::specta]
pub fn export_messages(
    state: State<'_, AppState>,
    chat_id: i64,
    start_message_id: i64,
    end_message_id: i64,
    include_attachments: bool,
    export_path: String,
) -> Result<ExportResult, MsgsError> {
    state.export(
        chat_id,
        start_message_id,
        end_message_id,
        include_attachments,
        &export_path,
    )
}

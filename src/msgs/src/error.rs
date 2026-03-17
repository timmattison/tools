use serde::Serialize;

#[derive(Debug, thiserror::Error, Serialize, specta::Type)]
#[allow(dead_code, reason = "variants reserved for future use in parser and attachment flows")]
pub enum MsgsError {
    #[error("Full Disk Access not granted. Open System Settings > Privacy & Security > Full Disk Access and add this app.")]
    NoFullDiskAccess,

    #[error("chat.db not found at ~/Library/Messages/chat.db. Is Messages configured?")]
    DatabaseNotFound,

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Failed to parse message body: {0}")]
    ParseError(String),

    #[error("Export failed: {0}")]
    ExportError(String),

    #[error("Attachment not found: {0}")]
    AttachmentNotFound(String),

    #[error("Text cache error: {0}")]
    CacheError(String),
}

impl From<rusqlite::Error> for MsgsError {
    fn from(e: rusqlite::Error) -> Self {
        MsgsError::DatabaseError(e.to_string())
    }
}

// Note: InvokeError conversion is provided automatically by Tauri's blanket
// `impl<T: Serialize> From<T> for InvokeError`. No manual impl needed here.

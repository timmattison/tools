use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "variants reserved for future use in parser and attachment flows"
    )
)]
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

/// Serialize the error as its message. Tauri sends the serialized error as
/// the rejection value of the invoke (through its blanket
/// `impl<T: Serialize> From<T> for InvokeError`), and the frontend shows that
/// value as text.
impl Serialize for MsgsError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each variant reaches the frontend as its message, because Tauri sends
    /// the serialized error as the rejection value of the invoke.
    #[test]
    fn each_variant_serializes_to_its_message() {
        let variants = [
            MsgsError::NoFullDiskAccess,
            MsgsError::DatabaseNotFound,
            MsgsError::DatabaseError("db".to_string()),
            MsgsError::ParseError("parse".to_string()),
            MsgsError::ExportError("export".to_string()),
            MsgsError::AttachmentNotFound("att".to_string()),
            MsgsError::CacheError("cache".to_string()),
        ];
        for variant in variants {
            let value = serde_json::to_value(&variant).expect("serialize the error");
            assert_eq!(value, serde_json::Value::String(variant.to_string()));
        }
    }
}

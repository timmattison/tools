use serde::Serialize;
use thiserror::Error;

/// Errors that can occur in the NLE application.
#[derive(Debug, Error)]
pub enum NleError {
    #[error("Failed to load recording: {0}")]
    LoadError(String),

    #[error("Failed to save recording: {0}")]
    SaveError(String),

    #[error("Export failed: {0}")]
    ExportError(String),

    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Invalid format: {0}")]
    InvalidFormat(String),
}

// Tauri requires errors to implement Serialize
impl Serialize for NleError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

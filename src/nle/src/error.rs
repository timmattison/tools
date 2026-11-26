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

    #[error("Background task failed: {0}")]
    TaskError(String),

    #[error("Invalid time: {0} (a time must be finite and not below zero)")]
    InvalidTime(f64),

    #[error("Invalid project: {0}")]
    InvalidProject(String),

    #[error("Edit refused: {0}")]
    EditError(String),

    #[error("Render failed: {0}")]
    RenderError(String),
}

// An error serializes as its message, so JSON output shows the same text as the Display form.
impl Serialize for NleError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

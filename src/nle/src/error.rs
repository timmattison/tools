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

    #[error("Invalid speed: {0} (a speed must be finite and above zero)")]
    InvalidSpeed(f64),

    #[error("Invalid redaction: {0}")]
    InvalidRedaction(String),

    #[error("Invalid typing rate: {0} (a typing rate must be finite and above zero)")]
    InvalidTypingRate(f64),

    #[error("Invalid jitter: {0} (a jitter must be from 0 up to, and not including, 1)")]
    InvalidJitter(f64),

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

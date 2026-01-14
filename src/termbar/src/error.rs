//! Error types for the termbar library.

use thiserror::Error;

/// Errors that can occur during progress bar operations.
#[derive(Error, Debug)]
pub enum TermbarError {
    /// Failed to create a progress style.
    #[error("Failed to create progress style: {0}")]
    StyleCreation(String),

    /// Invalid template format.
    #[error("Invalid template format: {0}")]
    InvalidTemplate(String),
}

/// Result type for termbar operations.
pub type Result<T> = std::result::Result<T, TermbarError>;

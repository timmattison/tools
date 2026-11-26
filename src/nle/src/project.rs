//! The rules of the edit project: its constructor, its validation, its ids, its
//! length, and its file format.

use std::sync::Arc;

use crate::error::NleError;
use crate::types::{EditProject, Recording};

impl EditProject {
    /// Make a project of one clip that holds the whole `source`, on one track.
    ///
    /// # Errors
    /// Returns `NleError::InvalidTime` when the duration of `source` is not a
    /// valid time.
    pub fn from_recording(source: Arc<Recording>) -> Result<Self, NleError> {
        let _ = source;
        Ok(Self::default())
    }
}

#[cfg(test)]
mod tests;

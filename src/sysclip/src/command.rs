//! The commands `pbcopy` and `pbpaste`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::ClipboardError;

pub(crate) struct Commands {
    search_path: Option<OsString>,
}

impl Commands {
    pub(crate) fn on(search_path: Option<OsString>) -> Self {
        Self { search_path }
    }

    pub(crate) fn none() -> Self {
        Self { search_path: None }
    }

    pub(crate) fn find(&self, _name: &str) -> Option<PathBuf> {
        let _ = &self.search_path;
        None
    }
}

pub(crate) fn copy(_program: &Path, _text: &str) -> Result<(), ClipboardError> {
    Err(ClipboardError::new(&"placeholder"))
}

pub(crate) fn paste(_program: &Path) -> Result<String, ClipboardError> {
    Err(ClipboardError::new(&"placeholder"))
}

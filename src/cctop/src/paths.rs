//! The directories where Claude Code keeps its files.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The directories of one Claude Code config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeDirs {
    config: PathBuf,
}

/// The reasons why cctop cannot find the config directory.
#[derive(Debug, thiserror::Error)]
pub enum PathsError {
    /// Neither `CLAUDE_CONFIG_DIR` nor `HOME` is set.
    #[error("set HOME or CLAUDE_CONFIG_DIR, so cctop can find the state files")]
    NoConfigDir,
}

impl ClaudeDirs {
    /// Find the config directory from the values of `CLAUDE_CONFIG_DIR` and
    /// `HOME`. A value that is empty counts as not set.
    ///
    /// # Errors
    ///
    /// Returns [`PathsError::NoConfigDir`] when no value is usable.
    pub fn resolve(
        claude_config_dir: Option<&OsStr>,
        home: Option<&OsStr>,
    ) -> Result<Self, PathsError> {
        let _ = (claude_config_dir, home);
        Err(PathsError::NoConfigDir)
    }

    /// Find the config directory from the environment of this process.
    ///
    /// # Errors
    ///
    /// Returns [`PathsError::NoConfigDir`] when no value is usable.
    pub fn from_process_env() -> Result<Self, PathsError> {
        Self::resolve(
            std::env::var_os("CLAUDE_CONFIG_DIR").as_deref(),
            std::env::var_os("HOME").as_deref(),
        )
    }

    /// The config directory.
    #[must_use]
    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    /// The directory of the state files.
    #[must_use]
    pub fn state_dir(&self) -> PathBuf {
        PathBuf::new()
    }

    /// The directory of the session registry of Claude Code.
    #[must_use]
    pub fn sessions_dir(&self) -> PathBuf {
        PathBuf::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_dir_variable_wins() {
        let dirs =
            ClaudeDirs::resolve(Some(OsStr::new("/cfg")), Some(OsStr::new("/home/tim"))).unwrap();
        assert_eq!(dirs.config_dir(), Path::new("/cfg"));
    }

    #[test]
    fn home_gives_the_default_directory() {
        let dirs = ClaudeDirs::resolve(None, Some(OsStr::new("/home/tim"))).unwrap();
        assert_eq!(dirs.config_dir(), Path::new("/home/tim/.claude"));
    }

    #[test]
    fn an_empty_config_dir_counts_as_unset() {
        let dirs =
            ClaudeDirs::resolve(Some(OsStr::new("")), Some(OsStr::new("/home/tim"))).unwrap();
        assert_eq!(dirs.config_dir(), Path::new("/home/tim/.claude"));
    }

    #[test]
    fn no_usable_value_is_an_error() {
        assert!(matches!(
            ClaudeDirs::resolve(None, None),
            Err(PathsError::NoConfigDir)
        ));
        assert!(matches!(
            ClaudeDirs::resolve(Some(OsStr::new("")), Some(OsStr::new(""))),
            Err(PathsError::NoConfigDir)
        ));
    }

    #[test]
    fn the_state_and_sessions_dirs_sit_under_the_config_dir() {
        let dirs = ClaudeDirs::resolve(Some(OsStr::new("/cfg")), None).unwrap();
        assert_eq!(dirs.state_dir(), PathBuf::from("/cfg/state/pane-state"));
        assert_eq!(dirs.sessions_dir(), PathBuf::from("/cfg/sessions"));
    }
}

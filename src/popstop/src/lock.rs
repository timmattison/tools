//! The instance lock.
//!
//! Only one copy of popstop runs for each user. A copy that runs holds an
//! exclusive advisory lock on the lock file in its [`StateDir`].

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// The name of the state directory in the data directory of the user.
const STATE_DIR_NAME: &str = "popstop";

/// The name of the lock file in the state directory.
const LOCK_FILE_NAME: &str = "popstop.lock";

/// The name of the log of a background copy in the state directory.
const LOG_FILE_NAME: &str = "popstop.log";

/// The directory that holds the lock file and the log of popstop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDir(PathBuf);

impl StateDir {
    /// Makes a state directory at `path`. The directory does not have to
    /// exist.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    /// Gives the state directory of the current user: `popstop` in the data
    /// directory of the user. On macOS that is
    /// `~/Library/Application Support/popstop`.
    ///
    /// The directory is not a cache directory and not `$TMPDIR`. macOS can
    /// delete old files in those. When the path of a held lock file goes, the
    /// next copy makes a new file and locks it, and two copies run.
    ///
    /// # Errors
    ///
    /// Returns an error of kind [`io::ErrorKind::NotFound`] when the data
    /// directory of the user is not known.
    pub fn for_user() -> io::Result<Self> {
        dirs::data_dir()
            .map(|data| Self(data.join(STATE_DIR_NAME)))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "the data directory of this user is not known",
                )
            })
    }

    /// Gives the path of the lock file: `popstop.lock` in the directory.
    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.0.join(LOCK_FILE_NAME)
    }

    /// Gives the path of the log of a background copy: `popstop.log` in the
    /// directory.
    #[must_use]
    pub fn log_path(&self) -> PathBuf {
        self.0.join(LOG_FILE_NAME)
    }

    /// Gives the path of the directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// How a copy of popstop runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The copy holds a terminal and stops on Ctrl-C.
    Foreground,
    /// The copy has no terminal. `popstop --stop` stops it.
    Background,
}

impl fmt::Display for Mode {
    fn fmt(&self, _formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, StateDir};
    use std::path::{Path, PathBuf};

    #[test]
    fn a_state_directory_holds_the_lock_file_and_the_log() {
        let dir = StateDir::new(PathBuf::from("/some/state"));

        assert_eq!(dir.path(), Path::new("/some/state"));
        assert_eq!(dir.lock_path(), Path::new("/some/state/popstop.lock"));
        assert_eq!(dir.log_path(), Path::new("/some/state/popstop.log"));
    }

    #[test]
    fn the_state_directory_of_the_user_is_in_the_data_directory() {
        // The data directory, not a cache directory and not `$TMPDIR`: macOS
        // can delete old files in those, and a copy that runs then loses the
        // path of its lock file.
        let data = dirs::data_dir().expect("this user has a data directory");

        let dir = StateDir::for_user().expect("the state directory of this user");

        assert_eq!(dir.path(), data.join("popstop"));
    }

    #[test]
    fn a_mode_displays_as_one_lowercase_word() {
        assert_eq!(Mode::Foreground.to_string(), "foreground");
        assert_eq!(Mode::Background.to_string(), "background");
    }
}

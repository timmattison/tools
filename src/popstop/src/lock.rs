//! The instance lock.
//!
//! Only one copy of popstop runs for each user. A copy that runs holds an
//! exclusive advisory lock on the lock file in its [`StateDir`].

use std::io;
use std::path::{Path, PathBuf};

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

    /// Gives the state directory of the current user.
    ///
    /// # Errors
    ///
    /// Returns an error of kind [`io::ErrorKind::NotFound`] when the data
    /// directory of the user is not known.
    pub fn for_user() -> io::Result<Self> {
        Ok(Self(PathBuf::new()))
    }

    /// Gives the path of the lock file.
    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.0.clone()
    }

    /// Gives the path of the log of a background copy.
    #[must_use]
    pub fn log_path(&self) -> PathBuf {
        self.0.clone()
    }

    /// Gives the path of the directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::StateDir;
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
}

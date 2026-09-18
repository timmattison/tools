//! The instance lock.
//!
//! Only one copy of popstop runs for each user. A copy that runs holds an
//! exclusive advisory lock on the lock file in its [`StateDir`].

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, Write};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// The copy holds a terminal and stops on Ctrl-C.
    Foreground,
    /// The copy has no terminal. `popstop --stop` stops it.
    Background,
}

impl fmt::Display for Mode {
    /// Writes `foreground` or `background`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Foreground => "foreground",
            Self::Background => "background",
        })
    }
}

/// The time at which a process started, in microseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StartTime(u64);

impl StartTime {
    /// Makes a start time from a number of microseconds since the Unix epoch.
    #[must_use]
    pub const fn from_unix_micros(micros: u64) -> Self {
        Self(micros)
    }

    /// Gives the number of microseconds since the Unix epoch.
    #[must_use]
    pub const fn unix_micros(self) -> u64 {
        self.0
    }
}

/// The record that the holder of the lock writes into the lock file.
///
/// A reader trusts a record only while the lock is held. After a crash, an
/// old record can stay in a file that nobody locks.
///
/// The lock file holds the record as one line of JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HolderRecord {
    /// The process ID of the holder.
    pub pid: u32,
    /// How the holder runs.
    pub mode: Mode,
    /// The time at which the kernel started the holder process.
    pub started_at: StartTime,
}

/// The proof that this process holds the lock.
///
/// A drop of the guard removes the record from the lock file and releases the
/// lock.
#[derive(Debug)]
#[must_use = "the lock is released when the guard drops"]
pub struct LockGuard {
    /// The lock file, which this process holds locked.
    file: File,
}

/// The reason why [`acquire`] did not get the lock.
#[derive(Debug, thiserror::Error)]
pub enum AcquireError {
    /// Another copy of popstop holds the lock. The record tells which copy.
    #[error(
        "another copy of popstop holds the lock (pid {}, {} mode)",
        .0.pid,
        .0.mode
    )]
    Held(HolderRecord),
    /// The lock file cannot be made, opened, locked, read, or written.
    #[error("the lock file cannot be used: {0}")]
    Io(io::Error),
}

impl LockGuard {
    /// Replaces the contents of the lock file with `record`, and puts the
    /// data on the disk.
    fn write_record(&mut self, record: &HolderRecord) -> io::Result<()> {
        let mut line = serde_json::to_string(record)?;
        line.push('\n');
        self.file.set_len(0)?;
        self.file.rewind()?;
        self.file.write_all(line.as_bytes())?;
        self.file.sync_data()
    }
}

/// Gets the lock for this process, and writes `record` into the lock file.
///
/// It makes the state directory and the lock file when they do not exist. It
/// writes the record before it returns the guard, so a reader that sees the
/// lock held finds the record at once.
///
/// # Errors
///
/// Returns [`AcquireError::Held`] with the record of the holder when another
/// copy holds the lock. Returns [`AcquireError::Io`] when the state directory
/// or the lock file cannot be used.
pub fn acquire(dir: &StateDir, record: &HolderRecord) -> Result<LockGuard, AcquireError> {
    fs::create_dir_all(dir.path()).map_err(AcquireError::Io)?;
    // No truncation here: the file can hold the record of a copy that runs.
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.lock_path())
        .map_err(AcquireError::Io)?;

    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(read_record(&mut file).map_or_else(AcquireError::Io, AcquireError::Held));
        }
        Err(TryLockError::Error(error)) => return Err(AcquireError::Io(error)),
    }

    let mut guard = LockGuard { file };
    guard.write_record(record).map_err(AcquireError::Io)?;
    Ok(guard)
}

/// Reads the record in the lock file.
///
/// Returns an error of kind [`io::ErrorKind::InvalidData`] when the file does
/// not hold a record.
fn read_record(file: &mut File) -> io::Result<HolderRecord> {
    file.rewind()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::{acquire, AcquireError, HolderRecord, Mode, StartTime, StateDir};
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    /// The record of the first holder in a test.
    const FIRST: HolderRecord = HolderRecord {
        pid: 4242,
        mode: Mode::Background,
        started_at: StartTime::from_unix_micros(1_758_000_000_123_456),
    };

    /// The record of a second copy that tries to get the lock.
    const SECOND: HolderRecord = HolderRecord {
        pid: 5353,
        mode: Mode::Foreground,
        started_at: StartTime::from_unix_micros(1_758_000_060_654_321),
    };

    /// Makes a temporary directory of its own for one test, and a state
    /// directory in it that does not exist yet.
    fn state_dir() -> (TempDir, StateDir) {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let dir = StateDir::new(temp.path().join("state"));
        (temp, dir)
    }

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

    #[test]
    fn a_second_acquire_refuses_and_returns_the_record_of_the_first_holder() {
        let (_temp, dir) = state_dir();

        let first = acquire(&dir, &FIRST).expect("the first acquire gets the lock");
        let second = acquire(&dir, &SECOND);

        match second {
            Err(AcquireError::Held(holder)) => assert_eq!(holder, FIRST),
            Err(AcquireError::Io(error)) => panic!("the second acquire failed: {error}"),
            Ok(_guard) => panic!("the second acquire got the lock while the first holds it"),
        }
        drop(first);
    }
}

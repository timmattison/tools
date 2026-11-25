//! The instance lock.
//!
//! Only one copy of popstop runs for each user. A copy that runs holds an
//! exclusive advisory lock on `popstop.lock` in its [`StateDir`], and writes
//! its [`HolderRecord`] into that file.
//!
//! The lock is the only source of truth. The kernel releases the lock when
//! the process ends for any cause, `SIGKILL` included, so a stale lock cannot
//! occur. A record can stay in the file after a crash, thus a reader trusts a
//! record only while the lock is held.
//!
//! Only a holder takes the exclusive lock. A reader ([`current_holder`] and
//! [`wait_for_release`]) takes a shared lock for a moment. Two shared locks
//! do not conflict, so a reader that finds the lock held knows that a holder
//! has it, not another reader. [`acquire`] also meets readers: when its
//! exclusive try fails, it tries a shared lock too. When it gets the shared
//! lock, only readers block it, and it tries again after a short sleep. Thus
//! a reader never makes a start refuse with the old record of a crashed copy.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// The name of the state directory in the data directory of the user.
const STATE_DIR_NAME: &str = "popstop";

/// The name of the lock file in the state directory.
const LOCK_FILE_NAME: &str = "popstop.lock";

/// The name of the log of a background copy in the state directory.
const LOG_FILE_NAME: &str = "popstop.log";

/// The longest time that [`acquire`] and [`current_holder`] wait for a short
/// state of the lock to end.
///
/// A holder writes its record directly after it gets the lock, and empties
/// the file directly before it releases the lock. So a reader can see a held
/// lock with no record for a very short time. Readers also hold a shared lock
/// for a moment, and that blocks the exclusive try of a start.
const PROBE_WAIT: Duration = Duration::from_secs(2);

/// The time between two looks at the lock while a short state lasts.
const PROBE_RETRY_INTERVAL: Duration = Duration::from_millis(10);

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

impl Drop for LockGuard {
    /// Removes the record from the lock file, then releases the lock.
    ///
    /// A drop cannot report an error. When a call fails here, the kernel still
    /// releases the lock when the file closes, and an old record means
    /// nothing to a reader that gets the lock.
    fn drop(&mut self) {
        let _ = self.file.set_len(0);
        let _ = self.file.unlock();
    }
}

/// Gets the exclusive lock for this process, and writes `record` into the
/// lock file.
///
/// It makes the state directory and the lock file when they do not exist. It
/// writes the record before it returns the guard, so a reader that sees the
/// lock held finds the record at once.
///
/// When the exclusive try fails, a reader or a holder blocks it. A shared try
/// on a second open of the file tells which. When the shared try works, only
/// readers block the exclusive lock: it releases the shared lock and tries
/// again after a short sleep. When the shared try fails too, a holder exists,
/// and it reads the record of that holder.
///
/// # Errors
///
/// Returns [`AcquireError::Held`] with the record of the holder when another
/// copy holds the lock. Returns [`AcquireError::Io`] when the state directory
/// or the lock file cannot be used, when readers block the lock for 2 s, or
/// when the record of the holder cannot be read for 2 s.
pub fn acquire(dir: &StateDir, record: &HolderRecord) -> Result<LockGuard, AcquireError> {
    fs::create_dir_all(dir.path()).map_err(AcquireError::Io)?;
    let lock_path = dir.lock_path();
    // No truncation here: the file can hold the record of a copy that runs.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(AcquireError::Io)?;
    // The second open of the file, for the shared try.
    let mut shared = File::open(&lock_path).map_err(AcquireError::Io)?;

    let outcome = probe(&lock_path, || {
        match file.try_lock() {
            Ok(()) => return Ok(Look::Free),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(error),
        }
        match shared.try_lock_shared() {
            Ok(()) => {
                shared.unlock()?;
                Ok(Look::OnlyReaders)
            }
            Err(TryLockError::WouldBlock) => look_at_record(&mut shared),
            Err(TryLockError::Error(error)) => Err(error),
        }
    })
    .map_err(AcquireError::Io)?;

    match outcome {
        Probe::Held(holder) => Err(AcquireError::Held(holder)),
        Probe::Free => {
            let mut guard = LockGuard { file };
            guard.write_record(record).map_err(AcquireError::Io)?;
            Ok(guard)
        }
    }
}

/// Gives the record of the copy that holds the lock, or `None` when no copy
/// holds it.
///
/// The lock is the only source of truth. This call tries a shared lock. When
/// it gets the shared lock, no holder has the exclusive lock, so no copy runs:
/// it releases the lock at once and gives `None`, even when an old record
/// stays in the file after a crash. It also gives `None` when the state
/// directory or the lock file does not exist.
///
/// The lock is shared because readers do not conflict with each other. Thus a
/// reader never takes another reader for a holder.
///
/// # Errors
///
/// Returns an error when the lock file cannot be opened, locked, or read, or
/// when the record of the holder cannot be read for 2 s.
pub fn current_holder(dir: &StateDir) -> io::Result<Option<HolderRecord>> {
    let Some(mut file) = open_existing_lock_file(dir)? else {
        return Ok(None);
    };
    let outcome = probe(&dir.lock_path(), || match file.try_lock_shared() {
        Ok(()) => {
            file.unlock()?;
            Ok(Look::Free)
        }
        Err(TryLockError::WouldBlock) => look_at_record(&mut file),
        Err(TryLockError::Error(error)) => Err(error),
    })?;
    Ok(match outcome {
        Probe::Free => None,
        Probe::Held(holder) => Some(holder),
    })
}

/// How a wait for the release of the lock ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// No copy holds the lock.
    Released,
    /// The holder kept the lock until the timeout.
    TimedOut,
}

/// The name of the thread that waits for the release of the lock.
const WAIT_THREAD_NAME: &str = "popstop-lock-wait";

/// Waits until no copy holds the lock, for `timeout` at most.
///
/// It does not poll. A thread opens the lock file and blocks on a shared
/// lock. When the thread gets the shared lock, no holder has the exclusive
/// lock: the thread releases the lock at once and tells the caller. It gives
/// [`Release::Released`] at once when the lock file does not exist.
///
/// The lock is shared, so readers do not delay the wait, and the wait does
/// not make a start take it for a holder.
///
/// After a timeout, the thread stays blocked until the holder releases the
/// lock or this process ends. Then it releases the lock at once and ends.
///
/// # Errors
///
/// Returns an error when the lock file cannot be opened or locked, or when the
/// thread cannot start.
pub fn wait_for_release(dir: &StateDir, timeout: Duration) -> io::Result<Release> {
    let Some(file) = open_existing_lock_file(dir)? else {
        return Ok(Release::Released);
    };

    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name(WAIT_THREAD_NAME.to_owned())
        .spawn(move || {
            let result = file.lock_shared().and_then(|()| file.unlock());
            // After a timeout nobody receives, and the result means nothing.
            let _ = sender.send(result);
        })?;

    match receiver.recv_timeout(timeout) {
        Ok(Ok(())) => Ok(Release::Released),
        Ok(Err(error)) => Err(error),
        Err(RecvTimeoutError::Timeout) => Ok(Release::TimedOut),
        Err(RecvTimeoutError::Disconnected) => Err(io::Error::other(
            "the thread that waits for the lock ended with no result",
        )),
    }
}

/// Opens the lock file for reading, or gives `None` when it does not exist.
///
/// A missing file is not an error: no copy ever ran in this state directory.
fn open_existing_lock_file(dir: &StateDir) -> io::Result<Option<File>> {
    match File::open(dir.lock_path()) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// What one look at the lock found.
enum Look {
    /// No holder has the lock. [`acquire`] holds the exclusive lock now. A
    /// reader released its shared lock already.
    Free,
    /// A holder has the lock. This is its record.
    Held(HolderRecord),
    /// A holder has the lock, but its record is empty or does not parse.
    NoRecord(serde_json::Error),
    /// Only readers block the exclusive lock. Only [`acquire`] sees this.
    OnlyReaders,
}

/// What a probe of the lock found at last.
enum Probe {
    /// No holder has the lock. [`acquire`] holds the exclusive lock now.
    Free,
    /// A holder has the lock. This is its record.
    Held(HolderRecord),
}

/// Looks at the lock file at `lock_path` with `look` until the answer is
/// [`Look::Free`] or [`Look::Held`].
///
/// A held lock with no record, and readers that block a start, are normal for
/// a short time. While one of them lasts, it sleeps for a short time and looks
/// again, for [`PROBE_WAIT`] at most. Then it returns an error that tells which
/// state lasted: of kind [`io::ErrorKind::InvalidData`] for a record that
/// cannot be read, and of kind [`io::ErrorKind::TimedOut`] for readers that
/// block a start.
fn probe(lock_path: &Path, mut look: impl FnMut() -> io::Result<Look>) -> io::Result<Probe> {
    let deadline = Instant::now() + PROBE_WAIT;
    loop {
        match look()? {
            Look::Free => return Ok(Probe::Free),
            Look::Held(record) => return Ok(Probe::Held(record)),
            Look::NoRecord(_) | Look::OnlyReaders if Instant::now() < deadline => {
                thread::sleep(PROBE_RETRY_INTERVAL);
            }
            Look::NoRecord(problem) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "a copy of popstop holds the lock in {}, but its record cannot be read \
                         after {PROBE_WAIT:?}: {problem}",
                        lock_path.display()
                    ),
                ));
            }
            Look::OnlyReaders => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "readers held a shared lock on {} for {PROBE_WAIT:?}, so this copy of \
                         popstop cannot get the lock",
                        lock_path.display()
                    ),
                ));
            }
        }
    }
}

/// Reads the record of the holder that blocks a shared try on `file`.
fn look_at_record(file: &mut File) -> io::Result<Look> {
    Ok(match read_record(file)? {
        Ok(record) => Look::Held(record),
        Err(problem) => Look::NoRecord(problem),
    })
}

/// Reads the record in the lock file once.
///
/// The outer result tells whether the read worked. The inner result tells
/// whether the file holds a record.
fn read_record(file: &mut File) -> io::Result<Result<HolderRecord, serde_json::Error>> {
    file.rewind()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes))
}

#[cfg(test)]
mod tests {
    use super::{
        acquire, current_holder, wait_for_release, AcquireError, HolderRecord, Mode, Release,
        StartTime, StateDir, PROBE_WAIT,
    };
    use std::fs::{self, File};
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    /// A bound that a wait which passes never comes near, even on a loaded
    /// machine.
    const GENEROUS_TIMEOUT: Duration = Duration::from_secs(30);

    /// The time for which a holder keeps the lock while another thread waits.
    const HOLD: Duration = Duration::from_millis(200);

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

    /// Writes `record` into the lock file and holds no lock, as a copy that
    /// crashed leaves it. Makes the state directory when necessary.
    fn leave_an_old_record(dir: &StateDir, record: HolderRecord) {
        fs::create_dir_all(dir.path()).expect("make the state directory");
        let line = serde_json::to_string(&record).expect("the record serializes") + "\n";
        fs::write(dir.lock_path(), line).expect("write an old record");
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

    #[test]
    fn the_record_that_the_holder_writes_is_the_record_that_a_reader_gets() {
        let (_temp, dir) = state_dir();
        let guard = acquire(&dir, &FIRST).expect("the holder gets the lock");

        let holder = current_holder(&dir).expect("the reader reads the lock file");

        assert_eq!(holder, Some(FIRST));
        drop(guard);
    }

    #[test]
    fn a_reader_gets_none_when_no_copy_holds_the_lock_even_with_an_old_record() {
        let (_temp, dir) = state_dir();

        assert_eq!(
            current_holder(&dir).expect("the reader handles a missing state directory"),
            None,
            "no copy ran yet, so the state directory does not exist"
        );

        fs::create_dir_all(dir.path()).expect("make the state directory");
        assert_eq!(
            current_holder(&dir).expect("the reader handles a missing lock file"),
            None,
            "the state directory holds no lock file"
        );

        // A copy that crashed leaves its record, and the kernel releases its
        // lock.
        leave_an_old_record(&dir, FIRST);
        assert_eq!(
            current_holder(&dir).expect("the reader reads the lock file"),
            None,
            "nobody holds the lock, so the old record means nothing"
        );
    }

    #[test]
    fn a_dropped_guard_releases_the_lock_and_leaves_no_record() {
        let (_temp, dir) = state_dir();
        let first = acquire(&dir, &FIRST).expect("the first holder gets the lock");

        drop(first);

        assert_eq!(
            current_holder(&dir).expect("the reader reads the lock file"),
            None,
            "the lock is free after the drop"
        );
        assert_eq!(
            fs::read_to_string(dir.lock_path()).expect("read the lock file"),
            "",
            "a clean release leaves no record, so an old record stays only after a crash"
        );

        let second = acquire(&dir, &SECOND).expect("a new acquire gets the lock");
        assert_eq!(
            current_holder(&dir).expect("the reader reads the lock file"),
            Some(SECOND)
        );
        drop(second);
    }

    #[test]
    fn a_wait_gives_released_when_the_holder_drops_its_guard() {
        let (_temp, dir) = state_dir();
        let guard = acquire(&dir, &FIRST).expect("the holder gets the lock");
        let about_to_release = Arc::new(AtomicBool::new(false));
        let holder = {
            let about_to_release = Arc::clone(&about_to_release);
            thread::spawn(move || {
                thread::sleep(HOLD);
                // The flag goes up before the release, so a wait that ends
                // after the release always sees it.
                about_to_release.store(true, Ordering::SeqCst);
                drop(guard);
            })
        };

        let release = wait_for_release(&dir, GENEROUS_TIMEOUT).expect("the wait works");

        assert_eq!(release, Release::Released);
        assert!(
            about_to_release.load(Ordering::SeqCst),
            "the wait ended before the holder released the lock"
        );
        holder.join().expect("the holder thread ends");

        let (_other_temp, never_used) = state_dir();
        assert_eq!(
            wait_for_release(&never_used, GENEROUS_TIMEOUT).expect("the wait works"),
            Release::Released,
            "no lock file exists, so no copy holds the lock"
        );
    }

    #[test]
    fn a_wait_gives_timed_out_when_the_holder_keeps_the_lock() {
        let (_temp, dir) = state_dir();
        let guard = acquire(&dir, &FIRST).expect("the holder gets the lock");

        let started = Instant::now();
        let release = wait_for_release(&dir, HOLD).expect("the wait works");

        assert_eq!(release, Release::TimedOut);
        assert!(
            started.elapsed() >= HOLD,
            "the wait gave up after {:?}, before its timeout of {HOLD:?}",
            started.elapsed()
        );
        assert_eq!(
            current_holder(&dir).expect("the reader reads the lock file"),
            Some(FIRST),
            "the wait does not take the lock from the holder"
        );
        drop(guard);
    }

    #[test]
    fn a_held_lock_whose_record_never_appears_gives_invalid_data_after_the_bound() {
        let (_temp, dir) = state_dir();
        fs::create_dir_all(dir.path()).expect("make the state directory");
        // A holder that never writes its record.
        let holder = File::create(dir.lock_path()).expect("make the lock file");
        holder.lock().expect("the holder gets the lock");

        let started = Instant::now();
        let error = current_holder(&dir).expect_err("a held lock with no record is an error");
        let waited = started.elapsed();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
        assert!(
            waited >= PROBE_WAIT,
            "the reader gave up after {waited:?}, before the bound of {PROBE_WAIT:?}"
        );
        assert!(
            error
                .to_string()
                .contains(&dir.lock_path().display().to_string()),
            "the error names the lock file: {error}"
        );
        drop(holder);
    }

    /// Holds the lock like a holder that writes `record` `HOLD` after it gets
    /// the lock. Gives the locked file and the thread that writes the record.
    fn hold_and_write_the_record_late(
        dir: &StateDir,
        record: HolderRecord,
    ) -> (File, thread::JoinHandle<()>) {
        fs::create_dir_all(dir.path()).expect("make the state directory");
        let holder = File::create(dir.lock_path()).expect("make the lock file");
        holder.lock().expect("the holder gets the lock");
        let lock_path = dir.lock_path();
        let writer = thread::spawn(move || {
            thread::sleep(HOLD);
            let line = serde_json::to_string(&record).expect("the record serializes") + "\n";
            fs::write(lock_path, line).expect("write the record late");
        });
        (holder, writer)
    }

    #[test]
    fn a_record_that_appears_during_the_wait_is_the_record_that_a_reader_gets() {
        let (_temp, dir) = state_dir();
        let (holder, writer) = hold_and_write_the_record_late(&dir, FIRST);

        assert_eq!(
            current_holder(&dir).expect("the reader waits for the record"),
            Some(FIRST)
        );
        writer.join().expect("the writer thread ends");
        drop(holder);

        let (_other_temp, other) = state_dir();
        let (holder, writer) = hold_and_write_the_record_late(&other, FIRST);

        match acquire(&other, &SECOND) {
            Err(AcquireError::Held(record)) => assert_eq!(record, FIRST),
            Err(AcquireError::Io(error)) => {
                panic!("the refusal did not wait for the record: {error}")
            }
            Ok(_guard) => panic!("the acquire got the lock while the holder holds it"),
        }
        writer.join().expect("the writer thread ends");
        drop(holder);
    }

    /// Holds the lock with no record for `HOLD`, then releases it.
    ///
    /// That is what a reader does for a moment, and what a holder does
    /// between the moment it empties the lock file and its release. Gives the
    /// thread that releases the lock.
    fn hold_for_a_moment_with_no_record(dir: &StateDir) -> thread::JoinHandle<()> {
        fs::create_dir_all(dir.path()).expect("make the state directory");
        let holder = File::create(dir.lock_path()).expect("make the lock file");
        holder.lock().expect("the holder gets the lock");
        thread::spawn(move || {
            thread::sleep(HOLD);
            drop(holder);
        })
    }

    #[test]
    fn a_lock_that_is_released_during_the_wait_for_the_record_is_free() {
        let (_temp, dir) = state_dir();
        let releaser = hold_for_a_moment_with_no_record(&dir);

        let guard = acquire(&dir, &SECOND).expect("the acquire gets the lock after the release");

        assert_eq!(
            current_holder(&dir).expect("the reader reads the lock file"),
            Some(SECOND)
        );
        releaser.join().expect("the releasing thread ends");
        drop(guard);

        let (_other_temp, other) = state_dir();
        let releaser = hold_for_a_moment_with_no_record(&other);

        assert_eq!(
            current_holder(&other).expect("the reader sees the release"),
            None,
            "the lock was released during the wait, so no copy holds it"
        );
        releaser.join().expect("the releasing thread ends");
    }

    #[test]
    fn readers_at_the_same_time_all_get_none_when_only_an_old_record_stays() {
        // Each reader holds the lock for a moment. The number of looks makes
        // an overlap of two readers certain.
        const READERS: usize = 4;
        const LOOKS: usize = 2_000;
        let (_temp, dir) = state_dir();
        leave_an_old_record(&dir, FIRST);
        let start = Barrier::new(READERS);

        let wrong_answers: Vec<String> = thread::scope(|scope| {
            let readers: Vec<_> = (0..READERS)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        (0..LOOKS)
                            .map(|_| current_holder(&dir))
                            .filter(|answer| !matches!(answer, Ok(None)))
                            .map(|answer| format!("{answer:?}"))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            readers
                .into_iter()
                .flat_map(|reader| reader.join().expect("the reader thread ends"))
                .collect()
        });

        assert!(
            wrong_answers.is_empty(),
            "no copy holds the lock, but {} of {} looks said otherwise, for example {}",
            wrong_answers.len(),
            READERS * LOOKS,
            wrong_answers[0]
        );
    }

    #[test]
    fn readers_never_make_acquire_refuse_when_only_an_old_record_stays() {
        // Each reader holds the lock for a moment. The number of starts makes
        // an overlap of a start and a reader certain.
        const READERS: usize = 4;
        const STARTS: usize = 500;
        let (_temp, dir) = state_dir();
        leave_an_old_record(&dir, FIRST);
        let done = AtomicBool::new(false);
        // A backstop for the reader loops, in case the starts panic.
        let deadline = Instant::now() + GENEROUS_TIMEOUT;

        let wrong_answers: Vec<String> = thread::scope(|scope| {
            for _ in 0..READERS {
                scope.spawn(|| {
                    while !done.load(Ordering::SeqCst) && Instant::now() < deadline {
                        // The answers of the readers are not under test here.
                        let _ = current_holder(&dir);
                    }
                });
            }
            let wrong_answers = (0..STARTS)
                .filter_map(|_| {
                    // Each start finds the old record of a crashed copy.
                    leave_an_old_record(&dir, FIRST);
                    match acquire(&dir, &SECOND) {
                        Ok(guard) => {
                            drop(guard);
                            None
                        }
                        Err(error) => Some(format!("{error:?}")),
                    }
                })
                .collect();
            done.store(true, Ordering::SeqCst);
            wrong_answers
        });

        assert!(
            wrong_answers.is_empty(),
            "no copy holds the lock, but {} of {STARTS} starts failed, for example {}",
            wrong_answers.len(),
            wrong_answers[0]
        );
    }

    #[test]
    fn a_reader_does_not_delay_a_wait_for_the_release() {
        let (_temp, dir) = state_dir();
        leave_an_old_record(&dir, FIRST);
        // A reader holds a shared lock for a moment. Here the moment lasts
        // for the whole wait.
        let reader = File::open(dir.lock_path()).expect("open the lock file");
        reader.lock_shared().expect("the reader gets a shared lock");

        let release = wait_for_release(&dir, HOLD).expect("the wait works");

        assert_eq!(
            release,
            Release::Released,
            "no holder has the lock, so a reader must not delay the wait"
        );
        drop(reader);
    }

    #[test]
    fn readers_that_block_a_start_past_the_bound_give_an_error_that_says_so() {
        let (_temp, dir) = state_dir();
        leave_an_old_record(&dir, FIRST);
        // A reader that holds its shared lock for longer than the bound.
        let reader = File::open(dir.lock_path()).expect("open the lock file");
        reader.lock_shared().expect("the reader gets a shared lock");

        let started = Instant::now();
        let result = acquire(&dir, &SECOND);
        let waited = started.elapsed();

        match result {
            Err(AcquireError::Io(error)) => {
                assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
                let message = error.to_string();
                assert!(
                    message.contains("readers"),
                    "the error names readers: {message}"
                );
                assert!(
                    message.contains(&dir.lock_path().display().to_string()),
                    "the error names the lock file: {message}"
                );
            }
            Err(AcquireError::Held(holder)) => {
                panic!("only a reader blocks the start, but it refused with {holder:?}")
            }
            Ok(_guard) => panic!("the start got the exclusive lock while a reader holds it"),
        }
        assert!(
            waited >= PROBE_WAIT,
            "the start gave up after {waited:?}, before the bound of {PROBE_WAIT:?}"
        );
        drop(reader);
    }
}

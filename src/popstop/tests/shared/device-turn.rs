//! One turn at the audio device for the tests of popstop.
//!
//! The tests of popstop open the real default output device. `coreaudiod`
//! sets up a new client slowly while other clients open and close the same
//! device, and on a busy machine that setup took up to 143 s. So each test
//! that opens the device takes a turn first, and keeps it until each copy
//! that it started stopped.
//!
//! The turn is an exclusive lock (`flock`) on one file in the temporary
//! directory of the user. A lock on one open file conflicts with a lock on
//! another open file of the same path, also in the same process. Thus the turn
//! orders the threads of one test binary and the test binaries of other cargo
//! runs alike. The kernel releases the lock when the process ends for any
//! cause, thus a test that crashed holds no turn.
//!
//! Each test target that opens the device includes this file with `#[path]`,
//! so one copy of the code serves all of them.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

/// The name of the file whose lock is the turn.
const TURN_FILE_NAME: &str = "popstop-tests-audio-device.lock";

/// The longest time that a test waits for its turn.
///
/// The tests that wait run one at a time, and each one ends in seconds. A
/// test that waits this long found a test that keeps the device and does not
/// end.
const TURN_WAIT: Duration = Duration::from_secs(600);

/// The time between two tries for the turn.
const TURN_POLL: Duration = Duration::from_millis(20);

/// The turn of one test at the audio device. A drop ends the turn.
///
/// A test takes the turn before it opens the device, and declares it before
/// each value that holds the device, thus the turn ends after them.
pub struct DeviceTurn {
    /// The open lock file. The lock lasts as long as the file stays open.
    _file: File,
}

impl DeviceTurn {
    /// Waits until no other test has the audio device, and then takes the
    /// turn.
    ///
    /// The wait tries the lock again and again, and it never blocks in the
    /// kernel. A blocked try that a timeout left behind takes the lock later
    /// and keeps it, and then no test gets a turn.
    ///
    /// # Panics
    ///
    /// Panics when the lock file cannot be opened or locked, and when another
    /// test keeps the turn for [`TURN_WAIT`].
    pub fn take() -> Self {
        let path = turn_file();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .unwrap_or_else(|problem| panic!("the turn file {path:?} cannot be opened: {problem}"));
        let deadline = Instant::now() + TURN_WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Self { _file: file },
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(TURN_POLL);
                }
                Err(TryLockError::WouldBlock) => panic!(
                    "another test kept the audio device for {TURN_WAIT:?}. The lock file is \
                     {path:?}"
                ),
                Err(TryLockError::Error(problem)) => {
                    panic!("the turn file {path:?} cannot be locked: {problem}")
                }
            }
        }
    }
}

/// Gives the path of the file whose lock is the turn: one file in the
/// temporary directory of the user, for every test binary of popstop.
fn turn_file() -> PathBuf {
    std::env::temp_dir().join(TURN_FILE_NAME)
}

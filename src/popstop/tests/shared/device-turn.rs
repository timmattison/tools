//! One turn at the audio device for the tests of popstop.
//!
//! The tests of popstop open the real default output device. `coreaudiod`
//! sets up a new client slowly while other clients open and close the same
//! device, and on a busy machine that setup took up to 143 s. So each test
//! that opens the device takes a turn first, and keeps it until each copy
//! that it started stopped.
//!
//! Each test target that opens the device includes this file with `#[path]`,
//! so one copy of the code serves all of them.

use std::fs::{File, OpenOptions};
use std::path::PathBuf;

/// The name of the file whose lock is the turn.
const TURN_FILE_NAME: &str = "popstop-tests-audio-device.lock";

/// The turn of one test at the audio device. A drop ends the turn.
pub struct DeviceTurn {
    /// The open lock file.
    _file: File,
}

impl DeviceTurn {
    /// Waits until no other test has the audio device, and then takes the
    /// turn.
    ///
    /// # Panics
    ///
    /// Panics when the lock file cannot be opened.
    pub fn take() -> Self {
        let path = turn_file();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .unwrap_or_else(|problem| panic!("the turn file {path:?} cannot be opened: {problem}"));
        Self { _file: file }
    }
}

/// Gives the path of the file whose lock is the turn: one file in the
/// temporary directory of the user, for every test binary of popstop.
fn turn_file() -> PathBuf {
    std::env::temp_dir().join(TURN_FILE_NAME)
}

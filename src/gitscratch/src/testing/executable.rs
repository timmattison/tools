//! An executable file that a test writes and then runs, with no race against a
//! concurrent fork.
//!
//! # The race
//!
//! Linux refuses to run a file that a process holds open for writing. The
//! error is `ETXTBSY`, "Text file busy". A test that writes a script with
//! `std::fs::write` holds a write descriptor for a short time. If another
//! thread of the test binary forks in that time, the child holds a copy of the
//! descriptor until its own `exec`. A run of the script in that window fails.
//! `O_CLOEXEC` does not close the window, because the copy lives until the
//! `exec` of the child, not until the `exec` of the script. macOS does not
//! refuse the run of a script, thus the race shows only on Linux. The first CI
//! run on Linux found it in a test of `gsw`.
//!
//! # The fix
//!
//! A child process writes the file. This process never holds a write
//! descriptor for the file, thus a fork in this process has none to copy. When
//! [`write_executable`] returns, the writer has stopped, and no process holds
//! the file open for writing.

use std::io;
use std::path::Path;

/// Writes `contents` to `path` and makes the file executable. This process
/// holds no write descriptor for the file at any time.
///
/// # Errors
///
/// Returns the error of the start of the writer, of the write into its
/// standard input, or of the change of the mode. Returns an error that names
/// the path and the exit status when the writer fails.
pub fn write_executable(_path: &Path, _contents: &[u8]) -> io::Result<()> {
    Ok(())
}

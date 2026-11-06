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

use std::io::{self, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};

/// The shell that writes the file.
const WRITER_SHELL: &str = "/bin/sh";

/// The script of the writer. It copies its standard input into the path that
/// its first argument names. The path is an argument, not part of the script,
/// thus no character of the path can change the script.
const WRITER_SCRIPT: &str = r#"cat > "$1""#;

/// The name that the writer gets as `$0`.
const WRITER_NAME: &str = "write-executable";

/// The mode of the file. The owner can read, write, and run it, and each
/// other user can read and run it.
const EXECUTABLE_MODE: u32 = 0o755;

/// Writes `contents` to `path` and makes the file executable. This process
/// holds no write descriptor for the file at any time.
///
/// # Errors
///
/// Returns the error of the start of the writer, of the write into its
/// standard input, or of the change of the mode. Returns an error that names
/// the path and the exit status when the writer fails.
pub fn write_executable(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut writer = Command::new(WRITER_SHELL)
        .args(["-c", WRITER_SCRIPT, WRITER_NAME])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    // The drop of the pipe at the end of this block is the end of the input
    // of the writer.
    {
        let mut input = writer
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("the writer has no standard input"))?;
        input.write_all(contents)?;
    }

    let output = writer.wait_with_output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "the writer of {} stopped with {}: {}",
            path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    // chmod takes the path, thus it opens no descriptor for the file.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(EXECUTABLE_MODE))
}

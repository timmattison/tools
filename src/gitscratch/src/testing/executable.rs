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
/// The arguments are the arguments of [`std::fs::write`], thus a test that
/// writes a stub with `std::fs::write` and then changes its mode to `0o755`
/// replaces the two calls with this one.
///
/// # Errors
///
/// Returns the error of the start of the writer or of the change of the mode.
/// Returns an error that names the path, the exit status, and the standard
/// error of the writer when the writer fails. This error wins over the error
/// of the write, because a write into the closed pipe of a failed writer
/// returns `BrokenPipe` only as a result of that failure. Returns an error of
/// the same kind as the write error, with the path in its message, when the
/// writer succeeds and the write fails.
pub fn write_executable(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    let contents = contents.as_ref();
    let mut writer = Command::new(WRITER_SHELL)
        .args(["-c", WRITER_SCRIPT, WRITER_NAME])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    let Some(mut input) = writer.stdin.take() else {
        // Stop the writer, so that it has stopped on this path too.
        let _ = writer.kill();
        let _ = writer.wait();
        return Err(io::Error::other("the writer has no standard input"));
    };
    // Keep the result. The writer can stop before the write, and the write
    // then returns `BrokenPipe`. The exit status of the writer tells why.
    let written = input.write_all(contents);
    // The drop of the pipe is the end of the input of the writer.
    drop(input);

    let output = writer.wait_with_output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "the writer of {} stopped with {}: {}",
            path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    if let Err(error) = written {
        return Err(io::Error::new(
            error.kind(),
            format!(
                "the write into the writer of {} failed: {error}",
                path.display()
            ),
        ));
    }

    // chmod takes the path, thus it opens no descriptor for the file.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(EXECUTABLE_MODE))
}

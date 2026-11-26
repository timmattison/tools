//! The commands `pbcopy` and `pbpaste`.

use std::ffi::OsString;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use crate::ClipboardError;

/// The permission bits that let a process run a file.
#[cfg(unix)]
const EXECUTE_BITS: u32 = 0o111;

/// The commands on one search path.
pub(crate) struct Commands {
    search_path: Option<OsString>,
}

impl Commands {
    pub(crate) fn on(search_path: Option<OsString>) -> Self {
        Self { search_path }
    }

    pub(crate) fn none() -> Self {
        Self { search_path: None }
    }

    /// The first program called `name` on the search path.
    ///
    /// An entry that is not an absolute path is skipped. A shell reads an empty
    /// entry as the current directory, and this crate must not run `./pbcopy`.
    /// A program must be a regular file with an execute bit.
    pub(crate) fn find(&self, name: &str) -> Option<PathBuf> {
        let search_path = self.search_path.as_ref()?;
        std::env::split_paths(search_path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(name))
            .find(|candidate| is_program(candidate))
    }
}

/// Whether `path` is a regular file that a process can run.
fn is_program(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & EXECUTE_BITS != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Runs `program` and gives the text it writes. The program is `pbpaste`.
///
/// The text comes back byte for byte, with no trim.
pub(crate) fn paste(program: &Path) -> Result<String, ClipboardError> {
    let output = childenv::command(program)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|cause| start_failure(program, &cause))?;
    check_status(program, &output)?;
    String::from_utf8(output.stdout).map_err(|_| {
        ClipboardError::new(&format!(
            "{} wrote text that is not UTF-8",
            name_of(program)
        ))
    })
}

/// Runs `program` and gives it `text` on its standard input. The program is
/// `pbcopy`.
pub(crate) fn copy(program: &Path, text: &str) -> Result<(), ClipboardError> {
    let mut child = childenv::command(program)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|cause| start_failure(program, &cause))?;
    let stdin = child.stdin.take();
    // A thread writes, so a child that fills its stderr pipe before it reads
    // cannot hold this call forever.
    let (written, waited) = std::thread::scope(|scope| {
        let writer = scope.spawn(move || {
            let Some(mut stdin) = stdin else {
                return Ok(());
            };
            stdin.write_all(text.as_bytes())
        });
        let waited = child.wait_with_output();
        let written = writer.join().unwrap_or(Ok(()));
        (written, waited)
    });
    let output = waited.map_err(|cause| start_failure(program, &cause))?;
    check_status(program, &output)?;
    match written {
        Err(cause) if cause.kind() != ErrorKind::BrokenPipe => Err(ClipboardError::new(&format!(
            "cannot write to {}: {cause}",
            name_of(program)
        ))),
        _ => Ok(()),
    }
}

/// The name of `program`, for a message.
fn name_of(program: &Path) -> String {
    program
        .file_name()
        .unwrap_or(program.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// The message for a program that did not start.
fn start_failure(program: &Path, cause: &std::io::Error) -> ClipboardError {
    ClipboardError::new(&format!("cannot start {}: {cause}", program.display()))
}

/// Gives the failure of a program that did not exit with status 0.
///
/// The message is the first line of stderr that holds text. A program with no
/// such line gets its status named.
fn check_status(program: &Path, output: &Output) -> Result<(), ClipboardError> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if let Some(line) = stderr.lines().map(str::trim).find(|line| !line.is_empty()) {
        return Err(ClipboardError::new(&line));
    }
    let name = name_of(program);
    if let Some(code) = output.status.code() {
        return Err(ClipboardError::new(&format!(
            "{name} exited with status {code}"
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = output.status.signal() {
            return Err(ClipboardError::new(&format!(
                "{name} was stopped by signal {signal}"
            )));
        }
    }
    Err(ClipboardError::new(&format!("{name} failed")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    /// Writes a file in `dir` with `mode`.
    #[cfg(unix)]
    fn stub(dir: &Path, name: &str, mode: u32) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    #[cfg(unix)]
    fn joined(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn find_takes_the_first_match_on_the_path() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let wanted = stub(first.path(), "pbcopy", 0o755);
        stub(second.path(), "pbcopy", 0o755);
        let commands = Commands::on(Some(joined(&[first.path(), second.path()])));
        assert_eq!(commands.find("pbcopy"), Some(wanted));
    }

    #[cfg(unix)]
    #[test]
    fn find_skips_a_file_that_is_not_executable() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        stub(first.path(), "pbcopy", 0o644);
        let wanted = stub(second.path(), "pbcopy", 0o755);
        let commands = Commands::on(Some(joined(&[first.path(), second.path()])));
        assert_eq!(commands.find("pbcopy"), Some(wanted));
    }

    #[cfg(unix)]
    #[test]
    fn find_skips_a_directory_with_the_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("pbcopy")).unwrap();
        let commands = Commands::on(Some(joined(&[dir.path()])));
        assert_eq!(commands.find("pbcopy"), None);
    }

    #[cfg(unix)]
    #[test]
    fn find_skips_a_relative_and_an_empty_entry() {
        let dir = tempfile::tempdir().unwrap();
        stub(dir.path(), "pbcopy", 0o755);
        let relative = OsString::from(format!(
            ":relative/dir:{}",
            dir.path().strip_prefix("/").unwrap().display()
        ));
        let commands = Commands::on(Some(relative));
        assert_eq!(commands.find("pbcopy"), None);
    }

    #[test]
    fn find_finds_nothing_on_a_missing_or_empty_path() {
        assert_eq!(Commands::on(None).find("pbcopy"), None);
        assert_eq!(Commands::on(Some(OsString::new())).find("pbcopy"), None);
        assert_eq!(Commands::none().find("pbcopy"), None);
    }
}

#[cfg(all(test, unix))]
mod run_tests {
    use super::*;
    use gitscratch::testing::executable::write_executable;

    const TEXT: &str = "日本語 café 🎉\n";

    /// Writes an executable script in `dir` and gives its path.
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        write_executable(&path, body).unwrap();
        path
    }

    #[test]
    fn copy_writes_the_text_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(
            dir.path(),
            "pbcopy",
            "#!/bin/sh\nexec /bin/cat > \"${0%/*}/copied\"\n",
        );
        copy(&program, TEXT).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("copied")).unwrap(),
            TEXT.as_bytes()
        );
    }

    #[test]
    fn paste_gives_the_output_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("held"), TEXT).unwrap();
        let program = script(
            dir.path(),
            "pbpaste",
            "#!/bin/sh\nexec /bin/cat \"${0%/*}/held\"\n",
        );
        assert_eq!(paste(&program).unwrap(), TEXT);
    }

    #[test]
    fn a_failed_command_gives_the_first_line_of_its_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let body = "#!/bin/sh\necho >&2\necho '  mosh-open: did not paste: the client refused ' >&2\necho 'second line' >&2\nexit 1\n";
        let program = script(dir.path(), "pbpaste", body);
        let error = paste(&program).unwrap_err();
        assert_eq!(
            error.to_string(),
            "mosh-open: did not paste: the client refused"
        );
        let program = script(dir.path(), "pbcopy", body);
        let error = copy(&program, TEXT).unwrap_err();
        assert_eq!(
            error.to_string(),
            "mosh-open: did not paste: the client refused"
        );
    }

    #[test]
    fn a_failed_command_with_no_stderr_gives_its_status() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "pbpaste", "#!/bin/sh\nexit 3\n");
        assert_eq!(
            paste(&program).unwrap_err().to_string(),
            "pbpaste exited with status 3"
        );
    }

    #[test]
    fn a_command_that_a_signal_stops_gives_the_signal() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "pbcopy", "#!/bin/sh\nkill -9 $$\n");
        assert_eq!(
            copy(&program, "x").unwrap_err().to_string(),
            "pbcopy was stopped by signal 9"
        );
    }

    #[test]
    fn paste_fails_on_output_that_is_not_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "pbpaste", "#!/bin/sh\nprintf '\\377\\376'\n");
        assert_eq!(
            paste(&program).unwrap_err().to_string(),
            "pbpaste wrote text that is not UTF-8"
        );
    }

    #[test]
    fn a_command_that_cannot_start_gives_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("pbpaste");
        let message = paste(&program).unwrap_err().to_string();
        assert!(message.starts_with("cannot start "), "{message}");
        assert!(
            message.contains(&program.display().to_string()),
            "{message}"
        );
    }

    #[test]
    fn copy_survives_a_command_that_exits_without_reading() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "pbcopy", "#!/bin/sh\nexit 0\n");
        let big = "x".repeat(1 << 20);
        copy(&program, &big).unwrap();
    }
}

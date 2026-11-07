//! Start the test command.

use std::ffi::OsString;
use std::os::fd::AsFd;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

use anyhow::{bail, Context, Result};

/// Run `argv` in `dir` and wait for it.
///
/// The program gets no standard input. Its standard output goes to the
/// standard error of `bibibi`, so the standard output of `bibibi` holds only
/// the result. The program runs without a shell.
///
/// # Errors
///
/// Returns an error if `argv` is empty, or if the program cannot start.
pub fn run(argv: &[OsString], dir: &Path) -> Result<ExitStatus> {
    let Some((program, arguments)) = argv.split_first() else {
        bail!("the test command is empty");
    };
    let stdout = std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .context("could not duplicate the standard error")?;

    Command::new(program)
        .args(arguments)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .status()
        .with_context(|| {
            format!(
                "could not start the test command '{}'",
                program.to_string_lossy()
            )
        })
}

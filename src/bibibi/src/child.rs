//! Start the build command and the test command.

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
    let mut command = Command::new(program);
    command.args(arguments);
    run_command(command, dir).with_context(|| {
        format!(
            "could not start the test command '{}'",
            program.to_string_lossy()
        )
    })
}

/// Run `build` through `sh -c` in `dir` and wait for it.
///
/// The input and output rules are the same as for [`run`].
///
/// # Errors
///
/// Returns an error if the shell cannot start.
pub fn run_build(build: &str, dir: &Path) -> Result<ExitStatus> {
    let mut command = Command::new("sh");
    command.arg("-c").arg(build);
    run_command(command, dir).context("could not start the build command")
}

/// Give `command` its directory, no input, and the standard error of
/// `bibibi` as its output. Then run it and wait.
fn run_command(mut command: Command, dir: &Path) -> Result<ExitStatus> {
    let stdout = std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .context("could not duplicate the standard error")?;
    command
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .status()
        .map_err(Into::into)
}

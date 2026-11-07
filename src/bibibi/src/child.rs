//! Start the build command and the test command.

use std::ffi::OsString;
use std::os::fd::AsFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

use anyhow::{bail, Context, Result};

use crate::interrupt;

/// How a child ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// The child exited with this code.
    Exited(i32),
    /// This signal stopped the child.
    Killed(i32),
}

impl Ended {
    /// Convert a wait status. `wait` reports only a process that ended, so a
    /// status has a code or a signal.
    ///
    /// # Errors
    ///
    /// Returns an error that names the raw wait status if it has neither.
    fn from_status(status: ExitStatus) -> Result<Self> {
        if let Some(signal) = status.signal() {
            Ok(Self::Killed(signal))
        } else if let Some(code) = status.code() {
            Ok(Self::Exited(code))
        } else {
            bail!(
                "the child ended with an unknown wait status (raw status {})",
                status.into_raw()
            )
        }
    }
}

/// Run `argv` in `dir` and wait for it.
///
/// The program gets no standard input. Its standard output goes to the
/// standard error of `bibibi`, so the standard output of `bibibi` holds only
/// the result. The program runs without a shell.
///
/// # Errors
///
/// Returns an error if `argv` is empty, or if the program cannot start.
/// Returns [`interrupt::Interrupted`] if a signal stopped the run.
pub fn run(argv: &[OsString], dir: &Path) -> Result<Ended> {
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
/// Returns [`interrupt::Interrupted`] if a signal stopped the run.
pub fn run_build(build: &str, dir: &Path) -> Result<Ended> {
    let mut command = Command::new("sh");
    command.arg("-c").arg(build);
    run_command(command, dir).context("could not start the build command")
}

/// Give `command` its directory, no input, and the standard error of
/// `bibibi` as its output. The child leads a new process group, and the
/// signal thread knows that group while the child runs. Then run it and wait.
fn run_command(mut command: Command, dir: &Path) -> Result<Ended> {
    let stdout = std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .context("could not duplicate the standard error")?;
    let mut child = command
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .process_group(0)
        .spawn()?;
    // The group id of a child that leads its group is the pid of the child.
    interrupt::register_child(i32::try_from(child.id()).context("the pid is too large")?);
    let waited = child.wait();
    interrupt::clear_child();
    let status = waited?;
    interrupt::check()?;
    Ended::from_status(status)
}

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
/// `bibibi` as its output. The child starts a session of its own, thus it has
/// no controlling terminal. A child that opens `/dev/tty` gets an error at
/// once, and a change of the terminal settings raises no SIGTTOU or SIGTTIN.
/// So the child never stops on the terminal, and the wait always returns. A
/// session leader leads its own process group, and the signal thread knows
/// that group while the child runs. Then run it and wait.
///
/// The child does not inherit the git environment. A `GIT_DIR` that a hook
/// exported must not aim the test at another repository. The six names of
/// `gitscratch::USER_INTENT_GIT_ENVIRONMENT` stay, for example the
/// `GIT_SSH_COMMAND` of the user.
fn run_command(mut command: Command, dir: &Path) -> Result<Ended> {
    gitscratch::shed_inherited_git_environment_keeping_user_intent(&mut command);
    let stdout = std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .context("could not duplicate the standard error")?;
    command
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout));
    // SAFETY: the closure runs in the child between the fork and the exec, and
    // it calls one function. `setsid` is async-signal-safe and touches no
    // memory of this process. The child is never a process group leader in
    // that window, because the fork gave it a new process id and its process
    // group is still the one of the parent, so the one documented failure of
    // `setsid` (EPERM) cannot happen.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    // A session leader leads its own process group, thus the group id of the
    // child is the pid of the child.
    interrupt::register_child(i32::try_from(child.id()).context("the pid is too large")?);
    let waited = child.wait();
    interrupt::clear_child();
    let status = waited?;
    interrupt::check()?;
    Ended::from_status(status)
}

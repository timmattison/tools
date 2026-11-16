//! What one run of `glance` does with the status item of its target: start it
//! here, start it in the background, or stop it.
//!
//! Each target has one status item (see [`crate::instance`]). A run first
//! looks for the status item of its target. When one runs, the run stops it,
//! starts nothing, and ends. Thus a second run of the same command closes the
//! status item that the first run opened. A run for a different target
//! leaves that status item alone.
//!
//! A background start makes a copy of `glance` with the hidden flag
//! `--background-child`. It does not `fork` without `exec`: macOS refuses
//! AppKit in a forked child. The copy starts a new session, so the terminal
//! of the start cannot stop it, and it takes the lock itself. Thus two starts
//! at the same moment can never give two status items for one target.
//!
//! The stdout of the copy is a pipe to the start. The copy writes one line
//! into it when its status item is up, as a foreground run writes that line
//! to the terminal. The start copies that line to its own stdout and ends. A
//! copy that ends before its status item is up closes the pipe with no line,
//! and the start reports the failure. Thus the start needs no clock: the read
//! ends with the line or with the end of the copy. The status item writes
//! nothing more to its stdout, so the pipe that nobody reads after the start
//! ended does no harm.

use std::env;
use std::fmt;
use std::io::{self, BufRead, BufReader, Write};
use std::num::NonZeroU32;
use std::process::{Child, ExitCode, ExitStatus, Stdio};

use crate::connection::Remote;
use crate::flags::{
    flag, flag_with_value, BACKGROUND_CHILD, EXIT_AFTER, HOST, INTERVAL, REMOTE_BINARY,
};
use crate::instance::{self, ClaimError, LockFile, Pid, Target};
use crate::interval::Interval;
use crate::shell;

/// The exit code of a run that failed.
const EXIT_FAILURE: u8 = 1;

/// The working directory of the background copy.
///
/// The copy runs long after its start ended, and a volume cannot unmount
/// while a process holds a directory on it. The root directory is on no
/// volume that a user unmounts.
const CHILD_DIRECTORY: &str = "/";

/// How a run starts the status item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The status item runs in this process, which holds the terminal.
    Foreground,
    /// This process starts a copy that runs the status item, and returns.
    Background,
    /// This process is the copy that a background start made.
    BackgroundChild,
}

/// The settings of the status item.
#[derive(Debug, Clone)]
pub struct Settings {
    /// The time between two samples.
    pub interval: Interval,
    /// The second Mac that the status item shows, or `None` for this Mac.
    pub remote: Option<Remote>,
    /// The life of the status item after it is up, or `None` for no bound.
    pub exit_after: Option<NonZeroU32>,
}

impl Settings {
    /// Gives the target of these settings.
    #[must_use]
    pub fn target(&self) -> Target {
        self.remote.as_ref().map_or(Target::Local, |remote| {
            Target::Host(remote.destination.clone())
        })
    }
}

/// Gives the arguments that a background start gives to its copy.
///
/// The copy parses them with the same command line as a person's run, so
/// the copy gets the same settings.
#[must_use]
pub fn child_arguments(settings: &Settings) -> Vec<String> {
    let mut arguments = vec![
        flag(BACKGROUND_CHILD),
        flag_with_value(INTERVAL, &settings.interval.to_string()),
    ];
    if let Some(remote) = &settings.remote {
        arguments.push(flag_with_value(HOST, remote.destination.as_str()));
        arguments.push(flag_with_value(
            REMOTE_BINARY,
            remote.remote_binary.as_str(),
        ));
    }
    if let Some(seconds) = settings.exit_after {
        arguments.push(flag_with_value(EXIT_AFTER, &seconds.to_string()));
    }
    arguments
}

/// Starts or stops the status item of the target of `settings`, as `mode`
/// tells, and gives the exit code of the run.
///
/// A foreground run returns only when the status item cannot start. A
/// background run returns when the status item of its copy is up.
#[must_use]
pub fn run(mode: Mode, settings: Settings) -> ExitCode {
    let target = settings.target();
    let lock = match LockFile::for_user(&target) {
        Ok(lock) => lock,
        Err(error) => return fail(&format!("cannot find the lock file: {error}")),
    };
    if mode != Mode::BackgroundChild {
        match instance::holder(&lock) {
            Err(error) => return fail(&lock_problem(&lock, &error)),
            Ok(Some(pid)) => return stop(&lock, &target, pid),
            Ok(None) => {}
        }
    }
    match mode {
        Mode::Foreground => run_here(&lock, settings),
        Mode::Background => start_in_background(&lock, &settings),
        Mode::BackgroundChild => match instance::start_a_session() {
            Ok(()) => run_here(&lock, settings),
            Err(error) => fail(&format!("cannot start a new session: {error}")),
        },
    }
}

/// Stops the status item of `target`, which runs as `pid`.
fn stop(lock: &LockFile, target: &Target, pid: Pid) -> ExitCode {
    match instance::stop(lock, pid) {
        Ok(()) => {
            say(&format!("{}\n", stopped_line(target, pid)));
            ExitCode::SUCCESS
        }
        Err(error) => fail(&error.to_string()),
    }
}

/// Takes the lock and runs the status item in this process. When the status
/// item is up, writes the line that tells so to stdout: to the terminal of a
/// foreground run, or to the pipe of a background start.
fn run_here(lock: &LockFile, settings: Settings) -> ExitCode {
    // The claim must live until the process ends. The run loop of the shell
    // never returns while the status item runs.
    let _claim = match instance::claim(lock) {
        Ok(claim) => claim,
        Err(ClaimError::Held(pid)) => return fail(&another_started(pid)),
        Err(ClaimError::Io(error)) => return fail(&lock_problem(lock, &error)),
    };
    if let Err(error) = instance::end_with_success_on_terminate() {
        return fail(&format!("cannot catch SIGTERM: {error}"));
    }
    let line = format!(
        "{}\n",
        ready_line(&settings.target(), Pid::of_this_process())
    );
    let exit_after = settings.exit_after;
    shell::run(settings.interval, settings.remote, move || {
        say(&line);
        if let Some(seconds) = exit_after {
            instance::end_after(seconds);
        }
    })
}

/// Starts a copy that runs the status item, and waits until its status item
/// is up.
fn start_in_background(lock: &LockFile, settings: &Settings) -> ExitCode {
    if let Err(error) = lock.prepare() {
        return fail(&lock_problem(lock, &error));
    }
    let program = match env::current_exe() {
        Ok(program) => program,
        Err(error) => return fail(&format!("cannot find the path of glance: {error}")),
    };
    let spawned = childenv::command(program)
        .args(child_arguments(settings))
        .current_dir(CHILD_DIRECTORY)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return fail(&format!("cannot start the background copy: {error}")),
    };
    if let Some(line) = first_line(&mut child) {
        say(&line);
        return ExitCode::SUCCESS;
    }
    let status = child.wait();
    match instance::holder(lock) {
        Ok(Some(pid)) => fail(&another_started(pid)),
        _ => fail(&copy_stopped(&status)),
    }
}

/// Reads the first line of the stdout of `child`. Gives `None` when the
/// child closed its stdout before it wrote a whole line.
fn first_line(child: &mut Child) -> Option<String> {
    let stdout = child.stdout.take()?;
    let mut line = String::new();
    match BufReader::new(stdout).read_line(&mut line) {
        Ok(_) if line.ends_with('\n') => Some(line),
        _ => None,
    }
}

/// Writes `text` to stdout. A closed stdout is no reason to fail, because
/// the status item runs or stopped already.
fn say(text: &str) {
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

/// Writes `message` to stderr and gives the exit code of a failure.
fn fail(message: &str) -> ExitCode {
    eprintln!("glance: {message}");
    ExitCode::from(EXIT_FAILURE)
}

/// The line that tells that the status item of `target` is up.
fn ready_line(target: &Target, pid: Pid) -> String {
    match target {
        Target::Local => {
            format!("glance: the status item runs as PID {pid}. Run glance again to stop it.")
        }
        Target::Host(destination) => format!(
            "glance: the status item of the host {destination} runs as PID {pid}. \
             Run glance again with the same --host to stop it."
        ),
    }
}

/// The line that tells that a run stopped the status item of `target`.
fn stopped_line(target: &Target, pid: Pid) -> String {
    match target {
        Target::Local => format!("glance: stopped the status item that ran as PID {pid}."),
        Target::Host(destination) => format!(
            "glance: stopped the status item of the host {destination} that ran as PID {pid}."
        ),
    }
}

/// The reason of a start that found the lock taken after its look.
fn another_started(pid: Pid) -> String {
    format!("another glance started the same status item at the same time, as PID {pid}")
}

/// The reason of a lock file that cannot be used.
fn lock_problem(lock: &LockFile, error: &io::Error) -> String {
    format!("cannot use the lock file {lock}: {error}")
}

/// The reason of a background copy that ended before its status item was up.
fn copy_stopped(status: &io::Result<ExitStatus>) -> String {
    format!(
        "the background copy stopped before its status item was up ({})",
        Shown(status)
    )
}

/// Shows the result of a wait for a child.
struct Shown<'a>(&'a io::Result<ExitStatus>);

impl fmt::Display for Shown<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Ok(status) => write!(formatter, "{status}"),
            Err(error) => write!(formatter, "the wait failed: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::RemoteBinary;

    /// The PID of the lines in these tests.
    fn pid() -> Pid {
        Pid::new(4242).expect("a PID")
    }

    /// The settings of the host `destination` with the remote binary `binary`.
    fn host_settings(destination: &str, binary: &str) -> Settings {
        Settings {
            interval: Interval::DEFAULT,
            remote: Some(Remote {
                destination: destination.parse().expect("a destination"),
                remote_binary: binary.parse::<RemoteBinary>().expect("a remote binary"),
            }),
            exit_after: NonZeroU32::new(30),
        }
    }

    #[test]
    fn the_local_lines_name_the_pid_and_the_command_that_stops() {
        assert_eq!(
            ready_line(&Target::Local, pid()),
            "glance: the status item runs as PID 4242. Run glance again to stop it."
        );
        assert_eq!(
            stopped_line(&Target::Local, pid()),
            "glance: stopped the status item that ran as PID 4242."
        );
    }

    #[test]
    fn the_host_lines_name_the_destination() {
        let target = host_settings("tim@m1-ts", "glance").target();
        assert_eq!(
            ready_line(&target, pid()),
            "glance: the status item of the host tim@m1-ts runs as PID 4242. \
             Run glance again with the same --host to stop it."
        );
        assert_eq!(
            stopped_line(&target, pid()),
            "glance: stopped the status item of the host tim@m1-ts that ran as PID 4242."
        );
    }

    #[test]
    fn settings_with_no_remote_target_this_mac() {
        let settings = Settings {
            interval: Interval::DEFAULT,
            remote: None,
            exit_after: None,
        };
        assert_eq!(settings.target(), Target::Local);
        assert_eq!(
            child_arguments(&settings),
            ["--background-child", "--interval=1"]
        );
    }

    #[test]
    fn the_copy_of_a_host_gets_the_destination_the_binary_and_the_bound() {
        assert_eq!(
            child_arguments(&host_settings("m1-ts", "-odd/glance")),
            [
                "--background-child",
                "--interval=1",
                "--host=m1-ts",
                "--remote-binary=-odd/glance",
                "--exit-after=30",
            ]
        );
    }

    #[test]
    fn a_copy_that_stopped_names_its_status() {
        let failed: io::Result<ExitStatus> = Err(io::Error::other("no child"));
        assert_eq!(
            copy_stopped(&failed),
            "the background copy stopped before its status item was up (the wait failed: no child)"
        );
    }
}

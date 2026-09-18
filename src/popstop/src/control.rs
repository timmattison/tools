//! The commands that act on the copy that runs (macOS only).
//!
//! `popstop --status` shows the copy that runs. `popstop --stop` ends it.
//! Both read the instance lock first, because the lock is the only source of
//! truth about a copy that runs.

use std::io;
use std::time::Duration;

use crate::exit_status;
use crate::life_cycle::{Failure, Settings};
use crate::lock::{self, HolderRecord, Release, StateDir};
use crate::message;
use crate::output;
use crate::process::{self, Identity};

/// The longest time that a stop waits for the copy to release the lock.
///
/// The copy ramps its signal down before it stops, and that takes a moment.
/// A copy that does not release the lock in this time needs the user, thus
/// the wait ends here.
const RELEASE_BOUND: Duration = Duration::from_secs(5);

/// What a command that acts on the copy that runs found.
///
/// The text goes to stdout, because it is the answer to the question that the
/// user asked. The status goes to the caller of popstop, because a script
/// reads the status and not the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The exit status of popstop.
    status: u8,
    /// The lines for the user. They carry the name of popstop already.
    text: String,
}

impl Report {
    /// Gives the exit status of popstop.
    #[must_use]
    pub fn status(&self) -> u8 {
        self.status
    }

    /// Gives the lines for the user.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Shows the copy of popstop that runs.
///
/// A script reads the exit status and not the text: [`exit_status::SUCCESS`]
/// when a copy runs, and [`exit_status::NO_COPY_RUNS`] when no copy runs.
///
/// # Errors
///
/// Returns a [`Failure`] with the status [`exit_status::ERROR`] when the lock
/// cannot be read. A name of the default output device that cannot be read is
/// not a failure: a copy runs, and that is the answer.
pub fn status(settings: &Settings) -> Result<Report, Failure> {
    let dir = settings.state_dir()?;
    let Some(record) = holder(&dir)? else {
        return Ok(Report {
            status: exit_status::NO_COPY_RUNS,
            text: message::no_copy_runs(),
        });
    };
    // The default output unit follows the default output device, thus the
    // device that popstop reads here is the device that the copy keeps awake.
    let text = match output::default_output_device_name() {
        Ok(name) => message::status_lines(&record, &name),
        Err(problem) => message::status_lines_without_device_name(&record, &problem),
    };
    Ok(Report {
        status: exit_status::SUCCESS,
        text,
    })
}

/// Stops the copy of popstop that runs.
///
/// A stop is idempotent: a stop that finds no copy is a success.
///
/// # Errors
///
/// Returns a [`Failure`] with the status [`exit_status::ERROR`] when the lock
/// cannot be read, when the signal does not reach the copy, or when the copy
/// does not release the lock.
pub fn stop(settings: &Settings) -> Result<Report, Failure> {
    let dir = settings.state_dir()?;
    let Some(record) = holder(&dir)? else {
        return Ok(nothing_runs());
    };

    // The identity check comes before the signal. A holder writes its record
    // a moment after it takes the lock, thus a reader can see the record of a
    // copy that crashed. The system gives the PID of that copy to a new
    // process, and that process belongs to somebody else.
    let looked_up = process::start_time(record.pid);
    let identity = process::identity(&record, looked_up).map_err(|problem| {
        Failure::error(&format!(
            "the start time of pid {} cannot be read: {problem}",
            record.pid
        ))
    })?;
    match identity {
        Identity::Gone => Ok(nothing_runs()),
        Identity::Another => Err(Failure::new(
            exit_status::ERROR,
            message::stale_record(record.pid),
        )),
        Identity::TheSame => end_the_copy(&dir, record.pid),
    }
}

/// Gives the report of a command that found no copy of popstop.
fn nothing_runs() -> Report {
    Report {
        status: exit_status::SUCCESS,
        text: message::no_copy_runs(),
    }
}

/// Sends `SIGTERM` to the copy `pid`, and waits until it releases the lock.
///
/// The release is the proof that the copy is gone, thus the report comes
/// after it and not after the signal.
fn end_the_copy(dir: &StateDir, pid: u32) -> Result<Report, Failure> {
    send_the_stop_signal(pid).map_err(|problem| {
        Failure::error(&format!(
            "the signal did not reach the copy (pid {pid}): {problem}"
        ))
    })?;
    let released = lock::wait_for_release(dir, RELEASE_BOUND).map_err(|problem| {
        Failure::error(&format!(
            "the wait for the copy (pid {pid}) cannot be made: {problem}"
        ))
    })?;
    match released {
        Release::Released => Ok(Report {
            status: exit_status::SUCCESS,
            text: message::stopped(pid),
        }),
        // popstop never sends SIGKILL by itself. The user decides.
        Release::TimedOut => Err(Failure::new(
            exit_status::ERROR,
            message::did_not_stop(pid, RELEASE_BOUND),
        )),
    }
}

/// Gives the record of the copy that holds the lock in `dir`.
fn holder(dir: &StateDir) -> Result<Option<HolderRecord>, Failure> {
    lock::current_holder(dir)
        .map_err(|problem| Failure::error(&format!("the lock cannot be read: {problem}")))
}

/// Sends `SIGTERM` to the process `pid`.
///
/// `SIGTERM` is one of the signals that stop a copy of popstop, thus the copy
/// ramps its signal down and releases the lock before it ends.
fn send_the_stop_signal(pid: u32) -> io::Result<()> {
    let pid = libc::pid_t::try_from(pid).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{pid} is too large to be a process ID"),
        )
    })?;
    // SAFETY: `kill` takes two numbers by value, and it changes nothing in
    // this process. The caller compared the start time of the PID with the
    // record of the holder, thus the PID names the copy that runs.
    let sent = unsafe { libc::kill(pid, libc::SIGTERM) };
    if sent == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

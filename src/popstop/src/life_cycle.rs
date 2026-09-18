//! The life cycle of a copy of popstop (macOS only).
//!
//! A copy takes the instance lock, starts the keepalive signal, reports that
//! it plays, and then waits for a signal. On a signal it ramps the signal
//! down, stops the output unit, and releases the lock.
//!
//! [`run`] holds that life cycle for every mode. The mode and the way to
//! report a ready copy are its parameters, because a background copy tells
//! its parent through a pipe and a foreground copy writes to its terminal.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::{Handle, Signals};

use crate::exit_status;
use crate::keepalive::Keepalive;
use crate::lock::{self, AcquireError, HolderRecord, LockGuard, Mode, StateDir};
use crate::message;
use crate::process;

/// The priority inside the quality of service class. Zero is the priority of
/// the class itself, which is what popstop wants.
const RELATIVE_QOS: libc::c_int = 0;

/// The signals that stop a copy of popstop.
///
/// `SIGINT` is Ctrl-C at a terminal. `SIGHUP` arrives when the terminal of a
/// foreground copy closes, and it must stop that copy in the same quiet way.
/// `SIGTERM` is what `popstop --stop` sends, and what a system that shuts
/// down sends.
const STOP_SIGNALS: [libc::c_int; 3] = [SIGINT, SIGHUP, SIGTERM];

/// The name of the thread that waits for a signal.
const SIGNAL_THREAD_NAME: &str = "popstop-signals";

/// What a copy of popstop got on its command line.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// The directory of the lock file and the log, from `--state-dir`. The
    /// state directory of the user, when it is `None`.
    pub state_dir: Option<PathBuf>,
    /// The time after which the copy stops by itself, from `--exit-after`.
    pub exit_after: Option<Duration>,
}

impl Settings {
    /// Gives the state directory that these settings name.
    fn state_dir(&self) -> Result<StateDir, Failure> {
        match &self.state_dir {
            Some(path) => Ok(StateDir::new(path.clone())),
            None => StateDir::for_user().map_err(|error| Failure::error(&error)),
        }
    }

    /// Gives the path that `--state-dir` named, for the command that stops
    /// the copy that runs.
    fn state_dir_argument(&self) -> Option<&Path> {
        self.state_dir.as_deref()
    }
}

/// A run of popstop that did not reach its end.
///
/// The failure carries the exit status and the text for the user. It does not
/// write that text: a foreground copy writes it to stderr, and a background
/// copy sends it to the parent that started it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The exit status of the copy.
    status: u8,
    /// The lines for the user. They carry the name of popstop already.
    message: String,
}

impl Failure {
    /// Makes the failure of a problem that ends a run, with the status
    /// [`exit_status::ERROR`].
    fn error(problem: &dyn fmt::Display) -> Self {
        Self {
            status: exit_status::ERROR,
            message: message::problem_line(problem),
        }
    }

    /// Gives the exit status of the copy.
    #[must_use]
    pub fn status(&self) -> u8 {
        self.status
    }

    /// Gives the lines for the user.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// A copy that plays, for the report to the user.
#[derive(Debug, Clone, Copy)]
pub struct Ready<'a> {
    /// The name of the device that stays awake.
    pub device_name: &'a str,
    /// The process ID of the copy.
    pub pid: u32,
}

/// Why a copy stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// A signal arrived. This is its number.
    Signal(libc::c_int),
    /// The time of `--exit-after` passed.
    TimeLimit,
}

/// Runs a copy of popstop in the foreground, until a signal stops it.
///
/// It writes the status lines to stdout, and it stops on the signals of
/// [`STOP_SIGNALS`].
///
/// # Errors
///
/// Returns a [`Failure`] with the status [`exit_status::ANOTHER_COPY_RUNS`]
/// when another copy holds the lock, and a failure with the status
/// [`exit_status::ERROR`] for every other problem.
pub fn run_foreground(settings: &Settings) -> Result<(), Failure> {
    run(Mode::Foreground, settings, |ready| {
        let mut stdout = io::stdout().lock();
        writeln!(
            stdout,
            "{}",
            message::ready_lines(ready.device_name, ready.pid)
        )?;
        stdout.flush()
    })
}

/// Runs a copy of popstop in `mode`, until a signal stops it.
///
/// `report_ready` tells the user, or the parent process, that the copy plays.
///
/// # Errors
///
/// Returns a [`Failure`] with the status [`exit_status::ANOTHER_COPY_RUNS`]
/// when another copy holds the lock, and a failure with the status
/// [`exit_status::ERROR`] for every other problem.
pub fn run(
    mode: Mode,
    settings: &Settings,
    report_ready: impl FnOnce(&Ready<'_>) -> io::Result<()>,
) -> Result<(), Failure> {
    if let Err(problem) = set_background_qos() {
        // A class that the scheduler refused costs a little power, and
        // nothing else. The run continues.
        let _ = writeln!(io::stderr(), "{}", message::warning_line(&problem));
    }

    // The signals come first. From here on, a signal stops this copy in the
    // way that this module says, and never in the way of the system, which
    // ends the process where it stands.
    let signals = SignalWatch::start()?;

    let dir = settings.state_dir()?;
    let record = own_record(mode)?;
    let guard = acquire(&dir, &record, settings)?;

    let keepalive = Keepalive::start().map_err(|problem| Failure::error(&problem))?;
    let ready = Ready {
        device_name: keepalive.device_name(),
        pid: record.pid,
    };
    let ran = report_ready(&ready)
        .map_err(|problem| {
            Failure::error(&format!("the status lines cannot be written: {problem}"))
        })
        .and(signals.wait_for_stop(settings.exit_after).map(|_| ()));

    // The keepalive stops on every path, and the lock goes only after it.
    let stopped = keepalive.stop().map_err(|problem| Failure::error(&problem));
    drop(guard);
    ran.and(stopped)
}

/// Gives the record of this process, for the lock file.
fn own_record(mode: Mode) -> Result<HolderRecord, Failure> {
    let pid = std::process::id();
    let started_at = process::start_time(pid).map_err(|problem| {
        Failure::error(&format!(
            "the start time of this process cannot be read: {problem}"
        ))
    })?;
    Ok(HolderRecord {
        pid,
        mode,
        started_at,
    })
}

/// Takes the instance lock for `record`.
///
/// A copy that holds the lock already makes the refusal that the user reads,
/// with the command that stops that copy.
fn acquire(
    dir: &StateDir,
    record: &HolderRecord,
    settings: &Settings,
) -> Result<LockGuard, Failure> {
    lock::acquire(dir, record).map_err(|error| match error {
        AcquireError::Held(holder) => Failure {
            status: exit_status::ANOTHER_COPY_RUNS,
            message: message::refusal(
                &holder,
                &message::stop_command(settings.state_dir_argument()),
            ),
        },
        AcquireError::Io(problem) => Failure::error(&problem),
    })
}

/// The registration of the signals that stop popstop.
///
/// A thread of signal-hook sends each signal into a channel, and the main
/// thread waits on that channel. Thus the main thread does not poll, and the
/// work after a signal happens on the main thread and not in a handler.
struct SignalWatch {
    /// The signals that arrived.
    signals: Receiver<libc::c_int>,
    /// The handle that ends the iterator of the thread.
    handle: Handle,
    /// The thread that reads the signals.
    thread: Option<JoinHandle<()>>,
}

impl SignalWatch {
    /// Registers the signals that stop popstop, and starts the thread that
    /// reads them.
    fn start() -> Result<Self, Failure> {
        let mut signals = Signals::new(STOP_SIGNALS).map_err(|problem| {
            Failure::error(&format!(
                "the handlers of the signals cannot be registered: {problem}"
            ))
        })?;
        let handle = signals.handle();
        let (sender, receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name(SIGNAL_THREAD_NAME.to_owned())
            .spawn(move || {
                for signal in &mut signals {
                    if sender.send(signal).is_err() {
                        break;
                    }
                }
            })
            .map_err(|problem| {
                Failure::error(&format!(
                    "the thread that waits for a signal cannot start: {problem}"
                ))
            })?;
        Ok(Self {
            signals: receiver,
            handle,
            thread: Some(thread),
        })
    }

    /// Waits until a signal arrives.
    ///
    /// `exit_after` has no effect yet.
    fn wait_for_stop(&self, exit_after: Option<Duration>) -> Result<StopReason, Failure> {
        let _ = exit_after;
        match self.signals.recv() {
            Ok(signal) => Ok(StopReason::Signal(signal)),
            Err(_) => Err(Failure::error(
                &"the thread that waits for a signal ended, and no signal arrived",
            )),
        }
    }
}

impl Drop for SignalWatch {
    /// Ends the thread and removes the handlers of the signals.
    fn drop(&mut self) {
        self.handle.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Puts the calling thread into the background quality of service class.
///
/// popstop plays a signal that nobody hears, thus it never needs the
/// processor before another program does. The class tells the scheduler so.
///
/// # Errors
///
/// Returns the error of the system when the class cannot be set.
fn set_background_qos() -> io::Result<()> {
    // SAFETY: the call takes its class and its relative priority by value,
    // and it changes only the thread that calls it.
    let status = unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_BACKGROUND, RELATIVE_QOS)
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status))
    }
}

#[cfg(test)]
mod tests {
    use super::set_background_qos;
    use std::ptr;
    use std::thread;

    /// The number of the background class, as `sys/qos.h` gives it.
    const QOS_CLASS_BACKGROUND: u32 = 0x09;

    /// Gives the quality of service class of the calling thread, as a number.
    ///
    /// It reads the class into a `u32`, not into `libc::qos_class_t`. The
    /// system writes any number there, and a number that names no variant of
    /// that enum is not a valid value of it.
    fn qos_class_of_this_thread() -> u32 {
        let mut class = u32::MAX;
        let mut relative_priority = 0;
        // SAFETY: `pthread_get_qos_class_np` writes one `qos_class_t`, which
        // is a `u32`, into `class`, and one `c_int` into
        // `relative_priority`. Both live for the whole call.
        let status = unsafe {
            libc::pthread_get_qos_class_np(
                libc::pthread_self(),
                ptr::from_mut(&mut class).cast::<libc::qos_class_t>(),
                ptr::from_mut(&mut relative_priority),
            )
        };
        assert_eq!(status, 0, "the class of this thread cannot be read");
        class
    }

    #[test]
    fn the_background_class_holds_the_thread_that_asked_for_it() {
        // A thread of its own: the class of a thread of the test harness is
        // not this test to change.
        let class = thread::spawn(|| {
            let before = qos_class_of_this_thread();
            set_background_qos().expect("the background class");
            (before, qos_class_of_this_thread())
        })
        .join()
        .expect("the thread ends");

        assert_eq!(
            class.1, QOS_CLASS_BACKGROUND,
            "the thread runs in class {:#04x} after the call, and it ran in class {:#04x} before \
             it",
            class.1, class.0
        );
    }
}

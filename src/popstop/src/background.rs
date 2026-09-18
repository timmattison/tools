//! The background start and the copy that it makes (macOS only).
//!
//! `popstop --background` starts a copy of popstop and returns. The copy keeps
//! the device awake after the terminal of the start closes, and
//! `popstop --stop` ends it.
//!
//! The start makes the copy with `std::env::current_exe` and the flag
//! [`CHILD_FLAG`]. It does not `fork` without `exec`: macOS refuses a call of
//! Core Foundation in a forked child when the parent used Core Foundation, and
//! Core Audio is built on Core Foundation. Thus the start opens no device
//! itself, and the copy that it makes starts fresh.
//!
//! The copy takes the lock, and the start does not. So two starts at the same
//! moment can never make two copies play.
//!
//! The two processes share a pipe: the stdout of the copy is the stdin of the
//! start. The copy writes one [`Handshake`] into it and the start reads that
//! one line. Then the start writes the answer for the user and ends, and the
//! copy sends its later output to the log.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::control::Report;
use crate::exit_status;
use crate::handshake::Handshake;
use crate::life_cycle::{self, Failure, Ready, Settings};
use crate::lock::{Mode, StateDir};
use crate::message;

/// The flag that a start gives to the copy that it makes. A user never needs
/// it, thus `--help` hides it.
pub const CHILD_FLAG: &str = "--background-child";

/// The flag that gives a copy its time limit.
const EXIT_AFTER_FLAG: &str = "--exit-after";

/// The longest time that a start waits for the report of its copy.
///
/// The copy opens the default output device in that time, and a device that
/// needs longer needs the user. The wait has a bound because a start that
/// never ends is worse than a start that fails.
const HANDSHAKE_BOUND: Duration = Duration::from_secs(10);

/// The longest time that a start waits for a copy that reported nothing.
const END_BOUND: Duration = Duration::from_secs(5);

/// The time between two looks at a copy that ends.
const END_POLL: Duration = Duration::from_millis(10);

/// The name of the thread that reads the report of the copy.
const READ_THREAD_NAME: &str = "popstop-handshake";

/// Starts a copy of popstop that has no terminal, and waits until that copy
/// reports.
///
/// The report is the proof that the device is awake, thus the answer for the
/// user comes after it and not after the start of the copy (story 9).
///
/// # Errors
///
/// Returns the [`Failure`] that the copy reported, with the status of that
/// copy, so a refusal reaches the user as status
/// [`exit_status::ANOTHER_COPY_RUNS`] with the text of a foreground refusal.
/// Returns a failure with the status [`exit_status::ERROR`] when the copy
/// cannot start, and when it reports nothing.
pub fn start(settings: &Settings) -> Result<Report, Failure> {
    let dir = settings.state_dir()?;
    let log_path = dir.log_path();
    let log = open_the_log(&dir).map_err(|problem| {
        Failure::error(&format!(
            "the log {} cannot be opened: {problem}",
            log_path.display()
        ))
    })?;
    let mut copy = make_the_copy(settings, log)?;

    match read_the_report(&mut copy)? {
        Answer::Reported(Handshake::Ready { device_name, pid }) => Ok(Report::new(
            exit_status::SUCCESS,
            message::background_ready_lines(
                &device_name,
                pid,
                &message::stop_command(settings.state_dir_argument()),
            ),
        )),
        Answer::Reported(Handshake::Failed { status, message }) => {
            // The copy ends by itself after it reported. The wait reaps it.
            wait_for_the_end(&mut copy);
            Err(Failure::new(status, message))
        }
        Answer::Nothing => {
            // The copy ends by itself, or it ended already. The wait reaps it
            // and gives the log time to reach the disk.
            wait_for_the_end(&mut copy);
            Err(Failure::new(
                exit_status::ERROR,
                message::did_not_report(&log_path),
            ))
        }
        Answer::Silence => {
            end_the_copy(&mut copy);
            Err(Failure::new(
                exit_status::ERROR,
                message::did_not_report_within(HANDSHAKE_BOUND, &log_path),
            ))
        }
    }
}

/// Runs the life cycle as the copy that a background start made.
///
/// The copy reports through its stdout, which is the pipe to the start. After
/// the report it sends its stdout to the log, so its later output goes there
/// and a write cannot fail after the start ended.
///
/// # Errors
///
/// Returns the [`Failure`] of the life cycle: the status
/// [`exit_status::ANOTHER_COPY_RUNS`] when another copy holds the lock, and
/// the status [`exit_status::ERROR`] for every other problem.
pub fn run_child(settings: &Settings) -> Result<(), Failure> {
    let outcome = start_a_session().and_then(|()| {
        life_cycle::run(Mode::Background, settings, report_ready).map(|_stopped| ())
    });
    if let Err(failure) = &outcome {
        tell_the_start_about(failure);
    }
    outcome
}

/// Tells the start that made this copy why the copy did not play.
///
/// The start writes the text for the user and ends with the status of the
/// copy. Thus a refusal reaches the user with the words of a foreground
/// refusal, and a failure of the copy is never silent (story 10).
///
/// A failure after the report of a copy that plays goes to the log, because
/// the stdout of the copy is the log from that moment on.
fn tell_the_start_about(failure: &Failure) {
    let report = Handshake::Failed {
        status: failure.status(),
        message: failure.message().to_owned(),
    };
    let mut stdout = io::stdout().lock();
    // A write that fails here has no other place to report. The start then
    // finds no report, and it names the log of this copy.
    let _ = stdout.write_all(report.line().as_bytes());
    let _ = stdout.flush();
}

/// Puts this process into a session of its own, before it does anything else.
///
/// The copy then has no controlling terminal and leads its own process group.
/// Thus the terminal of the user cannot signal it, and a window that closes
/// leaves it playing (story 7 and story 8).
///
/// A start makes the copy with `spawn`, so the copy leads no process group
/// yet and `setsid` always works for it.
fn start_a_session() -> Result<(), Failure> {
    // SAFETY: `setsid` takes nothing by value and changes only the session,
    // the process group, and the controlling terminal of this process.
    let session = unsafe { libc::setsid() };
    if session == -1 {
        return Err(Failure::error(&format!(
            "the copy cannot start a session of its own: {}",
            io::Error::last_os_error()
        )));
    }
    Ok(())
}

/// Tells the start that the copy plays, and then sends the later output of
/// the copy to the log.
fn report_ready(ready: &Ready<'_>) -> io::Result<()> {
    let report = Handshake::Ready {
        device_name: ready.device_name.to_owned(),
        pid: ready.pid,
    };
    let mut stdout = io::stdout().lock();
    stdout.write_all(report.line().as_bytes())?;
    stdout.flush()?;
    drop(stdout);
    send_the_later_output_to_the_log()
}

/// Sends the stdout of this process to the log of the copy.
///
/// The start gave this copy the log as its stderr, thus a copy of that
/// descriptor on the descriptor of stdout sends stdout to the log too. The
/// call also closes the pipe to the start, thus a write after the start ended
/// cannot fail with a broken pipe.
///
/// The caller writes the report and empties the buffer of stdout before this
/// call, so no text of the report stays behind.
fn send_the_later_output_to_the_log() -> io::Result<()> {
    // SAFETY: `dup2` takes two numbers by value. It closes the descriptor of
    // stdout and makes it a copy of the descriptor of stderr. It changes
    // nothing else in this process.
    let made = unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) };
    if made == libc::STDOUT_FILENO {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Opens the log of the background copies in `dir`, and makes the directory
/// when it does not exist.
///
/// The open empties the log, thus the log holds the copy that runs and
/// nothing older, and it cannot grow without limit.
fn open_the_log(dir: &StateDir) -> io::Result<File> {
    fs::create_dir_all(dir.path())?;
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dir.log_path())
}

/// Starts the copy: this program again, with the flag that runs the life
/// cycle in the background mode.
///
/// The copy reads nothing, it reports through the pipe of its stdout, and it
/// writes everything else into `log`.
fn make_the_copy(settings: &Settings, log: File) -> Result<Child, Failure> {
    let program = std::env::current_exe().map_err(|problem| {
        Failure::error(&format!(
            "the program of this process cannot be found: {problem}"
        ))
    })?;
    let mut command = Command::new(program);
    command.arg(CHILD_FLAG);
    if let Some(dir) = settings.state_dir_argument() {
        command.arg(message::STATE_DIR_FLAG).arg(dir);
    }
    if let Some(limit) = settings.exit_after {
        command
            .arg(EXIT_AFTER_FLAG)
            .arg(limit.as_secs().to_string());
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log));
    command
        .spawn()
        .map_err(|problem| Failure::error(&format!("the copy cannot start: {problem}")))
}

/// What a start read from the stdout of its copy.
enum Answer {
    /// The copy reported.
    Reported(Handshake),
    /// The copy wrote no report, and it writes none: it closed its stdout, or
    /// it wrote a line that is not a report.
    Nothing,
    /// The copy wrote nothing within [`HANDSHAKE_BOUND`].
    Silence,
}

/// Reads the one line that the copy writes, for [`HANDSHAKE_BOUND`] at most.
///
/// A thread reads the line and sends it into a channel, and this call waits on
/// that channel with a bound. Thus a copy that writes nothing ends the wait,
/// and a read that never returns holds nothing.
fn read_the_report(copy: &mut Child) -> Result<Answer, Failure> {
    let Some(stdout) = copy.stdout.take() else {
        return Ok(Answer::Nothing);
    };
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name(READ_THREAD_NAME.to_owned())
        .spawn(move || {
            let mut line = String::new();
            let read = BufReader::new(stdout).read_line(&mut line);
            // After a timeout nobody receives, and the line means nothing.
            let _ = sender.send(read.map(|_| line));
        })
        .map_err(|problem| {
            Failure::error(&format!(
                "the thread that reads the report cannot start: {problem}"
            ))
        })?;

    Ok(match receiver.recv_timeout(HANDSHAKE_BOUND) {
        Ok(Ok(line)) => Handshake::parse(&line).map_or(Answer::Nothing, Answer::Reported),
        Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => Answer::Nothing,
        Err(RecvTimeoutError::Timeout) => Answer::Silence,
    })
}

/// Ends a copy that reported nothing, and waits for it.
///
/// It sends `SIGTERM`, which is the signal that stops a copy of popstop, and
/// never `SIGKILL`. A copy that already plays then ramps its signal down.
fn end_the_copy(copy: &mut Child) {
    if let Ok(pid) = libc::pid_t::try_from(copy.id()) {
        // SAFETY: `kill` takes two numbers by value. The PID is the PID of a
        // child of this process that nothing reaped yet, thus it names that
        // child and no other process.
        let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    wait_for_the_end(copy);
}

/// Waits for the copy to end, for [`END_BOUND`] at most.
///
/// The wait reaps the copy, so the start leaves no process behind. A copy that
/// stays gets no more attention: the start reports and ends, and the copy
/// holds no lock, because a copy takes the lock before it reports.
fn wait_for_the_end(copy: &mut Child) {
    let deadline = Instant::now() + END_BOUND;
    loop {
        match copy.try_wait() {
            Ok(None) => {}
            Ok(Some(_)) | Err(_) => return,
        }
        if Instant::now() >= deadline {
            return;
        }
        thread::sleep(END_POLL);
    }
}

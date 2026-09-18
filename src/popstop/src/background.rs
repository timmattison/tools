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
//!
//! The log belongs to the copy that holds the lock. The start opens the log
//! to append, and it gives the log to the copy as its stderr. The copy
//! empties the log only after it takes the lock, and a copy that another copy
//! refused writes nothing into it.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::AsFd;
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
///
/// Measured on a Mac mini with USB speakers: eight starts, three seconds
/// apart, took 0.19 s to 0.29 s each, so a start of one copy holds a margin
/// of more than 30 times this bound. Twelve starts with no time between them
/// took up to 6.8 s each, and one of the twelve passed the bound. The cause
/// is Core Audio and not popstop: a device that a copy released a moment
/// before opens slowly. So a test file that starts many copies together can
/// fail here, and a person who starts one copy cannot.
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

/// Runs the life cycle as the copy that a background start made, and gives
/// the exit status of the copy.
///
/// The copy reports through its stdout, which is the pipe to the start. After
/// the report it sends its stdout to the log, so its later output goes there
/// and a write cannot fail after the start ended.
///
/// The copy empties the log once it holds the lock (see [`empty_the_log`]).
/// It tells the start about a failure until it reported that it plays, and it
/// decides which failure goes into the log (see [`send_the_failure`]). Thus
/// the caller writes nothing.
///
/// The status is [`exit_status::SUCCESS`] when the copy stopped as it must,
/// [`exit_status::ANOTHER_COPY_RUNS`] when another copy holds the lock, and
/// [`exit_status::ERROR`] for every other problem.
#[must_use]
pub fn run_child(settings: &Settings) -> u8 {
    let mut start = PipeToTheStart::new(io::stdout());
    let outcome = start_a_session().and_then(|()| {
        life_cycle::run(Mode::Background, settings, empty_the_log, |ready| {
            report_ready(&mut start, ready)
        })
        .map(|_stopped| ())
    });
    match outcome {
        Ok(()) => exit_status::SUCCESS,
        Err(failure) => {
            send_the_failure(&failure, &mut start, &mut io::stderr());
            failure.status()
        }
    }
}

/// The stdout of a copy, which is the pipe to the start that made it.
///
/// The start reads one [`Handshake`] from it and then ends, thus the pipe
/// carries one report and no more. After the copy reported that it plays,
/// its stdout is the log (see [`send_the_later_output_to_the_log`]). A second
/// report then puts a line of JSON into the log and reaches no start.
struct PipeToTheStart<W> {
    /// The stdout of the copy.
    stdout: W,
    /// True when a report went out.
    reported: bool,
}

impl<W: Write> PipeToTheStart<W> {
    /// Makes the pipe of a copy that sent no report yet.
    fn new(stdout: W) -> Self {
        Self {
            stdout,
            reported: false,
        }
    }

    /// Sends `report` to the start, when no report went out before it. After
    /// the first report, this call writes nothing.
    fn send(&mut self, report: &Handshake) -> io::Result<()> {
        if self.reported {
            return Ok(());
        }
        self.stdout.write_all(report.line().as_bytes())?;
        self.stdout.flush()?;
        self.reported = true;
        Ok(())
    }
}

/// Tells the start why the copy did not play, through `start`, and writes the
/// reason into `log`, which is the stderr of the copy.
///
/// The start writes the text for the user and ends with the status of the
/// copy. Thus a refusal reaches the user with the words of a foreground
/// refusal, and a failure of the copy is never silent (story 10).
///
/// The log keeps the reason for the user to read after the start ended. A
/// refusal stays out of the log. Another copy holds the lock then, thus the
/// log belongs to that copy, and the refusal reaches the user through the
/// report to the start. Every other failure goes to the end of the log, thus
/// the log of a copy that reported nothing holds the reason too.
///
/// A failure after the copy reported that it plays sends no report, because
/// the start read its one report already (see [`PipeToTheStart`]). The
/// reason of that failure reaches the log once, as text.
fn send_the_failure(
    failure: &Failure,
    start: &mut PipeToTheStart<impl Write>,
    log: &mut impl Write,
) {
    // A write that fails here has no other place to report. The start then
    // finds no report, and it names the log of this copy.
    let _ = start.send(&Handshake::Failed {
        status: failure.status(),
        message: failure.message().to_owned(),
    });
    if failure.status() == exit_status::ANOTHER_COPY_RUNS {
        return;
    }
    // A write to the log that fails has no other place to report.
    let _ = writeln!(log, "{}", failure.message());
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

/// Tells the start that the copy plays, through `start`, and then sends the
/// later output of the copy to the log.
fn report_ready(start: &mut PipeToTheStart<impl Write>, ready: &Ready<'_>) -> io::Result<()> {
    start.send(&Handshake::Ready {
        device_name: ready.device_name.to_owned(),
        pid: ready.pid,
    })?;
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

/// Empties the log, once this copy holds the lock.
///
/// The copy that holds the lock owns the log. Thus the log holds the output
/// of that copy and nothing older, and it does not grow with each copy that
/// plays. A copy that another copy refused never gets here, thus it never
/// empties the log of the copy that runs.
///
/// A log that cannot be emptied gets a warning, and the copy plays on. Old
/// text in the log is no reason to let the device sleep.
fn empty_the_log() {
    let emptied = io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .map(File::from)
        .and_then(|log| empty(&log));
    if let Err(problem) = emptied {
        let _ = writeln!(
            io::stderr(),
            "{}",
            message::warning_line(&format!("the log cannot be emptied: {problem}"))
        );
    }
}

/// Empties `log` when it is a regular file, and does nothing to it when it is
/// not.
///
/// The stderr of a copy that a person started by hand can be a terminal, a
/// pipe, or `/dev/null`. None of these is a log, thus such a copy has nothing
/// to empty. A terminal and a pipe also refuse the call that empties a file.
fn empty(log: &File) -> io::Result<()> {
    if log.metadata()?.is_file() {
        log.set_len(0)
    } else {
        Ok(())
    }
}

/// Opens the log of the background copies in `dir` for the copy that a start
/// makes, and makes the directory when it does not exist.
///
/// The open keeps the text in the log, because another copy can hold the
/// lock, and that text is its output. The copy that takes the lock empties
/// the log (see [`empty_the_log`]). Each write goes to the end of the log,
/// thus a write never lands inside text that another process wrote.
fn open_the_log(dir: &StateDir) -> io::Result<File> {
    fs::create_dir_all(dir.path())?;
    OpenOptions::new()
        .create(true)
        .append(true)
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

#[cfg(test)]
mod tests {
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Write};
    use std::os::fd::OwnedFd;

    use super::{empty, open_the_log, send_the_failure, PipeToTheStart};
    use crate::exit_status;
    use crate::handshake::Handshake;
    use crate::life_cycle::Failure;
    use crate::lock::StateDir;

    /// The text of a copy that ran before.
    const OLD_TEXT: &str = "an old line of a copy that ran before\n";

    /// Gives each line that went through `start`, as the start reads it.
    fn reports(start: &PipeToTheStart<Vec<u8>>) -> Vec<Option<Handshake>> {
        std::str::from_utf8(&start.stdout)
            .expect("the pipe to the start carries text")
            .lines()
            .map(Handshake::parse)
            .collect()
    }

    /// Gives the text that went into `log`.
    fn text(log: &[u8]) -> &str {
        std::str::from_utf8(log).expect("the log holds text")
    }

    #[test]
    fn a_failure_after_the_report_reaches_the_log_once_and_sends_no_second_report() {
        let ready = Handshake::Ready {
            device_name: "Klipsch R-51PM".to_owned(),
            pid: 4242,
        };
        let failure = Failure::error(&"the signal cannot stop");
        let mut start = PipeToTheStart::new(Vec::new());
        let mut log = Vec::new();
        // The copy reports that it plays. From here on its stdout is the log.
        start
            .send(&ready)
            .expect("send the report that the copy plays");

        send_the_failure(&failure, &mut start, &mut log);

        assert_eq!(
            reports(&start),
            [Some(ready)],
            "a copy that reported that it plays sent a second report, and that \
             report lands in the log as JSON"
        );
        assert_eq!(
            text(&log),
            format!("{}\n", failure.message()),
            "the log does not hold the reason once, as text"
        );
    }

    #[test]
    fn a_failure_before_the_report_goes_to_the_start_and_into_the_log() {
        let failure = Failure::error(&"the default output device cannot be opened");
        let mut start = PipeToTheStart::new(Vec::new());
        let mut log = Vec::new();

        send_the_failure(&failure, &mut start, &mut log);

        assert_eq!(
            reports(&start),
            [Some(Handshake::Failed {
                status: exit_status::ERROR,
                message: failure.message().to_owned(),
            })],
            "the start did not get the one report of the failure"
        );
        assert_eq!(
            text(&log),
            format!("{}\n", failure.message()),
            "the log does not hold the reason once, as text"
        );
    }

    #[test]
    fn a_refusal_goes_to_the_start_and_leaves_the_log_of_the_other_copy_alone() {
        let refusal = Failure::new(
            exit_status::ANOTHER_COPY_RUNS,
            "popstop: another copy runs".to_owned(),
        );
        let mut start = PipeToTheStart::new(Vec::new());
        let mut log = Vec::new();

        send_the_failure(&refusal, &mut start, &mut log);

        assert_eq!(
            reports(&start),
            [Some(Handshake::Failed {
                status: exit_status::ANOTHER_COPY_RUNS,
                message: refusal.message().to_owned(),
            })],
            "the start did not get the one report of the refusal"
        );
        assert_eq!(
            text(&log),
            "",
            "a refused copy wrote into the log of the copy that runs"
        );
    }

    #[test]
    fn the_copy_that_holds_the_lock_empties_a_log_that_is_a_file() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let dir = StateDir::new(temp.path().join("state"));
        let log = open_the_log(&dir).expect("open the log for the copy");
        fs::write(dir.log_path(), OLD_TEXT).expect("write the old log");

        empty(&log).expect("empty the log");

        assert_eq!(
            fs::read_to_string(dir.log_path()).expect("read the log"),
            "",
            "the log still holds the text of the copy before it"
        );
    }

    #[test]
    fn a_stderr_that_is_no_regular_file_is_no_reason_to_fail() {
        // A pipe cannot be emptied: `ftruncate` refuses it. The reader stays
        // open, so the pipe is the pipe of a copy that a person watches.
        let (_reader, writer) = io::pipe().expect("make a pipe");
        let pipe = File::from(OwnedFd::from(writer));

        empty(&pipe).expect("a copy whose stderr is no log has nothing to empty");
    }

    #[test]
    fn a_log_that_cannot_be_emptied_gives_the_problem_and_keeps_its_text() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let path = temp.path().join("popstop.log");
        fs::write(&path, OLD_TEXT).expect("write the old log");
        // A descriptor that cannot write cannot empty the file either.
        let read_only = File::open(&path).expect("open the log to read");

        let emptied = empty(&read_only);

        assert!(
            emptied.is_err(),
            "a log that cannot be emptied gave no problem"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("read the log"),
            OLD_TEXT,
            "the log lost its text"
        );
    }

    #[test]
    fn a_write_of_the_copy_lands_after_the_text_that_another_process_wrote() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let dir = StateDir::new(temp.path().join("state"));
        let first_line = "the first line of the copy\n";
        let other_line = "a line of another process\n";
        let second_line = "the second line of the copy\n";

        let mut copy = open_the_log(&dir).expect("open the log for the copy");
        copy.write_all(first_line.as_bytes())
            .expect("write the first line of the copy");
        // Another process writes through a descriptor of its own, as a copy
        // that a later start made does.
        let mut other = OpenOptions::new()
            .append(true)
            .open(dir.log_path())
            .expect("open the log for another process");
        other
            .write_all(other_line.as_bytes())
            .expect("write the line of another process");
        copy.write_all(second_line.as_bytes())
            .expect("write the second line of the copy");

        assert_eq!(
            fs::read_to_string(dir.log_path()).expect("read the log"),
            format!("{first_line}{other_line}{second_line}"),
            "a write of the copy landed inside the text of another process"
        );
    }
}

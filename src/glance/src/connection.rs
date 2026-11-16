//! The `ssh` connection to a second Mac.
//!
//! The local `glance` opens one `ssh` connection. The remote Mac runs
//! `glance --stream`, and this module reads one JSON line for each sample.
//!
//! # The two threads
//!
//! A connection starts two threads, and neither one blocks the caller.
//!
//! - The standard error thread reads the standard error of the child to its
//!   end and keeps the last line that is not blank. A pipe that nobody reads
//!   fills up, and then `ssh` stops, so this thread always reads.
//! - The standard output thread reads one line at a time and sends one
//!   [`Event`] for each line. At the end of the output, it waits for the child
//!   to exit and then sends [`Event::Ended`].
//!
//! # The grace bounds
//!
//! The end of the output does not always mean that the child exits. So the
//! output thread waits at most [`EXIT_GRACE`] for the exit, and then it kills
//! the child. After that it waits at most [`STDERR_GRACE`] for the last line of
//! the standard error. A grandchild can keep a pipe open, and these bounds make
//! sure that [`Event::Ended`] always arrives.
//!
//! # Why the program is a parameter
//!
//! Production gives `ssh`. A test gives a fake script. So no test changes
//! `PATH`, which is global to the process.

use crate::host::HostName;
use crate::interval::Interval;
use crate::sample::Sample;
use crate::text::{parse_line, StreamLine};
use std::fmt;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::str::FromStr;
use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// The time between two checks for the exit of the child.
const EXIT_POLL: Duration = Duration::from_millis(20);

/// The longest wait for the exit of the child after the end of its output.
pub const EXIT_GRACE: Duration = Duration::from_secs(2);

/// The longest wait for the last line of the standard error after the exit.
pub const STDERR_GRACE: Duration = Duration::from_secs(1);

/// The start of the reason when the standard error was empty.
const ENDED_PREFIX: &str = "the connection ended with";

/// The first delay before a new connection attempt.
pub const FIRST_RETRY_DELAY: Duration = Duration::from_secs(5);

/// The longest delay before a new connection attempt.
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

/// The path of the remote `glance` when the person gives none.
pub const DEFAULT_REMOTE_BINARY: &str = "~/.cargo/bin/glance";

/// The reason that a text is not a destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationError {
    /// The text is empty.
    Empty,
    /// The text starts with a dash, so `ssh` reads it as an option.
    LeadingDash {
        /// The text that the person gave.
        text: String,
    },
}

impl fmt::Display for DestinationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DestinationError::Empty => write!(f, "the destination must not be empty"),
            DestinationError::LeadingDash { text } => {
                write!(
                    f,
                    "the destination must not start with \"-\", and \"{text}\" does"
                )
            }
        }
    }
}

impl std::error::Error for DestinationError {}

/// The place that `ssh` connects to: an alias, `user@host`, or an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination(String);

impl Destination {
    /// The destination as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Destination {
    type Err = DestinationError;

    /// Reads a destination from text.
    ///
    /// # Errors
    ///
    /// Returns an error if the text is empty or starts with `-`.
    fn from_str(text: &str) -> Result<Destination, DestinationError> {
        if text.is_empty() {
            return Err(DestinationError::Empty);
        }
        if text.starts_with('-') {
            return Err(DestinationError::LeadingDash {
                text: text.to_owned(),
            });
        }
        Ok(Destination(text.to_owned()))
    }
}

/// The reason that a text is not a remote binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteBinaryError {
    /// The text is empty.
    Empty,
}

impl fmt::Display for RemoteBinaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemoteBinaryError::Empty => write!(f, "the remote binary must not be empty"),
        }
    }
}

impl std::error::Error for RemoteBinaryError {}

/// The path of `glance` on the remote Mac.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBinary(String);

impl RemoteBinary {
    /// The path as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RemoteBinary {
    fn default() -> RemoteBinary {
        RemoteBinary(DEFAULT_REMOTE_BINARY.to_owned())
    }
}

impl FromStr for RemoteBinary {
    type Err = RemoteBinaryError;

    /// Reads a remote binary path from text.
    ///
    /// # Errors
    ///
    /// Returns an error if the text is empty.
    fn from_str(text: &str) -> Result<RemoteBinary, RemoteBinaryError> {
        if text.is_empty() {
            return Err(RemoteBinaryError::Empty);
        }
        Ok(RemoteBinary(text.to_owned()))
    }
}

/// The delay before the next connection attempt.
///
/// The delay doubles after each end of the connection, up to
/// [`MAX_RETRY_DELAY`]. A sample sets it back to [`FIRST_RETRY_DELAY`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backoff {
    /// The delay that the next end of the connection gets.
    next: Duration,
}

impl Backoff {
    /// A delay that starts at [`FIRST_RETRY_DELAY`].
    #[must_use]
    pub fn new() -> Backoff {
        Backoff {
            next: FIRST_RETRY_DELAY,
        }
    }

    /// Gives the delay for the end of a connection, then doubles it.
    pub fn after_end(&mut self) -> Duration {
        let delay = self.next;
        self.next = (self.next * 2).min(MAX_RETRY_DELAY);
        delay
    }

    /// Sets the delay back to [`FIRST_RETRY_DELAY`].
    pub fn after_sample(&mut self) {
        self.next = FIRST_RETRY_DELAY;
    }
}

impl Default for Backoff {
    fn default() -> Backoff {
        Backoff::new()
    }
}

/// The start of a path that the remote shell expands to the home directory.
const HOME_PREFIX: &str = "~/";

/// The words after the binary in the remote command.
const STREAM_FLAGS: &str = "--stream --interval";

/// The options that `ssh` gets before the destination. Each option has its own argument.
const SSH_OPTIONS: [&str; 10] = [
    "-T",
    "-o",
    "BatchMode=yes",
    "-o",
    "ConnectTimeout=10",
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=2",
    // Ends the options, so a destination can never act as one.
    "--",
];

/// The command that the remote shell runs.
///
/// A leading `~/` stays outside the quotes, so the remote shell expands it. The
/// rest of the path is quoted, so a space or a quote in it stays one word.
fn remote_command(binary: &RemoteBinary, interval: Interval) -> String {
    let path = binary.as_str();
    let quoted = match path.strip_prefix(HOME_PREFIX) {
        Some(rest) => format!("{HOME_PREFIX}{}", shellquote::shell_quote(rest)),
        None => shellquote::shell_quote(path),
    };
    format!("{quoted} {STREAM_FLAGS} {interval}")
}

/// The arguments of `ssh`.
fn ssh_arguments(
    destination: &Destination,
    interval: Interval,
    binary: &RemoteBinary,
) -> Vec<String> {
    let mut arguments: Vec<String> = SSH_OPTIONS
        .iter()
        .map(|&option| option.to_owned())
        .collect();
    arguments.push(destination.as_str().to_owned());
    arguments.push(remote_command(binary, interval));
    arguments
}

/// One thing that the connection reports.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// One sample line.
    Sample {
        /// The short name of the remote Mac.
        host: HostName,
        /// The sample.
        sample: Sample,
    },
    /// One error line of the remote Mac, or a line that does not parse.
    ///
    /// `host` is `Some` for an error line of the remote Mac. It is `None` for a
    /// line that does not parse.
    ReadError {
        /// The remote Mac that reported the error, if there is one.
        host: Option<HostName>,
        /// The reason of the error.
        reason: String,
    },
    /// The child stopped.
    ///
    /// The reason is the last line of its standard error. When the standard
    /// error was empty, the reason is its exit status.
    Ended {
        /// Why the child stopped.
        reason: String,
    },
}

/// The last line of `pipe` that is not blank, with no trailing white space.
///
/// Reads to the end of the pipe, so a full pipe never blocks the child.
fn last_line_of<R: Read>(pipe: R) -> Option<String> {
    let mut last = None;
    let mut reader = BufReader::new(pipe);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) | Err(_) => return last,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buffer);
                let trimmed = line.trim_end();
                if !trimmed.is_empty() {
                    last = Some(trimmed.to_owned());
                }
            }
        }
    }
}

/// Sends one event for each line of `pipe` that is not blank.
///
/// A byte that is not UTF-8 makes a line that does not parse. It does not stop
/// the loop. A read error ends the loop like the end of the file. A closed
/// receiver ends it too.
fn send_lines<R: Read>(pipe: R, sender: &mpsc::Sender<Event>) {
    let mut reader = BufReader::new(pipe);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&buffer);
        if line.trim().is_empty() {
            continue;
        }
        if sender.send(event_of(&line)).is_err() {
            return;
        }
    }
}

/// The event that one line of the stream gives.
fn event_of(line: &str) -> Event {
    match parse_line(line) {
        Ok(StreamLine::Sample { host, sample }) => Event::Sample { host, sample },
        Ok(StreamLine::Error { host, reason }) => Event::ReadError {
            host: Some(host),
            reason,
        },
        Err(error) => Event::ReadError {
            host: None,
            reason: error.to_string(),
        },
    }
}

/// Waits for the exit of the child, and kills it after [`EXIT_GRACE`].
///
/// Gives the exit status, or the error of the wait.
fn wait_for_exit(child: &Mutex<Child>) -> io::Result<ExitStatus> {
    let mut waited = Duration::ZERO;
    while waited < EXIT_GRACE {
        let status = child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_wait()?;
        if let Some(status) = status {
            return Ok(status);
        }
        thread::sleep(EXIT_POLL);
        waited += EXIT_POLL;
    }
    let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = child.kill();
    child.wait()
}

/// The reason that the child stopped.
///
/// This is the last line of its standard error. When there is none, it is the
/// exit status, or the error of the wait.
fn ending_reason(child: &Mutex<Child>, last_line: &mpsc::Receiver<Option<String>>) -> String {
    let status = wait_for_exit(child);
    let line = last_line.recv_timeout(STDERR_GRACE).ok().flatten();
    match (line, status) {
        (Some(line), _) => line,
        (None, Ok(status)) => format!("{ENDED_PREFIX} {status}"),
        (None, Err(error)) => error.to_string(),
    }
}

/// One running `ssh` child and the events that it sends.
#[derive(Debug)]
pub struct Connection {
    /// The child. A drop kills and reaps it.
    child: Arc<Mutex<Child>>,
    /// The events of the reader threads.
    events: mpsc::Receiver<Event>,
}

impl Connection {
    /// Starts `program` with the arguments of `ssh`.
    ///
    /// # Errors
    ///
    /// Returns the error of the operating system if the child does not start.
    pub fn start(
        program: &Path,
        destination: &Destination,
        interval: Interval,
        remote_binary: &RemoteBinary,
    ) -> io::Result<Connection> {
        let mut child = childenv::command(program)
            .args(ssh_arguments(destination, interval, remote_binary))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(child));
        let (sender, events) = mpsc::channel();
        let (last_line_sender, last_line) = mpsc::channel();
        thread::spawn(move || {
            let last = stderr.and_then(last_line_of);
            let _ = last_line_sender.send(last);
        });
        let watched = Arc::clone(&child);
        thread::spawn(move || {
            if let Some(pipe) = stdout {
                send_lines(pipe, &sender);
            }
            let reason = ending_reason(&watched, &last_line);
            let _ = sender.send(Event::Ended { reason });
        });
        Ok(Connection { child, events })
    }

    /// The events of the connection.
    #[must_use]
    pub fn receiver(&self) -> &mpsc::Receiver<Event> {
        &self.events
    }

    /// Stops the child. A drop does the same.
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// The program that starts each connection in production.
pub const SSH_PROGRAM: &str = "ssh";

/// The name that the status item shows for a second Mac.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HostNaming {
    /// The SSH destination, as the user typed it. The name stays the same
    /// for the full life of the status item.
    #[default]
    Destination,
    /// The name that the newest line of the second Mac reports, as
    /// `hostname -s` gives it there. Before the first line, the name is the
    /// destination. The flag `--resolve` selects this name.
    Reported,
}

/// The second Mac that a feed shows, and the path of `glance` there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// The SSH destination, as the user typed it.
    pub destination: Destination,
    /// The path of `glance` on the second Mac.
    pub remote_binary: RemoteBinary,
}

/// The start of the reason when the program of a connection does not start.
const START_FAILED: &str = "cannot start";

/// A connection to a second Mac that starts again after each end.
///
/// The first poll starts the first connection. After each end, the next
/// connection starts when the delay of a [`Backoff`] is over, and a sample
/// sets that delay back. The caller gives the time to each poll, so a test
/// can step the time of the backoff without a wait.
#[derive(Debug)]
pub struct Feed {
    /// The program that starts each connection.
    program: PathBuf,
    /// The second Mac.
    remote: Remote,
    /// The time between two samples.
    interval: Interval,
    /// The connection that runs, or `None` between two connections.
    connection: Option<Connection>,
    /// The delay before the next connection.
    backoff: Backoff,
    /// The time of the next connection, or `None` when it is due at once.
    retry_at: Option<Instant>,
}

impl Feed {
    /// Makes a feed. The first poll starts the first connection.
    #[must_use]
    pub fn new(program: impl Into<PathBuf>, remote: Remote, interval: Interval) -> Feed {
        Feed {
            program: program.into(),
            remote,
            interval,
            connection: None,
            backoff: Backoff::new(),
            retry_at: None,
        }
    }

    /// Gives the events since the last poll, and starts a connection when one
    /// is due at the time `now`.
    ///
    /// A program that does not start gives one [`Event::Ended`] at once, and
    /// the next try waits for the backoff as after any other end.
    #[must_use]
    pub fn poll(&mut self, now: Instant) -> Vec<Event> {
        if self.connection.is_none() && self.retry_at.is_none_or(|due| now >= due) {
            self.retry_at = None;
            match Connection::start(
                &self.program,
                &self.remote.destination,
                self.interval,
                &self.remote.remote_binary,
            ) {
                Ok(connection) => self.connection = Some(connection),
                Err(error) => {
                    self.retry_at = Some(now + self.backoff.after_end());
                    return vec![Event::Ended {
                        reason: format!("{START_FAILED} {}: {error}", self.program.display()),
                    }];
                }
            }
        }
        let events: Vec<Event> = self
            .connection
            .as_ref()
            .map(|connection| connection.receiver().try_iter().collect())
            .unwrap_or_default();
        for event in &events {
            match event {
                Event::Sample { .. } => self.backoff.after_sample(),
                Event::Ended { .. } => {
                    self.connection = None;
                    self.retry_at = Some(now + self.backoff.after_end());
                }
                Event::ReadError { .. } => {}
            }
        }
        events
    }

    /// Stops the connection, if one runs, and reaps its child.
    ///
    /// A later poll starts a new connection.
    pub fn stop(&mut self) {
        self.connection = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interval(text: &str) -> Interval {
        text.parse().expect("an interval")
    }

    #[test]
    fn a_destination_refuses_an_empty_text() {
        assert_eq!("".parse::<Destination>(), Err(DestinationError::Empty));
    }

    #[test]
    fn a_destination_refuses_a_leading_dash() {
        let error = "-oProxyCommand=x".parse::<Destination>().unwrap_err();
        assert_eq!(
            error,
            DestinationError::LeadingDash {
                text: "-oProxyCommand=x".to_owned()
            }
        );
        assert_eq!(
            error.to_string(),
            "the destination must not start with \"-\", and \"-oProxyCommand=x\" does"
        );
        assert_eq!(
            DestinationError::Empty.to_string(),
            "the destination must not be empty"
        );
    }

    #[test]
    fn a_destination_keeps_each_other_text() {
        for text in ["m1-ts", "tim@m1.local", "192.168.1.5"] {
            let destination = text.parse::<Destination>().expect("a destination");
            assert_eq!(destination.as_str(), text);
            assert_eq!(destination.to_string(), text);
        }
    }

    #[test]
    fn a_remote_binary_refuses_an_empty_text() {
        assert_eq!("".parse::<RemoteBinary>(), Err(RemoteBinaryError::Empty));
    }

    #[test]
    fn a_remote_binary_keeps_each_other_text() {
        let binary = "~/my bin/glance".parse::<RemoteBinary>().expect("a binary");
        assert_eq!(binary.as_str(), "~/my bin/glance");
        assert_eq!(RemoteBinary::default().as_str(), DEFAULT_REMOTE_BINARY);
    }

    fn command_for(binary: &str) -> String {
        remote_command(&binary.parse().expect("a binary"), interval("1"))
    }

    #[test]
    fn the_default_binary_keeps_its_tilde_outside_the_quotes() {
        assert_eq!(
            remote_command(&RemoteBinary::default(), interval("1")),
            "~/'.cargo/bin/glance' --stream --interval 1"
        );
    }

    #[test]
    fn a_tilde_path_with_a_space_quotes_only_the_rest() {
        assert_eq!(
            command_for("~/my bin/glance"),
            "~/'my bin/glance' --stream --interval 1"
        );
    }

    #[test]
    fn only_a_leading_tilde_slash_gets_the_rule() {
        assert_eq!(
            command_for("~user/glance"),
            "'~user/glance' --stream --interval 1"
        );
        assert_eq!(
            command_for("/opt/a~b/glance"),
            "'/opt/a~b/glance' --stream --interval 1"
        );
        assert_eq!(command_for("glance"), "'glance' --stream --interval 1");
    }

    #[test]
    fn the_command_carries_the_interval() {
        assert_eq!(
            remote_command(&RemoteBinary::default(), interval("0.25")),
            "~/'.cargo/bin/glance' --stream --interval 0.25"
        );
    }

    #[test]
    fn ssh_gets_the_options_then_the_destination_then_the_command() {
        let destination = "m1-ts".parse::<Destination>().expect("a destination");
        let arguments = ssh_arguments(&destination, interval("1"), &RemoteBinary::default());
        assert_eq!(
            arguments,
            [
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=2",
                "--",
                "m1-ts",
                "~/'.cargo/bin/glance' --stream --interval 1",
            ]
        );
    }

    #[test]
    fn the_delay_doubles_up_to_the_maximum() {
        let mut backoff = Backoff::new();
        let delays: Vec<u64> = (0..6).map(|_| backoff.after_end().as_secs()).collect();
        assert_eq!(delays, [5, 10, 20, 40, 60, 60]);
    }

    #[test]
    fn a_sample_sets_the_delay_back() {
        let mut backoff = Backoff::default();
        backoff.after_end();
        backoff.after_end();
        backoff.after_sample();
        assert_eq!(backoff, Backoff::new());
        assert_eq!(backoff.after_end(), FIRST_RETRY_DELAY);
    }
}

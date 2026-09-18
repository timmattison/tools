//! End-to-end tests of the command line of popstop.
//!
//! Each test starts the real binary with a state directory of its own, so the
//! tests never touch the lock of the user and never see each other. Each copy
//! also gets `--exit-after`, and a drop guard kills a copy that still runs, so
//! a test that fails leaves no copy that plays for ever.
//!
//! These tests open the real default output device and play the inaudible
//! keepalive signal.

// popstop plays audio on macOS only.
#![cfg(target_os = "macos")]

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use popstop::lock::{current_holder, HolderRecord, Mode, StateDir};
use popstop::message::start_time_text;
use popstop::process::start_time;

/// The time after which a copy stops by itself, as `--exit-after` takes it.
/// A test ends long before, and this value is the backstop.
const EXIT_AFTER_SECONDS: &str = "60";

/// The longest time that a test waits for a line of a copy.
///
/// A copy that plays writes its first line in about one second. The bound is
/// much larger, because a build machine that runs many tests together starts
/// a process slowly, and a bound that measures the machine makes a test that
/// fails for a reason that is not the code.
const LINE_TIMEOUT: Duration = Duration::from_secs(60);

/// The longest time that a test waits for a copy to end.
const EXIT_TIMEOUT: Duration = Duration::from_secs(60);

/// The time between two looks at a copy that ends.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The number of `SIGINT`. POSIX sets it to 2.
const SIGINT: libc::c_int = 2;

/// The number of `SIGHUP`. POSIX sets it to 1. A terminal that closes sends
/// it to the copy that it holds.
const SIGHUP: libc::c_int = 1;

/// The number of `SIGTERM`. POSIX sets it to 15. `popstop --stop` sends it
/// to the copy that runs.
const SIGTERM: libc::c_int = 15;

/// The line that tells the user about the sleep of the Mac.
const NO_IDLE_SLEEP_LINE: &str = "popstop: this Mac does not idle sleep while popstop runs";

/// The line that tells the user how to stop a foreground copy.
const PRESS_CTRL_C_LINE: &str = "popstop: press Ctrl-C to stop";

/// A copy of popstop that a test started.
///
/// A drop kills the copy and reaps it, so a test that fails early leaves no
/// copy that plays.
struct Copy {
    /// The child process.
    child: Child,
    /// The lines that the copy wrote to stdout.
    stdout: Receiver<String>,
    /// The thread that reads the stderr of the copy, until [`Copy::finish`]
    /// takes its text.
    stderr: Option<thread::JoinHandle<String>>,
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Copy {
    /// Starts a copy of popstop with the state directory `dir`, and with the
    /// arguments `arguments` after it.
    fn start(dir: &Path, arguments: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_popstop"))
            .arg("--state-dir")
            .arg(dir)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start popstop");

        let stdout = child.stdout.take().expect("the stdout of the copy");
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });

        let mut pipe = child.stderr.take().expect("the stderr of the copy");
        let stderr = thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        });

        Self {
            child,
            stdout: receiver,
            stderr: Some(stderr),
        }
    }

    /// Starts a copy that runs until a signal ends it, or until
    /// [`EXIT_AFTER_SECONDS`] pass.
    fn start_in_the_foreground(dir: &Path) -> Self {
        Self::start(dir, &["--exit-after", EXIT_AFTER_SECONDS])
    }

    /// Gives the process ID of the copy.
    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Gives the next line that the copy wrote to stdout.
    ///
    /// It waits for [`LINE_TIMEOUT`] at most, so a copy that writes nothing
    /// fails the test instead of holding it. A copy that says nothing in that
    /// time ends here, and its stderr goes into the failure, because a copy
    /// that cannot start says why on stderr.
    fn next_line(&mut self) -> String {
        match self.stdout.recv_timeout(LINE_TIMEOUT) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => {
                let _ = self.child.kill();
                let (status, stderr) = self.finish();
                panic!(
                    "the copy wrote no line within {LINE_TIMEOUT:?} ({status}). Its stderr:\n\
                     {stderr}"
                )
            }
            Err(RecvTimeoutError::Disconnected) => {
                let (status, stderr) = self.finish();
                panic!("the copy ended before it wrote the line ({status}). Its stderr:\n{stderr}")
            }
        }
    }

    /// Sends the signal `signal` to the copy.
    fn send(&self, signal: libc::c_int) {
        let pid = libc::pid_t::try_from(self.pid()).expect("the PID fits in a pid_t");
        // SAFETY: `kill` takes two numbers by value. The PID is the PID of a
        // child that this test started and that nothing reaped yet, so it
        // names that child and no other process.
        let sent = unsafe { libc::kill(pid, signal) };
        assert_eq!(
            sent,
            0,
            "the signal {signal} did not reach the copy: {}",
            std::io::Error::last_os_error()
        );
    }

    /// Tells whether the copy still runs.
    fn still_runs(&mut self) -> bool {
        self.child.try_wait().expect("look at the copy").is_none()
    }

    /// Waits for the copy to end, for [`EXIT_TIMEOUT`] at most. Gives its
    /// exit status and its stderr.
    fn finish(&mut self) -> (ExitStatus, String) {
        let deadline = Instant::now() + EXIT_TIMEOUT;
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("look at the copy") {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "the copy did not end within {EXIT_TIMEOUT:?}"
            );
            thread::sleep(POLL_INTERVAL);
        };
        let stderr = self
            .stderr
            .take()
            .expect("the copy is finished only once")
            .join()
            .expect("read the stderr of the copy");
        (status, stderr)
    }
}

/// Gives the record of the copy that holds the lock in `dir`.
fn holder(dir: &Path) -> Option<HolderRecord> {
    current_holder(&StateDir::new(dir.to_path_buf())).expect("read the lock file")
}

/// Reads the three lines of a foreground start, and gives the name of the
/// device that the first line names.
fn device_of_the_ready_lines(copy: &mut Copy) -> String {
    let first = copy.next_line();
    let opening = "popstop: \"";
    let pid = copy.pid();
    let closing = format!("\" stays awake while popstop runs (pid {pid})");
    let device = first
        .strip_prefix(opening)
        .and_then(|rest| rest.strip_suffix(&closing))
        .unwrap_or_else(|| {
            panic!("the first line is {first:?}, and not {opening}<device>{closing}")
        })
        .to_owned();
    assert!(!device.trim().is_empty(), "the first line names no device");
    assert_eq!(copy.next_line(), NO_IDLE_SLEEP_LINE);
    assert_eq!(copy.next_line(), PRESS_CTRL_C_LINE);
    device
}

/// Starts a foreground copy, waits until it plays, and sends `signal` to it.
/// The copy must stop with success and release the lock.
///
/// The default action of each of these signals ends a process where it
/// stands, thus a copy that ends with a signal status registered no handler.
fn a_signal_stops_a_foreground_copy(signal: libc::c_int) {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let dir = temp.path().join("state");
    let mut copy = Copy::start_in_the_foreground(&dir);
    device_of_the_ready_lines(&mut copy);

    copy.send(signal);
    let (status, stderr) = copy.finish();

    assert_eq!(
        status.code(),
        Some(0),
        "the signal {signal} ends a copy with success: {status}. Its stderr:\n{stderr}"
    );
    assert_eq!(stderr, "", "a copy that stops says nothing on stderr");
    assert_eq!(holder(&dir), None, "the copy released the lock");
}

#[test]
fn a_hangup_stops_a_foreground_copy() {
    a_signal_stops_a_foreground_copy(SIGHUP);
}

#[test]
fn a_termination_signal_stops_a_foreground_copy() {
    a_signal_stops_a_foreground_copy(SIGTERM);
}

#[test]
fn a_copy_with_a_time_limit_stops_by_itself() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let dir = temp.path().join("state");
    let mut copy = Copy::start(&dir, &["--exit-after", "1"]);

    device_of_the_ready_lines(&mut copy);
    let (status, stderr) = copy.finish();

    assert_eq!(
        status.code(),
        Some(0),
        "a copy that reaches its time limit stops with success: {status}. Its stderr:\n{stderr}"
    );
    assert_eq!(stderr, "", "a copy that stops says nothing on stderr");
    assert_eq!(holder(&dir), None, "the copy released the lock");
}

#[test]
fn a_second_start_refuses_and_names_the_copy_that_runs() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let dir = temp.path().join("state");
    let mut first = Copy::start_in_the_foreground(&dir);
    device_of_the_ready_lines(&mut first);
    let record = holder(&dir).expect("the first copy holds the lock");

    let mut second = Copy::start_in_the_foreground(&dir);
    let (status, refusal) = second.finish();

    assert_eq!(
        status.code(),
        Some(3),
        "a second start ends with the status of a copy that runs: {status}. Its stderr:\n{refusal}"
    );
    for named in [
        format!("pid {}", first.pid()),
        "foreground".to_owned(),
        start_time_text(record.started_at),
        format!("popstop --stop --state-dir '{}'", dir.display()),
    ] {
        assert!(
            refusal.contains(&named),
            "the refusal does not name {named:?}:\n{refusal}"
        );
    }

    assert!(
        first.still_runs(),
        "the refusal of the second start ended the first copy"
    );
    assert_eq!(
        holder(&dir),
        Some(record),
        "the first copy still holds the lock"
    );

    first.send(SIGINT);
    let (status, stderr) = first.finish();
    assert_eq!(
        status.code(),
        Some(0),
        "the first copy stops with success: {status}. Its stderr:\n{stderr}"
    );
    assert_eq!(holder(&dir), None, "the first copy released the lock");
}

#[test]
fn a_foreground_copy_names_its_device_holds_the_lock_and_stops_on_sigint() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let dir = temp.path().join("state");
    let mut copy = Copy::start_in_the_foreground(&dir);

    device_of_the_ready_lines(&mut copy);

    assert_eq!(
        holder(&dir),
        Some(HolderRecord {
            pid: copy.pid(),
            mode: Mode::Foreground,
            started_at: start_time(copy.pid()).expect("the start time of the copy"),
        }),
        "the copy that plays holds the lock, and its record names it"
    );

    copy.send(SIGINT);
    let (status, stderr) = copy.finish();

    assert_eq!(
        status.code(),
        Some(0),
        "Ctrl-C ends a copy with success: {status}. Its stderr:\n{stderr}"
    );
    assert_eq!(stderr, "", "a copy that stops says nothing on stderr");
    assert_eq!(holder(&dir), None, "the copy released the lock");
}

//! A holder of the lock that gets `SIGKILL` leaves no stale lock.
//!
//! The kernel releases an advisory lock when the process that holds it ends,
//! for any cause. `SIGKILL` gives the process no chance to clean up, so its
//! record stays in the lock file. The next copy must still get the lock.
//!
//! The holder is a child process: this test binary itself, started again with
//! the name of an ignored helper test and an environment variable that carries
//! the state directory.

// `SIGKILL` and the signal that ended a process are Unix ideas.
#![cfg(unix)]

use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use popstop::lock::{acquire, current_holder, HolderRecord, Mode, StartTime, StateDir};

/// The environment variable that carries the state directory to the helper.
/// The helper does nothing when the variable is absent.
const HELPER_DIR_VARIABLE: &str = "POPSTOP_LOCK_KILL_HELPER_DIR";

/// The name of the helper test.
const HELPER_TEST_NAME: &str = "lock_kill_helper_holds_the_lock_until_killed";

/// The text that the helper writes to stdout after it holds the lock.
const LOCKED_MARKER: &str = "POPSTOP-HELPER-LOCKED";

/// The longest time the helper lives. `SIGKILL` ends it long before, and the
/// bound makes sure that a helper nobody kills does not live on.
const HELPER_LIFETIME: Duration = Duration::from_secs(60);

/// The longest time the test waits for the helper to hold the lock.
const LOCKED_TIMEOUT: Duration = Duration::from_secs(30);

/// The number of `SIGKILL`. POSIX sets it to 9.
const SIGKILL: i32 = 9;

/// Gives the current time as a start time.
fn now() -> StartTime {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970");
    StartTime::from_unix_micros(
        u64::try_from(since_epoch.as_micros()).expect("the time fits in 64 bits"),
    )
}

/// The helper process. A drop kills and reaps it, so a test that fails early
/// leaves no process.
struct Helper(Child);

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Reads the stdout of the helper until the marker, for `timeout` at most.
///
/// A thread reads the lines and sends them on a channel, so the wait has a
/// bound even when the helper writes nothing.
fn wait_for_marker(stdout: ChildStdout, timeout: Duration) {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + timeout;
    let mut output = Vec::new();
    loop {
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(line) if line.contains(LOCKED_MARKER) => return,
            Ok(line) => output.push(line),
            Err(RecvTimeoutError::Timeout) => {
                panic!(
                    "the helper did not hold the lock within {timeout:?}. Its output: {output:?}"
                )
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("the helper ended before it held the lock. Its output: {output:?}")
            }
        }
    }
}

#[test]
fn a_holder_that_gets_sigkill_leaves_no_stale_lock() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let dir = StateDir::new(temp.path().join("state"));

    let mut helper = Helper(
        Command::new(env::current_exe().expect("the path of this test binary"))
            .args(["--exact", HELPER_TEST_NAME, "--ignored", "--nocapture"])
            .env(HELPER_DIR_VARIABLE, dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start the helper"),
    );
    let stdout = helper.0.stdout.take().expect("the stdout of the helper");
    wait_for_marker(stdout, LOCKED_TIMEOUT);

    let holder = current_holder(&dir)
        .expect("read the holder")
        .expect("the helper holds the lock");
    assert_eq!(holder.pid, helper.0.id(), "the record names the helper");

    helper.0.kill().expect("send SIGKILL to the helper");
    let status = helper.0.wait().expect("reap the helper");
    assert_eq!(
        status.signal(),
        Some(SIGKILL),
        "SIGKILL ended the helper: {status}"
    );
    assert!(
        !fs::read_to_string(dir.lock_path())
            .expect("read the lock file")
            .is_empty(),
        "the killed helper did not clean up, so its record stays in the lock file"
    );

    assert_eq!(
        current_holder(&dir).expect("read the holder"),
        None,
        "the kernel released the lock of the killed helper"
    );
    let record = HolderRecord {
        pid: std::process::id(),
        mode: Mode::Foreground,
        started_at: now(),
    };
    let guard = acquire(&dir, &record).expect("the next acquire gets the lock");
    assert_eq!(current_holder(&dir).expect("read the holder"), Some(record));
    drop(guard);
}

/// The holder that the kill test starts in a child process.
///
/// It gets the lock in the state directory that [`HELPER_DIR_VARIABLE`]
/// names, writes [`LOCKED_MARKER`], and sleeps until `SIGKILL` ends it.
#[test]
#[ignore = "the kill test starts this helper in a child process"]
fn lock_kill_helper_holds_the_lock_until_killed() {
    let Some(dir) = env::var_os(HELPER_DIR_VARIABLE) else {
        return;
    };
    let dir = StateDir::new(PathBuf::from(dir));
    let record = HolderRecord {
        pid: std::process::id(),
        mode: Mode::Background,
        started_at: now(),
    };

    let _guard = acquire(&dir, &record).expect("the helper gets the lock");
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{LOCKED_MARKER}").expect("write the marker");
    stdout.flush().expect("flush the marker");
    drop(stdout);

    thread::sleep(HELPER_LIFETIME);
}

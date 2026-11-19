//! A helper that holds a lock in a different process, for tests.
//!
//! [`crate::instance::holder`] gives `None` in the process that holds the
//! lock, because a POSIX record lock never conflicts with its own process. A
//! test that needs a lock with a holder thus holds the lock in a child
//! process. This module starts that child from the test binary itself:
//!
//! 1. The crate adds an ignored test that calls [`serve_hold_request`]:
//!
//!    ```ignore
//!    #[test]
//!    #[ignore = "a helper that the lock tests start in a child process"]
//!    fn hold_lock_helper() {
//!        soleinstance::testing::serve_hold_request();
//!    }
//!    ```
//!
//! 2. A test calls [`hold_in_child`] with the lock and the full libtest path
//!    of that ignored test. For a unit test, the path is
//!    `module::tests::hold_lock_helper`. For a file under `tests/`, the path is
//!    `hold_lock_helper`.
//! 3. The child claims the lock, writes `SOLEINSTANCE HELD <pid>` to its
//!    stdout, and waits until its stdin ends. The [`HeldLock`] closes the
//!    stdin when it drops, and the child then ends and frees the lock.
//!
//! A child that its parent loses ends after [`HELPER_LIFE`] by itself.

use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::thread;
use std::time::Duration;

use crate::instance::{claim, LockFile, Pid};

/// The variable that holds the path of the lock file that the child claims.
pub const HOLD_LOCK_VARIABLE: &str = "SOLEINSTANCE_HOLD_LOCK";

/// The start of the line that the child writes when it holds the lock. The PID
/// of the child follows.
const HELD_PREFIX: &str = "SOLEINSTANCE HELD ";

/// The longest life of the child.
pub const HELPER_LIFE: Duration = Duration::from_secs(30);

/// Serves a hold request. Call it from the ignored helper test of the crate.
///
/// When [`HOLD_LOCK_VARIABLE`] is not set, the function returns at once. When
/// it holds a path, the function claims that lock file, writes the line that
/// names the PID of this process, and waits until stdin ends. The claim drops
/// when the function returns.
///
/// # Panics
///
/// Panics when the lock cannot be claimed.
pub fn serve_hold_request() {
    let Some(path) = env::var_os(HOLD_LOCK_VARIABLE) else {
        return;
    };
    let lock = LockFile::at_path(path);
    thread::spawn(|| {
        thread::sleep(HELPER_LIFE);
        std::process::exit(1);
    });
    let _claim = claim(&lock).unwrap_or_else(|error| panic!("the helper cannot claim: {error:?}"));
    println!("{HELD_PREFIX}{}", Pid::of_this_process());
    let _ = std::io::stdout().flush();
    let mut rest = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut rest);
}

/// A lock that a child process holds. A drop ends the child.
#[derive(Debug)]
pub struct HeldLock {
    pid: Pid,
    _helper: Helper,
}

impl HeldLock {
    /// The PID of the child that holds the lock.
    #[must_use]
    pub fn pid(&self) -> Pid {
        self.pid
    }
}

/// The child process, from its start. A drop closes its stdin and waits for
/// it, also when the start of the hold fails.
#[derive(Debug)]
struct Helper {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Open until the child ends, so its last lines find a reader.
    stdout: BufReader<ChildStdout>,
}

impl Drop for Helper {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

/// Starts a child process that holds `lock`, and waits until it holds it.
///
/// `helper_test` is the full libtest path of the ignored test that calls
/// [`serve_hold_request`].
///
/// # Panics
///
/// Panics when the child does not start, or when its stdout ends before the
/// line that tells that it holds the lock.
#[must_use]
pub fn hold_in_child(lock: &LockFile, helper_test: &str) -> HeldLock {
    let executable = env::current_exe().expect("the path of the test binary");
    let mut child = childenv::fixture_command(executable)
        .args([
            helper_test,
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HOLD_LOCK_VARIABLE, PathBuf::from(lock.path()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("the helper test starts");
    let stdout = child.stdout.take().expect("the stdout of the helper");
    let stdin = child.stdin.take();
    let mut helper = Helper {
        child,
        stdin,
        stdout: BufReader::new(stdout),
    };
    let mut line = String::new();
    loop {
        line.clear();
        if helper
            .stdout
            .read_line(&mut line)
            .expect("a line of the helper")
            == 0
        {
            break;
        }
        // libtest writes `test <name> ... ` before the output of the test on
        // the same line.
        if let Some((_, raw)) = line.split_once(HELD_PREFIX) {
            let raw = raw.trim().parse().expect("the PID of the helper");
            let pid = Pid::new(raw).expect("a PID above zero");
            return HeldLock {
                pid,
                _helper: helper,
            };
        }
    }
    panic!("the helper ended before it held the lock ({helper_test})");
}

//! The two jobs that can block: a focus and a notification. The terminal
//! binary and `cctop-bar` run the same code. Each job runs on its own thread
//! and sends its result to an [`EventSink`].

use std::io;
use std::sync::Arc;

use soleinstance::instance::{LockFile, Pid as LockHolder};

use crate::controller::AppEvent;
use crate::notification;
use crate::runner::Runner;

/// Where a job sends its event. The terminal binary sends into its channel.
/// `cctop-bar` posts to its main thread.
pub type EventSink = Arc<dyn Fn(AppEvent) + Send + Sync>;

/// The directory of the lock of `cctop-bar`, below the data directory of the
/// user.
pub const BAR_LOCK_DIRECTORY: &str = "cctop-bar";

/// The file name of the lock of `cctop-bar`.
pub const BAR_LOCK_FILE: &str = "cctop-bar.lock";

/// The lock of `cctop-bar` for this user. On macOS the path is
/// `~/Library/Application Support/cctop-bar/cctop-bar.lock`. Both binaries
/// take the path from here.
///
/// # Errors
///
/// Returns the error of the lookup of the data directory of the user.
pub fn bar_lock_for_user() -> io::Result<LockFile> {
    LockFile::for_user(BAR_LOCK_DIRECTORY, BAR_LOCK_FILE)
}

/// When a notification goes out.
#[derive(Debug, Clone)]
pub enum NotifyGate {
    /// Send each notification. `cctop-bar` uses it: it holds the lock itself,
    /// and a `holder` call in the holding process would close a descriptor of
    /// the lock file and release its own lock.
    Always,
    /// Send no notification while a process holds this lock.
    UnlessHeld(LockFile),
}

/// What [`notify_now`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyOutcome {
    /// The notification went out.
    Sent,
    /// A process holds the lock of the gate. The value is its pid.
    Skipped(LockHolder),
    /// The text of the error of `notification::send`.
    Failed(String),
}

/// Ask the gate, then send. The synchronous core of the notify job.
///
/// A failed look at the lock sends the notification: a second notification
/// is better than none.
pub fn notify_now(runner: &dyn Runner, gate: &NotifyGate, message: &str) -> NotifyOutcome {
    if let NotifyGate::UnlessHeld(lock) = gate {
        if let Ok(Some(holder)) = soleinstance::instance::holder(lock) {
            return NotifyOutcome::Skipped(holder);
        }
    }
    match notification::send(runner, message) {
        Ok(()) => NotifyOutcome::Sent,
        Err(text) => NotifyOutcome::Failed(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notification::{notification_argv, NOTIFICATION_TITLE};
    use crate::runner::fake::FakeRunner;
    use crate::runner::CommandOutput;

    fn expected_argv(message: &str) -> Vec<String> {
        notification_argv(NOTIFICATION_TITLE, message)
    }

    fn runner_that(output: CommandOutput, message: &str) -> FakeRunner {
        let words = expected_argv(message);
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        FakeRunner::new().answer(&words, Ok(output))
    }

    #[test]
    fn always_sends_the_notification_once() {
        let runner = runner_that(CommandOutput::ok(""), "tools needs you");
        let outcome = notify_now(&runner, &NotifyGate::Always, "tools needs you");
        assert_eq!(outcome, NotifyOutcome::Sent);
        assert_eq!(runner.calls(), vec![expected_argv("tools needs you")]);
    }

    #[test]
    fn a_lock_file_that_does_not_exist_has_no_holder() {
        let dir = tempfile::tempdir().unwrap();
        let lock = LockFile::in_data_directory(dir.path(), BAR_LOCK_DIRECTORY, BAR_LOCK_FILE);
        let runner = runner_that(CommandOutput::ok(""), "m");
        let outcome = notify_now(&runner, &NotifyGate::UnlessHeld(lock), "m");
        assert_eq!(outcome, NotifyOutcome::Sent);
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn a_failed_send_gives_the_text_of_the_error() {
        let runner = runner_that(CommandOutput::failed(1, " no permission \n"), "m");
        let outcome = notify_now(&runner, &NotifyGate::Always, "m");
        assert_eq!(
            outcome,
            NotifyOutcome::Failed("osascript failed: no permission".to_string())
        );
    }

    #[test]
    fn the_lock_of_cctop_bar_ends_with_its_directory_and_file() {
        if let Ok(lock) = bar_lock_for_user() {
            assert!(
                lock.path().ends_with("cctop-bar/cctop-bar.lock"),
                "{}",
                lock.path().display()
            );
        }
    }

    #[test]
    #[ignore = "a helper that the lock tests start in a child process"]
    fn hold_lock_helper() {
        soleinstance::testing::serve_hold_request();
    }

    #[test]
    fn a_lock_that_cctop_bar_holds_stops_the_notification() {
        let dir = tempfile::tempdir().unwrap();
        let lock = LockFile::in_data_directory(dir.path(), BAR_LOCK_DIRECTORY, BAR_LOCK_FILE);
        let held = soleinstance::testing::hold_in_child(&lock, "jobs::tests::hold_lock_helper");
        let runner = FakeRunner::new();
        let outcome = notify_now(&runner, &NotifyGate::UnlessHeld(lock), "m");
        assert_eq!(outcome, NotifyOutcome::Skipped(held.pid()));
        assert_eq!(runner.calls(), Vec::<Vec<String>>::new());
    }
}

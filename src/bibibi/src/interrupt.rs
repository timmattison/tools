//! Signals: what `bibibi` does when SIGINT, SIGTERM, or SIGHUP arrives.
//!
//! [`arm`] starts one thread that reads SIGINT, SIGTERM, and SIGHUP. On a signal, the
//! thread stores the signal number (only the first signal counts), then stops
//! the child that runs now: SIGTERM to its process group, and SIGKILL to the
//! same group after [`GRACE`] if the same child still runs. The main thread
//! checks [`received`] (through [`check`]) before each step and after each
//! child. It then returns up the stack, so the patch file and the scratch
//! worktree drop and clean up. Only after that does `main` print the message
//! and exit with `128 + n` ([`exit_status`]).
//!
//! # Why a thread
//!
//! A handler may call only functions that are async-signal-safe. The work here
//! takes a mutex and starts a thread, and the cleanup spawns git. A thread that
//! reads the signals turns them into ordinary code, so that work is legal. This
//! is the pattern of `swt` in `teardown.rs`. The difference: the cleanup here
//! runs by `Drop` on the main thread, not on the signal thread.
//!
//! # Why the children have their own session
//!
//! [`crate::child`] starts each child with `setsid`. The child leads a new
//! session and a new process group. A Ctrl-C from the terminal goes to the
//! foreground process group, and the child is in another session. So the
//! child never dies before the flag is set, and the main thread always sees
//! the flag before it reads the status of the child. A Ctrl-C therefore never
//! counts as a verdict (a test that a signal stops is `bad`).
//! The signal thread stops the child on purpose, group and all, so no
//! grandchild outlives the run.
//!
//! # Known limits
//!
//! - A Ctrl-C or a hangup of the terminal also reaches a git child of
//!   `gitscratch`, because that child is in the process group of `bibibi`. If
//!   that git child fails before the signal thread sets the flag, the error
//!   looks like a git error and the run exits 2 in place of 130 or 129. The cleanup still happens.
//! - A second Ctrl-C during the cleanup reaches the `git worktree remove` of the
//!   scratch drop for the same reason. It can stop that command and leave the
//!   scratch worktree behind.
//! - SIGKILL cannot be caught.

use std::fmt;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::Duration;

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

/// The time between SIGTERM and SIGKILL for the group of a child.
const GRACE: Duration = Duration::from_secs(2);

/// The conventional shell status for a death by signal is this plus the
/// signal number.
const SIGNAL_EXIT_BASE: i32 = 128;

/// The name of the thread that reads the signals.
const SIGNAL_THREAD_NAME: &str = "bibibi-signals";

/// The first signal that arrived, or 0.
static RECEIVED: AtomicI32 = AtomicI32::new(0);

/// The process group of the child that runs now.
static RUNNING: Mutex<Option<i32>> = Mutex::new(None);

/// The error that says a signal stopped the run. [`crate::run::run`] turns it
/// into an interrupted outcome.
#[derive(Debug)]
pub struct Interrupted;

impl fmt::Display for Interrupted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a signal stopped the run")
    }
}

impl std::error::Error for Interrupted {}

/// The exit status of a run that signal `signal` stopped: `128 + signal`.
pub fn exit_status(signal: i32) -> i32 {
    SIGNAL_EXIT_BASE + signal
}

/// The first signal that arrived, if one did.
pub fn received() -> Option<i32> {
    match RECEIVED.load(Ordering::SeqCst) {
        0 => None,
        signal => Some(signal),
    }
}

/// Return [`Interrupted`] if a signal arrived.
///
/// # Errors
///
/// Returns [`Interrupted`] when [`received`] has a signal.
pub fn check() -> Result<(), Interrupted> {
    match received() {
        Some(_) => Err(Interrupted),
        None => Ok(()),
    }
}

/// Start the signal thread, one time for each process.
///
/// This is best effort. If the signals cannot be registered or the thread
/// cannot start, the run goes on with the default action of the signals.
pub fn arm() {
    static ARMED: OnceLock<()> = OnceLock::new();
    ARMED.get_or_init(|| {
        let Ok(mut signals) = Signals::new([SIGHUP, SIGINT, SIGTERM]) else {
            return;
        };
        // If the spawn fails, the closure drops and the registration ends with
        // it, so no signal is left with nobody who reads it.
        drop(
            thread::Builder::new()
                .name(SIGNAL_THREAD_NAME.to_string())
                .spawn(move || {
                    for signal in &mut signals {
                        // Only the first signal counts.
                        let _ = RECEIVED.compare_exchange(
                            0,
                            signal,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        );
                        if let Some(group) = *running() {
                            stop_group(group);
                        }
                    }
                }),
        );
    });
}

/// Record the process group of the child that runs now.
///
/// If a signal arrived already, the child stops at once.
pub fn register_child(group: i32) {
    *running() = Some(group);
    if received().is_some() {
        stop_group(group);
    }
}

/// Forget the running child. Call it after the child was waited for.
pub fn clear_child() {
    *running() = None;
}

/// Lock the registry. A panic elsewhere does not change a whole-value store.
fn running() -> MutexGuard<'static, Option<i32>> {
    RUNNING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Send `signal` to every process of `group`.
fn kill_group(group: i32, signal: i32) {
    // SAFETY: `kill` takes two integers and touches no memory. A negative pid
    // names the process group. A group that is gone gives an error that has no
    // use here.
    unsafe {
        libc::kill(-group, signal);
    }
}

/// Send SIGTERM to `group`. After [`GRACE`], send SIGKILL if the same child
/// still runs.
fn stop_group(group: i32) {
    kill_group(group, libc::SIGTERM);
    let spawned = thread::Builder::new()
        .name(format!("{SIGNAL_THREAD_NAME}-kill"))
        .spawn(move || {
            thread::sleep(GRACE);
            if *running() == Some(group) {
                kill_group(group, libc::SIGKILL);
            }
        });
    if spawned.is_err() {
        kill_group(group, libc::SIGKILL);
    }
}

#[cfg(test)]
mod tests {
    use super::exit_status;

    #[test]
    fn a_signal_number_maps_to_128_plus_the_number() {
        assert_eq!(exit_status(libc::SIGHUP), 129);
        assert_eq!(exit_status(libc::SIGINT), 130);
        assert_eq!(exit_status(libc::SIGTERM), 143);
    }
}

//! Test-only control of the `colored` crate's global override: one lock, one
//! entrance.
//!
//! Some tests must see real ANSI bytes. The `colored` crate emits no escape
//! codes when it writes to something that is not a terminal, and a test run is
//! never a terminal, so those tests turn the codes on with
//! `colored::control::set_override(true)`. That override is process-global, and
//! `cargo test` runs the tests of one binary on many threads at the same time.
//! One test that turns the override on therefore changes what a different test
//! sees, and one test that turns it off again removes the escapes a different
//! test is in the middle of reading. The result is a failure that appears and
//! disappears between runs.
//!
//! The cure is a lock that every such test holds. A lock only serializes the
//! tests that share the same lock, so a second copy of this mutex in a second
//! module gives no protection at all: two mutexes let two tests hold one each
//! and both run. For that reason this module keeps the only mutex, and it keeps
//! it private. [`with_forced_ansi`] is the sole way to reach it, from every test
//! module in the crate.

use std::sync::{Mutex, PoisonError};

/// The one lock on the `colored` crate's global override.
///
/// It is private on purpose. A caller that could lock it directly could also
/// hold it across an unrelated body, or forget to put the override back, which
/// is the exact failure this module removes. [`with_forced_ansi`] holds it for
/// one body and gives it up at the end of that body.
static OVERRIDE: Mutex<()> = Mutex::new(());

/// Run `body` with the `colored` crate forced to emit ANSI escape codes, and
/// give the process-global override back to its earlier state at the end.
///
/// The lock is held for the whole call, so two tests that both want real ANSI
/// bytes run one after the other. The override goes back even when `body`
/// panics, so one failed test leaves the next test a clean process.
///
/// A poisoned mutex is recovered, not propagated. Poison here records that some
/// earlier test panicked while it held the lock. The data behind the lock is a
/// unit value with no invariant to break, so the panic tells this module
/// nothing, and a propagated poison turns one real failure into a cascade of
/// unrelated ones.
pub(crate) fn with_forced_ansi<T>(body: impl FnOnce() -> T) -> T {
    let _lock = OVERRIDE.lock().unwrap_or_else(PoisonError::into_inner);
    force(body)
}

/// The forcing half of [`with_forced_ansi`], without the lock.
///
/// This split exists for the tests of this module. [`OVERRIDE`] is a plain
/// `Mutex`, which is not reentrant, so a test that holds the lock to read the
/// override before and after the body cannot also call [`with_forced_ansi`] —
/// that call would wait for a lock the same thread already holds, and the test
/// would deadlock. Such a test calls `force` instead and holds the lock itself.
/// No other caller has a reason to use this function.
fn force<T>(body: impl FnOnce() -> T) -> T {
    colored::control::set_override(true);
    // Build the guard before the body runs. A call to
    // `colored::control::unset_override()` after `body()` runs only when `body`
    // returns; a panic in `body` unwinds past it and leaves the override on for
    // every later test. A guard restores the override on both paths, because
    // unwinding drops it.
    let _restore = Restore;
    body()
}

/// Puts the `colored` crate's global override back when it goes out of scope.
///
/// A test body panics as its normal way to fail, so the restore must survive a
/// panic. Only a `Drop` implementation does that.
struct Restore;

impl Drop for Restore {
    fn drop(&mut self) {
        colored::control::unset_override();
    }
}

#[cfg(test)]
mod tests {
    use super::{force, with_forced_ansi, OVERRIDE};

    use std::sync::PoisonError;

    use colored::control::SHOULD_COLORIZE;
    use colored::Colorize;

    /// The first byte of every ANSI escape sequence the `colored` crate writes.
    const ESCAPE: &str = "\u{1b}[";

    #[test]
    fn forcing_makes_colored_emit_escapes() {
        let painted = with_forced_ansi(|| "x".red().to_string());
        assert!(
            painted.contains(ESCAPE),
            "expected ANSI escapes in {painted:?}"
        );
    }

    #[test]
    fn the_override_goes_back_when_the_body_ends() {
        // Hold the lock for all three reads. No other test can change the
        // override between them, so the before and after values are comparable.
        // The test holds the lock itself and calls `force`, because the lock is
        // not reentrant.
        let _lock = OVERRIDE.lock().unwrap_or_else(PoisonError::into_inner);

        let ambient = SHOULD_COLORIZE.should_colorize();
        let inside = force(|| SHOULD_COLORIZE.should_colorize());

        assert!(inside, "the body must run with the override on");
        assert_eq!(
            SHOULD_COLORIZE.should_colorize(),
            ambient,
            "the override must go back to what it was before the body"
        );
    }
}

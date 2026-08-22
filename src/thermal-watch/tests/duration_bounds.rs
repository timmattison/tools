//! Tests that `--duration` refuses a value the tool cannot hold.
//!
//! Both bounds run before the tool reads the DVFS table and before it starts
//! `powermetrics`, so these tests need no root and no Apple Silicon. They run
//! the built binary and read what it prints.
//!
//! Parallel safety: each test starts its own short-lived process and reads its
//! own pipes. Nothing is keyed on a fixed path, port, or name.

use std::path::PathBuf;
use std::process::Command;

/// The largest duration the tool accepts, as the messages give it.
const LIMIT: &str = "86400";

/// One second more than the limit.
const PAST_LIMIT: &str = "86401";

/// A value far past every limit, which is the one that panicked.
const ABSURD: &str = "18446744073709551615";

/// The part of the refusal that belongs to a value past the limit.
const TOO_LARGE: &str = "--duration accepts no more than 86400 seconds";

/// The refusal of a duration of zero, which the tool gave before this bound.
const NOT_POSITIVE: &str = "--duration needs a positive number of seconds";

/// What one run of the tool did.
struct Run {
    /// True when the run ended with a failure status.
    failed: bool,
    /// Standard output and standard error together, in that order.
    text: String,
}

/// A directory that does not exist, named for this process.
///
/// The child gets this as its whole `PATH`, so its search for `powermetrics`
/// ends at once. Nothing makes the directory. The name carries the process id,
/// so a parallel run cannot make one of these by accident.
fn unreachable_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "thermal-watch-no-powermetrics-{}",
        std::process::id()
    ))
}

/// Run the tool with the duration given, and collect what it printed.
///
/// `powermetrics` is put out of reach, so a duration the bounds accept ends the
/// run in a moment instead of starting a watch of that length. The bounds run
/// long before that spawn, so a refusal is still visible.
fn run(duration: &str) -> Run {
    collect(
        Command::new(env!("CARGO_BIN_EXE_thermal-watch"))
            .arg("--duration")
            .arg(duration)
            .env("PATH", unreachable_path()),
    )
}

/// Run the tool with the duration given, with `powermetrics` reachable.
///
/// The panic this suite guards against happens after the spawn of
/// `powermetrics`, so the run that must show it keeps the path of the test. The
/// run still ends in a moment: the tool asks for one sample at a time and stops
/// its child when it ends.
fn run_with_powermetrics(duration: &str) -> Run {
    collect(
        Command::new(env!("CARGO_BIN_EXE_thermal-watch"))
            .arg("--duration")
            .arg(duration),
    )
}

/// Run a command to its end and read both of its outputs.
fn collect(command: &mut Command) -> Run {
    let output = command.output().expect("the tool must start");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Run {
        failed: !output.status.success(),
        text,
    }
}

#[test]
fn an_absurd_duration_is_refused_instead_of_panicking() {
    let run = run_with_powermetrics(ABSURD);

    assert!(
        run.failed,
        "a duration of {ABSURD} must end the run with a failure, and it printed:\n{}",
        run.text
    );
    assert!(
        !run.text.contains("panic"),
        "a duration of {ABSURD} must give an error, not a panic, and it printed:\n{}",
        run.text
    );
    assert!(
        run.text.contains(LIMIT),
        "the refusal must name the limit of {LIMIT} seconds, and it printed:\n{}",
        run.text
    );
}

#[test]
fn a_duration_of_zero_is_still_refused() {
    let run = run("0");

    assert!(
        run.failed,
        "a duration of zero must end the run with a failure, and it printed:\n{}",
        run.text
    );
    assert!(
        run.text.contains(NOT_POSITIVE),
        "a duration of zero must give the message about a positive number, and it printed:\n{}",
        run.text
    );
}

#[test]
fn one_second_past_the_limit_is_refused() {
    let run = run(PAST_LIMIT);

    assert!(
        run.failed,
        "a duration of {PAST_LIMIT} must end the run with a failure, and it printed:\n{}",
        run.text
    );
    assert!(
        run.text.contains(TOO_LARGE),
        "a duration of {PAST_LIMIT} must give the message about the limit, and it printed:\n{}",
        run.text
    );
}

#[test]
fn the_limit_itself_passes_the_bounds_check() {
    let run = run(LIMIT);

    // The run ends with an error on most machines, because `powermetrics` is
    // out of reach here and the DVFS table is absent on a machine that is not
    // an Apple Silicon Mac. Both are later steps. Only the two bounds are
    // under test, so only their messages are read.
    assert!(
        !run.text.contains(TOO_LARGE),
        "a duration of {LIMIT} must pass the upper bound, and it printed:\n{}",
        run.text
    );
    assert!(
        !run.text.contains(NOT_POSITIVE),
        "a duration of {LIMIT} must pass the lower bound, and it printed:\n{}",
        run.text
    );
}

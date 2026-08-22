//! Tests for the lifecycle of a `powermetrics` run.
//!
//! These drive the stream with a stand-in command rather than with
//! `powermetrics` itself, so they need no root and stay fast. The stand-in
//! prints the same shapes of output that `powermetrics` prints, including the
//! banner it writes before its first sample and the refusal it writes when it
//! is not run as the superuser.
//!
//! Parallel safety: each test spawns its own short-lived process and reads its
//! pipe. Nothing is keyed on a fixed path, port, or name.

use std::process::Command;

use thermal_watch::mhz::Mhz;
use thermal_watch::powermetrics::SampleStream;

/// One sample block, with the clock given.
fn block(mhz: u32) -> String {
    format!(
        "*** Sampled system activity (1000.00ms elapsed) ***\n\
         P-Cluster HW active frequency: {mhz} MHz\n\
         P-Cluster HW active residency:  99.00%\n\
         CPU Power: 30000 mW\n\
         Current pressure level: Nominal\n"
    )
}

/// A stand-in for `powermetrics` that prints `script` and exits with `code`.
fn stand_in(script: &str, code: i32) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(format!("printf '%s' \"$0\"; exit {code}"));
    command.arg(script);
    command
}

#[test]
fn reads_every_sample_a_run_prints() {
    let script = format!("{}{}{}", block(4_500), block(4_000), block(3_500));
    let stream = SampleStream::from_command(stand_in(&script, 0)).expect("a stream");
    let clocks: Vec<Option<Mhz>> = stream.map(|sample| sample.p_freq).collect();
    assert_eq!(
        clocks,
        vec![
            Some(Mhz::new(4_500)),
            Some(Mhz::new(4_000)),
            Some(Mhz::new(3_500)),
        ],
        "the last block must be given too, not dropped at the end of the run"
    );
}

#[test]
fn text_before_the_first_sample_header_is_not_a_sample() {
    // `powermetrics` prints a banner before its first sample.
    let script = format!(
        "Machine model: Mac16,11\nSMC version: Unknown\nOS version: 25F1234\n\n{}",
        block(4_500)
    );
    let stream = SampleStream::from_command(stand_in(&script, 0)).expect("a stream");
    let samples: Vec<_> = stream.collect();
    assert_eq!(
        samples.len(),
        1,
        "the banner is not a measurement, so it must never become a sample"
    );
    assert_eq!(samples[0].p_freq, Some(Mhz::new(4_500)));
}

#[test]
fn a_run_that_prints_no_sample_gives_no_sample() {
    let stream = SampleStream::from_command(stand_in("", 0)).expect("a stream");
    assert_eq!(stream.count(), 0);
}

#[test]
fn reports_the_status_of_a_run_that_failed() {
    // This is what `powermetrics` does without root: it writes a refusal and
    // exits non-zero without printing one sample.
    let mut stream = SampleStream::from_command(stand_in("", 1)).expect("a stream");
    assert_eq!(stream.by_ref().count(), 0);

    let status = stream
        .exit_status()
        .expect("the run must report its status");
    assert!(
        !status.success(),
        "a run that failed must be told apart from a run that measured nothing"
    );
}

#[test]
fn reports_the_status_of_a_run_that_finished() {
    let script = block(4_500);
    let mut stream = SampleStream::from_command(stand_in(&script, 0)).expect("a stream");
    assert_eq!(stream.by_ref().count(), 1);

    let status = stream
        .exit_status()
        .expect("the run must report its status");
    assert!(status.success());
}

#[test]
fn the_status_is_absent_until_the_run_ends() {
    // Nothing reads the pipe while the stream waits, so a wait before the
    // output closes stops forever once the pipe is full. The status stays
    // absent until the run ends.
    let script = block(4_500);
    let mut stream = SampleStream::from_command(stand_in(&script, 7)).expect("a stream");

    assert_eq!(
        stream.exit_status(),
        None,
        "a stream with output still to read must report no status"
    );

    assert_eq!(stream.by_ref().count(), 1);
    assert_eq!(
        stream.exit_status().expect("a status").code(),
        Some(7),
        "the status must be available once the output closed"
    );
}

#[test]
fn the_status_is_the_same_however_often_it_is_asked_for() {
    let mut stream = SampleStream::from_command(stand_in("", 3)).expect("a stream");
    assert_eq!(stream.by_ref().count(), 0);

    let first = stream.exit_status().expect("a status").code();
    let second = stream.exit_status().expect("a status").code();
    assert_eq!(first, Some(3));
    assert_eq!(
        second,
        Some(3),
        "the status must be kept, not waited for twice"
    );
}

//! Tests that the load generator obeys its deadline.
//!
//! A load generator whose only stop is a call after the loop is a load
//! generator that never stops when the caller dies. These tests hold that
//! guarantee: the workers end on their own deadline, with nothing calling
//! `stop`.
//!
//! Parallel safety: every test here uses only threads of its own process and a
//! deadline it computes itself. Nothing is keyed on a shared name.

use std::thread::sleep;
use std::time::{Duration, Instant};

use thermal_watch::load::{performance_core_count, Load};

#[test]
fn starts_one_worker_for_each_thread_asked_for() {
    let load = Load::start(3, Instant::now() + Duration::from_millis(200));
    assert_eq!(load.worker_count(), 3);
    load.stop();
}

#[test]
fn every_worker_ends_at_its_deadline_with_nothing_calling_stop() {
    let deadline = Instant::now() + Duration::from_millis(300);
    let load = Load::start(2, deadline);

    // Deliberately never call `stop`. The deadline alone must end the work.
    sleep(Duration::from_millis(1_500));

    let before = Instant::now();
    load.stop();
    assert!(
        before.elapsed() < Duration::from_millis(500),
        "the workers had already ended, so joining them must be immediate"
    );
    assert!(Instant::now() > deadline);
}

#[test]
fn stop_ends_the_workers_well_before_a_distant_deadline() {
    let load = Load::start(2, Instant::now() + Duration::from_secs(600));

    let before = Instant::now();
    load.stop();
    assert!(
        before.elapsed() < Duration::from_secs(5),
        "stop must not wait for a deadline ten minutes away"
    );
}

#[test]
fn a_deadline_already_past_starts_no_lasting_work() {
    let load = Load::start(2, Instant::now() - Duration::from_secs(1));
    let before = Instant::now();
    load.stop();
    assert!(before.elapsed() < Duration::from_secs(2));
}

#[test]
fn asking_for_no_workers_starts_none() {
    let load = Load::start(0, Instant::now() + Duration::from_secs(600));
    assert_eq!(load.worker_count(), 0);
    load.stop();
}

#[test]
fn the_machine_reports_at_least_one_performance_core() {
    let count = performance_core_count();
    assert!(
        count >= 1,
        "every machine has at least one core, got {count}"
    );
    assert!(count < 1_024, "a count of {count} cores is not credible");
}

#[test]
fn the_workers_actually_consume_processor_time() {
    let load = Load::start(2, Instant::now() + Duration::from_secs(3));
    let before = processor_time();
    sleep(Duration::from_millis(700));
    let used = processor_time() - before;
    load.stop();

    assert!(
        used > Duration::from_millis(400),
        "two workers over 700ms must burn more than 400ms of processor time, got {used:?}"
    );
}

/// Processor time this process has used, both in user code and in the kernel.
fn processor_time() -> Duration {
    // SAFETY: `getrusage` writes one `rusage` into the pointer it is given, and
    // the pointer is to a live local. The zeroed value is a valid `rusage`.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, &mut usage), 0);
        usage
    };
    let seconds = |t: libc::timeval| {
        Duration::from_secs(t.tv_sec.unsigned_abs())
            + Duration::from_micros(t.tv_usec.unsigned_abs().into())
    };
    seconds(usage.ru_utime) + seconds(usage.ru_stime)
}

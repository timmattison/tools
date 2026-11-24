//! The one test that reads the real Mac.
//!
//! It runs `observe` over the Mac that runs the tests, with the real `ps`, the
//! real kernel calls, and a real sleep of one second. It states only the
//! shapes that hold on any Mac: the numbers differ on each run. Each rule that
//! `observe` feeds has tests over plain values in `tests/observe.rs`. This
//! test proves that the reader of the Mac answers each question of the trait
//! with a value that those rules can use.

#![cfg(target_os = "macos")]

use std::time::Duration;

use faulte::pid::{Pid, Uid};
use whyslow::groups::Group;
use whyslow::machine::macos::Mac;
use whyslow::machine::{observe, Machine};

/// The window that this test asks for.
const INTERVAL: &str = "1s";

/// The reader of the real Mac gives facts with the shapes that any Mac has: a
/// processor, CPU ticks that moved, a window of the interval, and a group for
/// `launchd` and for the program of this test.
#[test]
fn the_real_mac_gives_facts_of_the_expected_shape() {
    let facts = observe(&Mac::new(), INTERVAL.parse().expect("1s is a span"))
        .expect("this Mac answers every read");
    assert!(facts.processors >= 1, "processors {}", facts.processors);
    assert!(facts.ticks.total() > 0, "ticks {:?}", facts.ticks);
    assert!(
        facts.window >= Duration::from_secs(1),
        "window {:?}",
        facts.window
    );
    assert!(!facts.groups.is_empty());
    let launchd = facts
        .groups
        .iter()
        .find(|total| total.group == Group::Named("launchd".to_owned()))
        .expect("launchd, the process 1 of the root account, is counted");
    assert!(launchd.processes >= 1);
    let program = std::env::current_exe()
        .expect("the test knows its own program")
        .file_name()
        .expect("the program has a name")
        .to_string_lossy()
        .into_owned();
    assert!(
        facts
            .groups
            .iter()
            .any(|total| total.group == Group::Named(program.clone())),
        "no group for the program {program}"
    );
}

/// The kernel tells a process that exited apart from a process that it
/// refuses. A child that the test reaped holds no memory, so its footprint is
/// zero. The test process itself has a footprint above zero. Process 1 belongs
/// to the root account, so the kernel refuses it and the footprint is `None`,
/// unless the test runs as root.
#[test]
fn the_footprint_of_an_exited_process_is_zero_and_a_refusal_is_none() {
    let mac = Mac::new();
    let mut child = childenv::fixture_command("/usr/bin/true")
        .spawn()
        .expect("/usr/bin/true starts");
    let child_pid = child.id();
    child.wait().expect("the child ends");
    assert_eq!(mac.footprint(Pid::new(child_pid)), Some(0));

    let own = mac
        .footprint(Pid::new(std::process::id()))
        .expect("the kernel gives the footprint of this process");
    assert!(own > 0, "footprint {own}");

    let own_row = mac
        .process_table()
        .expect("this Mac gives its process table")
        .into_iter()
        .find(|row| row.pid == Pid::new(std::process::id()))
        .expect("the table holds this process");
    if own_row.uid != Uid::new(0) {
        assert_eq!(mac.footprint(Pid::new(1)), None);
    }
}

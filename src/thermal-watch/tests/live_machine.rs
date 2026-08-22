//! Tests that read the machine this suite runs on.
//!
//! These read the IO Registry, which needs no special privilege. Nothing here
//! runs `powermetrics`, so nothing here needs root.
//!
//! Parallel safety: the IO Registry is read-only here. Nothing is written.

use thermal_watch::dvfs::DvfsTable;

#[test]
fn reads_a_credible_dvfs_table_from_this_machine() {
    let read = DvfsTable::read();

    // An Intel Mac and every non-Apple platform carry no such property, so
    // reporting the absence is the correct result there. On Apple Silicon it is
    // a defect, and the test says so rather than passing quietly.
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        if let Err(error) = read {
            println!("no DVFS table on this platform, as expected: {error}");
            return;
        }
    }

    let table = read.expect("an Apple Silicon Mac must report a DVFS table");

    let p_max = table.p_max();
    assert!(
        p_max.megahertz() > 1_000,
        "a P-core maximum of {p_max} is too low to be real"
    );
    assert!(
        p_max.megahertz() < 10_000,
        "a P-core maximum of {p_max} is too high to be real"
    );
    assert!(
        table.p_steps().len() > 1,
        "a DVFS table with one step would mean no frequency scaling at all"
    );
    assert_eq!(
        p_max,
        table.p_steps().iter().copied().max().expect("a step"),
        "the reported maximum must be the largest step"
    );

    let e_max = table.e_max();
    assert!(e_max.megahertz() > 0, "the E-cluster table must not be empty");
    assert!(
        e_max < p_max,
        "the efficiency cores ({e_max}) must be slower than the performance cores ({p_max})"
    );

    println!("this machine: P max {p_max}, E max {e_max}");
}

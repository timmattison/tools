//! Tests that read the machine this suite runs on.
//!
//! These read the IO Registry, which needs no special privilege. Nothing here
//! runs `powermetrics`, so nothing here needs root.
//!
//! Parallel safety: the IO Registry is read-only here. Nothing is written.

use std::process::Command;

use thermal_watch::dvfs::DvfsTable;

/// The sysctl that reads 1 inside a virtual machine on macOS.
const VIRTUAL_MACHINE_SYSCTL: &str = "kern.hv_vmm_present";

/// Whether this Mac is a virtual machine.
///
/// A virtual Mac shows its guest no DVFS table, because the host controls the
/// clock. A GitHub Actions runner on macOS is such a machine.
fn is_a_virtual_machine() -> bool {
    Command::new("/usr/sbin/sysctl")
        .args(["-n", VIRTUAL_MACHINE_SYSCTL])
        .output()
        .is_ok_and(|output| {
            output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "1"
        })
}

#[test]
fn reads_a_credible_dvfs_table_from_this_machine() {
    let read = DvfsTable::read();

    // An Intel Mac, a virtual Mac, and every non-Apple platform carry no such
    // property, so reporting the absence is the correct result there. On a
    // real Apple Silicon Mac it is a defect, and the test says so rather than
    // passing quietly.
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) || is_a_virtual_machine() {
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
    assert!(
        e_max.megahertz() > 0,
        "the E-cluster table must not be empty"
    );
    assert!(
        e_max < p_max,
        "the efficiency cores ({e_max}) must be slower than the performance cores ({p_max})"
    );

    println!("this machine: P max {p_max}, E max {e_max}");
}

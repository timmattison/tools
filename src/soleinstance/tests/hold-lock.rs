//! The test of the helper that holds a lock in a different process.

#![deny(unsafe_code)]
#![cfg(unix)]

use soleinstance::instance::{holder, LockFile};
use soleinstance::testing::{hold_in_child, serve_hold_request};

/// The name of the ignored helper test below, as libtest names it.
const HELPER_TEST: &str = "hold_lock_helper";

/// The child process runs this test. Run alone, it returns at once.
#[test]
#[ignore = "a helper that the lock tests start in a child process"]
fn hold_lock_helper() {
    serve_hold_request();
}

#[test]
fn a_lock_that_another_process_holds_names_that_process() {
    let data = tempfile::Builder::new()
        .prefix("soleinstance-hold-")
        .tempdir()
        .expect("make a data directory");
    let lock = LockFile::in_data_directory(data.path(), "tool", "held.lock");
    let held = hold_in_child(&lock, HELPER_TEST);
    assert_eq!(holder(&lock).expect("look at the lock"), Some(held.pid()));
}

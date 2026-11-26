//! The walk while a stream of `SIGUSR1` signals arrives.
//!
//! A handler that has no `SA_RESTART` makes a system call that a signal
//! interrupts return `EINTR`. This test installs such a handler, sends the
//! signal to the process many times during the walk, and asserts that the walk
//! still finds each planted file.
//!
//! A signal handler belongs to the whole process. Cargo builds one binary for
//! each file in `tests/`, thus this handler cannot reach a test in another
//! file. This file holds one test only, so no other test of this binary can
//! see the handler either.
//!
//! On a local APFS or ext4 disk, a directory read waits for the disk without a
//! wake-up for a signal. The signals then interrupt no call of the walk, and
//! this test passes also against a walk that treats `EINTR` as a fatal error.
//! `tests/interrupt.rs` holds the test that fails against such a walk: it puts
//! `EINTR` between the walk and the file system. This test proves the other
//! half: on the real file system, a storm of signals costs the walk nothing.

#[path = "common/tree.rs"]
mod tree;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use ff::fs::RealFs;
use ff::pattern::{Case, Matcher};
use ff::walk::{walk, Options, WalkRoot};
use tree::{sorted, Found, Tree};

/// The count of the signals that the handler got.
static DELIVERED: AtomicU64 = AtomicU64::new(0);

/// The handler. It only counts, because a handler can call only
/// async-signal-safe code, and an atomic add is such code.
extern "C" fn count_signal(_signal: libc::c_int) {
    DELIVERED.fetch_add(1, Ordering::Relaxed);
}

/// Installs [`count_signal`] for `SIGUSR1`, with `SA_RESTART` clear.
fn install_handler_without_restart() {
    // SAFETY: a zeroed `sigaction` is a valid value: no handler, no flags, and
    // an empty mask, which `sigemptyset` then sets again.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = count_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    action.sa_flags = 0;
    // SAFETY: `action.sa_mask` is a valid signal set that this function owns.
    unsafe {
        libc::sigemptyset(&raw mut action.sa_mask);
    }
    // SAFETY: `action` is a valid `sigaction`, and the old action is not
    // asked for.
    let status = unsafe { libc::sigaction(libc::SIGUSR1, &raw const action, std::ptr::null_mut()) };
    assert_eq!(status, 0, "install the handler of SIGUSR1");
}

/// The count of the walks to do while the signals arrive.
const WALKS: usize = 20;

#[test]
fn the_walk_finds_each_planted_file_while_sigusr1_interrupts_it() {
    install_handler_without_restart();
    let tree = Tree::new("sigusr1");
    for directory in 0..300 {
        for file in 0..40 {
            tree.file(&format!(
                "bulk/{}/{directory}/file-{file}.txt",
                directory % 13
            ));
        }
    }
    let planted: Vec<_> = (0..20)
        .map(|index| tree.file(&format!("deep/a/b/c/d/e/f/g/h/{index}/120.mp4")))
        .collect();
    let planted = sorted(planted);
    let matcher = Matcher::glob("120.mp4", Case::Insensitive).expect("a valid glob");

    let stop = Arc::new(AtomicBool::new(false));
    let sender = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            // SAFETY: `getpid` cannot fail.
            let process = unsafe { libc::getpid() };
            while !stop.load(Ordering::Relaxed) {
                // SAFETY: `kill` sends a signal to this process, and the
                // handler of that signal is installed.
                unsafe {
                    libc::kill(process, libc::SIGUSR1);
                }
                thread::sleep(Duration::from_micros(50));
            }
        })
    };

    let mut failures = Vec::new();
    for attempt in 0..WALKS {
        let found = Found::default();
        let report = walk(
            &RealFs,
            &[WalkRoot::new(tree.root())],
            Options::default(),
            &matcher,
            found.sink(),
        );
        if found.sorted() != planted || !report.is_complete() {
            failures.push(format!(
                "walk {attempt}: found {} of {} planted files, {:?}",
                found.sorted().len(),
                planted.len(),
                report.incomplete
            ));
        }
    }
    stop.store(true, Ordering::Relaxed);
    sender.join().expect("the sender thread ends");

    let delivered = DELIVERED.load(Ordering::Relaxed);
    eprintln!("{delivered} signals arrived during {WALKS} walks");
    assert!(
        delivered > 0,
        "no signal arrived, thus the test proves nothing"
    );
    assert!(failures.is_empty(), "{failures:#?}");
}

//! The turn at the audio device lets one test at a time open the device.
//!
//! The turn is shared code of the test targets of popstop (see
//! `shared/device-turn.rs`). These tests prove it here, once, and not in each
//! target that includes it.

#[path = "shared/device-turn.rs"]
mod device_turn;

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use device_turn::DeviceTurn;

/// The time for which the test looks for a second turn that must not begin.
///
/// A turn that does not exclude a second one gives the second one at once,
/// thus a short look finds it.
const A_SECOND_TURN_WOULD_BEGIN_WITHIN: Duration = Duration::from_millis(500);

#[test]
fn a_second_turn_waits_until_the_first_turn_ends() {
    let first = DeviceTurn::take();
    let (sender, receiver) = mpsc::channel();
    let waiter = thread::spawn(move || {
        let second = DeviceTurn::take();
        // The test can end before the send, and then nobody receives.
        let _ = sender.send(());
        drop(second);
    });

    assert!(
        receiver
            .recv_timeout(A_SECOND_TURN_WOULD_BEGIN_WITHIN)
            .is_err(),
        "a second test got the audio device while the first test still had it"
    );

    drop(first);
    // The take of the second turn has a bound of its own, and a take that
    // passes it ends the thread, so this wait ends too.
    receiver
        .recv()
        .expect("the second turn began after the first turn ended");
    waiter.join().expect("the thread of the second turn ended");
}

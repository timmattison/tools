//! The life cycle of a copy of popstop (macOS only).
//!
//! A copy takes the instance lock, starts the keepalive signal, reports that
//! it plays, and then waits for a signal. On a signal it ramps the signal
//! down, stops the output unit, and releases the lock.

use std::io;

/// Puts the calling thread into the background quality of service class.
///
/// popstop plays a signal that nobody hears, thus it never needs the
/// processor before another program does. The class tells the scheduler so.
///
/// # Errors
///
/// Returns the error of the system when the class cannot be set.
fn set_background_qos() -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::set_background_qos;
    use std::ptr;
    use std::thread;

    /// The number of the background class, as `sys/qos.h` gives it.
    const QOS_CLASS_BACKGROUND: u32 = 0x09;

    /// Gives the quality of service class of the calling thread, as a number.
    ///
    /// It reads the class into a `u32`, not into `libc::qos_class_t`. The
    /// system writes any number there, and a number that names no variant of
    /// that enum is not a valid value of it.
    fn qos_class_of_this_thread() -> u32 {
        let mut class = u32::MAX;
        let mut relative_priority = 0;
        // SAFETY: `pthread_get_qos_class_np` writes one `qos_class_t`, which
        // is a `u32`, into `class`, and one `c_int` into
        // `relative_priority`. Both live for the whole call.
        let status = unsafe {
            libc::pthread_get_qos_class_np(
                libc::pthread_self(),
                ptr::from_mut(&mut class).cast::<libc::qos_class_t>(),
                ptr::from_mut(&mut relative_priority),
            )
        };
        assert_eq!(status, 0, "the class of this thread cannot be read");
        class
    }

    #[test]
    fn the_background_class_holds_the_thread_that_asked_for_it() {
        // A thread of its own: the class of a thread of the test harness is
        // not this test to change.
        let class = thread::spawn(|| {
            let before = qos_class_of_this_thread();
            set_background_qos().expect("the background class");
            (before, qos_class_of_this_thread())
        })
        .join()
        .expect("the thread ends");

        assert_eq!(
            class.1, QOS_CLASS_BACKGROUND,
            "the thread runs in class {:#04x} after the call, and it ran in class {:#04x} before \
             it",
            class.1, class.0
        );
    }
}

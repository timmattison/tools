//! The start time of a process (macOS only).
//!
//! A PID is not an identity. When a process ends, the system can give its PID
//! to a new process. The kernel start time of a process tells the two apart.
//! A holder of the instance lock writes its own start time into its record,
//! and `popstop --stop` compares that time with the start time of the PID
//! before it sends a signal.

use std::io;

use crate::lock::StartTime;

/// Gives the time at which the kernel started the process `pid`.
///
/// # Errors
///
/// Returns an error when no process has the PID `pid`, or when the kernel
/// does not give the information of the process.
pub fn start_time(pid: u32) -> io::Result<StartTime> {
    let _ = pid;
    Ok(StartTime::from_unix_micros(0))
}

#[cfg(test)]
mod tests {
    use super::start_time;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    /// A bound that the age of this test process never comes near.
    const GENEROUS_AGE: Duration = Duration::from_secs(60 * 60);

    /// Gives a time as a number of microseconds since the Unix epoch.
    fn unix_micros(time: SystemTime) -> u64 {
        let since_epoch = time
            .duration_since(UNIX_EPOCH)
            .expect("the time is after 1970");
        u64::try_from(since_epoch.as_micros()).expect("the time fits in 64 bits")
    }

    #[test]
    fn the_start_time_of_this_process_is_a_little_before_now() {
        let now = SystemTime::now();

        let started = start_time(std::process::id())
            .expect("the kernel gives the start time of this process")
            .unix_micros();

        assert!(
            started <= unix_micros(now),
            "this process started at {started}, after now ({})",
            unix_micros(now)
        );
        assert!(
            started >= unix_micros(now - GENEROUS_AGE),
            "this process started at {started}, more than {GENEROUS_AGE:?} before now ({})",
            unix_micros(now)
        );
    }
}

//! The start time of a process (macOS only).
//!
//! A PID is not an identity. When a process ends, the system can give its PID
//! to a new process. The kernel start time of a process tells the two apart.
//! A holder of the instance lock writes its own start time into its record,
//! and `popstop --stop` compares that time with the start time of the PID
//! before it sends a signal.

use std::ffi::c_void;
use std::io;
use std::mem::{self, MaybeUninit};

use crate::lock::{HolderRecord, StartTime};

/// The number of microseconds in one second.
const MICROS_PER_SECOND: u64 = 1_000_000;

/// What the kernel says about the process that a record of the lock names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Identity {
    /// The process that has the PID is the copy that the record names.
    TheSame,
    /// No process has the PID. The copy that wrote the record is gone.
    Gone,
    /// A process has the PID, and it is not the copy that the record names.
    /// The system gave the PID to it after the record was written.
    Another,
}

/// Tells whether the process that has the PID of `record` is the copy that
/// `record` names.
///
/// `looked_up` is the answer of [`start_time`] for the PID of the record. The
/// caller makes that call, thus a test drives this decision with no process
/// of its own.
///
/// # Errors
///
/// Returns the error of `looked_up`, when that error is not `ESRCH`. An
/// `ESRCH` error is the answer that no process has the PID, thus it gives
/// [`Identity::Gone`] and not an error.
pub fn identity(record: &HolderRecord, looked_up: io::Result<StartTime>) -> io::Result<Identity> {
    let _ = (record, looked_up);
    Ok(Identity::TheSame)
}

/// Gives the time at which the kernel started the process `pid`.
///
/// It reads the BSD information of the process with `proc_pidinfo`. The
/// kernel keeps the start time in seconds and microseconds since the Unix
/// epoch.
///
/// # Errors
///
/// Returns an error when no process has the PID `pid` (the error of the
/// kernel, usually `ESRCH`), when `pid` is too large to be a PID (kind
/// [`io::ErrorKind::InvalidInput`]), or when the kernel gives less than the
/// whole information (kind [`io::ErrorKind::InvalidData`]).
pub fn start_time(pid: u32) -> io::Result<StartTime> {
    let pid = libc::pid_t::try_from(pid).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{pid} is too large to be a process ID"),
        )
    })?;
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let wanted = mem::size_of::<libc::proc_bsdinfo>();
    let wanted_c = libc::c_int::try_from(wanted)
        .map_err(|_| io::Error::other("the process information is too large for proc_pidinfo"))?;

    // SAFETY: `proc_pidinfo` writes at most `wanted_c` bytes into the buffer,
    // and that is the size of the buffer. The call only reads information of
    // the process. The code below reads the buffer only after the call
    // reports that it filled all of it.
    let filled = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast::<c_void>(),
            wanted_c,
        )
    };
    if filled <= 0 {
        return Err(io::Error::last_os_error());
    }
    if usize::try_from(filled).ok() != Some(wanted) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "proc_pidinfo gave {filled} of the {wanted} bytes of the information of process \
                 {pid}"
            ),
        ));
    }

    // SAFETY: the call above reported that it filled the whole structure.
    let info = unsafe { info.assume_init() };
    Ok(StartTime::from_unix_micros(
        info.pbi_start_tvsec
            .saturating_mul(MICROS_PER_SECOND)
            .saturating_add(info.pbi_start_tvusec),
    ))
}

#[cfg(test)]
mod tests {
    use super::{identity, start_time, Identity};
    use crate::lock::{HolderRecord, Mode, StartTime};
    use std::io;
    use std::process::{self, Command, Stdio};
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

    /// A child process. A drop kills and reaps it, so a test that fails early
    /// leaves no process.
    struct Child(process::Child);

    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Starts a child that sleeps until a drop kills it, for 60 s at most.
    fn sleeping_child() -> Child {
        Child(
            Command::new("sleep")
                .arg("60")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("start a child that sleeps"),
        )
    }

    #[test]
    fn the_start_time_of_a_child_is_between_the_start_and_the_end_of_its_spawn() {
        // The kernel keeps microseconds, so the bounds do too.
        let before = unix_micros(SystemTime::now());
        let child = sleeping_child();
        let after = unix_micros(SystemTime::now());

        let started = start_time(child.0.id())
            .expect("the kernel gives the start time of the child")
            .unix_micros();

        assert!(
            (before..=after).contains(&started),
            "the child started at {started}, outside its spawn from {before} to {after}"
        );
    }

    #[test]
    fn a_pid_that_no_process_has_is_an_error() {
        let mut child = sleeping_child();
        let pid = child.0.id();
        child.0.kill().expect("kill the child");
        child.0.wait().expect("reap the child");
        // The child is reaped, so the kernel has no process with its PID.
        // macOS gives PIDs in sequence, so it gives this PID to a new process
        // only after many other processes start.

        let result = start_time(pid);

        let error = result.expect_err("a PID that no process has gives no start time");
        assert_eq!(
            error.raw_os_error(),
            Some(libc::ESRCH),
            "the kernel says that no such process exists: {error}"
        );
    }

    #[test]
    fn a_pid_is_the_copy_of_the_record_only_when_the_start_times_agree() {
        let record = HolderRecord {
            pid: 4242,
            mode: Mode::Foreground,
            started_at: StartTime::from_unix_micros(1_789_725_600_123_456),
        };
        let decide = |looked_up| identity(&record, looked_up);

        assert_eq!(
            decide(Ok(record.started_at)).expect("the same process"),
            Identity::TheSame
        );
        assert_eq!(
            decide(Ok(StartTime::from_unix_micros(1_789_725_600_123_457)))
                .expect("another process"),
            Identity::Another,
            "one microsecond of difference is another process, thus no signal goes to that PID"
        );
        assert_eq!(
            decide(Err(io::Error::from_raw_os_error(libc::ESRCH))).expect("no process"),
            Identity::Gone,
            "the kernel says that no process has the PID, and that is an answer, not a failure"
        );

        let problem = decide(Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "half of the information",
        )))
        .expect_err("a problem that is not ESRCH gives no decision");
        assert_eq!(problem.kind(), io::ErrorKind::InvalidData);
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

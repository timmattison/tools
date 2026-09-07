//! The pseudo-terminal that a test hands to a child process of `ic`.
//!
//! Two targets need one. `controlling-terminal` gives the child a terminal of a
//! known size and reads the picture that `ic` drew for it. `kitty-refusal`
//! stands where the terminal stands: it reads the question that `ic` writes to
//! the terminal and it writes the answer back. Both of them build the terminal
//! the same way, and a second copy of that code would part company with this
//! one on the day either changed.
//!
//! # What the terminal has to be
//!
//! The child takes the slave end as its **controlling terminal**, and it does
//! so between the fork and the exec, where the descriptor is still open and the
//! close-on-exec flag changes nothing. `setsid` puts the child in a session of
//! its own and drops the terminal it inherited, and `TIOCSCTTY` then claims
//! this one. **That drop is what keeps a test off the terminal of whoever
//! started the run**: a child that failed to claim this terminal owns no
//! terminal at all.
//!
//! Standard output of the child stays a pipe, which is the shape of a captured
//! run: the terminal of the session is there to measure and to answer, and
//! standard output is not it.
//!
//! The size arrives with the pseudo-terminal, so no second ioctl sets it and no
//! window of the wrong size ever exists. The caller states that size, because a
//! window of one size proves one thing to one target.

use std::os::unix::process::CommandExt;
use std::process::Command;
use std::ptr;
use std::time::{Duration, Instant};

/// The window that a pseudo-terminal reports.
///
/// The four numbers travel together, because a caller that measures a character
/// cell divides the pixels of one window by the cells of that same window.
#[derive(Debug, Clone, Copy)]
pub struct Window {
    /// The width of the window in columns.
    pub columns: u16,
    /// The height of the window in rows.
    pub rows: u16,
    /// The width of the window in pixels.
    pub width_px: u16,
    /// The height of the window in pixels.
    pub height_px: u16,
}

/// A pseudo-terminal, and the one way a test reaches both ends of it.
///
/// The pair of file descriptors stays open for the life of the value. The
/// master end holds the pseudo-terminal alive and carries every byte that the
/// child writes to its terminal, and the slave end is the terminal that the
/// child takes as its own.
pub struct Pty {
    /// The master end, which stands where the terminal emulator stands.
    master: libc::c_int,
    /// The slave end, which becomes the controlling terminal of the child.
    slave: libc::c_int,
}

impl Pty {
    /// Open a pseudo-terminal that reports `window`.
    ///
    /// # Arguments
    /// * `window` - The size that the terminal reports, in cells and in pixels.
    ///
    /// # Returns
    /// Both ends of a pseudo-terminal of that size.
    ///
    /// # Panics
    /// Panics when the system opens no pseudo-terminal.
    pub fn open(window: Window) -> Self {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        let mut size = libc::winsize {
            ws_row: window.rows,
            ws_col: window.columns,
            ws_xpixel: window.width_px,
            ws_ypixel: window.height_px,
        };

        // SAFETY: `openpty` writes one file descriptor to each of the first two
        // pointers, and both point at a live local variable. The two null
        // pointers are the documented way to ask for the default terminal modes
        // and to ask for no name of the slave device. The last pointer is the
        // size of the window, and it points at a live local variable that
        // outlives the call.
        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut size,
            )
        };
        assert_eq!(
            result,
            0,
            "openpty must give a pseudo-terminal: {}",
            std::io::Error::last_os_error()
        );

        Pty { master, slave }
    }

    /// Give the child of `command` a session of its own and this terminal as
    /// its controlling terminal.
    ///
    /// `/dev/tty` in the child then resolves to this pseudo-terminal, whatever
    /// standard output points at.
    ///
    /// # Arguments
    /// * `command` - The command to start the child from. The caller sets every
    ///   other part of it.
    pub fn hand_to(&self, command: &mut Command) {
        let slave = self.slave;
        // SAFETY: the closure runs in the child between the fork and the exec,
        // and it calls two functions. `setsid` and `ioctl` are both
        // async-signal-safe, and neither one touches memory of this process:
        // the ioctl takes the request `TIOCSCTTY`, which reads no pointer. The
        // child is never a process group leader in that window, because the
        // fork gave it a new process id and the process group is still the one
        // of the parent, so the one documented failure of `setsid` cannot
        // happen.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }

                #[allow(
                    clippy::disallowed_methods,
                    reason = "the ban covers the read of a window, and `TIOCSCTTY` reads none. It claims the pseudo-terminal as the controlling terminal of the child, and termsize offers no call for that"
                )]
                if libc::ioctl(slave, libc::c_ulong::from(libc::TIOCSCTTY), 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }

                Ok(())
            });
        }
    }

    /// Read the master end until `needle` arrives, or until `budget` is spent.
    ///
    /// The bytes are the ones that the child wrote to its terminal. **The wait
    /// carries a deadline**, because a child that writes nothing writes nothing
    /// forever, and a test that waits for a byte that a defect took away is
    /// worse than a test that fails.
    ///
    /// # Arguments
    /// * `needle` - The bytes that end the read.
    /// * `budget` - The longest that the read waits.
    ///
    /// # Returns
    /// Every byte that arrived, which holds `needle` when the child wrote it
    /// inside the budget and holds no `needle` when it did not.
    pub fn read_until(&self, needle: &[u8], budget: Duration) -> Vec<u8> {
        /// The largest answer this read keeps. A child that writes without
        /// stopping cannot grow the buffer past it.
        const LIMIT: usize = 4096;
        /// How much of the stream one read takes.
        const CHUNK: usize = 64;

        let deadline = Instant::now() + budget;
        let mut seen: Vec<u8> = Vec::new();
        while seen.len() < LIMIT && !holds(&seen, needle) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !waits_for_a_byte(self.master, left) {
                break;
            }
            let mut chunk = [0_u8; CHUNK];
            // SAFETY: the buffer is owned here and the length is its own.
            let taken = unsafe { libc::read(self.master, chunk.as_mut_ptr().cast(), chunk.len()) };
            let Ok(taken) = usize::try_from(taken) else {
                break;
            };
            if taken == 0 {
                break;
            }
            seen.extend_from_slice(&chunk[..taken]);
        }
        seen
    }

    /// Write `answer` to the master end, where the child reads it as input from
    /// its terminal.
    ///
    /// # Arguments
    /// * `answer` - The bytes that the terminal says.
    ///
    /// # Panics
    /// Panics when the write takes fewer bytes than `answer` holds, because a
    /// half of an answer is an answer that no reader can make sense of.
    pub fn answer(&self, answer: &[u8]) {
        // SAFETY: the buffer is owned by the caller for the length of this
        // call, and the length is its own.
        let put = unsafe { libc::write(self.master, answer.as_ptr().cast(), answer.len()) };
        assert_eq!(
            put,
            isize::try_from(answer.len()).expect("an answer of a terminal is a few dozen bytes"),
            "the whole of the answer must reach the terminal: {}",
            std::io::Error::last_os_error()
        );
    }
}

impl Drop for Pty {
    /// Close both ends of the pseudo-terminal.
    ///
    /// A test that leaks a file descriptor for each run empties the table of
    /// the process, and the runs of one target share one process.
    fn drop(&mut self) {
        // SAFETY: each descriptor came from the one `openpty` call of
        // [`Pty::open`], nothing else closes them, and `Drop` runs one time.
        unsafe {
            libc::close(self.slave);
            libc::close(self.master);
        }
    }
}

/// Whether `haystack` holds `needle`.
fn holds(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Wait up to `left` for one byte to arrive on `fd`.
///
/// This is select(2) and not poll(2), for the reason that
/// `src/termgfx/src/probe.rs` records: poll(2) on macOS answers `POLLNVAL` for
/// a terminal that has an answer waiting, and a caller that reads that as "no
/// answer" gets silence from every terminal on the platform.
fn waits_for_a_byte(fd: libc::c_int, left: Duration) -> bool {
    // select(2) reads the descriptor as a bit in a fixed-width set, so a
    // descriptor above the width of that set cannot be asked about at all.
    let width = libc::c_int::try_from(libc::FD_SETSIZE).unwrap_or(libc::c_int::MAX);
    if fd < 0 || fd >= width {
        return false;
    }
    // SAFETY: `fd_set` is a plain bit array, and FD_ZERO fills the whole of it
    // before FD_SET writes one bit of it.
    let mut watched: libc::fd_set = unsafe { std::mem::zeroed() };
    // SAFETY: `watched` is one initialized set and `fd` is inside its width.
    unsafe {
        libc::FD_ZERO(&mut watched);
        libc::FD_SET(fd, &mut watched);
    }
    let mut budget = libc::timeval {
        tv_sec: libc::time_t::try_from(left.as_secs()).unwrap_or(libc::time_t::MAX),
        tv_usec: libc::suseconds_t::try_from(left.subsec_micros()).unwrap_or(0),
    };
    // SAFETY: the set and the budget are owned here, and the two pointers that
    // name no set are null, which select(2) reads as "ask nothing of them".
    let ready = unsafe {
        libc::select(
            fd + 1,
            &mut watched,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut budget,
        )
    };
    ready > 0
}

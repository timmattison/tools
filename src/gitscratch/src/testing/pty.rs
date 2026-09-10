//! A pseudo-terminal of a size that a test chose.
//!
//! A tool that lays text out for a terminal measures the width of that
//! terminal. `cargo test` gives the test binary the terminal of whoever started
//! the run. So a test that lets the tool measure that terminal gets one answer
//! in a wide window and another answer in a narrow one. A test therefore opens
//! a pseudo-terminal of a size it chose, and gives it to the child.
//!
//! [`Pty::give_as_controlling_terminal`] makes the pseudo-terminal the
//! controlling terminal of the child. `/dev/tty` in the child then resolves to
//! it, whatever standard output points at.
//!
//! A pseudo-terminal that nobody sized reports zero columns, and the
//! `TIOCGWINSZ` ioctl succeeds on it. Every terminal here therefore arrives
//! sized, and the size arrives with the `openpty` call so that no window of the
//! wrong size ever exists.
//!
//! The helper lives in `gitscratch` because `grind` and `grime` both need it. A
//! copy in each tool puts the same `unsafe` code in two places, and the two
//! copies part company on the day one of them changes.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::ptr;

/// A pseudo-terminal of a size that a test chose.
///
/// The master end holds the pseudo-terminal alive, and the slave end is the
/// terminal that the child takes as its own. Both ends stay open for the life
/// of the value, so a test keeps the value until its child is done.
///
/// Each end is an [`OwnedFd`], so each descriptor closes when the value drops,
/// on the path of a panic too. A test that leaks a descriptor for each run
/// empties the table of the process, and the tests of one target share one
/// process. A pseudo-terminal is scarce as well. Every process of the machine
/// shares one supply of them, and `openpty` fails when that supply is empty.
pub struct Pty {
    /// The master end. Nothing reads it. It keeps the pseudo-terminal alive
    /// until the value drops.
    _master: OwnedFd,
    /// The slave end. It becomes the controlling terminal of the child.
    slave: OwnedFd,
}

impl Pty {
    /// The number of rows of every pseudo-terminal here.
    ///
    /// A layout reads the columns and no row, so this number reaches nothing
    /// that a test of a layout asserts. It is above zero because a terminal of
    /// zero rows carries no window, and `termsize` refuses such a terminal. A
    /// tool then falls back to its default width.
    pub const ROWS: u16 = 24;

    /// Open a pseudo-terminal `columns` columns wide.
    ///
    /// # Returns
    /// The two ends of a pseudo-terminal that reports `columns` columns by
    /// [`Pty::ROWS`] rows.
    ///
    /// # Panics
    /// Panics when the system opens no pseudo-terminal.
    pub fn open(columns: u16) -> Self {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        let mut size = libc::winsize {
            ws_row: Self::ROWS,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
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
            io::Error::last_os_error()
        );

        // SAFETY: `openpty` succeeded, so each variable holds a descriptor that
        // it opened for this call. Nothing else owns either descriptor, so each
        // `OwnedFd` is the one owner of its descriptor, and it closes that
        // descriptor one time, when it drops.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };

        Pty {
            _master: master,
            slave,
        }
    }

    /// Give the child of `command` a session of its own, and this terminal as
    /// its controlling terminal.
    ///
    /// The child starts a session of its own and then claims the slave end as
    /// its controlling terminal. `/dev/tty` in the child therefore resolves to
    /// this pseudo-terminal, whatever standard output points at. The new
    /// session also drops the terminal that the child inherited, so the child
    /// never reaches the terminal of whoever started the run.
    ///
    /// The command keeps a copy of the slave end of its own for the claim. So
    /// the claim reads a descriptor that is open when the child starts, and a
    /// caller that drops this value too early cannot aim the claim at a
    /// descriptor that another open took over. Keep the value alive for the
    /// whole run all the same, because the master end keeps the terminal
    /// alive.
    ///
    /// # Arguments
    /// * `command` - The command to start the child from. The caller sets every
    ///   other part of it.
    ///
    /// # Panics
    /// Panics when the system gives no copy of the slave end.
    pub fn give_as_controlling_terminal(&self, command: &mut Command) {
        let slave = self
            .slave
            .try_clone()
            .expect("the system must give a copy of the slave end for the claim");

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
                    return Err(io::Error::last_os_error());
                }

                #[allow(
                    clippy::disallowed_methods,
                    reason = "the ban covers the read of a window, and `TIOCSCTTY` reads none. It claims the pseudo-terminal as the controlling terminal of the child, and termsize offers no call for that"
                )]
                if libc::ioctl(slave.as_raw_fd(), libc::c_ulong::from(libc::TIOCSCTTY), 0) == -1 {
                    return Err(io::Error::last_os_error());
                }

                Ok(())
            });
        }
    }
}

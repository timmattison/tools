//! A pseudo-terminal of a size that a test chose.
//!
//! A tool that lays text out for a terminal measures the width of that
//! terminal. `cargo test` gives the test binary the terminal of whoever started
//! the run. So a test that lets the tool measure that terminal gets one answer
//! in a wide window and another answer in a narrow one. A test therefore opens
//! a pseudo-terminal of a size it chose, and gives it to the child.
//!
//! A test gives the terminal to the child in one of two shapes.
//!
//! - [`Pty::give_as_controlling_terminal`] makes the pseudo-terminal the
//!   controlling terminal of the child. `/dev/tty` in the child then resolves
//!   to it, whatever standard output points at. A test of a layout needs this
//!   shape, with standard output on a pipe that the test reads.
//! - [`Pty::run_with_stdout_on_terminal`] does the same, and also puts
//!   standard output of the child on the pseudo-terminal and reads back what
//!   the child wrote there. A test of a tool that decides color by whether
//!   standard output is a terminal needs this shape.
//!
//! A pseudo-terminal that nobody sized reports zero columns, and the
//! `TIOCGWINSZ` ioctl succeeds on it. Every terminal here therefore arrives
//! sized, and the size arrives with the `openpty` call so that no window of the
//! wrong size ever exists.
//!
//! The helper lives in `gitscratch` because `grind` and `grime` both need it. A
//! copy in each tool puts the same `unsafe` code in two places, and the two
//! copies part company on the day one of them changes.

use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, Output, Stdio};
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
/// Each end also closes on the exec of a child, so a child holds the terminal
/// only where a test gives the terminal to it.
pub struct Pty {
    /// The master end. It carries every byte that reaches the terminal, and it
    /// keeps the pseudo-terminal alive until the value drops.
    master: OwnedFd,
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

    /// Open a pseudo-terminal `columns` columns wide, which gives back each
    /// byte as the child wrote it.
    ///
    /// The terminal has the default modes of the system, less one: output
    /// processing is off. With output processing on, the terminal changes each
    /// newline into a carriage return and a newline on its way to the master
    /// end. A test that compares the bytes of a tool on a terminal with the
    /// bytes of the same tool on a pipe then compares two different texts.
    ///
    /// The modes change before any child exists, so no child ever writes to a
    /// terminal that changes its bytes. They change after the `openpty` call
    /// and not in it. To give `openpty` a set of modes, the helper needs the
    /// default modes of the system, and only a new terminal reports them.
    ///
    /// # Returns
    /// The two ends of a pseudo-terminal that reports `columns` columns by
    /// [`Pty::ROWS`] rows.
    ///
    /// # Panics
    /// Panics when the system opens no pseudo-terminal, when an end does not
    /// take the close-on-exec flag, or when the terminal does not give or take
    /// its modes.
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

        // Both ends close on the exec of a child. `openpty` takes no such flag,
        // so each end otherwise reaches every child that this process starts,
        // and each such child holds the terminal open for as long as it runs.
        // A child of this terminal still gets it. Its standard output is a copy
        // that the spawn puts in place, and the claim of the controlling
        // terminal runs between the fork and the exec, where the flag changes
        // nothing. A fork in the few instructions between `openpty` and these
        // calls still takes a copy.
        for end in [&master, &slave] {
            // SAFETY: `fcntl` with `F_SETFD` reads no pointer, and the
            // descriptor is open, because an `OwnedFd` owns it.
            let flagged = unsafe { libc::fcntl(end.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
            assert_ne!(
                flagged,
                -1,
                "each end of the terminal must close on an exec: {}",
                io::Error::last_os_error()
            );
        }

        let mut modes = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: `tcgetattr` writes one `termios` through the pointer, which
        // points at storage of that type that outlives the call. The descriptor
        // is the slave end, which is open.
        let given = unsafe { libc::tcgetattr(slave.as_raw_fd(), modes.as_mut_ptr()) };
        assert_eq!(
            given,
            0,
            "the terminal must give its modes: {}",
            io::Error::last_os_error()
        );
        // SAFETY: `tcgetattr` succeeded, so it wrote every field of the
        // `termios`.
        let mut modes = unsafe { modes.assume_init() };
        modes.c_oflag &= !libc::OPOST;
        // SAFETY: `tcsetattr` reads one `termios` through the pointer, which
        // points at a live local value. The descriptor is the slave end, which
        // is open.
        let taken = unsafe { libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &modes) };
        assert_eq!(
            taken,
            0,
            "the terminal must take its modes: {}",
            io::Error::last_os_error()
        );

        Pty { master, slave }
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

    /// Run `command` with its standard output on this terminal, and give back
    /// what the child left behind.
    ///
    /// A tool that decides color by whether standard output is a terminal
    /// needs this shape of run. A pipe is not a terminal, so a test that reads
    /// standard output through a pipe never sees the color that a person at a
    /// terminal sees.
    ///
    /// Standard input of the child is null, as [`Command::output`] gives it.
    /// Standard error is a pipe, so a test can print what the child said about
    /// a failure. The bytes of standard output come back from the master end,
    /// which carries every byte that reaches the terminal.
    ///
    /// The method takes the value, because the read of the master end ends
    /// only when no copy of the slave end is left open, and the value holds
    /// one. So when the child starts, the method closes every copy that this
    /// process holds. The read then ends when the child and its own children
    /// close theirs. A child that leaves a process behind with the terminal
    /// still open holds the read open for as long as that process lives.
    ///
    /// The master end is read on a thread of its own, while the calling thread
    /// reads standard error and then waits for the child. A child that writes
    /// more than a buffer holds stops until somebody reads that buffer. A read
    /// of one stream to its end before a read of the other therefore deadlocks
    /// on output bigger than a buffer, and a wait before either read deadlocks
    /// on less.
    ///
    /// The terminal is also the controlling terminal of the child, as
    /// [`Pty::give_as_controlling_terminal`] makes it. A tool that measures its
    /// width through `/dev/tty` then measures this terminal, and not the
    /// terminal of whoever started the run.
    ///
    /// # Arguments
    /// * `command` - The command to start the child from. This method sets its
    ///   three standard streams, and the caller sets every other part of it.
    ///
    /// # Returns
    /// The exit status of the child, every byte it wrote to the terminal, and
    /// every byte it wrote to standard error, in the shape that
    /// [`Command::output`] gives.
    ///
    /// # Panics
    /// Panics when the system gives no copy of the slave end, when the child
    /// does not start, when a read of the master end fails for a reason other
    /// than the end of the output, or when the wait for the child fails.
    pub fn run_with_stdout_on_terminal(self, mut command: Command) -> Output {
        self.give_as_controlling_terminal(&mut command);
        let stdout = self
            .slave
            .try_clone()
            .expect("the system must give a copy of the slave end for standard output");
        command
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(Stdio::piped());

        let Pty { master, slave } = self;
        let reader = std::thread::spawn(move || read_until_hangup(master));
        let child = command.spawn();

        // Every copy of the slave end that this process holds closes here: the
        // copy that the command holds for standard output, and the copy that
        // the terminal opened with. The child holds the rest. They close before
        // the result of the spawn is read, so a child that did not start still
        // ends the read, and the reader ends with it.
        drop(command);
        drop(slave);

        let child = child.unwrap_or_else(|error| panic!("the child must start: {error}"));
        let finished = child
            .wait_with_output()
            .unwrap_or_else(|error| panic!("the wait for the child must succeed: {error}"));
        let stdout = reader
            .join()
            .expect("the reader of the master end must not panic")
            .unwrap_or_else(|error| {
                panic!("the master end must give back what the child wrote: {error}")
            });

        Output { stdout, ..finished }
    }
}

/// Read `master` until no copy of the slave end is left open.
///
/// When the last copy of the slave end closes, the read of the master end
/// gives 0 bytes or fails with `EIO`, and which of the two depends on the
/// system. Both are the end of the output, not an error.
///
/// # Errors
/// Returns the error of a read that fails for any other reason.
fn read_until_hangup(master: OwnedFd) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    match File::from(master).read_to_end(&mut bytes) {
        Ok(_) => Ok(bytes),
        // `read_to_end` keeps every byte it read before the error, so the
        // output is whole.
        Err(error) if error.raw_os_error() == Some(libc::EIO) => Ok(bytes),
        Err(error) => Err(error),
    }
}

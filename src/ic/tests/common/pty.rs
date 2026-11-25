//! What terminal a test gives to a child process of `ic`.
//!
//! There are two answers, and a test states which one it wants.
//! [`take_the_terminal_away`] gives the child no terminal at all, and [`Pty`]
//! gives it a pseudo-terminal that the test holds the other end of. **Both keep
//! the test off the terminal of whoever started the run**, because both put the
//! child in a session of its own. They are here together for that reason: the
//! choice between them is one choice, and a copy of either one in a target
//! would part company with this one on the day it changed.
//!
//! # Which one a test needs
//!
//! A test that reads an exit status and two pipes needs
//! [`take_the_terminal_away`]. `ic` asks its terminal through `/dev/tty`, a
//! session with no controlling terminal answers `ENXIO` to every open of that
//! name, and a run that reached no terminal answers with the name the
//! environment carries. That is the answer a silent terminal gives, so such a
//! test reads the same verdict for less.
//!
//! A test that reads the bytes the child wrote to a terminal, or that answers
//! the child as a terminal does, needs [`Pty`]. `controlling-terminal` gives the
//! child a terminal of a known size and reads the picture that `ic` drew for it.
//! `kitty-refusal` stands where the terminal stands: it reads the question that
//! `ic` writes to the terminal and it writes the answer back.
//!
//! **A pseudo-terminal is scarce.** The machine holds a few hundred of them in
//! all, which `kern.tty.ptmx_max` states, and every process on the machine
//! shares that one number. The tests of one target run beside each other, so a
//! target that opens one for each test asks for more of them than the machine
//! has left, and [`Pty::open`] then fails with `ENXIO`. A test asks for one when
//! it reads one, and it asks for none when it does not.
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

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

/// The request of the primary device attributes.
///
/// Every terminal answers it, so its answer is what ends a read of the
/// terminal, and it stands last in the query that `ic` writes. A read that
/// waits for it therefore waits for the whole of a query.
const ATTRIBUTES_REQUEST: &[u8] = b"\x1b[c";

/// How long one turn of [`run_with_terminal`] waits for a byte of the terminal.
///
/// The runner reads the terminal and asks after the child in turn, and this is
/// the length of one turn. A run that asks writes its query inside the first
/// one, and [`Pty::read_until`] gives that query back the moment the request of
/// the attributes ends it.
///
/// **It is the floor of a turn as well as the ceiling.** [`Pty::read_until`]
/// gives the rest of the slice back the moment the needle arrives, and it gives
/// the whole of it back when the read of the terminal fails. The runner sleeps
/// out what such a read left, so a wait for the child costs no core of the
/// machine.
const READ_SLICE: Duration = Duration::from_millis(100);

/// How long [`run_with_terminal`] waits for `ic` to exit.
///
/// Ten seconds, which is generous because a loaded machine starts a process
/// late. **The wall clock is the bound, and a count of turns is not**: a count
/// is a deadline only while each turn costs what the count assumes, and a turn
/// that ends early then shortens the whole wait without saying so. A change
/// that stopped `ic` from exiting must fail a test instead of holding it, and
/// it must take the stated ten seconds to say so.
const EXIT_BUDGET: Duration = Duration::from_secs(10);

/// Take the controlling terminal away from the child of `command`.
///
/// `setsid` puts the child in a session of its own, and a session with no
/// controlling terminal answers `ENXIO` to every open of `/dev/tty`. So the
/// child measures no terminal, it reads no byte of the terminal of whoever
/// started the run, and it writes none there. A run of `ic` that reached no
/// terminal answers with the name the environment carries, which is the answer
/// a silent terminal gives it.
///
/// **A pipe for standard output is not enough on its own.** `cargo test`
/// captures the standard output of a test binary, and a probe that reads
/// `/dev/tty` still measures the terminal of the person who typed the command.
/// Such a test passes in a redirected run and fails from a terminal, on a
/// condition it does not control. The new session is how a test states which
/// terminal it wants, and this call states none.
///
/// [`Pty::hand_to`] is the other entrance, and it starts here: it takes the
/// terminal away first and gives a pseudo-terminal back.
///
/// # Arguments
/// * `command` - The command to start the child from. The caller sets every
///   other part of it.
pub fn take_the_terminal_away(command: &mut Command) {
    // SAFETY: the closure runs in the child between the fork and the exec, and
    // it calls one function. `setsid` is async-signal-safe, it takes no
    // argument and it touches no memory of this process, so it is safe in that
    // window. The child is never a process group leader there, because the fork
    // gave it a new process id and the process group is still the one of the
    // parent, so the one documented failure of `setsid` cannot happen.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }

            Ok(())
        });
    }
}

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
        //
        // The size goes in as a raw `*mut` pointer, not as `&mut size`. The
        // `libc` crate declares that parameter `*mut winsize` on macOS and
        // `*const winsize` on Linux. A `*mut` pointer converts to both, but a
        // `&mut` reference to a `*const` parameter makes clippy report
        // `unnecessary_mut_passed` on Linux.
        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::from_mut(&mut size),
            )
        };
        assert_eq!(
            result,
            0,
            "openpty must give a pseudo-terminal: {}. `Device not configured` here is the ceiling of `kern.tty.ptmx_max`, which every process of the machine shares, so a target that holds one pseudo-terminal for each of its concurrent tests runs a busy machine out of them",
            std::io::Error::last_os_error()
        );

        // Both ends close on the exec of a child that starts after this call.
        // `openpty` sets no such flag, so each end used to reach every child of
        // every other test of the target, and each of those children held this
        // pseudo-terminal open for as long as it ran. The machine holds a few
        // hundred pseudo-terminals in all, so a copy in a child that has no use
        // for it is a slot the next `openpty` cannot have. The child of this
        // terminal still claims it, because `TIOCSCTTY` runs between the fork
        // and the exec, where the flag changes nothing. A fork inside the few
        // instructions between the two calls still takes a copy, and no call of
        // this platform opens a pseudo-terminal with the flag already set.
        for end in [master, slave] {
            // SAFETY: `fcntl` with `F_SETFD` reads no pointer, and both
            // descriptors came from the `openpty` above.
            let flagged = unsafe { libc::fcntl(end, libc::F_SETFD, libc::FD_CLOEXEC) };
            assert_ne!(
                flagged,
                -1,
                "each end of a pseudo-terminal must close on an exec: {}",
                std::io::Error::last_os_error()
            );
        }

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
        take_the_terminal_away(command);

        let slave = self.slave;
        // SAFETY: the closure runs in the child between the fork and the exec,
        // and after the closure of [`take_the_terminal_away`], because a child
        // runs these closures in the order the caller registered them. It calls
        // one function. `ioctl` is async-signal-safe, and it touches no memory
        // of this process: it takes the request `TIOCSCTTY`, which reads no
        // pointer.
        unsafe {
            command.pre_exec(move || {
                #[expect(
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

/// What one run of [`run_with_terminal`] did.
pub struct TerminalRun {
    /// The exit status of the run, as a number.
    pub code: Option<i32>,
    /// Every byte that the run wrote to standard output.
    pub stdout: String,
    /// Every byte that the run wrote to standard error.
    pub stderr: String,
    /// Every byte that the run wrote to its terminal.
    pub wrote: Vec<u8>,
}

/// Run `command` with a pseudo-terminal of `window` as its controlling
/// terminal.
///
/// [`Pty::hand_to`] puts the child in a session of its own, which drops the
/// terminal the child inherited from the test runner and claims this one
/// instead. So the run asks this terminal, which answers nothing, and it
/// reaches no terminal of a person.
///
/// **The read of the terminal runs beside the wait for the child, and not
/// behind it.** A process that closes its controlling terminal waits for the
/// output queue of that terminal to drain, and the master end is where it
/// drains to. So a run that wrote a query the runner had not read yet would
/// hang on the way out, and a wait in front of the read would hold the two of
/// them there for good.
///
/// # Arguments
/// * `command` - The command to run. The caller states the environment and the
///   arguments, and this call states the terminal and the three standard
///   streams.
/// * `window` - The size that the pseudo-terminal reports.
///
/// # Returns
/// The exit status, the two captured streams, and every byte that the run wrote
/// to the terminal.
///
/// # Panics
/// Panics when `ic` does not start, and when it does not exit inside the
/// deadline that [`EXIT_BUDGET`] states.
pub fn run_with_terminal(command: &mut Command, window: Window) -> TerminalRun {
    let pty = Pty::open(window);

    // `spawn` inherits the streams of the test runner, where `output` gives a
    // pipe for each of them. So this states all three.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    pty.hand_to(command);

    let mut child = command.spawn().expect("ic must run");

    let deadline = Instant::now() + EXIT_BUDGET;
    let mut wrote = Vec::new();
    let mut exited = None;
    while Instant::now() < deadline {
        let slice = deadline
            .saturating_duration_since(Instant::now())
            .min(READ_SLICE);
        let turn = Instant::now();
        wrote.extend(pty.read_until(ATTRIBUTES_REQUEST, slice));
        if let Some(status) = child.try_wait().expect("failed to ask after ic") {
            exited = Some(status);
            break;
        }
        let spent = turn.elapsed();
        if spent < slice {
            thread::sleep(slice - spent);
        }
    }
    let Some(status) = exited else {
        // The child outlived the deadline, which is the failure this wait is
        // here to report. It ends first: a child that nobody kills and nobody
        // waits for holds a slot of the process table of the machine for as
        // long as the test runner lives.
        let _ = child.kill();
        let _ = child.wait();
        panic!("ic must exit inside the deadline of this file");
    };
    // The child has exited, so every byte it ever wrote to the terminal is
    // waiting there already. One last turn takes the bytes it wrote on its way
    // out.
    wrote.extend(pty.read_until(ATTRIBUTES_REQUEST, READ_SLICE));

    TerminalRun {
        code: status.code(),
        stdout: drain(child.stdout.take(), "stdout"),
        stderr: drain(child.stderr.take(), "stderr"),
        wrote,
    }
}

/// Take every byte that one pipe of the child holds.
///
/// The child has exited by the time this runs, so the whole of what it wrote is
/// waiting in the pipe and the read ends at once.
///
/// # Arguments
/// * `pipe` - The end of the pipe that the child wrote to.
/// * `name` - The name of that stream, for the message of a failure.
fn drain(pipe: Option<impl Read>, name: &str) -> String {
    let mut bytes = Vec::new();
    pipe.unwrap_or_else(|| panic!("the run must hold a pipe for {name}"))
        .read_to_end(&mut bytes)
        .unwrap_or_else(|error| panic!("failed to read the {name} of ic: {error}"));
    String::from_utf8_lossy(&bytes).into_owned()
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
    // The microseconds go through `u64`. `suseconds_t` is `i32` on macOS and
    // `i64` on Linux, thus a conversion from `u32` can fail on macOS only, and
    // clippy on Linux reports it as a fallible conversion that cannot fail. A
    // conversion from `u64` can fail on both platforms.
    let mut budget = libc::timeval {
        tv_sec: libc::time_t::try_from(left.as_secs()).unwrap_or(libc::time_t::MAX),
        tv_usec: libc::suseconds_t::try_from(u64::from(left.subsec_micros())).unwrap_or(0),
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

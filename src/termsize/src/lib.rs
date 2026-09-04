//! The one reader of the terminal size, which takes a zero as no answer.
//!
//! Two crates of this workspace read the size of a terminal, and each of them
//! reads a different file descriptor and answers in a shape of its own. A tool
//! that wants the width of the window it draws in therefore has to know which
//! crate to call, what that crate measures, and what the answer of that crate
//! means. This crate is the one entrance, and it holds those answers in one
//! place.
//!
//! # A zero is not a width
//!
//! A terminal answers the `TIOCGWINSZ` ioctl with the number of columns and
//! the number of rows of its window. A terminal that carries no window answers
//! that same ioctl with zero columns and zero rows, and the ioctl succeeds. A
//! pseudo-terminal that nobody ever sized is such a terminal, and
//! `script -q /dev/null` makes one.
//!
//! `crossterm` hands that zero back as a success: it answers `Ok((0, 0))`.
//! Every caller that reads the answer therefore takes the zero as the width of
//! a window. `krt` shows what a caller then does with it. A replay under such
//! a terminal printed a table of three columns, where the same replay through
//! a pipe printed ten: the table dropped column after column to fit a window
//! of no columns, and no line of the output said why.
//!
//! `terminal_size` refuses a zero on Unix today, and it does not refuse one on
//! Windows, where it subtracts the left edge of the console rectangle from the
//! right one and puts no limit under the difference. So the two crates
//! disagree with each other, and one of them disagrees with itself between two
//! platforms. A caller that wants to know which of the four answers it holds
//! has to read the source of both crates.
//!
//! This crate states the one rule instead. A size whose columns or whose rows
//! are zero is no answer, and every function here gives `None` for it. A
//! caller that already falls back on `None` gets its fallback, and a caller
//! that has no fallback gets the error path it already carries. That is the
//! whole of the behavior, and it is why the ban in `clippy.toml` sends every
//! caller here.
//!
//! # Why three probes stand here
//!
//! The three probes read different file descriptors, and they stay apart for
//! that reason. [`controlling_size`] reads the controlling terminal:
//! `crossterm` opens `/dev/tty`, and it reads standard output when that open
//! fails. [`stdout_size`] reads standard output, and it reads standard error
//! and then standard input when standard output is no terminal.
//!
//! The two answer differently, and that difference is the point of the split.
//! A run whose standard output is a pipe still holds the terminal that started
//! it, so [`controlling_size`] answers with the size of that window and
//! [`stdout_size`] answers `None`. A tool that draws a progress bar on the
//! terminal asks the first question, because the bar goes to the terminal
//! whatever the output of the run goes to. A tool that formats text for
//! whatever reads its output asks the second question, because a pipe holds no
//! width to format to. One probe that quietly replaced the other would change
//! which file descriptor a caller measures, and no line of the call site would
//! say so.
//!
//! [`drawing_window`] is the third probe, and it answers for a tool that draws
//! a picture. It reads four file descriptors in one order: standard output,
//! then standard error, then standard input, then `/dev/tty`. Standard output
//! comes first, because the picture goes there. Standard error and standard
//! input follow, because that is the chain [`stdout_size`] already reads
//! through, so this probe can only add an answer where that one had none, and
//! it can never take one away. `/dev/tty` comes last, because a standard output
//! that somebody captured is no proof that there is no terminal. A caller that
//! wants the bytes of a run as a string still sits at the terminal that started
//! the run, and that terminal is the one the picture appears on. A process that
//! holds no controlling terminal fails the open of `/dev/tty` with `ENXIO`, and
//! the answer is then `None`, which is the right answer for a job of `cron` and
//! for a container.
//!
//! # Why the third probe carries the pixel size
//!
//! The other two probes answer in character cells alone, and the third one
//! answers in character cells and in pixels. A tool that draws a picture needs
//! both, because it lays the picture out in pixels and reserves room for it in
//! rows of text. One character cell is the pixel size of a window over the cell
//! counts of the same window, so the tool divides the one answer by the other.
//!
//! That division is only a measure of a cell when both halves name one
//! terminal, and one whole `winsize` off one file descriptor is how this crate
//! promises that. `termgfx` made the other choice and shows the cost. It read
//! the pixel size from standard output and the cell counts from
//! [`stdout_size`], which reads standard output, then standard error, then
//! standard input. A run whose standard output is a pipe and whose standard
//! error is a terminal therefore divided the pixels of one window by the cells
//! of another, and a run whose standard output is a pipe measured neither and
//! drew at a guess. GitHub issue #350 reports the second of those two.

/// The window of a terminal: its size in character cells, and its size in
/// pixels when the terminal reports one.
///
/// A value of this type exists only for a window that a caller prints into,
/// because [`Window::measured`] refuses a window of no columns and a window of
/// no rows. The type therefore carries that rule for every consumer, and no
/// consumer has to guard against a zero of its own.
///
/// The numbers of one window come off one file descriptor in one ioctl, so
/// they always name one terminal. The documentation of this module says why a
/// tool that divides one of them by the other needs that promise.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Window {
    /// The number of character cells across the window. It is above zero.
    columns: u16,
    /// The number of character cells down the window. It is above zero.
    rows: u16,
    /// The width and the height of the same window in pixels, when the terminal
    /// reports them. Both of the two numbers are above zero.
    pixels: Option<(u16, u16)>,
}

impl Window {
    /// The window that a probe read, when that window is one a caller prints
    /// into.
    ///
    /// Every probe of this crate hands its answer to this one function, so one
    /// rule about what counts as a window binds all of them. The rule is that a
    /// zero is no answer. A window of no columns holds no character of a line,
    /// and a window of no rows holds no line at all, so neither number is a
    /// measure of anything. Both numbers come out of the one ioctl as well, so
    /// a zero in either of them says that the terminal reported no window, and
    /// the number beside the zero is then no more trustworthy than the zero is.
    ///
    /// A pixel size obeys the same rule about a zero, and it fails softer. A
    /// terminal that reports cells and no pixels is still a terminal that a
    /// caller prints text into, so the window stands and the pixel size of it
    /// goes away. A pane of Zellij reports no pixel size, a ttyd panel reports
    /// none, and both of them hold a window of text that a tool must keep
    /// working in.
    ///
    /// # Arguments
    /// * `columns` - The number of character cells across the window.
    /// * `rows` - The number of character cells down the window.
    /// * `pixels` - The width and the height of the same window in pixels, when
    ///   the probe read them.
    ///
    /// # Returns
    /// The window, or `None` when the columns are zero or the rows are zero.
    /// The pixel size of the answer is `None` when the caller gave none, and
    /// `None` when either one of the two pixel numbers is zero.
    #[must_use]
    pub fn measured(columns: u16, rows: u16, pixels: Option<(u16, u16)>) -> Option<Self> {
        if columns == 0 || rows == 0 {
            return None;
        }

        Some(Self {
            columns,
            rows,
            pixels: pixels.filter(|&(wide, tall)| wide > 0 && tall > 0),
        })
    }

    /// The size of the window in character cells: columns, then rows.
    ///
    /// # Returns
    /// The number of columns and then the number of rows. Both are above zero,
    /// because [`Window::measured`] makes no window of a zero.
    #[must_use]
    pub fn cells(self) -> (u16, u16) {
        (self.columns, self.rows)
    }

    /// The size of the same window in pixels, when the terminal reports one.
    ///
    /// # Returns
    /// The width and then the height of the window in pixels, or `None` when
    /// the terminal reports no pixel size. Zellij and ttyd report none.
    #[must_use]
    pub fn pixels(self) -> Option<(u16, u16)> {
        self.pixels
    }
}

/// The size that a probe read, when that size is a window a caller prints
/// into.
///
/// The two probes that answer in character cells alone hand their answer to
/// this one function, so one rule about what counts as a size binds both of
/// them. The rule itself lives in [`Window::measured`], and this function drops
/// the pixel size that a `Window` carries, because neither of those two probes
/// reads one. The workspace therefore holds one statement of the rule.
///
/// The rule lives in one place, and not in each probe, for two reasons. Every
/// probe then answers the same question about its answer, which is what makes
/// the contract of this crate one sentence. And a test reaches the rule with a
/// pair of numbers, where a test of a probe needs a terminal of a size it
/// cannot ask for.
fn measured(size: (u16, u16)) -> Option<(u16, u16)> {
    let (columns, rows) = size;
    Window::measured(columns, rows, None).map(Window::cells)
}

/// The size of the controlling terminal, in columns and then rows.
///
/// The probe reads `/dev/tty`, and it reads standard output when that open
/// fails. A run whose standard output is a pipe therefore still measures the
/// window that started it.
///
/// The answer is `None` when the process holds no terminal to measure, and
/// when the terminal it measured reported a zero. The documentation of this
/// module says why a zero is no answer. The reason that the probe failed goes
/// away with the answer, because a caller has one thing to do with a failed
/// probe, which is to fall back to a width of its own choosing.
#[must_use]
pub fn controlling_size() -> Option<(u16, u16)> {
    #[allow(
        clippy::disallowed_methods,
        reason = "this is the helper the ban points every other caller at; the one call that reads the size of the controlling terminal lives here"
    )]
    let size = crossterm::terminal::size().ok()?;
    measured(size)
}

/// The number of columns of the controlling terminal.
///
/// This is the first number of [`controlling_size`]. Most callers want the
/// columns alone, because the columns say how wide a line of text prints and
/// the rows say how many lines a window holds at once.
#[must_use]
pub fn controlling_columns() -> Option<u16> {
    controlling_size().map(|(columns, _)| columns)
}

/// The size of the terminal that standard output writes to, in columns and
/// then rows.
///
/// The probe reads standard output. It reads standard error and then standard
/// input when standard output is no terminal, so a run that writes its text to
/// a file and its complaints to a window still measures that window.
///
/// The answer is `None` when none of the three is a terminal, and when the
/// terminal it measured reported a zero. The documentation of this module says
/// why a zero is no answer.
#[must_use]
pub fn stdout_size() -> Option<(u16, u16)> {
    #[allow(
        clippy::disallowed_methods,
        reason = "this is the helper the ban points every other caller at; the one call that reads the size of standard output lives here"
    )]
    let (terminal_size::Width(columns), terminal_size::Height(rows)) =
        terminal_size::terminal_size()?;
    measured((columns, rows))
}

/// The number of columns of the terminal that standard output writes to.
///
/// This is the first number of [`stdout_size`].
#[must_use]
pub fn stdout_columns() -> Option<u16> {
    stdout_size().map(|(columns, _)| columns)
}

/// The probe that reads one whole window off one file descriptor.
///
/// The `TIOCGWINSZ` ioctl answers with the columns, the rows and the pixel size
/// of one window, and this module reads all four numbers in one call. A caller
/// that measures a character cell divides the pixels by the cells, and that
/// division only measures a cell when both halves name one terminal.
#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::os::fd::{AsRawFd, OwnedFd, RawFd};

    use crate::Window;

    /// The device that names the controlling terminal of a process.
    const CONTROLLING_TERMINAL: &str = "/dev/tty";

    /// The three standard file descriptors, in the order that [`drawing_window`]
    /// reads them.
    const STANDARD_DESCRIPTORS: [RawFd; 3] =
        [libc::STDOUT_FILENO, libc::STDERR_FILENO, libc::STDIN_FILENO];

    /// The window that one file descriptor reports.
    ///
    /// The probe is the `TIOCGWINSZ` ioctl, which answers with the columns, the
    /// rows and the pixel size of one window. All four numbers come out of the
    /// one call, so the cells and the pixels of an answer always name the same
    /// terminal.
    ///
    /// # Arguments
    /// * `fd` - The file descriptor to measure.
    ///
    /// # Returns
    /// The window, or `None` when the descriptor names no terminal and when the
    /// terminal it names reports no window.
    pub(super) fn window_of(fd: RawFd) -> Option<Window> {
        let mut window = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        #[allow(
            clippy::disallowed_methods,
            reason = "this is the helper the ban points every other caller at; the one call that reads the window of a terminal lives here"
        )]
        // SAFETY: `TIOCGWINSZ` reads the size of the window of a terminal, and
        // it writes that size into the `winsize` that the third argument points
        // at. The pointer names a live local variable of exactly that type, and
        // the call touches no other memory of this process. The call changes
        // nothing about the terminal. A descriptor that names no terminal, and a
        // descriptor that no file is open on, both give an error.
        let answer = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut window) };
        if answer != 0 {
            return None;
        }

        Window::measured(
            window.ws_col,
            window.ws_row,
            Some((window.ws_xpixel, window.ws_ypixel)),
        )
    }

    /// The window of the first descriptor that reports one, and the window of
    /// the controlling terminal when no descriptor of the list reports any.
    ///
    /// The list and the open of the controlling terminal both arrive as
    /// arguments, and they do that for the tests. A test hands this function
    /// descriptors that the test itself made, so no test reads the terminal of
    /// the person who runs `cargo test`, and two runs at the same time cannot
    /// disturb each other.
    ///
    /// The closure runs one time at most, and it runs only when no descriptor
    /// of the list reports a window. An open of `/dev/tty` costs a system call
    /// and a file descriptor, and a run that already measured standard output
    /// needs neither of them.
    ///
    /// # Arguments
    /// * `standard` - The file descriptors to measure, in the order to measure
    ///   them.
    /// * `controlling` - The open of the controlling terminal, which this
    ///   function calls only after every descriptor of `standard` reported
    ///   nothing.
    ///
    /// # Returns
    /// The first window that a descriptor reported, or `None` when no
    /// descriptor reported one and the controlling terminal reported none
    /// either.
    pub(super) fn first_window(
        standard: &[RawFd],
        controlling: impl FnOnce() -> Option<OwnedFd>,
    ) -> Option<Window> {
        for fd in standard {
            if let Some(window) = window_of(*fd) {
                return Some(window);
            }
        }

        let terminal = controlling()?;
        window_of(terminal.as_raw_fd())
    }

    /// The open of the controlling terminal of this process.
    ///
    /// The descriptor arrives as an [`OwnedFd`], so it closes itself when the
    /// caller drops it. A probe that leaked one descriptor for each call would
    /// empty the descriptor table of a long-lived tool.
    ///
    /// # Returns
    /// The controlling terminal, or `None` when the process holds none. A
    /// process with no controlling terminal fails this open with `ENXIO`, and a
    /// job of `cron` and a container are both such a process.
    fn controlling_terminal() -> Option<OwnedFd> {
        File::open(CONTROLLING_TERMINAL).ok().map(OwnedFd::from)
    }

    /// The window that a tool draws its picture into.
    ///
    /// The probe reads standard output, then standard error, then standard
    /// input, and then the controlling terminal. The documentation of this
    /// crate says why that is the order.
    ///
    /// # Returns
    /// The first window that one of those four descriptors reported, or `None`
    /// when none of them holds a window.
    pub(super) fn drawing_window() -> Option<Window> {
        first_window(&STANDARD_DESCRIPTORS, controlling_terminal)
    }
}

/// The window that a tool draws its picture into.
///
/// A tool that draws a picture lays it out in pixels and reserves room for it
/// in rows of text, so it needs the size of one window in both units. This
/// probe reads one whole window off one file descriptor, so the two units of an
/// answer always name one terminal.
///
/// On Unix the probe reads standard output, then standard error, then standard
/// input, and then `/dev/tty`. The documentation of this module says why that
/// is the order and why `/dev/tty` stands at the end of it.
///
/// Every other platform answers from [`stdout_size`] and carries no pixel size.
/// A console of Windows reports no pixel size at all, so that answer is honest
/// rather than short.
///
/// # Returns
/// The window, or `None` when no descriptor of the process holds a terminal
/// with a window. A caller that must draw something whatever the answer picks
/// its own fallback size, and a caller with a second way to draw takes the way
/// that needs no size.
#[must_use]
pub fn drawing_window() -> Option<Window> {
    #[cfg(unix)]
    {
        unix::drawing_window()
    }
    #[cfg(not(unix))]
    {
        let (columns, rows) = stdout_size()?;
        Window::measured(columns, rows, None)
    }
}

#[cfg(test)]
mod tests {
    use super::{measured, Window};

    #[cfg(unix)]
    use std::cell::Cell;
    #[cfg(unix)]
    use std::os::fd::{FromRawFd, OwnedFd, RawFd};
    #[cfg(unix)]
    use std::ptr;

    #[cfg(unix)]
    use super::unix::{first_window, window_of};

    /// The size of a window that a terminal really holds.
    const REAL: (u16, u16) = (80, 24);

    #[test]
    fn a_real_size_comes_back_whole() {
        assert_eq!(
            measured(REAL),
            Some(REAL),
            "a terminal that answers with a window keeps both numbers of it, in the order it gave them"
        );
    }

    #[test]
    fn the_smallest_window_is_still_a_window() {
        assert_eq!(
            measured((1, 1)),
            Some((1, 1)),
            "one column and one row hold one character, so the rule stops at zero and no higher"
        );
    }

    #[test]
    fn a_terminal_of_no_columns_gives_no_answer() {
        assert_eq!(
            measured((0, 24)),
            None,
            "no text prints into zero columns, so a zero column count is no width"
        );
    }

    #[test]
    fn a_terminal_of_no_rows_gives_no_answer() {
        assert_eq!(
            measured((80, 0)),
            None,
            "a window of no rows shows no line, and the column count beside it comes from the same ioctl that reported the zero"
        );
    }

    #[test]
    fn a_terminal_that_reported_nothing_gives_no_answer() {
        assert_eq!(
            measured((0, 0)),
            None,
            "a terminal that carries no window answers the ioctl with two zeros, and the ioctl succeeds"
        );
    }

    /// The width in pixels of the window that the probe tests measure.
    ///
    /// 1600 pixels over 80 columns measures a character cell 20 pixels wide.
    #[cfg(unix)]
    const REAL_WIDTH_PX: u16 = 1600;

    /// The height in pixels of that same window.
    ///
    /// 960 pixels over 24 rows measures a character cell 40 pixels high.
    #[cfg(unix)]
    const REAL_HEIGHT_PX: u16 = 960;

    /// The pixel size of that same window.
    #[cfg(unix)]
    const REAL_PIXELS: (u16, u16) = (REAL_WIDTH_PX, REAL_HEIGHT_PX);

    /// A second window, of a size that no other test names.
    ///
    /// A test that finds this window knows which of two terminals the probe
    /// read, because the two windows share no number.
    #[cfg(unix)]
    const OTHER: (u16, u16) = (132, 43);

    /// The pixel size of that second window.
    #[cfg(unix)]
    const OTHER_PIXELS: (u16, u16) = (2112, 1720);

    /// A pseudo-terminal that a test opens, and that closes itself.
    ///
    /// The master end holds the pseudo-terminal alive and the slave end is the
    /// terminal that a probe measures. A test that leaks a pair of descriptors
    /// for each run empties the descriptor table of the process, and every test
    /// of this file shares one process.
    #[cfg(unix)]
    struct Pty {
        /// The master end. No test reads from it or writes to it.
        master: RawFd,
        /// The slave end. This is the terminal that a probe measures.
        slave: RawFd,
    }

    #[cfg(unix)]
    impl Pty {
        /// Open a pseudo-terminal of a window that the caller names.
        ///
        /// The size arrives with the pseudo-terminal, so no second ioctl sets
        /// it and no window of the wrong size ever exists.
        ///
        /// # Arguments
        /// * `cells` - The columns and the rows of the window.
        /// * `pixels` - The width and the height of the same window in pixels.
        ///
        /// # Returns
        /// A pseudo-terminal that reports that window.
        ///
        /// # Panics
        /// Panics when the system opens no pseudo-terminal.
        fn of_window(cells: (u16, u16), pixels: (u16, u16)) -> Self {
            let (columns, rows) = cells;
            let (pixels_wide, pixels_tall) = pixels;
            Self::open(Some(libc::winsize {
                ws_row: rows,
                ws_col: columns,
                ws_xpixel: pixels_wide,
                ws_ypixel: pixels_tall,
            }))
        }

        /// Open a pseudo-terminal that nobody ever sized.
        ///
        /// Every field of the window of such a terminal is zero, and the ioctl
        /// that reads it succeeds. `script -q /dev/null` makes one.
        ///
        /// # Returns
        /// A pseudo-terminal that reports no window.
        ///
        /// # Panics
        /// Panics when the system opens no pseudo-terminal.
        fn never_sized() -> Self {
            Self::open(None)
        }

        /// Open a pseudo-terminal, with the window that the caller names.
        ///
        /// # Arguments
        /// * `size` - The window of the new pseudo-terminal, or `None` to leave
        ///   the window unset.
        ///
        /// # Returns
        /// The two ends of a new pseudo-terminal.
        ///
        /// # Panics
        /// Panics when the system opens no pseudo-terminal.
        fn open(size: Option<libc::winsize>) -> Self {
            let mut master: RawFd = -1;
            let mut slave: RawFd = -1;
            let mut size = size;
            let size_pointer = size.as_mut().map_or_else(ptr::null_mut, ptr::from_mut);

            // SAFETY: `openpty` writes one file descriptor to each of the first
            // two pointers, and both point at a live local variable. The two
            // null pointers are the documented way to ask for the default
            // terminal modes and to ask for no name of the slave device. The
            // last pointer is either null, which asks for no window, or it
            // points at a live local variable that outlives the call.
            let result = unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    size_pointer,
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

        /// The descriptor of the terminal end.
        ///
        /// # Returns
        /// The slave descriptor, which is the end that reports the window.
        fn terminal(&self) -> RawFd {
            self.slave
        }
    }

    #[cfg(unix)]
    impl Drop for Pty {
        /// Close both ends of the pseudo-terminal.
        fn drop(&mut self) {
            // SAFETY: each descriptor came from the one `openpty` call of
            // [`Pty::open`], nothing else closes them, and `Drop` runs one time.
            unsafe {
                libc::close(self.slave);
                libc::close(self.master);
            }
        }
    }

    /// A pipe that a test opens, and that closes itself.
    ///
    /// A pipe is the descriptor of a captured run. It names no terminal, so the
    /// ioctl on it fails and the probe must move on.
    #[cfg(unix)]
    struct Pipe {
        /// The read end.
        read: RawFd,
        /// The write end.
        write: RawFd,
    }

    #[cfg(unix)]
    impl Pipe {
        /// Open a pipe.
        ///
        /// # Returns
        /// The two ends of a new pipe.
        ///
        /// # Panics
        /// Panics when the system opens no pipe.
        fn open() -> Self {
            let mut ends: [RawFd; 2] = [-1, -1];
            // SAFETY: `pipe` writes two file descriptors into the array that
            // the pointer names, and the array is a live local of exactly two
            // `c_int`. The call reads no other memory.
            let result = unsafe { libc::pipe(ends.as_mut_ptr()) };
            assert_eq!(
                result,
                0,
                "pipe must give two descriptors: {}",
                std::io::Error::last_os_error()
            );

            Pipe {
                read: ends[0],
                write: ends[1],
            }
        }

        /// The descriptor that a probe measures.
        ///
        /// # Returns
        /// The read end of the pipe.
        fn end(&self) -> RawFd {
            self.read
        }
    }

    #[cfg(unix)]
    impl Drop for Pipe {
        /// Close both ends of the pipe.
        fn drop(&mut self) {
            // SAFETY: each descriptor came from the one `pipe` call of
            // [`Pipe::open`], nothing else closes them, and `Drop` runs one
            // time.
            unsafe {
                libc::close(self.read);
                libc::close(self.write);
            }
        }
    }

    /// Copy a descriptor into an [`OwnedFd`], the way an open of `/dev/tty`
    /// gives one.
    ///
    /// The copy stands for the open of the controlling terminal. It closes
    /// itself when `first_window` drops it, which is what the real open does.
    ///
    /// # Arguments
    /// * `fd` - The descriptor to copy.
    ///
    /// # Returns
    /// A new descriptor that names the same terminal.
    ///
    /// # Panics
    /// Panics when the system copies no descriptor.
    #[cfg(unix)]
    fn duplicate(fd: RawFd) -> Option<OwnedFd> {
        // SAFETY: `dup` reads one descriptor of this process and gives a new
        // one that names the same open file. It touches no memory of this
        // process.
        let copy = unsafe { libc::dup(fd) };
        assert!(
            copy >= 0,
            "dup must copy the descriptor: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: `dup` gave this descriptor and nothing else holds it, so the
        // `OwnedFd` becomes its one owner and closes it one time.
        Some(unsafe { OwnedFd::from_raw_fd(copy) })
    }

    #[cfg(unix)]
    #[test]
    fn a_terminal_of_a_real_window_reports_its_cells_and_its_pixels() {
        let pty = Pty::of_window(REAL, REAL_PIXELS);

        let window = window_of(pty.terminal())
            .expect("a pseudo-terminal that reports a window must measure one");

        assert_eq!(
            window.cells(),
            REAL,
            "the ioctl carries the columns and the rows of the window that openpty set"
        );
        assert_eq!(
            window.pixels(),
            Some(REAL_PIXELS),
            "the same ioctl carries the pixel size of that same window"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_terminal_of_no_pixels_keeps_its_cells() {
        // A pane of Zellij reports its columns and its rows and leaves the two
        // pixel fields at zero, and a ttyd panel does the same. Both of them
        // hold a window of text that a tool must keep working in, so the cells
        // stand and the pixel size goes away.
        let pty = Pty::of_window(REAL, (0, 0));

        let window = window_of(pty.terminal())
            .expect("a terminal that reports cells and no pixels still holds a window");

        assert_eq!(
            window.cells(),
            REAL,
            "the columns and the rows of the window stand on their own"
        );
        assert_eq!(
            window.pixels(),
            None,
            "a pixel size of zero measures nothing, so it is no pixel size"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_terminal_that_reports_no_window_measures_nothing() {
        // A pseudo-terminal that nobody ever sized answers the ioctl with four
        // zeros, and the ioctl succeeds. `script -q /dev/null` makes one.
        let pty = Pty::never_sized();

        assert_eq!(
            window_of(pty.terminal()),
            None,
            "zero columns and zero rows are no window, whatever the ioctl says about its own success"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_measures_nothing() {
        let pipe = Pipe::open();

        assert_eq!(
            window_of(pipe.end()),
            None,
            "a pipe names no terminal, so the ioctl fails and the probe has no window"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_standard_descriptor_that_answers_keeps_the_controlling_terminal_shut() {
        let pty = Pty::of_window(REAL, REAL_PIXELS);
        let opened = Cell::new(false);

        let window = first_window(&[pty.terminal()], || {
            opened.set(true);
            None
        })
        .expect("a standard descriptor that names a sized terminal must give its window");

        assert_eq!(
            window.cells(),
            REAL,
            "the answer must come from the descriptor that the tool writes to"
        );
        assert!(
            !opened.get(),
            "an open of /dev/tty costs a system call and a descriptor, so a run that already measured a standard descriptor must not pay for one"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_captured_run_measures_its_controlling_terminal() {
        // This is the shape that GitHub issue #350 reports. A caller captures
        // standard output, so every standard descriptor is a pipe, and the
        // caller still sits at the terminal that the picture appears on.
        let out = Pipe::open();
        let err = Pipe::open();
        let input = Pipe::open();
        let pty = Pty::of_window(OTHER, OTHER_PIXELS);

        let window = first_window(&[out.end(), err.end(), input.end()], || {
            duplicate(pty.terminal())
        })
        .expect("a captured run still holds the terminal that started it");

        assert_eq!(
            window.cells(),
            OTHER,
            "the window must be the one of the controlling terminal, and no other"
        );
        assert_eq!(
            window.pixels(),
            Some(OTHER_PIXELS),
            "the cells and the pixels come off one descriptor, so they name one terminal"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_run_with_no_terminal_at_all_measures_nothing() {
        let out = Pipe::open();
        let err = Pipe::open();
        let input = Pipe::open();

        assert_eq!(
            first_window(&[out.end(), err.end(), input.end()], || None),
            None,
            "a job of cron holds no controlling terminal, and the open of /dev/tty fails with ENXIO"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_standard_terminal_of_no_window_does_not_stop_the_search() {
        // A terminal that measures nothing is not an answer. The search must
        // walk past it to the descriptor that does hold a window.
        let blind = Pty::never_sized();
        let controlling = Pty::of_window(OTHER, OTHER_PIXELS);

        let window = first_window(&[blind.terminal()], || duplicate(controlling.terminal()))
            .expect("a terminal that reports no window must not end the search");

        assert_eq!(
            window.cells(),
            OTHER,
            "the search must reach the controlling terminal, which does hold a window"
        );
    }

    #[test]
    fn a_window_takes_a_zero_as_no_window() {
        assert_eq!(
            Window::measured(0, 24, Some(REAL)),
            None,
            "no character of a line prints into zero columns, so zero columns are no window"
        );
        assert_eq!(
            Window::measured(80, 0, Some(REAL)),
            None,
            "a window of no rows shows no line, so zero rows are no window"
        );
    }

    #[test]
    fn a_window_of_no_pixels_keeps_its_cells() {
        let no_width = Window::measured(REAL.0, REAL.1, Some((0, 960)))
            .expect("a terminal that reports cells holds a window, whatever it says about pixels");
        assert_eq!(no_width.cells(), REAL, "the cells of the window stand");
        assert_eq!(
            no_width.pixels(),
            None,
            "a window of no pixels across measures no cell, so the pixel size goes away whole"
        );

        let no_height = Window::measured(REAL.0, REAL.1, Some((1600, 0)))
            .expect("a terminal that reports cells holds a window, whatever it says about pixels");
        assert_eq!(no_height.cells(), REAL, "the cells of the window stand");
        assert_eq!(
            no_height.pixels(),
            None,
            "a window of no pixels down measures no cell, so the pixel size goes away whole"
        );

        let none = Window::measured(REAL.0, REAL.1, None)
            .expect("a probe that reads no pixel size still reads a window");
        assert_eq!(none.cells(), REAL, "the cells of the window stand");
        assert_eq!(
            none.pixels(),
            None,
            "a probe that read no pixel size reports none"
        );
    }
}

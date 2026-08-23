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
//! # Why two probes stand here
//!
//! The two probes read different file descriptors, and they stay apart for
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

/// The size that a probe read, when that size is a window a caller prints
/// into.
///
/// Both probes hand their answer to this one function, so one rule about what
/// counts as a size binds both of them. The rule is that a zero is no answer.
/// A window of no columns holds no character of a line, and a window of no
/// rows holds no line at all, so neither number is a measure of anything. Both
/// numbers come out of the one ioctl as well, so a zero in either of them says
/// that the terminal reported no window, and the number beside the zero is
/// then no more trustworthy than the zero is.
///
/// The rule lives in one private function, and not in each probe, for two
/// reasons. The two probes then answer the same question about their answers,
/// which is what makes the contract of this crate one sentence. And a test
/// reaches this function with a pair of numbers, where a test of a probe needs
/// a terminal of a size it cannot ask for.
fn measured(size: (u16, u16)) -> Option<(u16, u16)> {
    let (columns, rows) = size;
    if columns == 0 || rows == 0 {
        return None;
    }
    Some(size)
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

#[cfg(test)]
mod tests {
    use super::measured;

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
}

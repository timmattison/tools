//! The one reader of the terminal size.
//!
//! Two crates of this workspace read the size of a terminal, and each of them
//! reads a different file descriptor and answers in a shape of its own. A tool
//! that wants the width of the window it draws in therefore has to know which
//! crate to call, what that crate measures, and what the answer of that crate
//! means. This crate is the one entrance, and it holds those answers in one
//! place.
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

/// The size that a probe read.
///
/// Both probes hand their answer to this one function, so one rule about what
/// counts as a size binds both of them. The rule today takes every answer that
/// a probe gives.
fn measured(size: (u16, u16)) -> Option<(u16, u16)> {
    Some(size)
}

/// The size of the controlling terminal, in columns and then rows.
///
/// The probe reads `/dev/tty`, and it reads standard output when that open
/// fails. A run whose standard output is a pipe therefore still measures the
/// window that started it.
///
/// The answer is `None` when the process holds no terminal to measure. The
/// reason that the probe failed goes away with the answer, because a caller
/// has one thing to do with a failed probe, which is to fall back to a width
/// of its own choosing.
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
/// The answer is `None` when none of the three is a terminal.
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

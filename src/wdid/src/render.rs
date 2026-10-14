//! The report: the text that a person reads, the JSON that a program reads,
//! and the list of the commands that did not parse.

use crate::tally::{Tally, Window};

/// Gives the text report. `width` is the table width. `None` sets no limit.
#[must_use]
pub fn text(tally: &Tally, window: &Window, width: Option<u16>) -> String {
    let _ = (tally, window, width);
    String::new()
}

/// Gives the JSON document, pretty-printed, with a final newline.
#[must_use]
pub fn json(tally: &Tally, window: &Window) -> String {
    let _ = (tally, window);
    String::new()
}

/// Gives the unparsed commands, then the unparsed functions.
#[must_use]
pub fn unparsed(tally: &Tally) -> String {
    let _ = tally;
    String::new()
}

/// Gives the table width.
///
/// `stated` is the value of `COLUMNS`. It wins when it is a positive integer
/// that fits in a `u16`, because POSIX says that this value overrides the width
/// that the system selects. A value of 0 and a text that is not a number are
/// ignored. `terminal` is the width that the terminal reports. It answers when
/// `stated` does not, and only when it is positive. When neither source
/// answers, the result is `None`, and the table has no width limit.
#[must_use]
pub fn table_width(stated: Option<&str>, terminal: Option<u16>) -> Option<u16> {
    stated
        .and_then(|text| text.trim().parse::<u16>().ok())
        .filter(|width| *width > 0)
        .or_else(|| terminal.filter(|width| *width > 0))
}

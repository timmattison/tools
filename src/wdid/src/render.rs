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

/// Gives the table width: `stated` wins, then `terminal`, else `None`.
#[must_use]
pub fn table_width(stated: Option<&str>, terminal: Option<u16>) -> Option<u16> {
    let _ = (stated, terminal);
    None
}

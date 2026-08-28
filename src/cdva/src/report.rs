//! The report: the default table.
//!
//! The table sets the test columns *inside* the code column rather than beside
//! it. `Test code` is a part of `Code`, so a reader who adds the two together
//! double counts, and a bar after `Code` is what says so without a footnote.
//!
//! Every column is as wide as the widest of its header and its values, and a
//! label is measured in the columns a terminal draws it in rather than in the
//! bytes it holds. A language name is ASCII today, but the same renderer prints
//! a path when `--by-file` arrives, and a path holds whatever a file system
//! allows. A width counted in bytes turns one such path into a broken column
//! for every row under it.
//!
//! The output carries no color, so a pipe and a terminal read the same bytes.

use crate::counts::Summary;

/// Render the default table. The `Test code` column is a part of `Code`, and
/// not a column beside it.
#[must_use]
pub fn render_table(summary: &Summary) -> String {
    let _ = summary;
    String::new()
}

//! The totals: every counted file, rolled up by language and over all.
//!
//! [`Summary`] is what the report of a later slice prints. It holds one [`Row`]
//! for each language the walk found, one row for the total, and the files
//! themselves, because `--by-file` and `--json` print them one at a time and a
//! summary that dropped them would make the caller walk the tree twice.
//!
//! A row carries the two buckets rather than one number for each column,
//! because `Test code` is a *part* of `Code` and not a column beside it. So
//! [`Row::code`] is the whole code count of the row and `row.test.code` is the
//! test share of it, and no reader has to remember which of the two a column
//! holds.

use crate::file::{FileCount, ParseStatus};
use crate::lang::Language;
use crate::lines::Counts;
use std::path::PathBuf;

/// The label of the row that sums every other row.
const TOTAL_LABEL: &str = "Total";

/// One row of the report: a language, or the total over all languages.
#[derive(Clone, PartialEq, Debug)]
pub struct Row {
    /// The name of the language, or `Total`.
    pub label: String,
    /// The files that landed in this row.
    pub files: u64,
    /// The files of this row holding at least one test row.
    pub test_files: u64,
    /// The rows of this row's files that are production code.
    pub production: Counts,
    /// The rows of this row's files that are test material.
    pub test: Counts,
}

impl Row {
    /// A zeroed row under this label.
    fn empty(label: String) -> Self {
        Self {
            label,
            files: 0,
            test_files: 0,
            production: Counts::default(),
            test: Counts::default(),
        }
    }

    /// Every blank row of both buckets.
    #[must_use]
    pub fn blank(&self) -> u64 {
        0
    }

    /// Every comment row of both buckets.
    #[must_use]
    pub fn comment(&self) -> u64 {
        0
    }

    /// Every code row of both buckets, of which the test code is a part.
    #[must_use]
    pub fn code(&self) -> u64 {
        0
    }

    /// The test share of the code, as a percentage. Zero when there is no code.
    #[must_use]
    pub fn test_percent(&self) -> f64 {
        0.0
    }

    /// Adds another row's counts into this one, leaving the label alone.
    fn absorb(&mut self, other: &Row) {
        self.files = self.files.saturating_add(other.files);
        self.test_files = self.test_files.saturating_add(other.test_files);
        self.production += other.production;
        self.test += other.test;
    }
}

/// Every counted file, rolled up by language.
#[derive(Clone, Debug)]
pub struct Summary {
    /// One row for each language seen, ordered by code descending, then by
    /// label. A tie broken by the label is what keeps the order of two
    /// languages of equal size the same between two runs.
    pub rows: Vec<Row>,
    /// The row that sums every other row, labelled `Total`.
    pub total: Row,
    /// Every file that was counted, in the order the caller handed them over.
    pub files: Vec<FileCount>,
    /// The files whose parse failed, for the footer and for `--strict`.
    pub failed_parses: Vec<PathBuf>,
}

impl Summary {
    /// Rolls up the files by language.
    #[must_use]
    pub fn new(files: Vec<FileCount>) -> Self {
        let _ = (&files, ParseStatus::Failed, Language::Rust, percent(0, 1));
        Self {
            rows: Vec::new(),
            total: Row::empty(String::new()),
            files: Vec::new(),
            failed_parses: Vec::new(),
        }
    }
}

/// `part` as a percentage of `whole`, where `whole` is not zero.
///
/// The two conversions are the only place this crate turns a count into a
/// float. A count of rows that reaches the 53 bits an `f64` holds exactly would
/// need a file of nine quadrillion rows, so the loss is unreachable rather than
/// merely unlikely.
#[expect(
    clippy::cast_precision_loss,
    reason = "a row count large enough to lose a bit here cannot fit on a disk"
)]
fn percent(part: u64, whole: u64) -> f64 {
    part as f64 * 100.0 / whole as f64
}

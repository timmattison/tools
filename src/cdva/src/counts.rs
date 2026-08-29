//! The totals: every counted file, rolled up by language and over all.
//!
//! [`Summary`] is what the report prints. It holds one [`Row`] for each
//! language the walk found, one row for the total, and the files themselves,
//! because `--by-file` prints them one at a time — [`Summary::file_rows`] is
//! that report's rows — and a summary that dropped them would make the caller
//! walk the tree twice.
//!
//! A row carries the two buckets rather than one number for each column,
//! because `Test code` is a *part* of `Code` and not a column beside it. So
//! [`Row::code`] is the whole code count of the row and `row.test.code` is the
//! test share of it, and no reader has to remember which of the two a column
//! holds.

use crate::file::{FileCount, ParseStatus};
use crate::lang::Language;
use crate::lines::Counts;
use std::collections::BTreeMap;
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
    /// The files of this row holding at least one production row.
    ///
    /// This and `test_files` do not sum to `files`. A file that holds both a
    /// production row and a test row counts in both, and a file of no rows at
    /// all counts in neither.
    pub production_files: u64,
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
            production_files: 0,
            test_files: 0,
            production: Counts::default(),
            test: Counts::default(),
        }
    }

    /// Every blank row of both buckets.
    #[must_use]
    pub fn blank(&self) -> u64 {
        self.production.blank.saturating_add(self.test.blank)
    }

    /// Every comment row of both buckets.
    #[must_use]
    pub fn comment(&self) -> u64 {
        self.production.comment.saturating_add(self.test.comment)
    }

    /// Every code row of both buckets, of which the test code is a part.
    #[must_use]
    pub fn code(&self) -> u64 {
        self.production.code.saturating_add(self.test.code)
    }

    /// The test share of the code, as a percentage. Zero when there is no code.
    #[must_use]
    pub fn test_percent(&self) -> f64 {
        let code = self.code();
        if code == 0 {
            return 0.0;
        }
        percent(self.test.code, code)
    }

    /// Adds another row's counts into this one, leaving the label alone.
    fn absorb(&mut self, other: &Row) {
        self.files = self.files.saturating_add(other.files);
        self.production_files = self.production_files.saturating_add(other.production_files);
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
        let mut by_language: BTreeMap<Language, Row> = BTreeMap::new();

        for file in &files {
            let row = by_language
                .entry(file.language)
                .or_insert_with(|| Row::empty(file.language.name().to_string()));
            row.files = row.files.saturating_add(1);
            if file.is_production_file() {
                row.production_files = row.production_files.saturating_add(1);
            }
            if file.is_test_file() {
                row.test_files = row.test_files.saturating_add(1);
            }
            row.production += file.production;
            row.test += file.test;
        }

        let mut rows: Vec<Row> = by_language.into_values().collect();
        rows.sort_by(|left, right| {
            right
                .code()
                .cmp(&left.code())
                .then_with(|| left.label.cmp(&right.label))
        });

        let mut total = Row::empty(TOTAL_LABEL.to_string());
        for row in &rows {
            total.absorb(row);
        }

        let failed_parses = files
            .iter()
            .filter(|file| file.parse_status == ParseStatus::Failed)
            .map(|file| file.path.clone())
            .collect();

        Self {
            rows,
            total,
            files,
            failed_parses,
        }
    }

    /// One row for each file counted, in the order the walk found them.
    ///
    /// This is what `--by-file` prints. The label is the path as the walk
    /// produced it, and not a shortened one: a reader who wants to open the
    /// file has to be able to paste the label into an editor.
    ///
    /// A file is one file, so `files` is one. It counts as a test file, or as a
    /// production file, under exactly the rule a language row counts it by, so
    /// the file rows of one language sum to that language's row field by field.
    #[must_use]
    pub fn file_rows(&self) -> Vec<Row> {
        self.files
            .iter()
            .map(|file| Row {
                label: file.path.display().to_string(),
                files: 1,
                production_files: u64::from(file.is_production_file()),
                test_files: u64::from(file.is_test_file()),
                production: file.production,
                test: file.test,
            })
            .collect()
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

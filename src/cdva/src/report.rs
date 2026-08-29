//! The report: the table, and the four flags that shape it.
//!
//! The table sets the test columns *inside* the code column rather than beside
//! it. `Test code` is a part of `Code`, so a reader who adds the two together
//! double counts, and a bar after `Code` is what says so without a footnote.
//!
//! [`ReportOptions`] is the whole of the shaping. One rendering reads it rather
//! than five near-copies of a table renderer, because the flags compose: a
//! report of the test bucket alone, one row for each file, ordered by the
//! comment column, trimmed to ten rows, is one report and not a fifth format.
//!
//! Every column is as wide as the widest of its header and its values, and a
//! label is measured in the columns a terminal draws it in rather than in the
//! bytes it holds. A language name is ASCII, but `--by-file` prints a path, and
//! a path holds whatever a file system allows. A width counted in bytes turns
//! one such path into a broken column for every row under it, and a width
//! counted in characters breaks the same column the other way, because one
//! Japanese character draws in two.
//!
//! # The total is the total of the run
//!
//! `--top` hides rows, and a bucket drops the ones that have nothing to say.
//! Neither changes the total, which always covers every file the walk counted.
//! A display flag that changed what a number meant would be a flag nobody could
//! trust: the rows of a trimmed report visibly do not sum to its total, and
//! that is the honest reading — three rows out of two hundred, over the total
//! of the two hundred.
//!
//! The output carries no color, so a pipe and a terminal read the same bytes.
//!
//! # Two reports for a program, and one for a reader
//!
//! [`render_json`] and [`render_csv`] answer the same question the table does,
//! for a reader that is a program. A table leaves out what will not fit; these
//! two leave out nothing, and every row of either carries both buckets *and*
//! their sum, whichever bucket the table was asked to print. The flags that
//! choose *rows* — one row for each file, the column they are ordered by, how
//! many are kept, and which bucket has anything to say — go on choosing them,
//! and all three reports read [`report_rows`] to make that choice once. Two
//! copies of it is how a document and a table of one run come to name a
//! different file as the largest.

use crate::counts::{Row, Summary};
use crate::lines::Counts;
use num_format::{Locale, ToFormattedString};
use serde::Serialize;
use std::cmp::Ordering;
use unicode_width::UnicodeWidthStr;

/// The heading of the first column when each row is a language.
const LANGUAGE_HEADER: &str = "Language";

/// The heading of the first column when each row is a file.
const FILE_HEADER: &str = "File";

/// The headings of the four columns that report the chosen bucket.
const BUCKET_HEADERS: [&str; 4] = ["Files", "Blank", "Comment", "Code"];

/// The headings of the three columns that report the test share, which only a
/// report of both buckets prints.
const TEST_HEADERS: [&str; 3] = ["Test files", "Test code", "Test %"];

/// The index of the first column, which is the only one that aligns left.
const LABEL_COLUMN: usize = 0;

/// The index of the column the bar follows.
const CODE_COLUMN: usize = 4;

/// What separates two columns.
const SEPARATOR: &str = "  ";

/// What follows the `Code` column, before the separator: the mark that says the
/// test columns are a part of the code column and not a column beside it.
const BAR: &str = " |";

/// The glyph the rule lines are drawn with.
const RULE_GLYPH: char = '-';

/// Which bucket the main columns report.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Bucket {
    /// Both buckets: the main columns are the whole file, and the test columns
    /// sit inside them.
    #[default]
    Both,
    /// The test bucket alone.
    TestsOnly,
    /// The production bucket alone.
    ProductionOnly,
}

/// The column a report is ordered by.
///
/// A numeric column orders the largest first, because a reader of a code
/// counter is looking for the big things; the label orders the other way, as a
/// list of names reads. Every order breaks its ties on the label, so the order
/// is total and two runs over one tree print the rows the same way round.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum SortColumn {
    /// The label: the language, or the path of the file.
    Language,
    /// The count of files.
    Files,
    /// The blank rows.
    Blank,
    /// The comment rows.
    Comment,
    /// The code rows, which is what a reader of a code counter asks for first.
    #[default]
    Code,
    /// The count of files holding a test row.
    TestFiles,
    /// The test share of the code, as a count of rows.
    TestCode,
    /// The test share of the code, as a percentage.
    TestPercent,
}

/// What shapes a report: which rows it holds, in which order, and how many.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReportOptions {
    /// One row per file, rather than one row per language.
    pub by_file: bool,
    /// Which bucket the main columns report.
    pub bucket: Bucket,
    /// The column the rows are ordered by.
    pub sort: SortColumn,
    /// Keep only the first N rows. The total still covers every file.
    pub top: Option<usize>,
}

/// Which side of its column a cell sits on.
#[derive(Clone, Copy)]
enum Align {
    /// The cell starts at the left edge of the column, as a name reads.
    Left,
    /// The cell ends at the right edge of the column, as a number reads.
    Right,
}

/// The four numbers one row prints beside its label, under one bucket.
///
/// This is the one place a bucket turns into the numbers of a column, so the
/// cells of a row and the order of the rows read the same four numbers. A
/// second copy of this choice is how a report comes to be ordered by a column
/// it does not print.
#[derive(Clone, Copy)]
struct View {
    /// The files of the row that hold a row of the chosen bucket.
    files: u64,
    /// The blank rows of the chosen bucket.
    blank: u64,
    /// The comment rows of the chosen bucket.
    comment: u64,
    /// The code rows of the chosen bucket.
    code: u64,
}

impl View {
    /// What `row` prints under `bucket`.
    fn of(row: &Row, bucket: Bucket) -> Self {
        match bucket {
            Bucket::Both => Self {
                files: row.files,
                blank: row.blank(),
                comment: row.comment(),
                code: row.code(),
            },
            Bucket::TestsOnly => Self {
                files: row.test_files,
                blank: row.test.blank,
                comment: row.test.comment,
                code: row.test.code,
            },
            Bucket::ProductionOnly => Self {
                files: row.production_files,
                blank: row.production.blank,
                comment: row.production.comment,
                code: row.production.code,
            },
        }
    }
}

/// One row of a report, and the language it belongs to.
///
/// The language is the label itself on a language row, the language of the
/// file on a file row, and nothing at all on the total, which belongs to every
/// language the run found. The table has no column for it, because a language
/// row already prints it and a reader of a by-file table reads the extension.
/// A program cannot: an extension is not a language, and a machine format that
/// left it out would make every consumer of `--by-file` build the same
/// half-right table of suffixes.
struct ReportRow {
    /// The counts of the row, under the label the report prints.
    row: Row,
    /// The language of the row, which only the total is without.
    language: Option<String>,
}

/// The rows of a report, chosen and ordered as `options` says.
///
/// Every report reads this one: the table, the JSON document, and the CSV.
/// Which rows a report holds and what order they come in is one decision, and
/// a second copy of it is how a document and a table of the same run come to
/// disagree about which file is the largest — the kind of disagreement nobody
/// finds, because nobody reads both at once.
///
/// The bucket chooses rows here, and never columns: a report of one bucket
/// drops the rows that have nothing to say about it, and the machine formats
/// still carry every number of the rows that are left.
fn report_rows(summary: &Summary, options: ReportOptions) -> Vec<ReportRow> {
    // `file_rows` maps over `files` one for one and in order, so a file and its
    // row line up by position and neither list needs a key.
    let mut rows: Vec<ReportRow> = if options.by_file {
        summary
            .files
            .iter()
            .zip(summary.file_rows())
            .map(|(file, row)| ReportRow {
                row,
                language: Some(file.language.name().to_string()),
            })
            .collect()
    } else {
        summary
            .rows
            .iter()
            .map(|row| ReportRow {
                language: Some(row.label.clone()),
                row: row.clone(),
            })
            .collect()
    };
    rows.retain(|row| !is_empty(&row.row, options.bucket));
    rows.sort_by(|left, right| compare(&left.row, &right.row, options));
    if let Some(top) = options.top {
        rows.truncate(top);
    }
    rows
}

/// Render the table, as `options` shapes it. The `Test code` column is a part
/// of `Code`, and not a column beside it.
///
/// A summary of no rows still prints a header, one rule, and a zeroed total,
/// because the shape of the answer is a part of the answer: a reader who counted
/// an empty tree learns more from a table of zeros than from an empty screen.
/// A report trimmed to no rows at all prints the same three lines, for the same
/// reason.
#[must_use]
pub fn render_table(summary: &Summary, options: ReportOptions) -> String {
    let rows = report_rows(summary, options);

    let header = headers(options);
    let body: Vec<Vec<String>> = rows
        .iter()
        .map(|row| cells(&row.row, options.bucket))
        .collect();
    let total = cells(&summary.total, options.bucket);

    let widths = widths(&header, &body, &total);
    let header_line = line(&header, &widths);
    let rule: String = std::iter::repeat_n(RULE_GLYPH, header_line.width()).collect();

    let mut table = String::new();
    push_line(&mut table, &header_line);
    push_line(&mut table, &rule);
    for row in &body {
        push_line(&mut table, &line(row, &widths));
    }
    // With no row between them the two rules would sit on top of one another,
    // and one line of dashes says everything two of them would.
    if !body.is_empty() {
        push_line(&mut table, &rule);
    }
    push_line(&mut table, &line(&total, &widths));
    table
}

/// Whether this row has nothing to say about the chosen bucket.
///
/// A report of one bucket drops such a row rather than printing it as a row of
/// zeros: a language with no test code says nothing in a test-only report, and
/// a screen of zeroed rows buries the languages that do have something to say.
///
/// A report of both buckets drops nothing. A row of blank rows alone is still a
/// row of the tree, and the default table has always printed it.
fn is_empty(row: &Row, bucket: Bucket) -> bool {
    match bucket {
        Bucket::Both => false,
        Bucket::TestsOnly => row.test.total() == 0,
        Bucket::ProductionOnly => row.production.total() == 0,
    }
}

/// Which of `left` and `right` the report prints first.
///
/// The label breaks every tie, so the order is total: two rows that tie on the
/// chosen column keep the same order between two runs, and a report is
/// reproducible.
fn compare(left: &Row, right: &Row, options: ReportOptions) -> Ordering {
    let (near, far) = (
        View::of(left, options.bucket),
        View::of(right, options.bucket),
    );
    // The larger row comes first, so a numeric column reads the arguments the
    // other way round. The label reads them this way round, as a name does.
    let ordering = match options.sort {
        SortColumn::Language => left.label.cmp(&right.label),
        SortColumn::Files => far.files.cmp(&near.files),
        SortColumn::Blank => far.blank.cmp(&near.blank),
        SortColumn::Comment => far.comment.cmp(&near.comment),
        SortColumn::Code => far.code.cmp(&near.code),
        SortColumn::TestFiles => right.test_files.cmp(&left.test_files),
        SortColumn::TestCode => right.test.code.cmp(&left.test.code),
        SortColumn::TestPercent => right.test_percent().total_cmp(&left.test_percent()),
    };
    ordering.then_with(|| left.label.cmp(&right.label))
}

/// The heading of each column, in the order the columns print.
fn headers(options: ReportOptions) -> Vec<String> {
    let label = if options.by_file {
        FILE_HEADER
    } else {
        LANGUAGE_HEADER
    };
    let mut headers: Vec<String> = std::iter::once(label)
        .chain(BUCKET_HEADERS)
        .map(str::to_string)
        .collect();
    if options.bucket == Bucket::Both {
        headers.extend(TEST_HEADERS.iter().copied().map(str::to_string));
    }
    headers
}

/// The cells of one row, as the table prints them.
///
/// Under [`Bucket::Both`] the counts of the two buckets are added here rather
/// than by the caller, because `Code` is then the whole code of the row and
/// `Test code` is the test share of that same number. A column that read one
/// bucket alone would print two numbers a reader would add together and double
/// count.
///
/// Under one bucket the three test columns are gone. `Code` *is* the test code
/// there, so a `Test code` column beside it would print one number twice, and a
/// `Test %` column would print one hundred for every row of a test-only report.
fn cells(row: &Row, bucket: Bucket) -> Vec<String> {
    let view = View::of(row, bucket);
    let mut cells = vec![
        row.label.clone(),
        thousands(view.files),
        thousands(view.blank),
        thousands(view.comment),
        thousands(view.code),
    ];
    if bucket == Bucket::Both {
        cells.push(thousands(row.test_files));
        cells.push(thousands(row.test.code));
        cells.push(format!("{:.1}%", row.test_percent()));
    }
    cells
}

/// A count, with a separator every three digits.
fn thousands(count: u64) -> String {
    count.to_formatted_string(&Locale::en)
}

/// The width of each column: the widest of its header and its values.
fn widths(header: &[String], body: &[Vec<String>], total: &[String]) -> Vec<usize> {
    let mut widths = vec![0_usize; header.len()];
    for row in std::iter::once(header)
        .chain(body.iter().map(Vec::as_slice))
        .chain(std::iter::once(total))
    {
        for (width, cell) in widths.iter_mut().zip(row.iter()) {
            *width = (*width).max(cell.width());
        }
    }
    widths
}

/// One row of the table, padded column by column.
///
/// The bar follows the code column, and a report that prints no column after it
/// prints no bar: the bar says the columns behind it are a part of the code
/// column, and there is nothing behind it to say that about.
fn line(row: &[String], widths: &[usize]) -> String {
    let mut line = String::new();
    for (index, (cell, width)) in row.iter().zip(widths.iter()).enumerate() {
        if index > 0 {
            if index == CODE_COLUMN + 1 {
                line.push_str(BAR);
            }
            line.push_str(SEPARATOR);
        }
        let align = if index == LABEL_COLUMN {
            Align::Left
        } else {
            Align::Right
        };
        line.push_str(&pad(cell, *width, align));
    }
    line
}

/// `cell`, padded with spaces to `width` columns of a terminal.
///
/// A cell wider than its column is left alone rather than cut. The widths come
/// from the cells themselves, so that cannot happen here; a cut would be a
/// silent lie about the contents, and a wide line is one a reader can see.
fn pad(cell: &str, width: usize, align: Align) -> String {
    let filler: String = std::iter::repeat_n(' ', width.saturating_sub(cell.width())).collect();
    match align {
        Align::Left => format!("{cell}{filler}"),
        Align::Right => format!("{filler}{cell}"),
    }
}

/// Adds one line to the table, and the break that ends it.
fn push_line(table: &mut String, line: &str) {
    table.push_str(line);
    table.push('\n');
}

/// One row of a machine format: every number the tool knows about that row.
///
/// This is a type of its own rather than a `Serialize` on [`Row`], because
/// these names are a contract with a program somebody else wrote. A field
/// renamed inside this crate is a refactor; a key renamed in this document is
/// a broken script, and the two must not be the same edit.
///
/// `blank`, `comment`, and `code` are the whole row, which is `production`
/// plus `test` field by field. They are carried rather than left to be
/// derived, because deriving them is exactly the arithmetic a consumer gets
/// wrong — and a consumer that subtracts one bucket from the whole to reach
/// the other gets it wrong in a way that still looks like a number.
#[derive(Serialize)]
struct Record {
    /// The language, or the path of the file, or `Total`.
    label: String,
    /// The language of the row. The total names none, and carries no key at
    /// all rather than a null: a key that is sometimes null is a key every
    /// consumer has to test before it can read.
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    /// Every file of the row.
    files: u64,
    /// The files of the row holding at least one production row.
    production_files: u64,
    /// The files of the row holding at least one test row.
    test_files: u64,
    /// The blank rows of both buckets.
    blank: u64,
    /// The comment rows of both buckets.
    comment: u64,
    /// The code rows of both buckets, of which the test code is a part.
    code: u64,
    /// The production bucket.
    production: Counts,
    /// The test bucket.
    test: Counts,
    /// The test share of the code, as a percentage rounded to one decimal.
    test_percent: f64,
}

/// The whole JSON report.
#[derive(Serialize)]
struct Document {
    /// The rows the report holds, in the order it holds them.
    rows: Vec<Record>,
    /// The total, which covers every file the run counted whatever `--top` or
    /// a bucket left out of the rows above.
    total: Record,
    /// The files whose parse failed. Always present, and empty when none did:
    /// a consumer that has to tell an absent key from an empty list is a
    /// consumer with two code paths where one would do.
    failed_parses: Vec<String>,
}

/// The header of the CSV, which is the order of every record under it.
const CSV_HEADERS: [&str; 15] = [
    "label",
    "language",
    "files",
    "production_files",
    "test_files",
    "blank",
    "comment",
    "code",
    "production_blank",
    "production_comment",
    "production_code",
    "test_blank",
    "test_comment",
    "test_code",
    "test_percent",
];

/// The empty field the total carries where every other record names a
/// language.
const NO_LANGUAGE: &str = "";

/// Render the report as one JSON document, pretty-printed and ended by a
/// break.
///
/// Every row carries the full breakdown, whatever `--tests-only` or
/// `--production-only` asked the table for: those flags choose *rows*, and a
/// machine format that dropped a column would make its reader re-derive the
/// number by subtraction. The row flags — one row for each file, the column
/// the rows are ordered by, and how many are kept — shape this report exactly
/// as they shape the table, because both read [`report_rows`].
///
/// # Panics
///
/// Never, in practice. `serde_json` refuses a map whose keys are not strings
/// and a float that is not a number, and this document holds neither: every
/// key is a field name, and its one float is a share of two counts, which is
/// zero when there is no code to take a share of.
#[must_use]
pub fn render_json(summary: &Summary, options: ReportOptions) -> String {
    let document = Document {
        rows: report_rows(summary, options).iter().map(record).collect(),
        total: record(&ReportRow {
            row: summary.total.clone(),
            language: None,
        }),
        failed_parses: summary
            .failed_parses
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
    };

    let mut json = serde_json::to_string_pretty(&document)
        .expect("a document of counts under field names always serializes");
    json.push('\n');
    json
}

/// Render the report as CSV: the header, one record for each row, and the
/// total last.
///
/// The columns are the keys of the JSON document flattened, so the two reports
/// carry the same numbers under the same names. The share of the test code
/// carries no percent sign, because a `%` inside a field is a glyph every
/// consumer has to strip before it can read a number.
///
/// # Panics
///
/// Never, in practice. The records go into a vector in memory, which answers
/// every write, and every field is a `String`, so the bytes that come back out
/// are the UTF-8 that went in.
#[must_use]
pub fn render_csv(summary: &Summary, options: ReportOptions) -> String {
    let bytes =
        csv_bytes(summary, options).expect("a writer over a vector in memory answers every write");
    String::from_utf8(bytes).expect("every field of every record is a String")
}

/// The CSV, as the bytes the writer produced.
fn csv_bytes(summary: &Summary, options: ReportOptions) -> Result<Vec<u8>, csv::Error> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(CSV_HEADERS)?;
    for row in report_rows(summary, options) {
        writer.write_record(csv_fields(&record(&row)))?;
    }
    writer.write_record(csv_fields(&record(&ReportRow {
        row: summary.total.clone(),
        language: None,
    })))?;
    writer
        .into_inner()
        .map_err(|error| csv::Error::from(error.into_error()))
}

/// The fields of one CSV record, in the order [`CSV_HEADERS`] names them.
///
/// The two are arrays of one length, so a column added to one and forgotten in
/// the other does not compile.
fn csv_fields(record: &Record) -> [String; CSV_HEADERS.len()] {
    [
        record.label.clone(),
        record
            .language
            .clone()
            .unwrap_or_else(|| NO_LANGUAGE.to_string()),
        record.files.to_string(),
        record.production_files.to_string(),
        record.test_files.to_string(),
        record.blank.to_string(),
        record.comment.to_string(),
        record.code.to_string(),
        record.production.blank.to_string(),
        record.production.comment.to_string(),
        record.production.code.to_string(),
        record.test.blank.to_string(),
        record.test.comment.to_string(),
        record.test.code.to_string(),
        format!("{:.1}", record.test_percent),
    ]
}

/// Every number one row of a machine format carries.
fn record(row: &ReportRow) -> Record {
    Record {
        label: row.row.label.clone(),
        language: row.language.clone(),
        files: row.row.files,
        production_files: row.row.production_files,
        test_files: row.row.test_files,
        blank: row.row.blank(),
        comment: row.row.comment(),
        code: row.row.code(),
        production: row.row.production,
        test: row.row.test,
        test_percent: rounded_percent(&row.row),
    }
}

/// The test share of `row`, rounded to the one decimal the table prints.
///
/// The rounding runs through the same `{:.1}` the table formats with, rather
/// than through arithmetic. Multiplying by ten, rounding, and dividing is a
/// *second* rounding of a number that is already not the decimal it looks
/// like, and the two part company at a half: a fifteen-hundredth share prints
/// as `0.1` and rounds arithmetically to `0.2`. A consumer reading `0.2` out
/// of the document while the table printed `0.1%` has found a disagreement
/// inside a tool whose whole job is to count.
///
/// A string that `{:.1}` produced always reads back as a number, so the
/// fallback is unreachable; it is there so that this stays a total function
/// rather than a panic waiting on an input nobody has thought of.
fn rounded_percent(row: &Row) -> f64 {
    let percent = row.test_percent();
    format!("{percent:.1}").parse().unwrap_or(percent)
}

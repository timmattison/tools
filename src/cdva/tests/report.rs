//! The default table, read through the public API.
//!
//! The first test is the specification of the format. Every other test in this
//! file asserts one rule of it that a golden string alone would not pin: a rule
//! that only shows itself on data the golden table does not hold, such as a
//! seven-figure count, a row of no code, or a label that is not ASCII.
//!
//! Every summary here is built by hand rather than counted from a tree. The
//! renderer is what these tests cover, so a fixture tree would put the walk and
//! the classifier between the assertion and the thing it asserts, and a failure
//! anywhere in that chain would read as a failure of the table. Nothing here
//! touches the file system, so two copies of this file running at once share
//! nothing at all.

use cdva::{render_table, Counts, Row, Summary};
use unicode_width::UnicodeWidthStr;

/// The table of the two-language summary below, byte for byte.
///
/// The alignment a reader sees here is the alignment the tool prints. Each
/// column is as wide as the widest of its header and its values, two spaces
/// separate two columns, and a bar follows `Code` so the eye reads the test
/// columns as a part of it.
const GOLDEN_TABLE: &str = r"Language    Files   Blank  Comment    Code |  Test files  Test code  Test %
---------------------------------------------------------------------------
Rust          412   9,118    6,204  61,330 |         188     24,905   40.6%
TypeScript     77   1,204      812   9,441 |          31      3,120   33.0%
---------------------------------------------------------------------------
Total         489  10,322    7,016  70,771 |         219     28,025   39.6%
";

/// The lines a table of two language rows prints: a header, a rule, the two
/// rows, a rule, and the total.
const TWO_ROW_LINES: usize = 6;

/// One row, spelled out.
fn row(label: &str, files: u64, test_files: u64, production: Counts, test: Counts) -> Row {
    Row {
        label: label.to_string(),
        files,
        test_files,
        production,
        test,
    }
}

/// A summary of these rows and this total, holding no files.
///
/// The files themselves are what `--by-file` prints in a later slice. The table
/// reads the rows alone, so a summary of no files still renders every column.
fn summary(rows: Vec<Row>, total: Row) -> Summary {
    Summary {
        rows,
        total,
        files: Vec::new(),
        failed_parses: Vec::new(),
    }
}

/// The counts of one bucket.
const fn counts(blank: u64, comment: u64, code: u64) -> Counts {
    Counts {
        blank,
        comment,
        code,
    }
}

/// The summary the golden table renders: two languages and their total.
fn two_languages() -> Summary {
    summary(
        vec![
            row(
                "Rust",
                412,
                188,
                counts(6_000, 4_000, 36_425),
                counts(3_118, 2_204, 24_905),
            ),
            row(
                "TypeScript",
                77,
                31,
                counts(900, 600, 6_321),
                counts(304, 212, 3_120),
            ),
        ],
        row(
            "Total",
            489,
            219,
            counts(6_900, 4_600, 42_746),
            counts(3_422, 2_416, 28_025),
        ),
    )
}

/// The fields of the line that starts with `label`, with the bar dropped.
fn fields(table: &str, label: &str) -> Vec<String> {
    let line = table
        .lines()
        .find(|line| line.starts_with(label))
        .unwrap_or_else(|| panic!("the table holds a row for `{label}`:\n{table}"));
    line.split_whitespace()
        .filter(|field| *field != "|")
        .map(str::to_string)
        .collect()
}

/// The display width of every line of a table of two rows.
///
/// The count of the lines is asserted here rather than at each call site,
/// because a table of no lines makes every claim about the width of its lines
/// true and says nothing.
fn widths_of_two_rows(table: &str) -> Vec<usize> {
    let widths: Vec<usize> = table.lines().map(UnicodeWidthStr::width).collect();
    assert_eq!(
        widths.len(),
        TWO_ROW_LINES,
        "a table of two rows prints a header, a rule, both rows, a rule, and the total:\n{table}"
    );
    widths
}

#[test]
fn the_default_table_renders_exactly_this_way() {
    assert_eq!(
        render_table(&two_languages()),
        GOLDEN_TABLE,
        "the default table is the format this string spells out"
    );
}

#[test]
fn a_seven_figure_count_carries_every_thousands_separator() {
    let table = render_table(&summary(
        vec![row(
            "Rust",
            9_000,
            0,
            counts(0, 0, 1_234_567),
            counts(0, 0, 0),
        )],
        row("Total", 9_000, 0, counts(0, 0, 1_234_567), counts(0, 0, 0)),
    ));

    assert_eq!(
        fields(&table, "Rust"),
        vec!["Rust", "9,000", "0", "0", "1,234,567", "0", "0", "0.0%"],
        "a count over six figures carries a separator every three digits"
    );
}

#[test]
fn the_test_share_prints_one_decimal_and_a_row_of_no_code_prints_a_zero() {
    let table = render_table(&summary(
        vec![
            row("Rust", 2, 1, counts(0, 0, 7), counts(0, 0, 3)),
            row("Markdown", 1, 0, counts(4, 0, 0), counts(0, 0, 0)),
        ],
        row("Total", 3, 1, counts(4, 0, 7), counts(0, 0, 3)),
    ));

    assert_eq!(
        fields(&table, "Rust")[7],
        "30.0%",
        "the test share prints one decimal and a trailing percent sign"
    );
    assert_eq!(
        fields(&table, "Markdown")[7],
        "0.0%",
        "a row of no code prints a zero rather than a NaN"
    );
}

#[test]
fn a_long_label_widens_the_first_column_and_every_line_keeps_its_width() {
    let long = "AnExtremelyLongLanguageName";
    let table = render_table(&summary(
        vec![
            row(long, 1, 0, counts(1, 1, 1), counts(0, 0, 0)),
            row("Go", 1, 1, counts(0, 0, 0), counts(1, 1, 1)),
        ],
        row("Total", 2, 1, counts(1, 1, 1), counts(1, 1, 1)),
    ));

    let widths = widths_of_two_rows(&table);
    assert!(
        widths.windows(2).all(|pair| pair[0] == pair[1]),
        "every line of the table is the same width:\n{table}"
    );
    assert!(
        widths[0] > long.width(),
        "the table is wider than the label that widened its first column:\n{table}"
    );
}

#[test]
fn a_summary_of_no_files_still_prints_a_header_a_rule_and_a_total() {
    let table = render_table(&summary(
        Vec::new(),
        row("Total", 0, 0, counts(0, 0, 0), counts(0, 0, 0)),
    ));

    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "a summary of no rows prints a header, one rule, and the total:\n{table}"
    );
    assert!(
        lines[0].starts_with("Language") && lines[0].ends_with("Test %"),
        "the first line is the header:\n{table}"
    );
    assert!(
        !lines[1].is_empty() && lines[1].chars().all(|glyph| glyph == '-'),
        "the second line is the rule:\n{table}"
    );
    assert_eq!(
        fields(&table, "Total"),
        vec!["Total", "0", "0", "0", "0", "0", "0", "0.0%"],
        "the total of no files is zero in every column"
    );
}

#[test]
fn a_label_of_multi_byte_characters_keeps_the_columns_aligned() {
    let table = render_table(&summary(
        vec![
            row("日本語", 1, 0, counts(1, 1, 1), counts(0, 0, 0)),
            row("Go", 1, 1, counts(0, 0, 0), counts(1, 1, 1)),
        ],
        row("Total", 2, 1, counts(1, 1, 1), counts(1, 1, 1)),
    ));

    let widths = widths_of_two_rows(&table);
    assert!(
        widths.windows(2).all(|pair| pair[0] == pair[1]),
        "a label of multi-byte characters is measured in columns, not in bytes:\n{table}"
    );
}

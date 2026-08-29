//! The table, read through the public API, under every flag that shapes it.
//!
//! The first test is the specification of the default format, and three more
//! golden strings spell out the three shapes the flags reach: one row for each
//! file, the test bucket alone, and the production bucket alone. Every other
//! test in this file asserts one rule that a golden string alone would not pin:
//! a rule that only shows itself on data the golden table does not hold, such
//! as a seven-figure count, a row of no code, a tie between two rows, or a
//! label that is not ASCII.
//!
//! Every summary here is built by hand rather than counted from a tree. The
//! renderer is what these tests cover, so a fixture tree would put the walk and
//! the classifier between the assertion and the thing it asserts, and a failure
//! anywhere in that chain would read as a failure of the table. Nothing here
//! touches the file system, so two copies of this file running at once share
//! nothing at all.

use cdva::{
    render_table, Bucket, Counts, FileCount, Language, ParseStatus, ReportOptions, Row, SortColumn,
    Summary,
};
use std::path::PathBuf;
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

/// The table of the two files below under `--by-file`, byte for byte.
///
/// The first column reads `File` rather than `Language`, and holds the path of
/// the file as the walk produced it. Every other column reads exactly as it
/// does in the default table, because a file row and a language row are the
/// same row over a different set of files.
const GOLDEN_BY_FILE_TABLE: &str = r"File         Files  Blank  Comment  Code |  Test files  Test code  Test %
-------------------------------------------------------------------------
src/lib.rs       1     12        6   150 |           1         30   20.0%
tests/it.rs      1      4        2    44 |           1         44  100.0%
-------------------------------------------------------------------------
Total            2     16        8   194 |           2         74   38.1%
";

/// The table of the two-language summary under `--tests-only`, byte for byte.
///
/// The three test columns are gone, and so is the bar that marked them as a
/// part of `Code`: in this report `Code` *is* the test code, and a `Test code`
/// column beside it would print the same number twice.
const GOLDEN_TESTS_ONLY_TABLE: &str = r"Language    Files  Blank  Comment    Code
-----------------------------------------
Rust          188  3,118    2,204  24,905
TypeScript     31    304      212   3,120
-----------------------------------------
Total         219  3,422    2,416  28,025
";

/// The table of the two-language summary under `--production-only`, byte for
/// byte. The mirror of the table above, over the other bucket.
const GOLDEN_PRODUCTION_ONLY_TABLE: &str = r"Language    Files  Blank  Comment    Code
-----------------------------------------
Rust          400  6,000    4,000  36,425
TypeScript     70    900      600   6,321
-----------------------------------------
Total         470  6,900    4,600  42,746
";

/// The lines a table of two language rows prints: a header, a rule, the two
/// rows, a rule, and the total.
const TWO_ROW_LINES: usize = 6;

/// The glyph a rule line is drawn with, which no label ever starts with.
const RULE_GLYPH: char = '-';

/// One row, spelled out.
///
/// `files` is the three file counts of the row, in the order `[every file, the
/// files holding production code, the files holding test code]`. The three are
/// separate numbers rather than one and a difference, because a file that holds
/// both a production row and a test row counts in two of them.
fn row(label: &str, files: [u64; 3], production: Counts, test: Counts) -> Row {
    Row {
        label: label.to_string(),
        files: files[0],
        production_files: files[1],
        test_files: files[2],
        production,
        test,
    }
}

/// A summary of these rows and this total, holding no files.
///
/// The files themselves are what `--by-file` prints, and the tests of that flag
/// build their summaries out of files instead. The table of languages reads the
/// rows alone, so a summary of no files still renders every column.
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

/// One counted file, under a path the report prints as the row's label.
fn counted(path: &str, language: Language, production: Counts, test: Counts) -> FileCount {
    FileCount {
        path: PathBuf::from(path),
        language,
        production,
        test,
        spans: Vec::new(),
        parse_status: ParseStatus::NotParsed,
        test_mod_declarations: Vec::new(),
    }
}

/// The options of the default report, with `mutate` applied to them.
fn options(mutate: impl FnOnce(&mut ReportOptions)) -> ReportOptions {
    let mut options = ReportOptions::default();
    mutate(&mut options);
    options
}

/// The summary the golden table renders: two languages and their total.
fn two_languages() -> Summary {
    summary(
        vec![
            row(
                "Rust",
                [412, 400, 188],
                counts(6_000, 4_000, 36_425),
                counts(3_118, 2_204, 24_905),
            ),
            row(
                "TypeScript",
                [77, 70, 31],
                counts(900, 600, 6_321),
                counts(304, 212, 3_120),
            ),
        ],
        row(
            "Total",
            [489, 470, 219],
            counts(6_900, 4_600, 42_746),
            counts(3_422, 2_416, 28_025),
        ),
    )
}

/// The summary the by-file golden table renders: a library that holds a test
/// module, and a test file beside it. The rows and the total are rolled up by
/// the library, so the total of a by-file report is the total of the run.
fn two_files() -> Summary {
    Summary::new(vec![
        counted(
            "src/lib.rs",
            Language::Rust,
            counts(10, 5, 120),
            counts(2, 1, 30),
        ),
        counted(
            "tests/it.rs",
            Language::Rust,
            Counts::default(),
            counts(4, 2, 44),
        ),
    ])
}

/// Four rows that order differently under every column of the report.
///
/// The rows are built in an order that no column produces, so a renderer that
/// ignored the flag could not pass one of the orders below by accident. Two
/// pairs tie: `Ada` and `Zig` on the code, and `Go` and `Rust` on the files, so
/// the tie-break on the label is exercised rather than assumed.
fn sortable() -> Summary {
    summary(
        vec![
            row("Zig", [3, 3, 1], counts(1, 9, 30), Counts::default()),
            row("Rust", [10, 10, 4], counts(5, 5, 100), counts(1, 1, 50)),
            row("Ada", [3, 3, 0], counts(9, 1, 30), Counts::default()),
            row("Go", [10, 8, 4], counts(7, 2, 20), counts(2, 2, 100)),
        ],
        row("Total", [26, 24, 9], counts(22, 17, 180), counts(3, 3, 150)),
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

/// The label of every row the table prints, the total last.
///
/// The header and the rules are dropped, so the result is the order of the
/// report itself. The total is kept rather than dropped, because "the total
/// stays at the foot" is exactly what these tests assert.
fn labels(table: &str) -> Vec<String> {
    table
        .lines()
        .skip(1)
        .filter(|line| !line.starts_with(RULE_GLYPH))
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

/// The labels the four rows of [`sortable`] print under this column.
fn order(sort: SortColumn) -> Vec<String> {
    labels(&render_table(&sortable(), options(|it| it.sort = sort)))
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
        render_table(&two_languages(), ReportOptions::default()),
        GOLDEN_TABLE,
        "the default table is the format this string spells out"
    );
}

#[test]
fn the_by_file_table_renders_exactly_this_way() {
    assert_eq!(
        render_table(&two_files(), options(|it| it.by_file = true)),
        GOLDEN_BY_FILE_TABLE,
        "one row for each file, under a first column that reads `File`"
    );
}

#[test]
fn the_tests_only_table_renders_exactly_this_way() {
    assert_eq!(
        render_table(
            &two_languages(),
            options(|it| it.bucket = Bucket::TestsOnly)
        ),
        GOLDEN_TESTS_ONLY_TABLE,
        "the main columns report the test bucket, and the test columns are gone"
    );
}

#[test]
fn the_production_only_table_renders_exactly_this_way() {
    assert_eq!(
        render_table(
            &two_languages(),
            options(|it| it.bucket = Bucket::ProductionOnly)
        ),
        GOLDEN_PRODUCTION_ONLY_TABLE,
        "the main columns report the production bucket, and the test columns are gone"
    );
}

#[test]
fn a_seven_figure_count_carries_every_thousands_separator() {
    let table = render_table(
        &summary(
            vec![row(
                "Rust",
                [9_000, 9_000, 0],
                counts(0, 0, 1_234_567),
                Counts::default(),
            )],
            row(
                "Total",
                [9_000, 9_000, 0],
                counts(0, 0, 1_234_567),
                Counts::default(),
            ),
        ),
        ReportOptions::default(),
    );

    assert_eq!(
        fields(&table, "Rust"),
        vec!["Rust", "9,000", "0", "0", "1,234,567", "0", "0", "0.0%"],
        "a count over six figures carries a separator every three digits"
    );
}

#[test]
fn the_test_share_prints_one_decimal_and_a_row_of_no_code_prints_a_zero() {
    let table = render_table(
        &summary(
            vec![
                row("Rust", [2, 1, 1], counts(0, 0, 7), counts(0, 0, 3)),
                row("Markdown", [1, 1, 0], counts(4, 0, 0), Counts::default()),
            ],
            row("Total", [3, 2, 1], counts(4, 0, 7), counts(0, 0, 3)),
        ),
        ReportOptions::default(),
    );

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
    let table = render_table(
        &summary(
            vec![
                row(long, [1, 1, 0], counts(1, 1, 1), Counts::default()),
                row("Go", [1, 0, 1], Counts::default(), counts(1, 1, 1)),
            ],
            row("Total", [2, 1, 1], counts(1, 1, 1), counts(1, 1, 1)),
        ),
        ReportOptions::default(),
    );

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
    let table = render_table(
        &summary(
            Vec::new(),
            row("Total", [0, 0, 0], Counts::default(), Counts::default()),
        ),
        ReportOptions::default(),
    );

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
        !lines[1].is_empty() && lines[1].chars().all(|glyph| glyph == RULE_GLYPH),
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
    let table = render_table(
        &summary(
            vec![
                row("日本語", [1, 1, 0], counts(1, 1, 1), Counts::default()),
                row("Go", [1, 0, 1], Counts::default(), counts(1, 1, 1)),
            ],
            row("Total", [2, 1, 1], counts(1, 1, 1), counts(1, 1, 1)),
        ),
        ReportOptions::default(),
    );

    let widths = widths_of_two_rows(&table);
    assert!(
        widths.windows(2).all(|pair| pair[0] == pair[1]),
        "a label of multi-byte characters is measured in columns, not in bytes:\n{table}"
    );
}

#[test]
fn a_file_row_of_multi_byte_characters_keeps_every_line_the_same_width() {
    let path = "src/日本語/計算.rs";
    let table = render_table(
        &Summary::new(vec![
            counted(path, Language::Rust, counts(1, 1, 40), Counts::default()),
            counted(
                "src/it.rs",
                Language::Rust,
                Counts::default(),
                counts(1, 1, 20),
            ),
        ]),
        options(|it| it.by_file = true),
    );

    assert!(
        table.contains(path),
        "a file row is labelled with the path of the file:\n{table}"
    );
    let widths = widths_of_two_rows(&table);
    assert!(
        widths.windows(2).all(|pair| pair[0] == pair[1]),
        "a path of multi-byte characters is measured in columns, not in bytes:\n{table}"
    );
}

#[test]
fn sorting_by_language_reads_the_labels_upwards() {
    assert_eq!(
        order(SortColumn::Language),
        ["Ada", "Go", "Rust", "Zig", "Total"],
        "a list of names reads upwards, and the total stays at the foot"
    );
}

#[test]
fn sorting_by_files_puts_the_largest_first_and_breaks_both_ties_on_the_label() {
    assert_eq!(
        order(SortColumn::Files),
        ["Go", "Rust", "Ada", "Zig", "Total"],
        "ten files before three, and each of the two ties broken by the label"
    );
}

#[test]
fn sorting_by_blank_puts_the_largest_first() {
    assert_eq!(
        order(SortColumn::Blank),
        ["Ada", "Go", "Rust", "Zig", "Total"],
        "the blank rows of both buckets, largest first, the tie broken by the label"
    );
}

#[test]
fn sorting_by_comment_puts_the_largest_first() {
    assert_eq!(
        order(SortColumn::Comment),
        ["Zig", "Rust", "Go", "Ada", "Total"],
        "the comment rows of both buckets, largest first"
    );
}

#[test]
fn sorting_by_code_is_the_default_and_puts_the_largest_first() {
    let by_code = order(SortColumn::Code);
    assert_eq!(
        by_code,
        ["Rust", "Go", "Ada", "Zig", "Total"],
        "the code rows of both buckets, largest first, the tie broken by the label"
    );
    assert_eq!(
        labels(&render_table(&sortable(), ReportOptions::default())),
        by_code,
        "the default order is the order of the code column"
    );
}

#[test]
fn sorting_by_test_files_puts_the_largest_first() {
    assert_eq!(
        order(SortColumn::TestFiles),
        ["Go", "Rust", "Zig", "Ada", "Total"],
        "the files holding a test row, largest first, the tie broken by the label"
    );
}

#[test]
fn sorting_by_test_code_puts_the_largest_first() {
    assert_eq!(
        order(SortColumn::TestCode),
        ["Go", "Rust", "Ada", "Zig", "Total"],
        "the test code, largest first, the tie of two rows of none broken by the label"
    );
}

#[test]
fn sorting_by_test_percent_puts_the_largest_share_first() {
    assert_eq!(
        order(SortColumn::TestPercent),
        ["Go", "Rust", "Ada", "Zig", "Total"],
        "the test share of the code, largest first, however small the row is"
    );
}

#[test]
fn the_top_flag_keeps_the_first_rows_and_the_total_still_covers_every_file() {
    let table = render_table(&sortable(), options(|it| it.top = Some(2)));

    assert_eq!(
        labels(&table),
        ["Rust", "Go", "Total"],
        "the two largest rows are kept, and the total follows them"
    );
    assert_eq!(
        fields(&table, "Total"),
        fields(
            &render_table(&sortable(), ReportOptions::default()),
            "Total"
        ),
        "the total covers every file, not only the rows that are shown"
    );
}

#[test]
fn a_top_of_zero_keeps_no_rows_and_still_prints_the_total() {
    let table = render_table(&sortable(), options(|it| it.top = Some(0)));

    assert_eq!(
        labels(&table),
        ["Total"],
        "no row is shown, and the total is still shown"
    );
    assert_eq!(
        fields(&table, "Total"),
        fields(
            &render_table(&sortable(), ReportOptions::default()),
            "Total"
        ),
        "a report of no rows still totals every file"
    );
}

#[test]
fn a_top_larger_than_the_rows_keeps_every_row() {
    let table = render_table(&sortable(), options(|it| it.top = Some(99)));

    assert_eq!(
        labels(&table),
        ["Rust", "Go", "Ada", "Zig", "Total"],
        "asking for more rows than there are keeps the ones there are"
    );
    assert_eq!(
        fields(&table, "Total"),
        fields(
            &render_table(&sortable(), ReportOptions::default()),
            "Total"
        ),
        "the total is the same whether or not the rows were trimmed"
    );
}

#[test]
fn a_tests_only_report_drops_a_language_with_no_test_code() {
    let table = render_table(&sortable(), options(|it| it.bucket = Bucket::TestsOnly));

    assert_eq!(
        labels(&table),
        ["Go", "Rust", "Total"],
        "a language with no test code has nothing to say in a test-only report"
    );
    assert_eq!(
        fields(&table, "Rust"),
        vec!["Rust", "4", "1", "1", "50"],
        "the main columns report the test bucket, and Files counts the test files"
    );
}

#[test]
fn a_production_only_report_drops_a_language_with_no_production_code() {
    let table = render_table(
        &summary(
            vec![
                row("Rust", [2, 2, 1], counts(1, 2, 30), counts(0, 0, 5)),
                row("JSON", [3, 0, 3], Counts::default(), counts(0, 0, 40)),
            ],
            row("Total", [5, 2, 4], counts(1, 2, 30), counts(0, 0, 45)),
        ),
        options(|it| it.bucket = Bucket::ProductionOnly),
    );

    assert_eq!(
        labels(&table),
        ["Rust", "Total"],
        "a language that is nothing but test material is not production code"
    );
    assert_eq!(
        fields(&table, "Rust"),
        vec!["Rust", "2", "1", "2", "30"],
        "the main columns report the production bucket, and Files counts the production files"
    );
}

#[test]
fn the_files_column_of_a_production_only_report_counts_the_files_holding_production_code() {
    let table = render_table(
        &Summary::new(vec![
            counted(
                "src/lib.rs",
                Language::Rust,
                counts(0, 0, 10),
                Counts::default(),
            ),
            counted(
                "src/both.rs",
                Language::Rust,
                counts(0, 0, 10),
                counts(0, 0, 4),
            ),
            counted(
                "tests/it.rs",
                Language::Rust,
                Counts::default(),
                counts(0, 0, 6),
            ),
        ]),
        options(|it| it.bucket = Bucket::ProductionOnly),
    );

    assert_eq!(
        fields(&table, "Rust"),
        vec!["Rust", "2", "0", "0", "20"],
        "the file that holds nothing but test rows is not a production file"
    );
}

#[test]
fn by_file_and_tests_only_and_top_compose() {
    let table = render_table(
        &Summary::new(vec![
            counted(
                "src/lib.rs",
                Language::Rust,
                counts(0, 0, 100),
                counts(0, 0, 8),
            ),
            counted(
                "tests/big.rs",
                Language::Rust,
                Counts::default(),
                counts(0, 0, 40),
            ),
            counted(
                "src/plain.rs",
                Language::Rust,
                counts(0, 0, 30),
                Counts::default(),
            ),
        ]),
        options(|it| {
            it.by_file = true;
            it.bucket = Bucket::TestsOnly;
            it.top = Some(1);
        }),
    );

    assert_eq!(
        labels(&table),
        ["tests/big.rs", "Total"],
        "the file of no test code is dropped, and the smaller of the other two is trimmed"
    );
    assert!(
        table.starts_with("File "),
        "the first column still names the file:\n{table}"
    );
    assert_eq!(
        fields(&table, "Total"),
        vec!["Total", "2", "0", "0", "48"],
        "the total still covers every file that holds a test row"
    );
}

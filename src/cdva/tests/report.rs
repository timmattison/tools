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
//! The two machine formats are read the same way, and one test compares them
//! with the table directly: the labels of the JSON rows are the labels of the
//! table rows, under every column the report orders by and every trim. Two
//! copies of "which rows, in what order" is how a JSON report and a table of
//! one run come to disagree about which file is the largest.
//!
//! Every summary here is built by hand rather than counted from a tree. The
//! renderer is what these tests cover, so a fixture tree would put the walk and
//! the classifier between the assertion and the thing it asserts, and a failure
//! anywhere in that chain would read as a failure of the table. Nothing here
//! touches the file system, so two copies of this file running at once share
//! nothing at all.

use cdva::{
    render_csv, render_json, render_table, Bucket, Counts, FileCount, Language, ParseStatus,
    ReportOptions, Row, SortColumn, Summary,
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

/// One counted file whose parse failed, which a report names in its list of
/// failures rather than in a row of its own.
fn failed(path: &str, language: Language) -> FileCount {
    FileCount {
        path: PathBuf::from(path),
        language,
        production: counts(0, 0, 3),
        test: Counts::default(),
        spans: Vec::new(),
        parse_status: ParseStatus::Failed,
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

/// The JSON document of the two-language summary above, byte for byte.
///
/// Every row carries both buckets *and* their sum, because a consumer that had
/// to add `production.code` to `test.code` to learn the code of a row is a
/// consumer that will one day add them wrong. `language` names the language of
/// the row, and the total, which belongs to every language, carries no such
/// key at all rather than a null one.
const GOLDEN_JSON: &str = r#"{
  "rows": [
    {
      "label": "Rust",
      "language": "Rust",
      "files": 412,
      "production_files": 400,
      "test_files": 188,
      "blank": 9118,
      "comment": 6204,
      "code": 61330,
      "production": {
        "blank": 6000,
        "comment": 4000,
        "code": 36425
      },
      "test": {
        "blank": 3118,
        "comment": 2204,
        "code": 24905
      },
      "test_percent": 40.6
    },
    {
      "label": "TypeScript",
      "language": "TypeScript",
      "files": 77,
      "production_files": 70,
      "test_files": 31,
      "blank": 1204,
      "comment": 812,
      "code": 9441,
      "production": {
        "blank": 900,
        "comment": 600,
        "code": 6321
      },
      "test": {
        "blank": 304,
        "comment": 212,
        "code": 3120
      },
      "test_percent": 33.0
    }
  ],
  "total": {
    "label": "Total",
    "files": 489,
    "production_files": 470,
    "test_files": 219,
    "blank": 10322,
    "comment": 7016,
    "code": 70771,
    "production": {
      "blank": 6900,
      "comment": 4600,
      "code": 42746
    },
    "test": {
      "blank": 3422,
      "comment": 2416,
      "code": 28025
    },
    "test_percent": 39.6
  },
  "failed_parses": []
}
"#;

/// The CSV of the two-language summary above, byte for byte.
///
/// The header names every column the document holds, in the order the records
/// under it carry them. `language` is empty on the total, and the test share
/// carries no percent sign: a `%` inside a field is a glyph every consumer
/// then has to strip before it can read a number.
const GOLDEN_CSV: &str = "\
label,language,files,production_files,test_files,blank,comment,code,production_blank,production_comment,production_code,test_blank,test_comment,test_code,test_percent
Rust,Rust,412,400,188,9118,6204,61330,6000,4000,36425,3118,2204,24905,40.6
TypeScript,TypeScript,77,70,31,1204,812,9441,900,600,6321,304,212,3120,33.0
Total,,489,470,219,10322,7016,70771,6900,4600,42746,3422,2416,28025,39.6
";

/// The header of the CSV, as its own list, for the tests that read a record by
/// the name of its column rather than by counting commas.
const CSV_HEADER: [&str; 15] = [
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

/// The JSON document of this summary, parsed.
fn document(summary: &Summary, options: ReportOptions) -> serde_json::Value {
    let rendered = render_json(summary, options);
    serde_json::from_str(&rendered)
        .unwrap_or_else(|error| panic!("the document parses as JSON ({error}):\n{rendered}"))
}

/// Every record of the CSV of this summary, the header first.
fn records(summary: &Summary, options: ReportOptions) -> Vec<Vec<String>> {
    let rendered = render_csv(summary, options);
    csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(rendered.as_bytes())
        .records()
        .map(|record| {
            let record = record
                .unwrap_or_else(|error| panic!("the report parses as CSV ({error}):\n{rendered}"));
            record.iter().map(str::to_string).collect()
        })
        .collect()
}

/// The value under `key` of `row`, as a number.
fn number(row: &serde_json::Value, key: &str) -> f64 {
    row.get(key)
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_else(|| panic!("the row holds a number under `{key}`: {row}"))
}

/// The label of every row of the document, the total last.
///
/// The total is kept rather than dropped, so a document and a table are read
/// the same way and the two lists of labels can be compared field for field.
fn json_labels(summary: &Summary, options: ReportOptions) -> Vec<String> {
    let document = document(summary, options);
    let rows = document["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("the document holds an array of rows: {document}"));
    rows.iter()
        .chain(std::iter::once(&document["total"]))
        .map(|row| row["label"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A summary of one file of each bucket and one file of both, which no golden
/// string covers: the mixed file is what makes `blank`, `comment`, and `code`
/// three sums rather than three copies of one bucket.
fn mixed() -> Summary {
    Summary::new(vec![
        counted(
            "src/both.rs",
            Language::Rust,
            counts(3, 5, 70),
            counts(2, 1, 30),
        ),
        counted(
            "tests/it.rs",
            Language::Rust,
            Counts::default(),
            counts(4, 2, 44),
        ),
        counted(
            "web/app.ts",
            Language::TypeScript,
            counts(9, 8, 120),
            Counts::default(),
        ),
    ])
}

#[test]
fn the_json_document_renders_exactly_this_way() {
    assert_eq!(
        render_json(&two_languages(), ReportOptions::default()),
        GOLDEN_JSON,
        "the JSON report is the document this string spells out"
    );
}

#[test]
fn the_csv_renders_exactly_this_way() {
    assert_eq!(
        render_csv(&two_languages(), ReportOptions::default()),
        GOLDEN_CSV,
        "the CSV report is the text this string spells out"
    );
}

#[test]
fn every_json_row_adds_its_two_buckets_up_to_its_whole() {
    let document = document(&mixed(), options(|it| it.by_file = true));
    let rows = document["rows"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the document holds an array of rows: {document}"));

    assert_eq!(rows.len(), 3, "one row for each file: {document}");
    for row in rows.iter().chain(std::iter::once(&document["total"])) {
        for column in ["blank", "comment", "code"] {
            assert_eq!(
                number(row, column),
                number(&row["production"], column) + number(&row["test"], column),
                "the `{column}` of a row is its two buckets added up: {row}"
            );
        }
    }
}

#[test]
fn a_language_row_and_a_file_row_name_their_language_and_the_total_does_not() {
    let by_language = document(&mixed(), ReportOptions::default());
    assert_eq!(
        by_language["rows"][0]["language"], "Rust",
        "a language row names its language: {by_language}"
    );
    assert!(
        by_language["total"].get("language").is_none(),
        "the total belongs to every language and names none: {by_language}"
    );

    let by_file = document(&mixed(), options(|it| it.by_file = true));
    assert_eq!(
        by_file["rows"][0]["language"], "TypeScript",
        "a file row names the language of its file, which its label does not: {by_file}"
    );
    assert!(
        by_file["total"].get("language").is_none(),
        "the total of a by-file report names no language either: {by_file}"
    );
}

#[test]
fn by_file_gives_one_json_row_for_each_file_under_its_own_language() {
    let document = document(&mixed(), options(|it| it.by_file = true));
    let rows = document["rows"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the document holds an array of rows: {document}"));

    let named: Vec<(String, String)> = rows
        .iter()
        .map(|row| {
            (
                row["label"].as_str().unwrap_or_default().to_string(),
                row["language"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        named,
        vec![
            ("web/app.ts".to_string(), "TypeScript".to_string()),
            ("src/both.rs".to_string(), "Rust".to_string()),
            ("tests/it.rs".to_string(), "Rust".to_string()),
        ],
        "one row for each file, largest first, each under the language of that file"
    );
}

#[test]
fn the_json_rows_are_the_table_rows_under_every_sort_and_top() {
    for sort in [
        SortColumn::Language,
        SortColumn::Files,
        SortColumn::Blank,
        SortColumn::Comment,
        SortColumn::Code,
        SortColumn::TestFiles,
        SortColumn::TestCode,
        SortColumn::TestPercent,
    ] {
        for top in [None, Some(0), Some(1), Some(3), Some(99)] {
            let shaped = options(|it| {
                it.sort = sort;
                it.top = top;
            });
            assert_eq!(
                json_labels(&sortable(), shaped),
                labels(&render_table(&sortable(), shaped)),
                "the document holds the rows of the table, in the order of the table, \
                 under {sort:?} and {top:?}"
            );
            let records: Vec<String> = records(&sortable(), shaped)
                .into_iter()
                .skip(1)
                .filter_map(|record| record.first().cloned())
                .collect();
            assert_eq!(
                records,
                labels(&render_table(&sortable(), shaped)),
                "the CSV holds the rows of the table, in the order of the table, \
                 under {sort:?} and {top:?}"
            );
        }
    }
}

#[test]
fn the_test_share_is_one_decimal_in_both_formats_and_a_row_of_no_code_is_zero() {
    let summary = summary(
        vec![
            row("Rust", [2, 1, 1], counts(0, 0, 11), counts(0, 0, 3)),
            row("Markdown", [1, 1, 0], counts(4, 0, 0), Counts::default()),
        ],
        row("Total", [3, 2, 1], counts(4, 0, 11), counts(0, 0, 3)),
    );

    let document = document(&summary, ReportOptions::default());
    let share = |label: &str| {
        document["rows"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|row| row["label"] == label)
            .unwrap_or_else(|| panic!("the document holds a row for `{label}`: {document}"))
    };

    assert!(
        (number(&share("Rust"), "test_percent") - 21.4).abs() < f64::EPSILON,
        "the test share is rounded to the one decimal the table prints: {document}"
    );
    assert!(
        (number(&share("Markdown"), "test_percent") - 0.0).abs() < f64::EPSILON,
        "a row of no code holds a zero, and not a NaN that is not JSON at all: {document}"
    );
    assert!(
        !render_json(&summary, ReportOptions::default()).contains("NaN"),
        "a document holding NaN is not a JSON document at all"
    );

    let records = records(&summary, ReportOptions::default());
    let share_column = CSV_HEADER.len() - 1;
    let percents: Vec<&str> = records
        .iter()
        .skip(1)
        .filter_map(|record| record.get(share_column).map(String::as_str))
        .collect();
    assert_eq!(
        percents,
        ["21.4", "0.0", "21.4"],
        "the CSV carries the same one decimal, with no percent sign to strip"
    );

    let table = render_table(&summary, ReportOptions::default());
    assert_eq!(
        fields(&table, "Rust")[7],
        "21.4%",
        "the table prints the same number the two machine formats carry:\n{table}"
    );
}

#[test]
fn a_summary_of_no_files_renders_an_empty_document_and_a_csv_of_a_header_and_a_total() {
    let empty = summary(
        Vec::new(),
        row("Total", [0, 0, 0], Counts::default(), Counts::default()),
    );

    let document = document(&empty, ReportOptions::default());
    assert_eq!(
        document["rows"].as_array().map(Vec::len),
        Some(0),
        "a summary of no files holds an empty array of rows, and not a null: {document}"
    );
    assert_eq!(
        number(&document["total"], "code"),
        0.0,
        "the total of no files is zero: {document}"
    );
    assert_eq!(
        number(&document["total"], "test_percent"),
        0.0,
        "the test share of no code is zero: {document}"
    );

    let records = records(&empty, ReportOptions::default());
    assert_eq!(
        records.len(),
        2,
        "the CSV of no files is a header and a total: {records:?}"
    );
    assert_eq!(records[0], CSV_HEADER, "the header names every column");
    assert_eq!(
        records[1],
        ["Total", "", "0", "0", "0", "0", "0", "0", "0", "0", "0", "0", "0", "0", "0.0"],
        "the total of no files is zero in every column, under no language"
    );
}

#[test]
fn failed_parses_is_an_empty_array_when_every_parse_held_and_names_the_paths_when_one_did_not() {
    let held = document(&mixed(), ReportOptions::default());
    assert_eq!(
        held["failed_parses"].as_array().map(Vec::len),
        Some(0),
        "nothing failed, so the key is an empty array rather than absent: {held}"
    );

    let broken = Summary::new(vec![
        counted(
            "src/lib.rs",
            Language::Rust,
            counts(0, 0, 10),
            Counts::default(),
        ),
        failed("src/broken.rs", Language::Rust),
    ]);
    assert_eq!(
        document(&broken, ReportOptions::default())["failed_parses"],
        serde_json::json!(["src/broken.rs"]),
        "the document names the file whose parse failed"
    );
}

#[test]
fn a_label_holding_a_comma_a_quote_and_a_break_is_quoted_in_the_csv() {
    let awkward = "src/we,ird\"na\nme.rs";
    let summary = Summary::new(vec![counted(
        awkward,
        Language::Rust,
        counts(1, 1, 5),
        Counts::default(),
    )]);
    let rendered = render_csv(&summary, options(|it| it.by_file = true));

    assert!(
        rendered.contains("\"src/we,ird\"\"na\nme.rs\""),
        "the field is quoted, and the quote inside it is doubled:\n{rendered}"
    );
    let records = records(&summary, options(|it| it.by_file = true));
    assert_eq!(
        records.len(),
        3,
        "a break inside a quoted field is not the end of the record: {records:?}"
    );
    assert_eq!(
        records[1][0], awkward,
        "the label reads back exactly as it went in: {records:?}"
    );
}

#[test]
fn a_label_of_multi_byte_characters_survives_both_formats() {
    let path = "src/日本語/計算.rs";
    let summary = Summary::new(vec![counted(
        path,
        Language::Rust,
        counts(1, 1, 40),
        Counts::default(),
    )]);
    let shaped = options(|it| it.by_file = true);

    assert_eq!(
        document(&summary, shaped)["rows"][0]["label"],
        path,
        "the document carries the path as it is, and not as bytes"
    );
    assert_eq!(
        records(&summary, shaped)[1][0],
        path,
        "the CSV carries the path as it is"
    );
}

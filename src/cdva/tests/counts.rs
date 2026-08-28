//! Counting one file, and adding the files up.
//!
//! The first test of this file is the one that matters. The whole promise of
//! the tool is that splitting a count does not change it, so the split is
//! asserted against the classifier alone over a tree that holds a file of every
//! language the fixtures cover, field by field. A rule that dropped a row, or
//! that counted one twice, would still print a plausible table; only this
//! assertion tells the difference.
//!
//! Every tree these tests write goes under `tempfile::tempdir()`, and every
//! name in it is unique to the test that writes it, so two copies of this file
//! running at once never read each other's fixtures.

use cdva::{
    count, walk, Counter, Counts, FileCount, Language, ParseStatus, PathRules, Summary, WalkOptions,
};
use std::path::{Path, PathBuf};

/// A file of every language the invariant is asserted over, beside the path it
/// is written to.
///
/// The paths are chosen so that both halves of the split are exercised: some of
/// them a built-in glob marks as test material, and the rest are production
/// code. The sources hold blank rows, comment rows, and code rows, because a
/// corpus of code alone would let a bug that misplaced a comment row through.
const FIXTURES: &[(&str, &str)] = &[
    (
        "src/lib.rs",
        "//! A library.\n\n/// Adds two numbers.\npub fn add(a: u64, b: u64) -> u64 {\n    a + b /* the sum */\n}\n",
    ),
    (
        // A quote is the delimiter of a character literal and the first
        // character of a lifetime, and the classifier tells the two apart by
        // looking ahead. A row it read wrong would move every row behind it to
        // another bucket, and the split would still sum to the same wrong
        // total, so the invariant below is asserted over such a file too.
        "src/chars.rs",
        "// The characters a name must not hold.\nconst FORBIDDEN: [char; 3] = ['/', '\\\\', '\"'];\n\n/// Reads the first character of a name.\npub fn first<'a>(name: &'a str) -> Option<char> {\n    name.chars().next() /* the first one */\n}\n",
    ),
    (
        "tests/add.rs",
        "// The test of the sum.\n\n#[test]\nfn adds() {\n    assert_eq!(2 + 2, 4);\n}\n",
    ),
    (
        "cmd/main.go",
        "// Package main runs the thing.\npackage main\n\nfunc main() {\n\tprintln(\"hello\")\n}\n",
    ),
    (
        "cmd/main_test.go",
        "package main\n\nimport \"testing\"\n\nfunc TestMain(t *testing.T) {\n\t// nothing yet\n}\n",
    ),
    (
        "app/app.py",
        "\"\"\"The module docstring.\"\"\"\n\n\ndef add(a, b):\n    # the sum\n    return a + b\n",
    ),
    (
        "app/test_app.py",
        "import app\n\n\ndef test_add():\n    assert app.add(1, 1) == 2\n",
    ),
    (
        "web/app.js",
        "// The entry point.\nexport function add(a, b) {\n  return a + b;\n}\n",
    ),
    (
        "web/app.ts",
        "export function add(a: number, b: number): number {\n  /* the sum */\n  return a + b;\n}\n",
    ),
    (
        "web/app.test.ts",
        "import { add } from './app';\n\nit('adds', () => {\n  expect(add(1, 1)).toBe(2);\n});\n",
    ),
    (
        "web/App.tsx",
        "export const App = () => {\n  // the view\n  return <div>hi</div>;\n};\n",
    ),
    (
        "java/App.java",
        "package app;\n\n// The application.\npublic class App {\n    public static int add(int a, int b) {\n        return a + b;\n    }\n}\n",
    ),
    (
        "java/AppTest.java",
        "package app;\n\npublic class AppTest {\n    @Test\n    void adds() {}\n}\n",
    ),
    (
        "kotlin/App.kt",
        "package app\n\n// The application.\nfun add(a: Int, b: Int) = a + b\n",
    ),
    (
        "csharp/App.cs",
        "namespace App;\n\n// The application.\npublic static class Math\n{\n    public static int Add(int a, int b) => a + b;\n}\n",
    ),
    (
        "ruby/app.rb",
        "# The application.\ndef add(a, b)\n  a + b\nend\n",
    ),
    (
        "spec/app_spec.rb",
        "describe 'add' do\n  it 'adds' do\n    expect(add(1, 1)).to eq(2)\n  end\nend\n",
    ),
    (
        "swift/App.swift",
        "// The application.\nfunc add(_ a: Int, _ b: Int) -> Int {\n    return a + b\n}\n",
    ),
    (
        "elixir/app.ex",
        "defmodule App do\n  # the sum\n  def add(a, b), do: a + b\nend\n",
    ),
    (
        "zig/app.zig",
        "// The application.\npub fn add(a: u64, b: u64) u64 {\n    return a + b;\n}\n",
    ),
    (
        "c/app.c",
        "/* The application. */\n#include <stdio.h>\n\nint add(int a, int b) { return a + b; }\n",
    ),
    (
        "scripts/run.sh",
        "#!/usr/bin/env bash\n# Runs the thing.\n\nset -e\necho \"hello\"\n",
    ),
    (
        "config/settings.yaml",
        "# The settings.\nname: cdva\n\nvalues:\n  - one\n",
    ),
    (
        "config/settings.toml",
        "# The settings.\nname = \"cdva\"\n\n[values]\none = 1\n",
    ),
    ("docs/README.md", "# Title\n\nSome prose.\n"),
    (
        "web/index.html",
        "<!-- The page. -->\n<html>\n  <body>hi</body>\n</html>\n",
    ),
    (
        "db/schema.sql",
        "-- The schema.\nCREATE TABLE thing (\n  id INTEGER\n);\n",
    ),
    (
        "lua/app.lua",
        "-- The application.\n--[[ a block\n     comment ]]\nlocal function add(a, b)\n  return a + b\nend\n",
    ),
    ("data/data.json", "{\n  \"name\": \"cdva\"\n}\n"),
];

/// Writes one file, making the directories above it.
fn write(root: &Path, relative: &str, contents: &str) -> PathBuf {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the fixture directory is made");
    }
    std::fs::write(&path, contents).expect("the fixture file is written");
    path
}

/// A counter reading the built-in globs alone.
fn counter() -> Counter {
    Counter::new(PathRules::builtin())
}

/// One counted file, built by hand, for the roll-up tests.
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

/// The counts of a row, spelled out.
const fn counts(blank: u64, comment: u64, code: u64) -> Counts {
    Counts {
        blank,
        comment,
        code,
    }
}

#[test]
fn the_two_buckets_always_sum_to_the_unsplit_count() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    for (relative, source) in FIXTURES {
        write(root.path(), relative, source);
    }

    let found = walk(
        &[root.path().to_path_buf()],
        WalkOptions {
            hidden: false,
            // Nothing under this tree is ignored, and reading the ignore files
            // of whoever runs the test would let their configuration decide
            // which fixtures this assertion covers.
            no_ignore: true,
        },
    )
    .expect("the fixture tree walks");

    assert_eq!(
        found.len(),
        FIXTURES.len(),
        "the walk must find every fixture"
    );

    let counter = counter();
    for (path, relative) in &found {
        let source = std::fs::read_to_string(path).expect("the fixture reads back");
        let file = counter
            .count_path(path, relative)
            .expect("the fixture reads")
            .unwrap_or_else(|| panic!("{} is a language cdva counts", relative.display()));

        let unsplit = count(&source, file.language);
        let total = file.total();

        assert_eq!(
            total.blank,
            unsplit.blank,
            "blank rows of {}",
            relative.display()
        );
        assert_eq!(
            total.comment,
            unsplit.comment,
            "comment rows of {}",
            relative.display()
        );
        assert_eq!(
            total.code,
            unsplit.code,
            "code rows of {}",
            relative.display()
        );
    }
}

#[test]
fn a_path_rule_match_puts_every_row_in_the_test_bucket() {
    let source = "// a test\n\nfn adds() {\n    assert!(true);\n}\n";
    let file = counter()
        .count_source(
            Path::new("/repo/tests/add.rs"),
            Path::new("tests/add.rs"),
            source,
        )
        .expect("a Rust file is counted");

    assert_eq!(file.language, Language::Rust);
    assert_eq!(file.production, Counts::default());
    assert_eq!(file.test, count(source, Language::Rust));
    assert_eq!(file.parse_status, ParseStatus::NotParsed);

    assert_eq!(file.spans.len(), 1, "one span covers the whole file");
    let span = &file.spans[0];
    assert_eq!(span.first_row, 1);
    assert_eq!(span.last_row, 5, "the file has five rows");
    assert_eq!(span.rule, cdva::Rule::PathGlob("tests/**".to_string()));
}

#[test]
fn an_unmarked_file_puts_every_row_in_the_production_bucket() {
    let source = "// a library\n\npub fn add() {}\n";
    let file = counter()
        .count_source(
            Path::new("/repo/src/lib.rs"),
            Path::new("src/lib.rs"),
            source,
        )
        .expect("a Rust file is counted");

    assert_eq!(file.production, count(source, Language::Rust));
    assert_eq!(file.test, Counts::default());
    assert!(file.spans.is_empty(), "no rule marked anything");
}

#[test]
fn an_empty_file_counts_nothing_and_carries_no_span() {
    let file = counter()
        .count_source(
            Path::new("/repo/tests/empty.rs"),
            Path::new("tests/empty.rs"),
            "",
        )
        .expect("an empty Rust file is still a Rust file");

    assert_eq!(file.production, Counts::default());
    assert_eq!(file.test, Counts::default());
    assert_eq!(file.total(), Counts::default());
    assert!(
        file.spans.is_empty(),
        "an empty file must not carry a span of rows 1 to 0"
    );
}

#[test]
fn a_file_is_a_test_file_when_it_holds_a_test_row() {
    let counter = counter();

    let marked = counter
        .count_source(
            Path::new("/repo/tests/add.rs"),
            Path::new("tests/add.rs"),
            "fn adds() {}\n",
        )
        .expect("a Rust file is counted");
    assert!(marked.is_test_file(), "a marked file holds a test row");

    let unmarked = counter
        .count_source(
            Path::new("/repo/src/lib.rs"),
            Path::new("src/lib.rs"),
            "fn add() {}\n",
        )
        .expect("a Rust file is counted");
    assert!(
        !unmarked.is_test_file(),
        "an unmarked file holds no test row"
    );

    let empty = counter
        .count_source(
            Path::new("/repo/tests/empty.rs"),
            Path::new("tests/empty.rs"),
            "",
        )
        .expect("an empty Rust file is still a Rust file");
    assert!(
        !empty.is_test_file(),
        "an empty file holds no row at all, so it holds no test row"
    );
}

#[test]
fn a_file_of_an_unknown_extension_is_not_counted() {
    let counter = counter();

    assert!(
        counter
            .count_source(
                Path::new("/repo/notes.qqzz"),
                Path::new("notes.qqzz"),
                "some notes\n"
            )
            .is_none(),
        "cdva counts no language spelled .qqzz"
    );

    assert!(
        counter
            .count_source(
                Path::new("/repo/notes.rs"),
                Path::new("notes.rs"),
                "fn main() {}\n"
            )
            .is_some(),
        "the same source under a known extension is counted"
    );
}

#[test]
fn a_file_that_is_not_utf8_is_skipped_rather_than_counted() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    let binary = root.path().join("binary.rs");
    std::fs::write(&binary, [0xff_u8, 0xfe, 0x00]).expect("the fixture file is written");
    let text = write(root.path(), "text.rs", "fn main() {}\n");

    let counter = counter();

    let skipped = counter
        .count_path(&binary, Path::new("binary.rs"))
        .expect("a file that is not text is not an error");
    assert!(skipped.is_none(), "a file that is not UTF-8 is skipped");

    let text_count = counter
        .count_path(&text, Path::new("text.rs"))
        .expect("a text file reads");
    assert!(
        text_count.is_some(),
        "a text file beside it is still counted"
    );
}

#[test]
fn a_file_that_cannot_be_read_is_an_error() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    let missing = root.path().join("gone.rs");

    let error = counter()
        .count_path(&missing, Path::new("gone.rs"))
        .expect_err("a file that is not there cannot be read");
    assert!(
        error.to_string().contains("gone.rs"),
        "the error names the file: {error}"
    );
}

#[test]
fn rows_are_ordered_by_code_descending_then_by_label() {
    let summary = Summary::new(vec![
        counted("a.rs", Language::Rust, counts(0, 0, 10), Counts::default()),
        counted("b.go", Language::Go, counts(0, 0, 30), Counts::default()),
        counted(
            "c.py",
            Language::Python,
            counts(0, 0, 10),
            Counts::default(),
        ),
    ]);

    let labels: Vec<&str> = summary.rows.iter().map(|row| row.label.as_str()).collect();
    assert_eq!(
        labels,
        vec!["Go", "Python", "Rust"],
        "the largest first, and a tie broken by the label"
    );
}

#[test]
fn the_total_is_the_field_wise_sum_of_the_rows() {
    let summary = Summary::new(vec![
        counted("a.rs", Language::Rust, counts(1, 2, 3), counts(4, 5, 6)),
        counted("b.rs", Language::Rust, counts(1, 1, 1), Counts::default()),
        counted("c.go", Language::Go, counts(2, 0, 7), counts(0, 3, 0)),
    ]);

    assert_eq!(summary.total.label, "Total");
    assert_eq!(summary.total.files, 3);
    assert_eq!(summary.total.production, counts(4, 3, 11));
    assert_eq!(summary.total.test, counts(4, 8, 6));
    assert_eq!(summary.total.blank(), 8);
    assert_eq!(summary.total.comment(), 11);
    assert_eq!(summary.total.code(), 17);

    let summed = summary.rows.iter().fold(Counts::default(), |sum, row| {
        sum + row.production + row.test
    });
    assert_eq!(summed, summary.total.production + summary.total.test);

    assert_eq!(summary.files.len(), 3, "the summary keeps the files");
    assert!(
        summary.failed_parses.is_empty(),
        "nothing was parsed at all"
    );
}

#[test]
fn test_files_counts_only_the_files_holding_a_test_row() {
    let summary = Summary::new(vec![
        counted("a.rs", Language::Rust, counts(0, 0, 5), Counts::default()),
        counted("b.rs", Language::Rust, Counts::default(), counts(0, 0, 5)),
        counted("c.rs", Language::Rust, counts(0, 0, 5), Counts::default()),
    ]);

    let row = &summary.rows[0];
    assert_eq!(row.files, 3);
    assert_eq!(row.test_files, 1);
    assert_eq!(summary.total.test_files, 1);
}

#[test]
fn test_percent_is_the_test_share_of_the_code() {
    let summary = Summary::new(vec![
        counted("a.rs", Language::Rust, counts(0, 0, 30), counts(0, 0, 10)),
        counted(
            "b.md",
            Language::Markdown,
            counts(3, 0, 0),
            Counts::default(),
        ),
    ]);

    let rust = summary
        .rows
        .iter()
        .find(|row| row.label == "Rust")
        .expect("the Rust row is there");
    assert!(
        (rust.test_percent() - 25.0).abs() < 1e-9,
        "ten of forty code rows is a quarter: {}",
        rust.test_percent()
    );

    let markdown = summary
        .rows
        .iter()
        .find(|row| row.label == "Markdown")
        .expect("the Markdown row is there");
    assert!(
        markdown.test_percent().abs() < 1e-9,
        "a row of no code has no test share: {}",
        markdown.test_percent()
    );
}

#[test]
fn a_summary_of_no_files_is_empty_and_zeroed() {
    let summary = Summary::new(Vec::new());

    assert!(summary.rows.is_empty());
    assert!(summary.files.is_empty());
    assert!(summary.failed_parses.is_empty());
    assert_eq!(summary.total.label, "Total");
    assert_eq!(summary.total.files, 0);
    assert_eq!(summary.total.test_files, 0);
    assert_eq!(summary.total.production, Counts::default());
    assert_eq!(summary.total.test, Counts::default());
    assert!(summary.total.test_percent().abs() < 1e-9);
}

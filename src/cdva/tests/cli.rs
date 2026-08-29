//! The command line, driving the real binary.
//!
//! Every tree here goes under `tempfile::tempdir()`, and every run names its
//! roots on the command line, so two copies of this file running at once never
//! read each other's fixtures and neither one ever counts the repository it was
//! built from.
//!
//! Every run also passes `--no-ignore`. The assertions below say which files
//! were counted, and a global `.gitignore` of whoever runs the test could
//! otherwise decide that answer — a failure that reads as a bug in the tool and
//! belongs to the configuration of one machine.
//!
//! Every run that explains one file works from the tree itself, because a
//! relative path on the command line and a path the walk produced name one file
//! only when both are read from one place. That working directory is the
//! `tempfile::tempdir()` of the test, so such a run still reaches nothing
//! outside its own fixture.
//!
//! The binary reads no environment of its own, but the pre-commit hook exports
//! `GIT_DIR` and `GIT_INDEX_FILE` into `cargo test`, and a child that inherited
//! one of those would work in a repository nobody named. Every spawn below
//! therefore drops the whole `GIT_` prefix rather than a list of names, because
//! a list goes stale and then reports clean.

use std::path::Path;
use std::process::{Command, Output};

/// The prefix of every variable that points a tool at a git repository.
const GIT_PREFIX: &str = "GIT_";

/// The flag that turns every ignore file off.
const NO_IGNORE: &str = "--no-ignore";

/// The flag that explains one file rather than printing a table.
const EXPLAIN: &str = "--explain";

/// The start of the sentence an explanation prints in place of a list of spans,
/// for a file no rule marked.
const NO_RULE: &str = "No rule marked any row";

/// The name of the row the fixtures below land in.
const RUST_ROW: &str = "Rust";

/// The column of the files of a row.
const FILES: usize = 1;

/// The column of the code of a row, of which the test code is a part.
const CODE: usize = 4;

/// The column of the test files of a row.
const TEST_FILES: usize = 5;

/// The column of the test code of a row.
const TEST_CODE: usize = 6;

/// The header of the CSV report, which is a contract with whatever reads it.
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

/// A library file of three rows of code, which no glob marks.
const LIBRARY: &str = "pub fn add(a: u64, b: u64) -> u64 {\n    a + b\n}\n";

/// A helper file of three rows of code, which no built-in glob knows.
const HELPER: &str = "fn helper() -> u64 {\n    7\n}\n";

/// A test file of four rows of code, which the built-in `tests/**` marks.
const INTEGRATION_TEST: &str = "#[test]\nfn works() {\n    assert_eq!(1, 1);\n}\n";

/// A library whose test code is a module inside it: three rows of production
/// code, and seven rows the tree rule alone can find. No glob names it, so
/// `--no-tree` reports none of it as test code.
const LIBRARY_WITH_A_TEST_MODULE: &str = "pub fn add(a: u64, b: u64) -> u64 {\n    a + b\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn works() {\n        assert_eq!(1, 1);\n    }\n}\n";

/// A library of five rows of code that moves its test code into another file.
const DECLARES_A_TEST_MODULE: &str =
    "pub fn add(a: u64, b: u64) -> u64 {\n    a + b\n}\n\n#[cfg(test)]\nmod tests;\n";

/// The file that declaration names: four rows of code, and nothing in it that
/// any other rule of the tool would call a test.
const DECLARED_TEST_MODULE: &str = "use super::add;\n\nfn checked() -> u64 {\n    add(1, 2)\n}\n";

/// A file of no language the tool counts.
const NOTES: &str = "This is a note, and no language the tool counts.\n";

/// A library whose braces do not balance, with a test module under the break.
///
/// Tree-sitter recovers from the stray brace and hands back a tree all the
/// same, so nothing about the count *looks* wrong: the test module is never
/// found, and all twelve rows of code count as production code. That silence
/// is what the footer and `--strict` are for.
const BROKEN_LIBRARY: &str = "pub fn broken() -> i32 {\n    let x = 1;\n    x\n}\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn always() {\n        assert!(1 + 1 == 2);\n    }\n}\n";

/// The flag that fails the run when any parse failed.
const STRICT: &str = "--strict";

/// The flag that reads no syntax tree at all.
const NO_TREE: &str = "--no-tree";

/// The words of the footer that names the parses that failed, and which no
/// other line of any report holds.
const FAILED_TO_PARSE: &str = "failed to parse";

/// The footer a run whose one parse failed prints under its table.
const ONE_FAILURE_FOOTER: &str = "1 file failed to parse and counts as production code:";

/// A JavaScript library whose regular expression holds a backtick.
///
/// No row of the language table models a regular expression, so the backtick
/// opens a template string. A template string of JavaScript spans rows, so the
/// scan runs to the end of the file inside one: the comment row counts as code,
/// and the scan ends where the scan of a whole file never ends. `cloc` 2.10
/// reports comment 1 code 4 over this file, and `cdva` reports comment 0
/// code 5.
///
/// Nothing here is a syntax error, and no row of it holds a needle of the tree
/// rule, so the file reaches no parser and its parse never fails. The two
/// faults are separate, and this fixture is the one that proves they stay
/// separate.
const REGEX_HOLDING_A_BACKTICK: &str = "const backtick = /`/;\nconst a = 1;\n// this comment must stay a comment\nconst b = 2;\nconst c = 3;\n";

/// The words of the footer that names the scans that did not end, and which no
/// other line of any report holds.
const ENDED_INSIDE: &str = "ended inside a string or a block comment";

/// The footer a run whose one scan did not end prints under its table.
const ONE_UNTERMINATED_FOOTER: &str = "1 file ended inside a string or a block comment, so its \
                                       comment and code counts are not to be trusted:";

/// What the long help indents a flag by, which is less than it indents the
/// prose under one. A section of the help therefore ends at the next line that
/// starts a flag, and not at a line of prose that happens to name one.
const HELP_FLAG_INDENT: &str = "      --";

/// The binary, with every `GIT_*` variable of the caller removed.
fn cdva() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cdva"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with(GIT_PREFIX) {
            command.env_remove(&key);
        }
    }
    command
}

/// Writes one file, making the directories above it.
fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the fixture directory is made");
    }
    std::fs::write(&path, contents).expect("the fixture file is written");
}

/// What the binary wrote to standard output.
fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("the binary writes UTF-8 to standard output")
}

/// What the binary wrote to standard error.
fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("the binary writes UTF-8 to standard error")
}

/// The label of every row the table prints, the total last.
///
/// The header and the rules are dropped, so the result is the order of the
/// report itself, and the count of it is the count of the rows the report kept.
fn labels(table: &str) -> Vec<String> {
    table
        .lines()
        .skip(1)
        .filter(|line| !line.starts_with('-'))
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

/// The fields of the table row that starts with `label`, with the bar dropped.
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

/// A tree of one library that parses and one that does not.
///
/// The clean file is there so that every assertion below reads a run that
/// counted something as well as a run that failed to: a footer over a tree of
/// nothing but broken files would not show that the rest of the report carried
/// on.
fn broken_tree() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY);
    write(root.path(), "src/broken.rs", BROKEN_LIBRARY);
    root
}

/// A tree of one library that scans clean and one whose scan does not end.
///
/// The clean file is there for the reason it is there in [`broken_tree`]: a
/// footer over a tree of nothing but bad files would not show that the rest of
/// the report carried on.
fn unterminated_tree() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY);
    write(root.path(), "src/regex.js", REGEX_HOLDING_A_BACKTICK);
    root
}

/// The lines of the long help that document one flag, from its name to the
/// next flag after it.
///
/// The escape codes come out of the help first. `clap` paints a flag name bold
/// whenever it decides the run wants color, and it reads `CLICOLOR_FORCE` for
/// that decision, not only the terminal. The pre-commit hook of this
/// repository sets that variable, so the name of a flag arrives as
/// `\x1b[1m--strict\x1b[0m` under a commit and as `--strict` under a plain
/// `cargo test`. A reader of this section wants the glyphs either way. See
/// "Colored Output in Tests" in CLAUDE.md.
fn help_section(help: &str, flag: &str) -> String {
    let glyphs = testcolor::strip_ansi(help);
    glyphs
        .lines()
        .skip_while(|line| line.trim() != flag)
        .skip(1)
        .take_while(|line| !line.starts_with(HELP_FLAG_INDENT))
        .collect::<Vec<&str>>()
        .join("\n")
}

#[test]
fn the_version_flag_names_the_tool_and_the_build() {
    let output = cdva().arg("--version").output().expect("the binary runs");

    assert!(
        output.status.success(),
        "the version flag succeeds: {}",
        stderr(&output)
    );
    let line = stdout(&output);
    assert!(
        line.starts_with("cdva "),
        "the version line names the tool first: {line}"
    );
    assert!(
        line.trim_start_matches("cdva ")
            .starts_with(|glyph: char| glyph.is_ascii_digit()),
        "the version line names a version after the tool: {line}"
    );
}

#[test]
fn the_help_names_every_flag_of_the_command() {
    let output = cdva().arg("--help").output().expect("the binary runs");

    assert!(
        output.status.success(),
        "the help flag succeeds: {}",
        stderr(&output)
    );
    let help = stdout(&output);
    for flag in [
        "--by-file",
        "--sort",
        "--top",
        "--tests-only",
        "--production-only",
        "--hidden",
        NO_IGNORE,
        "--test-glob",
        "--production-glob",
        NO_TREE,
        "--tree",
        "--json",
        "--csv",
        STRICT,
        EXPLAIN,
    ] {
        assert!(help.contains(flag), "the help names `{flag}`:\n{help}");
    }
}

#[test]
fn a_tree_of_production_and_test_files_reports_both() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY);
    write(root.path(), "tests/it.rs", INTEGRATION_TEST);

    let output = cdva()
        .arg(NO_IGNORE)
        .arg(root.path())
        .output()
        .expect("the binary runs");

    assert!(
        output.status.success(),
        "a readable tree counts: {}",
        stderr(&output)
    );
    let table = stdout(&output);
    let row = fields(&table, RUST_ROW);
    assert_eq!(row[FILES], "2", "both files land in the Rust row:\n{table}");
    assert_eq!(row[CODE], "7", "every row of code is counted:\n{table}");
    assert_eq!(
        row[TEST_FILES], "1",
        "the file under tests/ is a test file:\n{table}"
    );
    assert_eq!(
        row[TEST_CODE], "4",
        "the code of the test file is test code:\n{table}"
    );
}

#[test]
fn a_user_glob_moves_a_file_into_the_test_bucket_and_another_takes_one_back_out() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY);
    write(root.path(), "helper.rs", HELPER);
    write(root.path(), "tests/it.rs", INTEGRATION_TEST);

    let plain = cdva()
        .arg(NO_IGNORE)
        .arg(root.path())
        .output()
        .expect("the binary runs");
    let plain_table = stdout(&plain);
    assert_eq!(
        fields(&plain_table, RUST_ROW)[TEST_CODE],
        "4",
        "the built-in table alone marks the file under tests/:\n{plain_table}"
    );

    let marked = cdva()
        .arg(NO_IGNORE)
        .arg("--test-glob")
        .arg("helper.rs")
        .arg(root.path())
        .output()
        .expect("the binary runs");
    let marked_table = stdout(&marked);
    assert_eq!(
        fields(&marked_table, RUST_ROW)[TEST_CODE],
        "7",
        "a test glob of the user moves a file the built-in table does not know:\n{marked_table}"
    );

    let released = cdva()
        .arg(NO_IGNORE)
        .arg("--production-glob")
        .arg("tests/**")
        .arg(root.path())
        .output()
        .expect("the binary runs");
    let released_table = stdout(&released);
    assert_eq!(
        fields(&released_table, RUST_ROW)[TEST_CODE],
        "0",
        "a production glob of the user beats the built-in table:\n{released_table}"
    );
}

#[test]
fn a_test_module_declaration_moves_the_file_it_names_into_the_test_bucket() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", DECLARES_A_TEST_MODULE);
    write(root.path(), "src/tests.rs", DECLARED_TEST_MODULE);

    let output = cdva()
        .arg(NO_IGNORE)
        .arg(root.path())
        .output()
        .expect("the binary runs");

    assert!(
        output.status.success(),
        "a tree of two files counts: {}",
        stderr(&output)
    );
    let table = stdout(&output);
    let row = fields(&table, RUST_ROW);
    assert_eq!(row[FILES], "2", "both files land in the Rust row:\n{table}");
    assert_eq!(row[CODE], "9", "every row of code is counted:\n{table}");
    assert_eq!(
        row[TEST_CODE], "6",
        "the four rows of src/tests.rs are test code, beside the two rows of the declaration:\n{table}"
    );
    assert_eq!(
        row[TEST_FILES], "2",
        "the file the declaration names is a test file, and so is the one that declares it:\n{table}"
    );
}

#[test]
fn a_root_that_does_not_exist_fails_and_names_the_path() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    let missing = root.path().join("no-such-tree");

    let output = cdva()
        .arg(NO_IGNORE)
        .arg(&missing)
        .output()
        .expect("the binary runs");

    assert!(
        !output.status.success(),
        "a root that does not exist is a failure:\n{}",
        stdout(&output)
    );
    let complaint = stderr(&output);
    assert!(
        complaint.contains("no-such-tree"),
        "the failure names the path that is missing: {complaint}"
    );
}

#[test]
fn two_roots_that_overlap_count_each_file_once() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "sub/lib.rs", LIBRARY);

    let output = cdva()
        .arg(NO_IGNORE)
        .arg(root.path())
        .arg(root.path().join("sub"))
        .output()
        .expect("the binary runs");

    assert!(
        output.status.success(),
        "two overlapping roots count: {}",
        stderr(&output)
    );
    let table = stdout(&output);
    let row = fields(&table, RUST_ROW);
    assert_eq!(
        row[FILES], "1",
        "the one file under both roots is one file:\n{table}"
    );
    assert_eq!(
        row[CODE], "3",
        "the rows of that file are counted once:\n{table}"
    );
}

#[cfg(unix)]
#[test]
fn a_file_that_cannot_be_read_is_warned_about_and_skipped() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "good.rs", LIBRARY);
    write(root.path(), "unreadable.rs", HELPER);

    let unreadable = root.path().join("unreadable.rs");
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000))
        .expect("the fixture file takes a mode of its own");
    if std::fs::read(&unreadable).is_ok() {
        // A run as root reads the file whatever its mode says, so there is
        // nothing here to fail over.
        return;
    }

    let output = cdva()
        .arg(NO_IGNORE)
        .arg(root.path())
        .output()
        .expect("the binary runs");

    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o644))
        .expect("the fixture file is readable again");

    assert!(
        output.status.success(),
        "one unreadable file does not stop the run: {}",
        stderr(&output)
    );
    let complaint = stderr(&output);
    assert!(
        complaint.contains("unreadable.rs"),
        "the warning names the file it skipped: {complaint}"
    );
    let table = stdout(&output);
    let row = fields(&table, RUST_ROW);
    assert_eq!(
        row[FILES], "1",
        "the file that could not be read is not counted:\n{table}"
    );
    assert_eq!(
        row[CODE], "3",
        "the rest of the tree is still counted:\n{table}"
    );
}

#[test]
fn the_fast_mode_reports_less_test_code_than_the_default_and_the_slow_mode_the_same() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY_WITH_A_TEST_MODULE);

    let run = |flags: &[&str]| {
        let mut command = cdva();
        command.arg(NO_IGNORE);
        for flag in flags {
            command.arg(flag);
        }
        let output = command.arg(root.path()).output().expect("the binary runs");
        assert!(
            output.status.success(),
            "a readable tree counts under {flags:?}: {}",
            stderr(&output)
        );
        stdout(&output)
    };

    let default = run(&[]);
    assert_eq!(
        fields(&default, RUST_ROW)[CODE],
        "10",
        "every row of code is counted:\n{default}"
    );
    assert_eq!(
        fields(&default, RUST_ROW)[TEST_CODE],
        "7",
        "the default mode parses the file and finds the module:\n{default}"
    );

    let fast = run(&["--no-tree"]);
    assert_eq!(
        fields(&fast, RUST_ROW)[TEST_CODE],
        "0",
        "--no-tree reads the path rule alone, and no glob names this file:\n{fast}"
    );
    assert_eq!(
        fields(&fast, RUST_ROW)[CODE],
        "10",
        "the rows are still counted, only bucketed differently:\n{fast}"
    );

    assert_eq!(
        run(&["--tree"]),
        default,
        "--tree skips the literal pre-filter and must reach the same table"
    );
}

#[test]
fn the_two_tree_flags_refuse_to_run_together() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY_WITH_A_TEST_MODULE);

    let output = cdva()
        .arg(NO_IGNORE)
        .arg("--no-tree")
        .arg("--tree")
        .arg(root.path())
        .output()
        .expect("the binary runs");

    assert!(
        !output.status.success(),
        "asking for no parse and for every parse at once is a mistake, not a \
         silent choice of one:\n{}",
        stdout(&output)
    );
    let complaint = stderr(&output);
    assert!(
        complaint.contains("--no-tree") && complaint.contains("--tree"),
        "the failure names the two flags that conflict: {complaint}"
    );
}

#[test]
fn the_two_bucket_flags_refuse_to_run_together() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY);

    let output = cdva()
        .arg(NO_IGNORE)
        .arg("--tests-only")
        .arg("--production-only")
        .arg(root.path())
        .output()
        .expect("the binary runs");

    assert!(
        !output.status.success(),
        "asking for the test bucket and the production bucket at once is a \
         mistake, not a silent choice of one:\n{}",
        stdout(&output)
    );
    let complaint = stderr(&output);
    assert!(
        complaint.contains("--tests-only") && complaint.contains("--production-only"),
        "the failure names the two flags that conflict: {complaint}"
    );
}

#[test]
fn the_sort_flag_takes_a_kebab_case_column_and_refuses_anything_else() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY);

    let accepted = cdva()
        .arg(NO_IGNORE)
        .arg("--sort")
        .arg("test-percent")
        .arg(root.path())
        .output()
        .expect("the binary runs");
    assert!(
        accepted.status.success(),
        "a column of two words is spelled with a hyphen: {}",
        stderr(&accepted)
    );

    let refused = cdva()
        .arg(NO_IGNORE)
        .arg("--sort")
        .arg("nonsense")
        .arg(root.path())
        .output()
        .expect("the binary runs");
    assert!(
        !refused.status.success(),
        "a column the report has no idea about is a mistake:\n{}",
        stdout(&refused)
    );
    let complaint = stderr(&refused);
    assert!(
        complaint.contains("test-percent") && complaint.contains("test-files"),
        "the failure lists the columns in the spelling the flag takes: {complaint}"
    );
}

#[test]
fn the_by_file_flag_names_the_files_and_the_top_flag_keeps_one_of_them() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY);
    write(root.path(), "tests/it.rs", INTEGRATION_TEST);

    let run = |flags: &[&str]| {
        let mut command = cdva();
        command.arg(NO_IGNORE);
        for flag in flags {
            command.arg(flag);
        }
        let output = command.arg(root.path()).output().expect("the binary runs");
        assert!(
            output.status.success(),
            "a readable tree counts under {flags:?}: {}",
            stderr(&output)
        );
        stdout(&output)
    };

    let by_file = run(&["--by-file"]);
    assert!(
        by_file.starts_with("File"),
        "the first column of a by-file report is the file:\n{by_file}"
    );
    let rows = labels(&by_file);
    assert_eq!(rows.len(), 3, "two files and the total:\n{by_file}");
    assert!(
        rows[0].ends_with("it.rs") && rows[1].ends_with("lib.rs"),
        "each file is named, the larger one first:\n{by_file}"
    );

    let trimmed = run(&["--by-file", "--top", "1"]);
    let kept = labels(&trimmed);
    assert_eq!(kept.len(), 2, "one file and the total:\n{trimmed}");
    assert!(
        kept[0].ends_with("it.rs"),
        "the file of the most code is the one kept:\n{trimmed}"
    );
    assert_eq!(
        fields(&trimmed, "Total")[FILES],
        "2",
        "the total still covers the file that was trimmed away:\n{trimmed}"
    );
}

#[test]
fn the_json_flag_writes_one_document_and_nothing_else() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY);
    write(root.path(), "tests/it.rs", INTEGRATION_TEST);

    let output = cdva()
        .arg(NO_IGNORE)
        .arg("--json")
        .arg(root.path())
        .output()
        .expect("the binary runs");

    assert!(
        output.status.success(),
        "a readable tree counts: {}",
        stderr(&output)
    );
    assert_eq!(
        output.stdout.first(),
        Some(&b'{'),
        "the first byte of the report is the document, and not a table or a heading:\n{}",
        stdout(&output)
    );

    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report parses as JSON");
    for key in ["rows", "total", "failed_parses", "unterminated_scans"] {
        assert!(
            document.get(key).is_some(),
            "the document holds `{key}`: {document}"
        );
    }
    assert_eq!(
        document["rows"][0]["language"], RUST_ROW,
        "the one language of the tree is named: {document}"
    );
    assert_eq!(
        document["total"]["code"], 7,
        "every row of code is counted: {document}"
    );
    assert_eq!(
        document["total"]["test"]["code"], 4,
        "the code of the file under tests/ is test code: {document}"
    );
    assert_eq!(
        document["total"]["production"]["code"], 3,
        "the production bucket is carried rather than left to be subtracted: {document}"
    );

    let table = stdout(&output);
    assert!(
        !table.contains("Language") && !table.contains("Test %"),
        "a machine format prints no table:\n{table}"
    );
}

#[test]
fn the_csv_flag_writes_the_documented_header_and_nothing_else() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY);
    write(root.path(), "tests/it.rs", INTEGRATION_TEST);

    let output = cdva()
        .arg(NO_IGNORE)
        .arg("--csv")
        .arg(root.path())
        .output()
        .expect("the binary runs");

    assert!(
        output.status.success(),
        "a readable tree counts: {}",
        stderr(&output)
    );

    let report = stdout(&output);
    let records: Vec<Vec<String>> = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(report.as_bytes())
        .records()
        .map(|record| {
            let record = record.expect("the report parses as CSV");
            record.iter().map(str::to_string).collect()
        })
        .collect();

    assert_eq!(records.len(), 3, "one language, then the total:\n{report}");
    assert_eq!(
        records[0], CSV_HEADER,
        "the first record is the documented header:\n{report}"
    );
    assert_eq!(
        records[2][0], "Total",
        "the total is the last record:\n{report}"
    );
    assert!(
        !report.contains("Test %"),
        "a machine format prints no table:\n{report}"
    );
}

#[test]
fn the_two_format_flags_refuse_to_run_together() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY);

    let output = cdva()
        .arg(NO_IGNORE)
        .arg("--json")
        .arg("--csv")
        .arg(root.path())
        .output()
        .expect("the binary runs");

    assert!(
        !output.status.success(),
        "asking for two reports at once is a mistake, not a silent choice of one:\n{}",
        stdout(&output)
    );
    let complaint = stderr(&output);
    assert!(
        complaint.contains("--json") && complaint.contains("--csv"),
        "the failure names the two flags that conflict: {complaint}"
    );
}

#[test]
fn a_machine_format_keeps_a_warning_on_standard_error_and_out_of_the_report() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY);
    write(root.path(), "tests/it.rs", INTEGRATION_TEST);

    for flag in ["--json", "--csv"] {
        let output = cdva()
            .arg(NO_IGNORE)
            .arg(flag)
            .arg("--by-file")
            .arg("--top")
            .arg("1")
            .arg(root.path())
            .output()
            .expect("the binary runs");

        assert!(
            output.status.success(),
            "{flag} counts a readable tree: {}",
            stderr(&output)
        );
        let report = stdout(&output);
        assert!(
            report.lines().count() > 1,
            "{flag} writes a report:\n{report}"
        );
        assert!(
            !report.contains("cdva:"),
            "{flag} keeps every warning off standard output:\n{report}"
        );
    }
}

#[test]
fn explaining_a_file_another_file_declared_as_its_test_module_names_the_declaration() {
    // The walk is what makes this answerable. The declaration lives in
    // src/lib.rs and the rows it marks live in src/tests.rs, so a run that
    // counted the named file alone would report no span at all — while the
    // table went on calling the whole file test code.
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", DECLARES_A_TEST_MODULE);
    write(root.path(), "src/tests.rs", DECLARED_TEST_MODULE);

    let output = cdva()
        .current_dir(root.path())
        .arg(NO_IGNORE)
        .arg(EXPLAIN)
        .arg("src/tests.rs")
        .output()
        .expect("the binary runs");

    assert!(
        output.status.success(),
        "a file the walk counted is explained: {}",
        stderr(&output)
    );
    let explanation = stdout(&output);
    assert!(
        explanation.contains("src/tests.rs"),
        "the header names the file that was asked about:\n{explanation}"
    );
    assert!(
        explanation.contains("rows 1..=5"),
        "the declaration in the other file marks the whole of this one:\n{explanation}"
    );
    assert!(
        explanation.contains("mod tests;"),
        "the span names the declaration that marked it:\n{explanation}"
    );
    assert!(
        !explanation.contains(NO_RULE),
        "a file the table calls test code has a reason, and this is it:\n{explanation}"
    );
}

#[test]
fn explaining_a_file_a_glob_marked_names_the_glob() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "tests/it.rs", INTEGRATION_TEST);

    let output = cdva()
        .current_dir(root.path())
        .arg(NO_IGNORE)
        .arg(EXPLAIN)
        .arg("tests/it.rs")
        .output()
        .expect("the binary runs");

    assert!(
        output.status.success(),
        "a file the walk counted is explained: {}",
        stderr(&output)
    );
    let explanation = stdout(&output);
    assert!(
        explanation.contains("tests/**"),
        "the glob of the built-in table is named:\n{explanation}"
    );
    assert!(
        explanation.contains("rows 1..=4"),
        "the glob marks the whole file:\n{explanation}"
    );
}

#[test]
fn explaining_a_file_that_does_not_exist_fails_and_says_so() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY);

    let output = cdva()
        .current_dir(root.path())
        .arg(NO_IGNORE)
        .arg(EXPLAIN)
        .arg("src/no-such-file.rs")
        .output()
        .expect("the binary runs");

    assert!(
        !output.status.success(),
        "a file that is not there is a mistake, not an empty explanation:\n{}",
        stdout(&output)
    );
    let complaint = stderr(&output);
    assert!(
        complaint.contains("src/no-such-file.rs") && complaint.contains("does not exist"),
        "the failure names the path and the reason: {complaint}"
    );
}

#[test]
fn explaining_a_file_of_an_extension_the_tool_does_not_count_names_the_extension() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY);
    write(root.path(), "notes.txt", NOTES);

    let output = cdva()
        .current_dir(root.path())
        .arg(NO_IGNORE)
        .arg(EXPLAIN)
        .arg("notes.txt")
        .output()
        .expect("the binary runs");

    assert!(
        !output.status.success(),
        "a file of no language the tool counts is a mistake:\n{}",
        stdout(&output)
    );
    let complaint = stderr(&output);
    assert!(
        complaint.contains("notes.txt") && complaint.contains("txt"),
        "the failure names the file and its extension: {complaint}"
    );
    assert!(
        complaint.contains("language"),
        "the failure says the extension names no language it counts: {complaint}"
    );
}

#[test]
fn explaining_a_file_an_ignore_file_excluded_says_so_and_suggests_the_flag_that_reaches_it() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), ".gitignore", "secret.rs\n");
    write(root.path(), "src/lib.rs", LIBRARY);
    write(root.path(), "secret.rs", HELPER);

    let ignored = cdva()
        .current_dir(root.path())
        .arg(EXPLAIN)
        .arg("secret.rs")
        .output()
        .expect("the binary runs");

    assert!(
        !ignored.status.success(),
        "a file the walk never reached cannot be explained:\n{}",
        stdout(&ignored)
    );
    let complaint = stderr(&ignored);
    assert!(
        complaint.contains("secret.rs") && complaint.contains(NO_IGNORE),
        "the failure names the file and the flag that would reach it: {complaint}"
    );

    let reached = cdva()
        .current_dir(root.path())
        .arg(NO_IGNORE)
        .arg(EXPLAIN)
        .arg("secret.rs")
        .output()
        .expect("the binary runs");

    assert!(
        reached.status.success(),
        "the same command reaches the file under {NO_IGNORE}: {}",
        stderr(&reached)
    );
    assert!(
        stdout(&reached).contains("secret.rs"),
        "the file the flag reached is the file explained:\n{}",
        stdout(&reached)
    );
}

#[test]
fn an_explanation_reads_the_same_flags_the_table_reads() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY_WITH_A_TEST_MODULE);

    let run = |flags: &[&str]| {
        let mut command = cdva();
        command.current_dir(root.path());
        command.arg(NO_IGNORE);
        for flag in flags {
            command.arg(flag);
        }
        let output = command
            .arg(EXPLAIN)
            .arg("src/lib.rs")
            .output()
            .expect("the binary runs");
        assert!(
            output.status.success(),
            "a file the walk counted is explained under {flags:?}: {}",
            stderr(&output)
        );
        stdout(&output)
    };

    let default = run(&[]);
    assert!(
        default.contains("mod_item"),
        "the tree rule of the default run found the test module:\n{default}"
    );

    let fast = run(&["--no-tree"]);
    assert!(
        !fast.contains(".."),
        "--no-tree reads no tree, so no span of one is explained:\n{fast}"
    );
    assert!(
        fast.contains(NO_RULE),
        "a file no rule marked says where its rows went:\n{fast}"
    );
}

#[test]
fn the_explanation_and_the_machine_formats_refuse_to_run_together() {
    let root = tempfile::tempdir().expect("a temporary directory is made");
    write(root.path(), "src/lib.rs", LIBRARY);

    for flag in ["--json", "--csv"] {
        let output = cdva()
            .current_dir(root.path())
            .arg(NO_IGNORE)
            .arg(flag)
            .arg(EXPLAIN)
            .arg("src/lib.rs")
            .output()
            .expect("the binary runs");

        assert!(
            !output.status.success(),
            "{flag} promises a machine format, and an explanation is not one:\n{}",
            stdout(&output)
        );
        let complaint = stderr(&output);
        assert!(
            complaint.contains(EXPLAIN) && complaint.contains(flag),
            "the failure names the two flags that conflict: {complaint}"
        );
    }
}

#[test]
fn the_help_names_the_strict_flag_and_its_no_tree_caveat() {
    let output = cdva().arg("--help").output().expect("the binary runs");

    assert!(
        output.status.success(),
        "the help flag succeeds: {}",
        stderr(&output)
    );
    let section = help_section(&stdout(&output), STRICT);
    assert!(
        !section.is_empty(),
        "the long help documents {STRICT}:\n{}",
        stdout(&output)
    );
    assert!(
        section.contains(NO_TREE),
        "the help of {STRICT} names the flag that quietly satisfies it:\n{section}"
    );
    assert!(
        section.contains("passes"),
        "the help of {STRICT} says what {NO_TREE} does to it:\n{section}"
    );
}

#[test]
fn a_file_whose_parse_failed_is_named_in_a_footer_and_a_clean_tree_prints_none() {
    let root = broken_tree();

    let output = cdva()
        .arg(NO_IGNORE)
        .arg(root.path())
        .output()
        .expect("the binary runs");

    assert!(
        output.status.success(),
        "a failed parse alone does not fail the run: {}",
        stderr(&output)
    );
    let report = stdout(&output);
    assert!(
        report.contains(ONE_FAILURE_FOOTER),
        "the footer says how many files failed, and where their rows went:\n{report}"
    );
    assert!(
        report.contains("broken.rs"),
        "the footer names the file, so a reader can go and look at it:\n{report}"
    );
    assert!(
        !report.contains("src/lib.rs"),
        "the file that parsed is not a failure:\n{report}"
    );

    let clean = tempfile::tempdir().expect("a temporary directory is made");
    write(clean.path(), "src/lib.rs", LIBRARY);
    let quiet = cdva()
        .arg(NO_IGNORE)
        .arg(clean.path())
        .output()
        .expect("the binary runs");

    assert!(
        !stdout(&quiet).contains(FAILED_TO_PARSE),
        "a tree that parsed clean has no footer at all:\n{}",
        stdout(&quiet)
    );
}

#[test]
fn strict_fails_the_run_over_a_failed_parse_and_prints_the_same_report() {
    let root = broken_tree();

    let lenient = cdva()
        .arg(NO_IGNORE)
        .arg(root.path())
        .output()
        .expect("the binary runs");
    let strict = cdva()
        .arg(NO_IGNORE)
        .arg(STRICT)
        .arg(root.path())
        .output()
        .expect("the binary runs");

    assert!(
        lenient.status.success(),
        "without {STRICT} a failed parse never changes the exit status: {}",
        stderr(&lenient)
    );
    assert!(
        !strict.status.success(),
        "{STRICT} is what turns a silent undercount into a failing run:\n{}",
        stdout(&strict)
    );
    assert_eq!(
        stdout(&strict),
        stdout(&lenient),
        "the report is the report, whatever the exit status says about it"
    );
}

#[test]
fn the_strict_complaint_goes_to_standard_error_and_not_into_the_report() {
    let root = broken_tree();

    let output = cdva()
        .arg(NO_IGNORE)
        .arg(STRICT)
        .arg(root.path())
        .output()
        .expect("the binary runs");

    assert!(
        !output.status.success(),
        "the run failed:\n{}",
        stdout(&output)
    );
    let complaint = stderr(&output);
    assert!(
        complaint.contains(STRICT) && complaint.contains('1'),
        "the complaint names the flag that refused, and how many files it refused over: {complaint}"
    );
    assert!(
        !stdout(&output).contains(STRICT),
        "standard output carries the report and nothing else:\n{}",
        stdout(&output)
    );
}

#[test]
fn strict_under_no_tree_passes_because_nothing_was_parsed() {
    let root = broken_tree();

    let fast = cdva()
        .arg(NO_IGNORE)
        .arg(NO_TREE)
        .arg(STRICT)
        .arg(root.path())
        .output()
        .expect("the binary runs");

    assert!(
        fast.status.success(),
        "{NO_TREE} parses nothing, so no parse can fail: {}",
        stderr(&fast)
    );
    assert!(
        !stdout(&fast).contains(FAILED_TO_PARSE),
        "a run that parsed nothing names no failure:\n{}",
        stdout(&fast)
    );

    let parsed = cdva()
        .arg(NO_IGNORE)
        .arg(STRICT)
        .arg(root.path())
        .output()
        .expect("the binary runs");

    assert!(
        !parsed.status.success(),
        "the same tree, and the same flag, over a run that did parse:\n{}",
        stdout(&parsed)
    );
}

#[test]
fn a_machine_format_prints_no_footer_and_the_json_names_the_failure() {
    let root = broken_tree();

    for flag in ["--json", "--csv"] {
        let output = cdva()
            .arg(NO_IGNORE)
            .arg(flag)
            .arg(root.path())
            .output()
            .expect("the binary runs");

        assert!(
            output.status.success(),
            "{flag} counts a tree holding a broken file: {}",
            stderr(&output)
        );
        assert!(
            !stdout(&output).contains(FAILED_TO_PARSE),
            "{flag} carries the failures as data, so a line of prose in it is a line to strip:\n{}",
            stdout(&output)
        );
    }

    let document = cdva()
        .arg(NO_IGNORE)
        .arg("--json")
        .arg(root.path())
        .output()
        .expect("the binary runs");
    let parsed: serde_json::Value =
        serde_json::from_slice(&document.stdout).expect("the report parses as JSON");
    let failures = parsed["failed_parses"]
        .as_array()
        .expect("the document carries a list of failures");
    assert_eq!(failures.len(), 1, "one file failed: {parsed}");
    assert!(
        failures[0]
            .as_str()
            .is_some_and(|path| path.ends_with("broken.rs")),
        "the document names the file, as the footer would have: {parsed}"
    );
}

#[test]
fn a_file_whose_scan_ended_unterminated_is_named_in_a_footer_and_a_clean_tree_prints_none() {
    let root = unterminated_tree();

    let output = cdva()
        .arg(NO_IGNORE)
        .arg(root.path())
        .output()
        .expect("the binary runs");

    assert!(
        output.status.success(),
        "a scan that did not end alone does not fail the run: {}",
        stderr(&output)
    );
    let report = stdout(&output);
    assert!(
        report.contains(ONE_UNTERMINATED_FOOTER),
        "the footer says how many scans did not end, and which counts that spoils:\n{report}"
    );
    assert!(
        report.contains("regex.js"),
        "the footer names the file, so a reader can go and look at it:\n{report}"
    );
    assert!(
        !report.contains("src/lib.rs"),
        "the file that scanned clean is not a fault:\n{report}"
    );
    assert!(
        !report.contains(FAILED_TO_PARSE),
        "a scan that did not end is not a parse that failed, and one footer must not answer for \
         the other:\n{report}"
    );

    let clean = tempfile::tempdir().expect("a temporary directory is made");
    write(clean.path(), "src/lib.rs", LIBRARY);
    let quiet = cdva()
        .arg(NO_IGNORE)
        .arg(clean.path())
        .output()
        .expect("the binary runs");

    assert!(
        !stdout(&quiet).contains(ENDED_INSIDE),
        "a tree that scanned clean has no footer at all:\n{}",
        stdout(&quiet)
    );

    let broken = cdva()
        .arg(NO_IGNORE)
        .arg(broken_tree().path())
        .output()
        .expect("the binary runs");

    assert!(
        !stdout(&broken).contains(ENDED_INSIDE),
        "a parse that failed is not a scan that did not end, and one footer must not answer for \
         the other:\n{}",
        stdout(&broken)
    );
}

#[test]
fn a_machine_format_prints_no_unterminated_footer_and_the_json_names_the_scan() {
    let root = unterminated_tree();

    for flag in ["--json", "--csv"] {
        let output = cdva()
            .arg(NO_IGNORE)
            .arg(flag)
            .arg(root.path())
            .output()
            .expect("the binary runs");

        assert!(
            output.status.success(),
            "{flag} counts a tree holding a file whose scan did not end: {}",
            stderr(&output)
        );
        assert!(
            !stdout(&output).contains(ENDED_INSIDE),
            "{flag} carries the scans as data, so a line of prose in it is a line to strip:\n{}",
            stdout(&output)
        );
    }

    let document = cdva()
        .arg(NO_IGNORE)
        .arg("--json")
        .arg(root.path())
        .output()
        .expect("the binary runs");
    let parsed: serde_json::Value =
        serde_json::from_slice(&document.stdout).expect("the report parses as JSON");
    let scans = parsed["unterminated_scans"]
        .as_array()
        .expect("the document carries a list of the scans that did not end");
    assert_eq!(scans.len(), 1, "one scan did not end: {parsed}");
    assert!(
        scans[0]
            .as_str()
            .is_some_and(|path| path.ends_with("regex.js")),
        "the document names the file, as the footer would have: {parsed}"
    );
    assert_eq!(
        parsed["failed_parses"].as_array().map(Vec::len),
        Some(0),
        "no parse failed over this tree, and the two lists are two lists: {parsed}"
    );
}

#[test]
fn strict_passes_over_an_unterminated_scan_because_no_row_left_its_bucket() {
    let root = unterminated_tree();

    let output = cdva()
        .arg(NO_IGNORE)
        .arg(STRICT)
        .arg(root.path())
        .output()
        .expect("the binary runs");

    // The two faults are not one fault. A parse that failed puts every row of
    // its file in the production bucket, which is the split this tool exists
    // to report. A scan that did not end moves rows between the comment count
    // and the code count of one file and moves no row between the buckets, so
    // it is the smaller fault and it is a documented limit of the language
    // table rather than a broken tree. `--strict` answers for the parse, and
    // the footer answers for the scan.
    assert!(
        output.status.success(),
        "{STRICT} asks about the parse, and no parse failed over this tree:\n{}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains(ENDED_INSIDE),
        "the report still names the file, whatever {STRICT} makes of it:\n{}",
        stdout(&output)
    );
}

#[test]
fn json_under_strict_writes_a_whole_document_and_still_fails() {
    let root = broken_tree();

    let output = cdva()
        .arg(NO_IGNORE)
        .arg("--json")
        .arg(STRICT)
        .arg(root.path())
        .output()
        .expect("the binary runs");

    assert!(
        !output.status.success(),
        "a machine that asked for {STRICT} asked for the failing status:\n{}",
        stdout(&output)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("a failing run still writes a document that parses");
    assert_eq!(
        parsed["failed_parses"].as_array().map(Vec::len),
        Some(1),
        "the document is whole: the exit status corrupts nothing: {parsed}"
    );
}

#[test]
fn an_explanation_prints_no_footer_and_still_answers_to_strict() {
    let root = broken_tree();

    let output = cdva()
        .current_dir(root.path())
        .arg(NO_IGNORE)
        .arg(EXPLAIN)
        .arg("src/broken.rs")
        .output()
        .expect("the binary runs");

    assert!(
        output.status.success(),
        "a file the walk counted is explained: {}",
        stderr(&output)
    );
    let explanation = stdout(&output);
    assert!(
        explanation.contains("the parse failed"),
        "the header of the one file asked about says what happened to it:\n{explanation}"
    );
    assert!(
        !explanation.contains(FAILED_TO_PARSE),
        "an explanation is about one file, and the footer is about the run:\n{explanation}"
    );

    let strict = cdva()
        .current_dir(root.path())
        .arg(NO_IGNORE)
        .arg(STRICT)
        .arg(EXPLAIN)
        .arg("src/broken.rs")
        .output()
        .expect("the binary runs");

    assert!(
        !strict.status.success(),
        "{STRICT} asks about the run, and this run held a failed parse:\n{}",
        stdout(&strict)
    );
    assert_eq!(
        stdout(&strict),
        explanation,
        "the explanation is the explanation, whatever the exit status says about it"
    );
}

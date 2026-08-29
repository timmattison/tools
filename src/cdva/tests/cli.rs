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
        "--no-tree",
        "--tree",
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

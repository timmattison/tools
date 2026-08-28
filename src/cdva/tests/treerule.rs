//! The tree rule, over the Rust fixture corpus.
//!
//! Each fixture is a pair of files under `tests/fixtures/rust/`: `<name>.rs`
//! holds production code and test code side by side, and `<name>.expected`
//! holds the same rows with `T ` in front of every row the tool must mark as a
//! test row and `. ` in front of every row it must not. The test renders what
//! the tool actually marked in that same format and compares the two strings,
//! so a failure names the row that moved. A count alone never does: two rows
//! swapping buckets leaves the count unchanged.
//!
//! There is one fixture per syntactic form the rule has to read, and a
//! coverage test asserts that the list below is the set of files on disk. A
//! fixture added without an expectation, or an expectation left behind by a
//! fixture that was renamed, shows up as a set difference rather than as
//! silence.
//!
//! Nothing here writes a file, and nothing here shells out. The fixtures are
//! read, never modified, so two copies of this file running at once cannot
//! tread on each other.

use cdva::{
    lines, Counter, FileCount, Language, ParseStatus, PathRules, PathVerdict, Rule, Span, TreeRules,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The directory that holds the Rust fixtures.
const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rust");

/// Every Rust fixture, one per syntactic form the rule has to read.
///
/// A coverage test asserts that this list is exactly the set of `.rs` files in
/// the fixture directory, and exactly the set of `.expected` files beside them.
const FIXTURES: &[&str] = &[
    "bench_fn",
    "cfg_all_test",
    "cfg_test_mod",
    "cfg_test_mod_declaration",
    "doc_test",
    "missing_node",
    "multibyte_mod",
    "nested_overlap",
    "no_tests",
    "rstest_stack",
    "syntax_error",
    "test_fn",
    "tokio_test_fn",
];

/// The fixtures whose source holds a defect, so the parse must fail.
const DEFECTIVE: &[&str] = &["missing_node", "syntax_error"];

/// The prefix that marks a test row in an expectation.
const TEST_PREFIX: &str = "T ";

/// The prefix that marks a production row in an expectation.
const PRODUCTION_PREFIX: &str = ". ";

/// The source of a fixture.
fn source(name: &str) -> String {
    read(&PathBuf::from(FIXTURE_DIR).join(format!("{name}.rs")))
}

/// The expectation beside a fixture.
fn expectation(name: &str) -> String {
    read(&PathBuf::from(FIXTURE_DIR).join(format!("{name}.expected")))
}

/// Reads a file of the corpus, and says which one when it cannot.
fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The path the rules see for a fixture.
///
/// It is deliberately not the path the fixture lives at. `tests/**` and
/// `fixtures/**` are both built-in test globs, so the real path would put every
/// fixture wholly in the test bucket and no assertion below would ever reach the
/// tree rule. A test asserts that this path is one no built-in glob marks.
fn as_counted(name: &str) -> PathBuf {
    PathBuf::from("src").join(format!("{name}.rs"))
}

/// Counts a fixture with both rules on.
fn counted(name: &str, source: &str) -> FileCount {
    let counter = Counter::new(PathRules::builtin()).with_tree_rules(TreeRules::new());
    let path = as_counted(name);
    counter
        .count_source(&path, &path, source)
        .unwrap_or_else(|| panic!("`{name}` is a language the tool counts"))
}

/// The 1-based rows the spans cover, each row once however many spans hold it.
fn marked_rows(spans: &[Span]) -> BTreeSet<u32> {
    spans
        .iter()
        .flat_map(|span| span.first_row..=span.last_row)
        .collect()
}

/// Renders the marking of a counted file in the format of an expectation.
fn render(source: &str, counted: &FileCount) -> String {
    let rows = marked_rows(&counted.spans);
    let mut rendered = String::new();
    for (offset, line) in source.lines().enumerate() {
        let row = u32::try_from(offset + 1).unwrap_or(u32::MAX);
        rendered.push_str(if rows.contains(&row) {
            TEST_PREFIX
        } else {
            PRODUCTION_PREFIX
        });
        rendered.push_str(line);
        rendered.push('\n');
    }
    rendered
}

/// Asserts that the tool marks a fixture exactly as its expectation says.
fn assert_marking(name: &str) {
    let source = source(name);
    let counted = counted(name, &source);
    assert_eq!(
        render(&source, &counted),
        expectation(name),
        "the marking of fixture `{name}`"
    );
}

/// The base names of the files in the fixture directory with this extension.
fn fixture_names(extension: &str) -> BTreeSet<String> {
    std::fs::read_dir(FIXTURE_DIR)
        .unwrap_or_else(|error| panic!("{FIXTURE_DIR}: {error}"))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()?.to_str()? != extension {
                return None;
            }
            Some(path.file_stem()?.to_str()?.to_string())
        })
        .collect()
}

/// The names this file claims to cover.
fn listed_names() -> BTreeSet<String> {
    FIXTURES.iter().map(|name| (*name).to_string()).collect()
}

#[test]
fn cfg_test_mod_marks_the_module_and_the_attribute_above_it() {
    assert_marking("cfg_test_mod");
}

#[test]
fn cfg_test_mod_declaration_marks_the_two_rows_of_the_declaration() {
    assert_marking("cfg_test_mod_declaration");
}

#[test]
fn test_fn_marks_a_bare_test_function() {
    assert_marking("test_fn");
}

#[test]
fn tokio_test_fn_marks_a_test_attribute_that_names_a_path() {
    assert_marking("tokio_test_fn");
}

#[test]
fn cfg_all_test_marks_a_module_gated_on_test_and_a_feature() {
    assert_marking("cfg_all_test");
}

#[test]
fn rstest_stack_reaches_back_over_the_whole_chain_of_attributes() {
    assert_marking("rstest_stack");
}

#[test]
fn bench_fn_marks_a_benchmark() {
    assert_marking("bench_fn");
}

#[test]
fn nested_overlap_marks_each_row_once() {
    assert_marking("nested_overlap");
}

#[test]
fn doc_test_marks_nothing_and_leaves_the_fenced_rows_comments() {
    assert_marking("doc_test");

    let source = source("doc_test");
    let counted = counted("doc_test", &source);
    assert_eq!(counted.test.total(), 0, "a doc test is not a test node");
    assert!(
        counted.production.comment >= 5,
        "the doc comment and its fence stay comment rows: {:?}",
        counted.production
    );
}

#[test]
fn no_tests_marks_nothing() {
    assert_marking("no_tests");
}

#[test]
fn syntax_error_marks_nothing() {
    assert_marking("syntax_error");
}

#[test]
fn missing_node_marks_nothing() {
    assert_marking("missing_node");
}

#[test]
fn multibyte_mod_marks_the_module_that_holds_characters_of_many_bytes() {
    assert_marking("multibyte_mod");
}

#[test]
fn every_fixture_has_an_expectation_and_every_expectation_a_fixture() {
    let sources = fixture_names("rs");
    let expectations = fixture_names("expected");
    assert_eq!(
        sources, expectations,
        "a fixture with no expectation, or an expectation with no fixture, in {FIXTURE_DIR}"
    );
    assert!(!sources.is_empty(), "the fixture directory is empty");
}

#[test]
fn every_fixture_on_disk_is_named_in_the_list_this_file_covers() {
    assert_eq!(
        fixture_names("rs"),
        listed_names(),
        "a fixture nobody asserts on, or a name in FIXTURES with no fixture"
    );
}

#[test]
fn every_expectation_carries_the_rows_of_its_fixture() {
    for name in FIXTURES {
        let stripped: String = expectation(name)
            .lines()
            .map(|line| {
                let row = line
                    .strip_prefix(TEST_PREFIX)
                    .or_else(|| line.strip_prefix(PRODUCTION_PREFIX))
                    .unwrap_or_else(|| panic!("`{name}`: a row with no marking: {line:?}"));
                format!("{row}\n")
            })
            .collect();
        assert_eq!(
            stripped,
            source(name),
            "the expectation of `{name}` no longer holds the rows of the fixture"
        );
    }
}

#[test]
fn no_fixture_path_is_marked_by_the_path_rule() {
    let rules = PathRules::builtin();
    for name in FIXTURES {
        assert_eq!(
            rules.verdict(&as_counted(name)),
            PathVerdict::Unmarked,
            "a built-in glob marks `{name}`, so the tree rule never runs over it"
        );
    }
}

#[test]
fn nested_overlap_yields_two_spans_over_one_set_of_rows() {
    let source = source("nested_overlap");
    let counted = counted("nested_overlap", &source);

    assert!(
        counted.spans.len() > 1,
        "the module and the function inside it are two spans: {:?}",
        counted.spans
    );
    let rows = marked_rows(&counted.spans);
    let length: usize = counted
        .spans
        .iter()
        .map(|span| (span.last_row - span.first_row + 1) as usize)
        .sum();
    assert!(
        length > rows.len(),
        "the two spans must overlap for this fixture to be worth anything"
    );
    assert_eq!(
        u64::try_from(rows.len()).unwrap_or(u64::MAX),
        counted.test.total(),
        "a row two spans hold is one test row, not two"
    );
}

#[test]
fn the_spans_and_the_test_bucket_agree_for_every_fixture() {
    for name in FIXTURES {
        let source = source(name);
        let counted = counted(name, &source);
        assert_eq!(
            u64::try_from(marked_rows(&counted.spans).len()).unwrap_or(u64::MAX),
            counted.test.total(),
            "`{name}`: the rows the spans cover are the rows in the test bucket"
        );
    }
}

#[test]
fn the_two_buckets_sum_to_the_unsplit_count_for_every_fixture() {
    for name in FIXTURES {
        let source = source(name);
        let counted = counted(name, &source);
        assert_eq!(
            counted.total(),
            lines::count(&source, Language::Rust),
            "`{name}`: the split changed the count"
        );
    }
}

#[test]
fn every_span_of_the_tree_rule_names_the_node_kind_it_matched() {
    let source = source("cfg_test_mod");
    let counted = counted("cfg_test_mod", &source);
    let kinds: Vec<&Rule> = counted.spans.iter().map(|span| &span.rule).collect();
    assert!(
        kinds.contains(&&Rule::TreeNode("mod_item".to_string())),
        "the module is a mod_item: {kinds:?}"
    );
    assert!(
        kinds.contains(&&Rule::TreeNode("function_item".to_string())),
        "each test is a function_item: {kinds:?}"
    );
}

#[test]
fn a_fixture_with_no_defect_parses_clean() {
    for name in FIXTURES {
        if DEFECTIVE.contains(name) {
            continue;
        }
        let source = source(name);
        assert_eq!(
            counted(name, &source).parse_status,
            ParseStatus::Clean,
            "`{name}` holds no defect"
        );
    }
}

#[test]
fn a_syntax_error_fails_the_parse_and_leaves_the_whole_file_production() {
    let source = source("syntax_error");
    let counted = counted("syntax_error", &source);
    assert_eq!(counted.parse_status, ParseStatus::Failed);
    assert_eq!(
        counted.test.total(),
        0,
        "a file we could not read must not carry a guessed test count"
    );
    assert_eq!(counted.production, lines::count(&source, Language::Rust));
}

#[test]
fn a_missing_token_fails_the_parse_although_the_tree_holds_no_error_node() {
    let source = source("missing_node");
    let counted = counted("missing_node", &source);
    assert_eq!(
        counted.parse_status,
        ParseStatus::Failed,
        "a node the parser inserted to recover is a defect too"
    );
    assert_eq!(counted.test.total(), 0);
}

#[test]
fn a_language_with_no_tree_rule_is_not_parsed() {
    let source = "package main\n\nfunc TestThing(t *testing.T) {\n\tt.Fatal(\"x\")\n}\n";
    let path = Path::new("src/thing.go");
    let counter = Counter::new(PathRules::builtin()).with_tree_rules(TreeRules::new());
    let counted = counter
        .count_source(path, path, source)
        .expect("Go is a language the tool counts");

    assert_eq!(counted.language, Language::Go);
    assert_eq!(counted.parse_status, ParseStatus::NotParsed);
    assert_eq!(
        counted.test.total(),
        0,
        "Go's rule arrives in a later slice"
    );
    assert!(TreeRules::new().outcome(source, Language::Go).is_none());
}

#[test]
fn a_file_the_path_rule_marked_is_not_parsed() {
    let source = source("cfg_test_mod");
    let path = Path::new("tests/cfg_test_mod.rs");
    let counter = Counter::new(PathRules::builtin()).with_tree_rules(TreeRules::new());
    let counted = counter
        .count_source(path, path, &source)
        .expect("Rust is a language the tool counts");

    assert_eq!(
        counted.parse_status,
        ParseStatus::NotParsed,
        "the file is already wholly test material, so a parse would buy nothing"
    );
    assert_eq!(counted.production.total(), 0);
    assert_eq!(counted.test, lines::count(&source, Language::Rust));
    assert_eq!(
        counted
            .spans
            .iter()
            .map(|span| &span.rule)
            .collect::<Vec<_>>(),
        vec![&Rule::PathGlob("tests/**".to_string())],
        "the glob marked it, not the tree"
    );
}

#[test]
fn a_counter_with_no_tree_rules_marks_no_test_row() {
    let source = source("cfg_test_mod");
    let path = as_counted("cfg_test_mod");
    let counted = Counter::new(PathRules::builtin())
        .count_source(&path, &path, &source)
        .expect("Rust is a language the tool counts");

    assert_eq!(
        counted.test.total(),
        0,
        "this is what --no-tree will report for a file with a #[cfg(test)] mod"
    );
    assert!(counted.spans.is_empty());
    assert_eq!(counted.parse_status, ParseStatus::NotParsed);
    assert_eq!(counted.production, lines::count(&source, Language::Rust));
}

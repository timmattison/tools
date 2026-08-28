//! The tree rule, over the fixture corpus of every language that has one.
//!
//! Each fixture is a pair of files under `tests/fixtures/<language>/`:
//! `<name>.<extension>` holds production code and test code side by side, and
//! `<name>.expected` holds the same rows with `T ` in front of every row the
//! tool must mark as a test row and `. ` in front of every row it must not. The
//! test renders what the tool actually marked in that same format and compares
//! the two strings, so a failure names the row that moved. A count alone never
//! does: two rows swapping buckets leaves the count unchanged.
//!
//! There is one fixture per syntactic form the rule has to read, and three
//! guards keep the corpus honest. A coverage test asserts that the list of each
//! [`Corpus`] is the set of files on disk, so a fixture added without an
//! expectation — or an expectation left behind by a fixture that was renamed —
//! shows up as a set difference rather than as silence. A second asserts that
//! the set of corpora is the set of languages the table gives a tree rule, so a
//! language that gains a rule and gains no fixture shows up the same way. A
//! third asserts that no fixture path is one the *path* rule would mark, since
//! a fixture the globs claim never reaches the tree rule at all and its
//! assertion would then pass for the wrong reason.
//!
//! Nothing here writes a file, and nothing here shells out. The fixtures are
//! read, never modified, so two copies of this file running at once cannot
//! tread on each other.

use cdva::{
    lines, Counter, FileCount, Language, ParseStatus, PathRules, PathVerdict, Rule, Span, TreeRules,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The directory that holds one directory of fixtures per language.
const FIXTURE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

/// The prefix that marks a test row in an expectation.
const TEST_PREFIX: &str = "T ";

/// The prefix that marks a production row in an expectation.
const PRODUCTION_PREFIX: &str = ". ";

/// The fixtures of one language, and where they live.
struct Corpus {
    /// The language the fixtures are counted under.
    language: Language,
    /// The directory under [`FIXTURE_ROOT`] that holds them.
    directory: &'static str,
    /// The extension every source file of this corpus carries, which is also
    /// what [`Language::from_path`] reads the language out of.
    extension: &'static str,
    /// Every fixture of the corpus, one per syntactic form the rule reads.
    fixtures: &'static [&'static str],
    /// The fixtures whose source holds a defect, so the parse must fail.
    defective: &'static [&'static str],
}

/// Every Rust fixture, one per syntactic form the rule has to read.
const RUST_FIXTURES: &[&str] = &[
    "bench_fn",
    "cfg_all_test",
    "cfg_test_mod",
    "cfg_test_mod_declaration",
    "doc_test",
    "missing_node",
    "multibyte_mod",
    "multibyte_mod_declaration",
    "nested_overlap",
    "no_tests",
    "plain_mod_declaration",
    "rstest_stack",
    "syntax_error",
    "test_fn",
    "tokio_test_fn",
];

/// Every Go fixture. There is one per prefix `go test` recognises, and one
/// negative that holds the two names the trailing `([A-Z_]|$)` of the pattern
/// exists to refuse.
const GO_FIXTURES: &[&str] = &[
    "benchmark_functions",
    "example_functions",
    "fuzz_functions",
    "multibyte",
    "negative",
    "test_functions",
];

/// Every Zig fixture. Zig needs the fewest of any language here, because its
/// grammar names a test outright and no heuristic over a name can go wrong.
const ZIG_FIXTURES: &[&str] = &["multibyte", "no_tests", "test_declaration"];

/// Every Python fixture, one per pattern of the five the query holds, plus the
/// two that pin what the parser does with a file it cannot read.
const PYTHON_FIXTURES: &[&str] = &[
    "classes",
    "decorated_named",
    "decorated_pytest",
    "multibyte",
    "named_tests",
    "negative",
    "recovered_indent",
    "syntax_error",
    "unittest_case",
];

/// Every JavaScript fixture. `each` and `concurrent` are the two spellings that
/// make the query match the whole function expression rather than an
/// identifier, and `negative` holds the two names its word boundary refuses.
const JAVASCRIPT_FIXTURES: &[&str] = &[
    "concurrent",
    "describe_nesting",
    "each",
    "multibyte",
    "negative",
];

/// Every TypeScript fixture. Both carry type annotations inside the test
/// region, which is what a JavaScript grammar would fail to parse.
const TYPESCRIPT_FIXTURES: &[&str] = &["annotated_describe", "multibyte", "only"];

/// Every TSX fixture. Both hold an element, which is what a TypeScript grammar
/// would fail to parse.
const TSX_FIXTURES: &[&str] = &["component", "multibyte"];

/// No fixture of this corpus holds a defect.
const NONE_DEFECTIVE: &[&str] = &[];

/// The corpora, one per language the table gives a tree rule. A test asserts
/// that this is exactly that set, in both directions.
const CORPORA: &[Corpus] = &[
    Corpus {
        language: Language::Rust,
        directory: "rust",
        extension: "rs",
        fixtures: RUST_FIXTURES,
        defective: &["missing_node", "syntax_error"],
    },
    Corpus {
        language: Language::Go,
        directory: "go",
        extension: "go",
        fixtures: GO_FIXTURES,
        defective: NONE_DEFECTIVE,
    },
    Corpus {
        language: Language::Zig,
        directory: "zig",
        extension: "zig",
        fixtures: ZIG_FIXTURES,
        defective: NONE_DEFECTIVE,
    },
    Corpus {
        language: Language::Python,
        directory: "python",
        extension: "py",
        fixtures: PYTHON_FIXTURES,
        defective: &["syntax_error"],
    },
    Corpus {
        language: Language::JavaScript,
        directory: "javascript",
        extension: "js",
        fixtures: JAVASCRIPT_FIXTURES,
        defective: NONE_DEFECTIVE,
    },
    Corpus {
        language: Language::TypeScript,
        directory: "typescript",
        extension: "ts",
        fixtures: TYPESCRIPT_FIXTURES,
        defective: NONE_DEFECTIVE,
    },
    Corpus {
        language: Language::Tsx,
        directory: "tsx",
        extension: "tsx",
        fixtures: TSX_FIXTURES,
        defective: NONE_DEFECTIVE,
    },
];

impl Corpus {
    /// The corpus of a language, which every language with a tree rule has.
    fn of(language: Language) -> &'static Corpus {
        CORPORA
            .iter()
            .find(|corpus| corpus.language == language)
            .unwrap_or_else(|| panic!("{} has a fixture corpus", language.name()))
    }

    /// The directory the fixtures live in.
    fn root(&self) -> PathBuf {
        PathBuf::from(FIXTURE_ROOT).join(self.directory)
    }

    /// The source of a fixture.
    fn source(&self, name: &str) -> String {
        read(&self.root().join(format!("{name}.{}", self.extension)))
    }

    /// The expectation beside a fixture.
    fn expectation(&self, name: &str) -> String {
        read(&self.root().join(format!("{name}.expected")))
    }

    /// The path the rules see for a fixture.
    ///
    /// It is deliberately not the path the fixture lives at. `tests/**` and
    /// `fixtures/**` are both built-in test globs, so the real path would put
    /// every fixture wholly in the test bucket and no assertion below would
    /// ever reach the tree rule. A test asserts that this path is one no
    /// built-in glob marks.
    fn as_counted(&self, name: &str) -> PathBuf {
        PathBuf::from("src").join(format!("{name}.{}", self.extension))
    }

    /// Counts a fixture with both rules on.
    fn counted(&self, name: &str) -> FileCount {
        let path = self.as_counted(name);
        counter()
            .count_source(&path, &path, &self.source(name))
            .unwrap_or_else(|| panic!("`{name}` is a language the tool counts"))
    }

    /// Asserts that the tool marks a fixture exactly as its expectation says.
    fn assert_marking(&self, name: &str) {
        let source = self.source(name);
        assert_eq!(
            render(&source, &self.counted(name)),
            self.expectation(name),
            "the marking of {} fixture `{name}`",
            self.language.name()
        );
    }

    /// The base names of the files in the fixture directory with this
    /// extension.
    fn names_on_disk(&self, extension: &str) -> BTreeSet<String> {
        let root = self.root();
        std::fs::read_dir(&root)
            .unwrap_or_else(|error| panic!("{}: {error}", root.display()))
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
    fn listed_names(&self) -> BTreeSet<String> {
        self.fixtures
            .iter()
            .map(|name| (*name).to_string())
            .collect()
    }
}

/// A counter with the path rule and the tree rule both on, which is what the
/// tool runs by default.
fn counter() -> Counter {
    Counter::new(PathRules::builtin()).with_tree_rules(TreeRules::new())
}

/// Reads a file of the corpus, and says which one when it cannot.
fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
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

/// The Rust corpus, which the tests below name most often.
fn rust() -> &'static Corpus {
    Corpus::of(Language::Rust)
}

/// The source of a Rust fixture.
fn source(name: &str) -> String {
    rust().source(name)
}

/// Counts a Rust fixture with both rules on.
fn counted(name: &str) -> FileCount {
    rust().counted(name)
}

/// Asserts that the tool marks a Rust fixture exactly as its expectation says.
fn assert_marking(name: &str) {
    rust().assert_marking(name);
}

/// Asserts that a fixture puts no row at all in the test bucket.
fn assert_marks_nothing(corpus: &Corpus, name: &str) {
    corpus.assert_marking(name);
    let counted = corpus.counted(name);
    assert_eq!(
        counted.test.total(),
        0,
        "{}: `{name}` holds no test code",
        corpus.language.name()
    );
    assert!(counted.spans.is_empty());
}

/// The modules a Rust fixture declares its test code lives in, in the order the
/// rule read them.
fn declarations(name: &str) -> Vec<String> {
    counted(name).test_mod_declarations
}

/// The Rust fixtures that move their test code into another file.
const DECLARING: &[&str] = &["cfg_test_mod_declaration", "multibyte_mod_declaration"];

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

    let counted = counted("doc_test");
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
fn plain_mod_declaration_marks_nothing() {
    assert_marking("plain_mod_declaration");
}

#[test]
fn multibyte_mod_declaration_marks_the_two_rows_of_a_declaration_of_many_bytes() {
    assert_marking("multibyte_mod_declaration");
}

#[test]
fn go_marks_a_test_function() {
    Corpus::of(Language::Go).assert_marking("test_functions");
}

#[test]
fn go_marks_a_benchmark() {
    Corpus::of(Language::Go).assert_marking("benchmark_functions");
}

#[test]
fn go_marks_a_fuzz_target() {
    Corpus::of(Language::Go).assert_marking("fuzz_functions");
}

#[test]
fn go_marks_an_example() {
    Corpus::of(Language::Go).assert_marking("example_functions");
}

#[test]
fn go_leaves_a_name_that_merely_opens_with_test_in_the_production_bucket() {
    assert_marks_nothing(Corpus::of(Language::Go), "negative");
}

#[test]
fn go_marks_a_test_that_holds_characters_of_many_bytes() {
    Corpus::of(Language::Go).assert_marking("multibyte");
}

#[test]
fn zig_marks_the_test_declaration_beside_the_function_it_reads() {
    Corpus::of(Language::Zig).assert_marking("test_declaration");
}

#[test]
fn zig_marks_nothing_in_a_file_of_production_code() {
    assert_marks_nothing(Corpus::of(Language::Zig), "no_tests");
}

#[test]
fn zig_marks_a_test_that_holds_characters_of_many_bytes() {
    Corpus::of(Language::Zig).assert_marking("multibyte");
}

#[test]
fn python_marks_a_function_whose_name_opens_with_test() {
    Corpus::of(Language::Python).assert_marking("named_tests");
}

#[test]
fn python_marks_a_pytest_decorated_function_whose_name_says_nothing() {
    Corpus::of(Language::Python).assert_marking("decorated_pytest");
}

#[test]
fn python_reaches_up_over_the_decorator_of_a_test_it_named() {
    let corpus = Corpus::of(Language::Python);
    corpus.assert_marking("decorated_named");

    // The decorator names no runner, so the third pattern of the query cannot
    // have marked it. The rows above the `def` are in the test bucket because
    // the second pattern captures the `decorated_definition` that holds it,
    // which is the whole reason that pattern is in the table.
    let source = corpus.source("decorated_named");
    let counted = corpus.counted("decorated_named");
    let first = marked_rows(&counted.spans)
        .into_iter()
        .next()
        .expect("the decorated test is marked");
    let opening = source
        .lines()
        .nth((first as usize) - 1)
        .expect("the first marked row is a row of the fixture");
    assert!(
        opening.starts_with('@'),
        "the span opens at the decorator, not at the `def`: {opening:?}"
    );
}

#[test]
fn python_marks_a_class_whose_name_opens_with_test() {
    Corpus::of(Language::Python).assert_marking("classes");
}

#[test]
fn python_marks_a_class_that_inherits_a_test_case() {
    Corpus::of(Language::Python).assert_marking("unittest_case");
}

#[test]
fn python_leaves_a_name_that_merely_holds_test_in_the_production_bucket() {
    assert_marks_nothing(Corpus::of(Language::Python), "negative");
}

#[test]
fn python_marks_a_test_that_holds_characters_of_many_bytes() {
    Corpus::of(Language::Python).assert_marking("multibyte");
}

#[test]
fn python_reads_a_file_whose_indentation_is_broken_without_calling_the_parse_failed() {
    let corpus = Corpus::of(Language::Python);
    corpus.assert_marking("recovered_indent");

    // Measured against tree-sitter-python 0.25: the external scanner that
    // issues the INDENT and DEDENT tokens normalises every indentation this
    // fixture could carry, so a file no Python interpreter would accept yields
    // a tree with neither an ERROR node nor a MISSING one. Broken indentation
    // is therefore NOT a way to reach the parse-failure path, and the fixture
    // that does reach it holds a defect the parser can see: a `def` with no
    // colon.
    assert_eq!(
        corpus.counted("recovered_indent").parse_status,
        ParseStatus::Clean,
        "the scanner recovered, so the marking of this file is still trusted"
    );
}

#[test]
fn python_fails_the_parse_of_a_defective_file_and_leaves_the_whole_of_it_production() {
    let corpus = Corpus::of(Language::Python);
    corpus.assert_marking("syntax_error");

    let source = corpus.source("syntax_error");
    let counted = corpus.counted("syntax_error");
    assert_eq!(
        counted.parse_status,
        ParseStatus::Failed,
        "the parse-failure path is not Rust-specific"
    );
    assert_eq!(
        counted.test.total(),
        0,
        "a file we could not read must not carry a guessed test count"
    );
    assert_eq!(counted.production, lines::count(&source, Language::Python));
    assert!(
        source.contains("def test_compute()"),
        "the fixture holds a node the query would match, so the zero above is \
         the parse failure and not an empty file"
    );
}

#[test]
fn javascript_marks_a_block_and_the_test_nested_in_it() {
    Corpus::of(Language::JavaScript).assert_marking("describe_nesting");
}

#[test]
fn javascript_marks_a_table_driven_test() {
    Corpus::of(Language::JavaScript).assert_marking("each");
}

#[test]
fn javascript_marks_a_test_that_names_a_mode_of_the_runner() {
    Corpus::of(Language::JavaScript).assert_marking("concurrent");
}

#[test]
fn javascript_leaves_a_name_that_merely_opens_with_test_in_the_production_bucket() {
    assert_marks_nothing(Corpus::of(Language::JavaScript), "negative");
}

#[test]
fn javascript_marks_a_test_that_holds_characters_of_many_bytes() {
    Corpus::of(Language::JavaScript).assert_marking("multibyte");
}

#[test]
fn typescript_marks_a_block_of_annotated_code() {
    Corpus::of(Language::TypeScript).assert_marking("annotated_describe");
}

#[test]
fn typescript_marks_a_focused_test() {
    Corpus::of(Language::TypeScript).assert_marking("only");
}

#[test]
fn typescript_marks_a_test_that_holds_characters_of_many_bytes() {
    Corpus::of(Language::TypeScript).assert_marking("multibyte");
}

#[test]
fn tsx_marks_the_block_beside_the_component_it_reads() {
    Corpus::of(Language::Tsx).assert_marking("component");
}

#[test]
fn tsx_marks_a_test_that_holds_characters_of_many_bytes() {
    Corpus::of(Language::Tsx).assert_marking("multibyte");
}

#[test]
fn the_tsx_row_is_wired_to_a_grammar_of_its_own() {
    // TSX and TypeScript are two grammars and not two modes of one, and the
    // table has a row for each. Counting the same source under the TypeScript
    // row is what tells the two entry points apart: an element is a type
    // assertion there, the parse fails, and the file counts wholly as
    // production code. A TSX row wired to `LANGUAGE_TYPESCRIPT` would do that
    // to every `.tsx` file in a tree and say nothing about it.
    let corpus = Corpus::of(Language::Tsx);
    let source = corpus.source("component");
    let path = Path::new("src/component.ts");
    let counted = counter()
        .count_source(path, path, &source)
        .expect("TypeScript is a language the tool counts");

    assert_eq!(counted.language, Language::TypeScript);
    assert_eq!(
        counted.parse_status,
        ParseStatus::Failed,
        "an element does not parse under the TypeScript grammar"
    );
    assert_eq!(counted.test.total(), 0);

    assert_eq!(
        corpus.counted("component").parse_status,
        ParseStatus::Clean,
        "the same source under the TSX row parses"
    );
    assert!(corpus.counted("component").test.total() > 0);
}

#[test]
fn a_declaration_with_no_body_names_the_module_that_holds_its_test_code() {
    assert_eq!(
        declarations("cfg_test_mod_declaration"),
        vec!["tests".to_string()],
        "`mod tests;` moves the test code of this file into another one"
    );
}

#[test]
fn a_test_module_with_a_body_names_no_other_file() {
    for name in [
        "cfg_test_mod",
        "cfg_all_test",
        "multibyte_mod",
        "nested_overlap",
    ] {
        assert!(
            declarations(name).is_empty(),
            "`{name}` holds its test code itself, so it names no other file"
        );
    }
}

#[test]
fn a_declaration_with_no_cfg_test_names_nothing() {
    assert!(
        declarations("plain_mod_declaration").is_empty(),
        "a plain `mod helpers;` is a module of production code"
    );
}

#[test]
fn a_declaration_of_a_module_whose_name_is_not_ascii_reads_the_whole_name() {
    assert_eq!(
        declarations("multibyte_mod_declaration"),
        vec!["テスト".to_string()],
        "the name is read as text and never sliced by byte"
    );
}

#[test]
fn a_fixture_that_declares_nothing_carries_no_declaration() {
    for corpus in CORPORA {
        for name in corpus.fixtures {
            if DECLARING.contains(name) {
                continue;
            }
            assert!(
                corpus.counted(name).test_mod_declarations.is_empty(),
                "{}: `{name}` holds no `#[cfg(test)] mod <name>;`",
                corpus.language.name()
            );
        }
    }
}

#[test]
fn every_fixture_has_an_expectation_and_every_expectation_a_fixture() {
    for corpus in CORPORA {
        let sources = corpus.names_on_disk(corpus.extension);
        let expectations = corpus.names_on_disk("expected");
        assert_eq!(
            sources,
            expectations,
            "a fixture with no expectation, or an expectation with no fixture, in {}",
            corpus.root().display()
        );
        assert!(
            !sources.is_empty(),
            "{} has an empty fixture directory",
            corpus.language.name()
        );
    }
}

#[test]
fn every_fixture_on_disk_is_named_in_the_list_this_file_covers() {
    for corpus in CORPORA {
        assert_eq!(
            corpus.names_on_disk(corpus.extension),
            corpus.listed_names(),
            "{}: a fixture nobody asserts on, or a name in the list with no fixture",
            corpus.language.name()
        );
    }
}

#[test]
fn every_expectation_carries_the_rows_of_its_fixture() {
    for corpus in CORPORA {
        for name in corpus.fixtures {
            let stripped: String = corpus
                .expectation(name)
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
                corpus.source(name),
                "{}: the expectation of `{name}` no longer holds the rows of the fixture",
                corpus.language.name()
            );
        }
    }
}

#[test]
fn no_fixture_path_is_marked_by_the_path_rule() {
    let rules = PathRules::builtin();
    for corpus in CORPORA {
        for name in corpus.fixtures {
            assert_eq!(
                rules.verdict(&corpus.as_counted(name)),
                PathVerdict::Unmarked,
                "{}: a built-in glob marks `{name}`, so the tree rule never runs over it",
                corpus.language.name()
            );
        }
    }
}

#[test]
fn every_language_with_a_tree_rule_has_a_corpus_and_every_corpus_a_tree_rule() {
    let with_rule: BTreeSet<&str> = Language::all()
        .iter()
        .filter(|language| language.tree_query().is_some())
        .map(|language| language.name())
        .collect();
    let with_corpus: BTreeSet<&str> = CORPORA
        .iter()
        .map(|corpus| corpus.language.name())
        .collect();

    assert_eq!(
        with_rule, with_corpus,
        "a language that gained a tree rule and no fixture directory, or a \
         corpus for a language that has no rule to exercise"
    );
    assert_eq!(
        with_corpus.len(),
        CORPORA.len(),
        "two corpora name the same language"
    );
}

#[test]
fn every_directory_of_the_fixture_tree_belongs_to_a_corpus() {
    let on_disk: BTreeSet<String> = std::fs::read_dir(FIXTURE_ROOT)
        .unwrap_or_else(|error| panic!("{FIXTURE_ROOT}: {error}"))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if !path.is_dir() {
                return None;
            }
            Some(path.file_name()?.to_str()?.to_string())
        })
        .collect();
    let claimed: BTreeSet<String> = CORPORA
        .iter()
        .map(|corpus| corpus.directory.to_string())
        .collect();

    assert_eq!(
        on_disk, claimed,
        "a directory of fixtures nobody reads, or a corpus whose directory is gone"
    );
}

#[test]
fn nested_overlap_yields_two_spans_over_one_set_of_rows() {
    let counted = counted("nested_overlap");

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
    for corpus in CORPORA {
        for name in corpus.fixtures {
            let counted = corpus.counted(name);
            assert_eq!(
                u64::try_from(marked_rows(&counted.spans).len()).unwrap_or(u64::MAX),
                counted.test.total(),
                "{}: `{name}`: the rows the spans cover are the rows in the test bucket",
                corpus.language.name()
            );
        }
    }
}

#[test]
fn the_two_buckets_sum_to_the_unsplit_count_for_every_fixture() {
    for corpus in CORPORA {
        for name in corpus.fixtures {
            let source = corpus.source(name);
            assert_eq!(
                corpus.counted(name).total(),
                lines::count(&source, corpus.language),
                "{}: `{name}`: the split changed the count",
                corpus.language.name()
            );
        }
    }
}

#[test]
fn every_span_of_the_tree_rule_names_the_node_kind_it_matched() {
    let counted = counted("cfg_test_mod");
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
    for corpus in CORPORA {
        for name in corpus.fixtures {
            if corpus.defective.contains(name) {
                continue;
            }
            assert_eq!(
                corpus.counted(name).parse_status,
                ParseStatus::Clean,
                "{}: `{name}` holds no defect",
                corpus.language.name()
            );
        }
    }
}

#[test]
fn a_syntax_error_fails_the_parse_and_leaves_the_whole_file_production() {
    let source = source("syntax_error");
    let counted = counted("syntax_error");
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
    let counted = counted("missing_node");
    assert_eq!(
        counted.parse_status,
        ParseStatus::Failed,
        "a node the parser inserted to recover is a defect too"
    );
    assert_eq!(counted.test.total(), 0);
}

#[test]
fn the_tree_rule_of_every_language_in_the_table_compiles() {
    let rules = TreeRules::new();
    for &language in Language::all() {
        let named = language.name();
        // The four accessors read one column, so a row cannot hold half a rule
        // — a query with no grammar, or an attribute chain belonging to a
        // language whose query never captures a candidate.
        assert_eq!(
            language.grammar().is_some(),
            language.tree_query().is_some(),
            "{named}: a grammar and a query arrive together or not at all"
        );
        if language.tree_query().is_none() {
            assert!(language.attribute_chain().is_none(), "{named}");
            assert!(language.scope_kinds().is_empty(), "{named}");
        }

        // Compiling the rule is what refuses a query that does not parse, a
        // capture name that marks nothing, and a pattern that is not a regular
        // expression. Asking for the outcome of an empty source is what makes
        // it compile, so this call is the assertion.
        let outcome = rules.outcome("", language);
        assert_eq!(
            outcome.is_some(),
            language.tree_query().is_some(),
            "{named}: a language answers the tree rule exactly when it has one"
        );
    }

    for corpus in CORPORA {
        assert!(
            corpus.language.tree_query().is_some(),
            "{} is a language this slice reads",
            corpus.language.name()
        );
    }
    assert!(
        Language::Markdown.tree_query().is_none(),
        "a language with no code to parse carries no rule"
    );
}

#[test]
fn the_three_script_languages_read_one_query() {
    // The three spell a test the same way, so they share a row of the table
    // rather than carrying three copies of one pattern that drift apart.
    assert_eq!(
        Language::JavaScript.tree_query(),
        Language::TypeScript.tree_query()
    );
    assert_eq!(
        Language::TypeScript.tree_query(),
        Language::Tsx.tree_query()
    );
}

#[test]
fn a_language_with_no_tree_rule_is_not_parsed() {
    let source = "#include <stdio.h>\n\nvoid test_thing(void) {\n    printf(\"x\\n\");\n}\n";
    let path = Path::new("src/thing.c");
    let counted = counter()
        .count_source(path, path, source)
        .expect("C is a language the tool counts");

    assert_eq!(counted.language, Language::C);
    assert_eq!(counted.parse_status, ParseStatus::NotParsed);
    assert_eq!(
        counted.test.total(),
        0,
        "C carries no tree rule, so nothing in it is ever marked"
    );
    assert!(TreeRules::new().outcome(source, Language::C).is_none());
}

#[test]
fn a_file_the_path_rule_marked_is_not_parsed() {
    let source = source("cfg_test_mod");
    let path = Path::new("tests/cfg_test_mod.rs");
    let counted = counter()
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
    let path = rust().as_counted("cfg_test_mod");
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

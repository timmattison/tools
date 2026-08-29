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
//! There is one fixture per syntactic form the rule has to read, and four
//! guards keep the corpus honest. A coverage test asserts that the list of each
//! [`Corpus`] is the set of files on disk, so a fixture added without an
//! expectation — or an expectation left behind by a fixture that was renamed —
//! shows up as a set difference rather than as silence. A second asserts that
//! the set of corpora is the set of languages the table gives a tree rule, so a
//! language that gains a rule and gains no fixture shows up the same way. A
//! third asserts that no fixture path is one the *path* rule would mark, since
//! a fixture the globs claim never reaches the tree rule at all and its
//! assertion would then pass for the wrong reason. A fourth asserts that every
//! corpus holds a fixture the rule marks nothing in, since a corpus whose
//! fixtures all hold a test says nothing about the production code the query
//! must leave alone.
//!
//! # The needle filter, and why the assertions read `TreeMode::Always`
//!
//! A parse costs far more than a scan of the rows, so the tool parses only a
//! file whose bytes hold a needle of its language. A missing needle is a silent
//! undercount: the file is never parsed, its test rows are never found, and the
//! number that comes out reads exactly like a correct one.
//!
//! Every assertion about what a *query* marks therefore runs in
//! [`TreeMode::Always`], which parses every file and says nothing about the
//! filter. Two tests then hold the mode the tool actually runs to that answer:
//! [`the_two_modes_mark_every_fixture_of_the_corpus_the_same`] over this
//! corpus, and [`the_two_modes_mark_every_file_of_this_repository_the_same`]
//! over the repository the crate is built from — which is where a needle
//! missing for a construct nobody wrote a fixture for shows up. A third test
//! names the fixtures the filter actually stops, so the agreement of the two
//! modes cannot pass by comparing a thing with itself.
//!
//! Nothing here writes a file, and nothing here shells out. The fixtures are
//! read, never modified, so two copies of this file running at once cannot
//! tread on each other.

use cdva::{
    lines, walk, Counter, FileCount, Language, ParseStatus, PathRules, PathVerdict, Rule, Span,
    TreeMode, TreeRules, WalkOptions,
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
    "cfg_attr_test",
    "cfg_feature_named_test",
    "cfg_not_test",
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
/// identifier, `negative` holds the two names the anchor at the front of the
/// pattern refuses, and `member_access` holds a member access on a variable
/// named after a runner beside a runner mode the query must still read.
const JAVASCRIPT_FIXTURES: &[&str] = &[
    "concurrent",
    "describe_nesting",
    "each",
    "member_access",
    "multibyte",
    "negative",
];

/// Every TypeScript fixture. The three that hold a test carry type annotations
/// inside the test region, which is what a JavaScript grammar would fail to
/// parse, `negative` is the module of typed production code that holds no test
/// at all, and `member_access` is the typed spelling of the member access the
/// query must leave alone.
const TYPESCRIPT_FIXTURES: &[&str] = &[
    "annotated_describe",
    "member_access",
    "multibyte",
    "negative",
    "only",
];

/// Every TSX fixture. The two that hold a test hold an element as well, which
/// is what a TypeScript grammar would fail to parse, `negative` is the
/// component that stands on its own, and `member_access` is the component whose
/// member accesses name a runner.
const TSX_FIXTURES: &[&str] = &["component", "member_access", "multibyte", "negative"];

/// Every Java fixture. `annotated` and `runwith` are the two nodes the query
/// names, `parameterized` holds both spellings of an annotation, and `negative`
/// is the class whose *name* holds `Test` and whose methods carry nothing.
const JAVA_FIXTURES: &[&str] = &[
    "annotated",
    "multibyte",
    "negative",
    "parameterized",
    "runwith",
];

/// Every Kotlin fixture. Kotlin spells one annotation kind rather than two, so
/// the corpus is a test, a lifecycle hook, and the function that carries
/// neither.
const KOTLIN_FIXTURES: &[&str] = &["annotated", "lifecycle", "multibyte", "negative"];

/// Every C# fixture, one per node the query names — a method under one
/// attribute, a method under a stack of them, and a class — plus the method
/// that carries none.
const CSHARP_FIXTURES: &[&str] = &["attributes", "fact", "multibyte", "negative", "theory"];

/// Every Ruby fixture. `rspec` carries the one receiver the query admits,
/// `bare_describe` carries none, `minitest` is the class the third pattern
/// reads, `negative` holds the call to a method named `describe` that carries
/// no block, and `receiver_block` holds the production methods that carry a
/// block on a receiver of their own.
const RUBY_FIXTURES: &[&str] = &[
    "bare_describe",
    "minitest",
    "multibyte",
    "negative",
    "receiver_block",
    "rspec",
];

/// Every Swift fixture: the class that inherits a case, the function that
/// carries an attribute, and the class that inherits something else.
const SWIFT_FIXTURES: &[&str] = &["attributed", "multibyte", "negative", "xctest"];

/// Every Elixir fixture. `test_block` is the block pattern on its own, inside a
/// module that says nothing about itself, and `exunit` is the module that says
/// `use ExUnit.Case` beside a module of production code — the pair that pins
/// where the climb of `@test_scope` starts and where it stops.
const ELIXIR_FIXTURES: &[&str] = &["exunit", "multibyte", "negative", "test_block"];

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
    Corpus {
        language: Language::Java,
        directory: "java",
        extension: "java",
        fixtures: JAVA_FIXTURES,
        defective: NONE_DEFECTIVE,
    },
    Corpus {
        language: Language::Kotlin,
        directory: "kotlin",
        extension: "kt",
        fixtures: KOTLIN_FIXTURES,
        defective: NONE_DEFECTIVE,
    },
    Corpus {
        language: Language::CSharp,
        directory: "csharp",
        extension: "cs",
        fixtures: CSHARP_FIXTURES,
        defective: NONE_DEFECTIVE,
    },
    Corpus {
        language: Language::Ruby,
        directory: "ruby",
        extension: "rb",
        fixtures: RUBY_FIXTURES,
        defective: NONE_DEFECTIVE,
    },
    Corpus {
        language: Language::Swift,
        directory: "swift",
        extension: "swift",
        fixtures: SWIFT_FIXTURES,
        defective: NONE_DEFECTIVE,
    },
    // The extension is `ex` and not `exs`, because `*_test.exs` is a built-in
    // glob and a fixture the path rule claims never reaches the tree rule.
    Corpus {
        language: Language::Elixir,
        directory: "elixir",
        extension: "ex",
        fixtures: ELIXIR_FIXTURES,
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

    /// Counts a fixture with both rules on, in the mode named.
    fn counted_in(&self, name: &str, mode: TreeMode) -> FileCount {
        let path = self.as_counted(name);
        counter_in(mode)
            .count_source(&path, &path, &self.source(name))
            .unwrap_or_else(|| panic!("`{name}` is a language the tool counts"))
    }

    /// Counts a fixture with both rules on, and with every file parsed.
    ///
    /// The assertions of this file are about what the *query* of a language
    /// marks, so they read the mode that parses every file. The mode the tool
    /// runs by default parses only a file whose bytes hold a needle, and the
    /// two are held to the same marking, file for file, by
    /// [`the_two_modes_mark_every_fixture_of_the_corpus_the_same`].
    fn counted(&self, name: &str) -> FileCount {
        self.counted_in(name, TreeMode::Always)
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

    /// Whether the tool leaves every row of a fixture in the production
    /// bucket.
    ///
    /// A fixture marks nothing when the count of it holds no span and no test
    /// row. [`assert_marks_nothing`] holds one fixture the test names to that,
    /// and [`every_corpus_holds_a_fixture_of_production_code_alone`] asks it of
    /// the fixtures of a corpus in turn.
    fn marks_nothing(&self, name: &str) -> bool {
        let counted = self.counted(name);
        counted.test.total() == 0 && counted.spans.is_empty()
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

/// A counter with the path rule and the tree rule both on, in the mode named.
fn counter_in(mode: TreeMode) -> Counter {
    Counter::new(PathRules::builtin()).with_tree_rules(TreeRules::new(), mode)
}

/// A counter that parses every file of a language that has a rule.
fn counter() -> Counter {
    counter_in(TreeMode::Always)
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

/// The rows of a fixture the tool marked, in order, with the indentation taken
/// off.
///
/// An expectation pins every row of a fixture, which is what catches a row that
/// moved. This reads one named row back out, for the assertions that have
/// something to say about *why* a row is marked — which branch of a query
/// reached it, or where a span opens.
fn marked_lines(corpus: &Corpus, name: &str) -> Vec<String> {
    let source = corpus.source(name);
    let rows = marked_rows(&corpus.counted(name).spans);
    source
        .lines()
        .enumerate()
        .filter(|(offset, _)| rows.contains(&u32::try_from(offset + 1).unwrap_or(u32::MAX)))
        .map(|(_, line)| line.trim().to_string())
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
    assert!(
        corpus.marks_nothing(name),
        "{}: `{name}` holds no test code: {:?}",
        corpus.language.name(),
        corpus.counted(name).spans
    );
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
fn cfg_not_test_leaves_a_module_that_the_tests_switch_off_in_the_production_bucket() {
    // `not(test)` names the code that is compiled when the tests are OFF, so
    // the module below it is production code. The `#[cfg(test)] mod tests` at
    // the foot of the fixture is what proves the rule still reads the word
    // `test` where the word decides.
    assert_marking("cfg_not_test");
}

#[test]
fn cfg_attr_test_leaves_a_function_that_only_changes_attributes_in_the_production_bucket() {
    // `cfg_attr` says which attributes apply and never whether the item
    // exists, so `#[cfg_attr(test, …)]` decorates production code. The `#[test]
    // fn` below it is what proves the rule still marks a test in this file.
    assert_marking("cfg_attr_test");
}

#[test]
fn cfg_feature_named_test_leaves_a_function_gated_on_a_feature_in_the_production_bucket() {
    // The `test` of `feature = "test-support"` is part of the name of a
    // feature, and a feature is shipped. The `#[cfg(test)] mod tests` below it
    // is what proves the rule still reads the option `test`.
    assert_marking("cfg_feature_named_test");
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
fn javascript_reads_a_runner_mode_and_leaves_every_other_member_access_alone() {
    // `context`, `it`, and `test` are ordinary variable names of this language,
    // and a call on a member of one of them is production code: a canvas, an
    // iterator, and a regular expression here. The same fixture holds
    // `describe.skip`, so a query narrowed until it refuses `context.fillRect`
    // and goes on to refuse a runner mode as well fails here rather than
    // reading clean.
    Corpus::of(Language::JavaScript).assert_marking("member_access");
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
fn typescript_reads_a_runner_mode_and_leaves_every_other_member_access_alone() {
    // The three script languages share one query, so the member access they
    // must all leave alone is pinned in all three corpora. This one carries the
    // type annotations of the language around it.
    Corpus::of(Language::TypeScript).assert_marking("member_access");
}

#[test]
fn typescript_marks_a_test_that_holds_characters_of_many_bytes() {
    Corpus::of(Language::TypeScript).assert_marking("multibyte");
}

#[test]
fn typescript_leaves_a_module_of_production_code_alone() {
    assert_marks_nothing(Corpus::of(Language::TypeScript), "negative");
}

#[test]
fn tsx_marks_the_block_beside_the_component_it_reads() {
    Corpus::of(Language::Tsx).assert_marking("component");
}

#[test]
fn tsx_reads_a_runner_mode_and_leaves_every_other_member_access_alone() {
    // A component draws on a canvas and reads an iterator exactly as a module
    // of the two languages above does, and the element beside them is what
    // holds the third grammar to the same answer.
    Corpus::of(Language::Tsx).assert_marking("member_access");
}

#[test]
fn tsx_marks_a_test_that_holds_characters_of_many_bytes() {
    Corpus::of(Language::Tsx).assert_marking("multibyte");
}

#[test]
fn tsx_leaves_a_component_of_production_code_alone() {
    assert_marks_nothing(Corpus::of(Language::Tsx), "negative");
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
fn java_marks_a_method_that_carries_a_bare_annotation() {
    Corpus::of(Language::Java).assert_marking("annotated");
}

#[test]
fn java_reads_both_spellings_of_an_annotation() {
    let corpus = Corpus::of(Language::Java);
    corpus.assert_marking("parameterized");

    // A bare `@ParameterizedTest` is a `marker_annotation` and `@RepeatedTest(3)`
    // is an `annotation`, which are two node kinds and not two spellings of one.
    // The second method is reachable through the `annotation` branch of the
    // alternation alone, so its presence here is what proves that branch is
    // doing work — the first method would pass on the marker branch by itself.
    let marked = marked_lines(corpus, "parameterized");
    assert!(
        marked.contains(&"@ParameterizedTest".to_string()),
        "the marker annotation is in the span: {marked:?}"
    );
    assert!(
        marked.contains(&"@ValueSource(ints = {1, 2})".to_string()),
        "the stack reaches back over both annotation rows: {marked:?}"
    );
    assert!(
        marked.contains(&"@RepeatedTest(3)".to_string()),
        "an annotation that carries arguments decides on its own: {marked:?}"
    );
}

#[test]
fn java_marks_a_class_that_names_a_runner() {
    Corpus::of(Language::Java).assert_marking("runwith");
}

#[test]
fn java_leaves_a_class_whose_name_merely_holds_test_in_the_production_bucket() {
    assert_marks_nothing(Corpus::of(Language::Java), "negative");
}

#[test]
fn java_marks_a_test_that_holds_characters_of_many_bytes() {
    Corpus::of(Language::Java).assert_marking("multibyte");
}

#[test]
fn kotlin_marks_a_function_that_carries_a_test_annotation() {
    Corpus::of(Language::Kotlin).assert_marking("annotated");
}

#[test]
fn kotlin_marks_a_function_that_runs_before_each_test() {
    Corpus::of(Language::Kotlin).assert_marking("lifecycle");
}

#[test]
fn kotlin_leaves_a_function_that_carries_no_annotation_in_the_production_bucket() {
    assert_marks_nothing(Corpus::of(Language::Kotlin), "negative");
}

#[test]
fn kotlin_marks_a_test_that_holds_characters_of_many_bytes() {
    Corpus::of(Language::Kotlin).assert_marking("multibyte");
}

#[test]
fn csharp_marks_a_method_that_carries_one_attribute() {
    Corpus::of(Language::CSharp).assert_marking("fact");
}

#[test]
fn csharp_marks_a_method_under_a_stack_of_attribute_lists() {
    let corpus = Corpus::of(Language::CSharp);
    corpus.assert_marking("theory");

    // Each `[…]` is an `attribute_list` of its own, and the deciding one is the
    // topmost of three. The span of the method holds all three, so the rows of
    // the data the test runs over are test rows too.
    let marked = marked_lines(corpus, "theory");
    assert!(
        marked.contains(&"[InlineData(2)]".to_string()),
        "the whole stack of attribute lists is in the span: {marked:?}"
    );
}

#[test]
fn csharp_marks_a_class_that_carries_a_fixture_attribute() {
    Corpus::of(Language::CSharp).assert_marking("attributes");
}

#[test]
fn csharp_leaves_a_method_that_carries_no_attribute_in_the_production_bucket() {
    assert_marks_nothing(Corpus::of(Language::CSharp), "negative");
}

#[test]
fn csharp_marks_a_test_that_holds_characters_of_many_bytes() {
    Corpus::of(Language::CSharp).assert_marking("multibyte");
}

#[test]
fn ruby_marks_a_block_whose_receiver_is_the_runner() {
    let corpus = Corpus::of(Language::Ruby);
    corpus.assert_marking("rspec");

    // The receiver does not enter the pattern, which is what makes one rule
    // read `RSpec.describe … do` and a bare `describe … do` alike. A pattern
    // anchored on a bare identifier would mark the `it` inside and lose the
    // block that holds it.
    let marked = marked_lines(corpus, "rspec");
    assert_eq!(
        marked.first().map(String::as_str),
        Some("RSpec.describe Calculator do"),
        "the span opens at the call that names a receiver: {marked:?}"
    );
}

#[test]
fn ruby_marks_a_bare_block_and_the_ones_nested_in_it() {
    Corpus::of(Language::Ruby).assert_marking("bare_describe");
}

#[test]
fn ruby_marks_a_class_that_inherits_a_test_case() {
    Corpus::of(Language::Ruby).assert_marking("minitest");
}

#[test]
fn ruby_leaves_a_call_that_carries_no_block_in_the_production_bucket() {
    assert_marks_nothing(Corpus::of(Language::Ruby), "negative");
}

#[test]
fn ruby_leaves_a_block_on_a_receiver_of_its_own_in_the_production_bucket() {
    let corpus = Corpus::of(Language::Ruby);
    corpus.assert_marking("receiver_block");

    // A name of the six on a receiver that is not the runner is a method of
    // that receiver, and a block after it is production code. The fixture
    // holds a test below the two such methods, so the assertion says both
    // things at once: the query leaves `logger.context … do` alone, and it
    // still reads the `RSpec.describe … do` under it.
    let marked = marked_lines(corpus, "receiver_block");
    assert_eq!(
        marked.first().map(String::as_str),
        Some("RSpec.describe Ledger do"),
        "the first marked row is the test, not the method above it: {marked:?}"
    );
}

#[test]
fn ruby_marks_a_test_that_holds_characters_of_many_bytes() {
    Corpus::of(Language::Ruby).assert_marking("multibyte");
}

#[test]
fn swift_marks_a_class_that_inherits_a_test_case() {
    Corpus::of(Language::Swift).assert_marking("xctest");
}

#[test]
fn swift_marks_a_function_that_carries_a_test_attribute() {
    Corpus::of(Language::Swift).assert_marking("attributed");
}

#[test]
fn swift_leaves_a_class_that_inherits_something_else_in_the_production_bucket() {
    assert_marks_nothing(Corpus::of(Language::Swift), "negative");
}

#[test]
fn swift_marks_a_test_that_holds_characters_of_many_bytes() {
    Corpus::of(Language::Swift).assert_marking("multibyte");
}

#[test]
fn elixir_marks_a_test_block_and_the_ones_nested_in_it() {
    let corpus = Corpus::of(Language::Elixir);
    corpus.assert_marking("test_block");

    // No `use ExUnit.Case` is in this file, so the block pattern is the only
    // one that can have marked anything. It marks the block and stops there:
    // the `defmodule` that holds it stays production code, which is what tells
    // this pattern apart from the scope of the fixture below.
    let marked = marked_lines(corpus, "test_block");
    assert_eq!(
        marked.first().map(String::as_str),
        Some("describe \"add/2\" do"),
        "the span opens at the block and not at the module: {marked:?}"
    );
    assert!(
        !marked.iter().any(|line| line.starts_with("defmodule")),
        "a block marks itself, never the module around it: {marked:?}"
    );
}

#[test]
fn elixir_marks_the_module_that_uses_the_case_and_leaves_the_one_beside_it_alone() {
    let corpus = Corpus::of(Language::Elixir);
    corpus.assert_marking("exunit");

    // `use ExUnit.Case` says nothing about its own row and everything about the
    // module that holds it, so the capture climbs. What the climb must not do
    // is overshoot: a `defmodule` is a `call` and so is the `use` inside it, so
    // a walk that took the outermost `call` of the *file* would swallow the
    // production module beside this one. This fixture holds both, and the two
    // assertions below are the two halves of that — the whole test module is
    // marked, and not one row before it is.
    let source = corpus.source("exunit");
    let counted = corpus.counted("exunit");
    let marked = marked_rows(&counted.spans);

    let opening = source
        .lines()
        .position(|line| line.starts_with("defmodule LedgerChecks"))
        .expect("the fixture holds a module that uses the test case");
    let first_test_row = u32::try_from(opening + 1).unwrap_or(u32::MAX);
    let last_row = u32::try_from(source.lines().count()).unwrap_or(u32::MAX);

    let production_rows: BTreeSet<u32> = (1..first_test_row).collect();
    assert!(
        !production_rows.is_empty(),
        "the fixture holds a module of production code before the test module"
    );
    assert!(
        marked.is_disjoint(&production_rows),
        "the climb stopped at the test module, so no row of the production \
         module above it is marked: {marked:?}"
    );
    assert_eq!(
        marked,
        (first_test_row..=last_row).collect::<BTreeSet<u32>>(),
        "every row of the test module is marked, `use` and `end` included"
    );
    assert_eq!(
        counted.production,
        lines::count(
            &source
                .lines()
                .take(opening)
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
            Language::Elixir
        ),
        "the production bucket is exactly the module the climb left alone"
    );
}

#[test]
fn elixir_leaves_a_module_of_production_code_alone() {
    assert_marks_nothing(Corpus::of(Language::Elixir), "negative");
}

#[test]
fn elixir_marks_a_test_that_holds_characters_of_many_bytes() {
    Corpus::of(Language::Elixir).assert_marking("multibyte");
}

#[test]
fn elixir_is_the_one_language_that_climbs_to_a_scope() {
    // `@test_scope` is the third capture name of the engine and this is its one
    // use in the whole table. A second language that quietly gained a scope
    // kind would be marking whole enclosing nodes with nothing to say so.
    let with_scope: BTreeSet<&str> = Language::all()
        .iter()
        .filter(|language| !language.scope_kinds().is_empty())
        .map(|language| language.name())
        .collect();

    assert_eq!(with_scope, BTreeSet::from(["Elixir"]));
    assert_eq!(
        Language::Elixir.scope_kinds(),
        ["call"],
        "a `defmodule` is a call, which is what the capture climbs to"
    );
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

/// Every corpus holds a fixture of production code alone.
///
/// A fixture that holds a test says what the query marks. It says nothing about
/// what the query leaves alone, because every row of the test region is a row
/// the expectation lets the query take. A corpus whose fixtures all hold a test
/// therefore keeps a query that marks production code green, and the false
/// positive reaches a user as an undercount of the production bucket.
///
/// The corpora that hold no such fixture show up as the difference of the two
/// sets, and not as a bare `false`.
#[test]
fn every_corpus_holds_a_fixture_of_production_code_alone() {
    let every: BTreeSet<&str> = CORPORA
        .iter()
        .map(|corpus| corpus.language.name())
        .collect();
    let with_one: BTreeSet<&str> = CORPORA
        .iter()
        .filter(|corpus| {
            corpus
                .fixtures
                .iter()
                .any(|name| corpus.marks_nothing(name))
        })
        .map(|corpus| corpus.language.name())
        .collect();

    assert_eq!(
        every, with_one,
        "a corpus whose every fixture holds a test, so nothing in it catches a \
         query that marks production code"
    );
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
fn a_nul_byte_in_a_literal_does_not_stop_the_parse_of_a_script_file() {
    // A NUL byte is legal inside a template literal, and the lexer of the
    // parser reads one as the end of the input. A file that holds one as data
    // would otherwise fail to parse whole, over a byte no syntax objects to.
    let source =
        "const key = `${a}\u{0}${b}`\n\ndescribe('thing', () => {\n  it('works', () => {})\n})\n";
    let path = Path::new("src/thing.ts");
    let counted = counter()
        .count_source(path, path, source)
        .expect("TypeScript is a language the tool counts");

    assert_eq!(
        counted.parse_status,
        ParseStatus::Clean,
        "a NUL byte inside a literal is data, not a defect"
    );
    assert_eq!(
        marked_rows(&counted.spans),
        BTreeSet::from([3, 4, 5]),
        "the rows are the rows of the same file with a space in place of the NUL byte"
    );
}

#[test]
fn a_nul_byte_in_a_literal_does_not_stop_the_parse_of_a_rust_file() {
    // The Rust grammar reads a NUL byte in a literal today, and the script
    // grammars do not. The tool hands every language the same substituted
    // source, so this pins that the substitution leaves the marking of a
    // grammar which needed nothing exactly where it was.
    let source = "const SEP: &str = \"\u{0}\";\n\n#[test]\nfn works() {}\n";
    let path = Path::new("src/thing.rs");
    let counted = counter()
        .count_source(path, path, source)
        .expect("Rust is a language the tool counts");

    assert_eq!(counted.parse_status, ParseStatus::Clean);
    assert_eq!(
        marked_rows(&counted.spans),
        BTreeSet::from([3, 4]),
        "the attribute and the function it decorates, as in a file of no NUL bytes"
    );
}

#[test]
fn a_nul_byte_does_not_hide_a_defect_that_is_next_to_it() {
    // The byte is taken out of the parser's way and nothing else is. A file
    // that holds a NUL byte and a real defect fails its parse, as it must:
    // the whole point of naming a failed parse is that the marking of such a
    // file is not to be trusted.
    let source = "const key = `${a}\u{0}${b}`\n\ndescribe('thing', () => {\n";
    let path = Path::new("src/thing.ts");
    let counted = counter()
        .count_source(path, path, source)
        .expect("TypeScript is a language the tool counts");

    assert_eq!(counted.parse_status, ParseStatus::Failed);
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

        // Compiling the rule is what refuses a query that does not parse and
        // a capture name that marks nothing. Asking for the outcome of an
        // empty source is what makes it compile, so this call is the
        // assertion.
        let outcome = rules.outcome("", language, TreeMode::Always);
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
    assert!(TreeRules::new()
        .outcome(source, Language::C, TreeMode::Always)
        .is_none());
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

/// A Rust source whose bytes hold no needle of the language: no `test`, and no
/// `bench`. An assertion below reads the needles of the table rather than
/// trusting this comment.
const NO_NEEDLE: &str = "pub fn add(a: u64, b: u64) -> u64 {\n    a + b\n}\n";

/// The two shapes of a needle that says nothing about the file it is in: one
/// inside a comment, and one inside a string.
///
/// Both carry characters of several bytes each before the needle, so the
/// occurrence the filter finds starts at a byte offset that is not a character
/// boundary of the source. The filter searches the raw bytes for exactly that
/// reason, and a filter that cut the source at the offset it found would panic
/// here.
const GENEROUS: &[(&str, &str)] = &[
    (
        "a comment",
        "// テスト — the test of the filter is deliberately generous.\npub fn add(a: u64, b: u64) -> u64 {\n    a + b\n}\n",
    ),
    (
        "a string",
        "pub fn label() -> &'static str {\n    \"テスト test bench\"\n}\n",
    ),
];

/// The root of the repository this crate is built from.
///
/// `src/cdva` is two levels below it. Nothing here shells out to `git`: a test
/// that ran `git rev-parse` would answer for whatever repository the
/// environment of the run pointed it at, which under a pre-commit hook is not
/// the one the sources are in.
fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Asserts that the two modes said the same thing about a file.
///
/// They must agree on everything a reader of the report can see: both buckets,
/// every span, and every declaration. They are allowed to differ in one field
/// and in one direction — `Auto` answers `NotParsed` for a file whose bytes
/// hold no needle, where `Always` opened it — because the tool must not claim
/// it read a file it never opened. The other direction is asserted as well,
/// although nothing can produce it: `Auto` parses a subset of what `Always`
/// parses, so a filter wired backwards fails here rather than reading clean.
fn assert_modes_agree(named: &str, auto: &FileCount, always: &FileCount) {
    assert_eq!(
        auto.production, always.production,
        "{named}: the production bucket differs, so a needle is missing"
    );
    assert_eq!(
        auto.test, always.test,
        "{named}: the test bucket differs, so a needle is missing"
    );
    assert_eq!(
        auto.spans, always.spans,
        "{named}: the spans differ, so a needle is missing"
    );
    assert_eq!(
        auto.test_mod_declarations, always.test_mod_declarations,
        "{named}: the declarations differ, so a needle is missing"
    );

    if auto.parse_status != always.parse_status {
        assert_eq!(
            auto.parse_status,
            ParseStatus::NotParsed,
            "{named}: the filtered mode may skip a parse, and may not report another one"
        );
        assert_ne!(
            always.parse_status,
            ParseStatus::NotParsed,
            "{named}: the unfiltered mode parses every file of a language with a rule"
        );
    }
}

#[test]
fn the_two_modes_mark_every_fixture_of_the_corpus_the_same() {
    for corpus in CORPORA {
        for name in corpus.fixtures {
            assert_modes_agree(
                &format!("{}: `{name}`", corpus.language.name()),
                &corpus.counted_in(name, TreeMode::Auto),
                &corpus.counted_in(name, TreeMode::Always),
            );
        }
    }
}

#[test]
fn the_needle_filter_holds_a_fixture_of_the_corpus_back_from_the_parser() {
    // The agreement above is worth nothing on its own: a filter that let every
    // file through would satisfy it by comparing a thing with itself. This
    // names the fixtures the filter actually stops, which is what makes the
    // agreement a statement about the needles rather than about nothing.
    let mut held_back = Vec::new();
    for corpus in CORPORA {
        for name in corpus.fixtures {
            let auto = corpus.counted_in(name, TreeMode::Auto);
            if auto.parse_status != ParseStatus::NotParsed {
                continue;
            }
            assert_eq!(
                auto.test.total(),
                0,
                "{}: `{name}` was never parsed, so nothing in it can be marked",
                corpus.language.name()
            );
            held_back.push(format!("{}/{name}", corpus.directory));
        }
    }

    assert!(
        !held_back.is_empty(),
        "no fixture of the corpus is held back, so the agreement of the two \
         modes says nothing about the needles"
    );
}

#[test]
fn the_two_modes_mark_every_file_of_this_repository_the_same() {
    // The corpus holds a fixture per syntactic form somebody thought of. This
    // reads the repository the tool was built from, which holds the forms
    // nobody thought of: a needle missing for a construct that has no fixture
    // shows up here as a file the two modes disagree about.
    let found = walk(&[repository_root()], WalkOptions::default())
        .expect("the repository this crate is built from can be walked");
    assert!(
        found.len() > 100,
        "the walk found {} files, which is not this repository",
        found.len()
    );

    let auto = counter_in(TreeMode::Auto);
    let always = counter_in(TreeMode::Always);
    let mut compared = 0_usize;
    let mut held_back = 0_usize;

    for (path, relative) in &found {
        // One read, two counts. A read per mode would let a file that somebody
        // saved between the two of them look like a file the two modes
        // disagree about, and this test would then fail for a reason that has
        // nothing to do with the needles.
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(source) = String::from_utf8(bytes) else {
            continue;
        };
        let Some(under_auto) = auto.count_source(path, relative, &source) else {
            continue;
        };
        let under_always = always
            .count_source(path, relative, &source)
            .expect("the two modes read the language of a path alike");

        assert_modes_agree(&path.display().to_string(), &under_auto, &under_always);
        compared += 1;
        if under_auto.parse_status == ParseStatus::NotParsed
            && under_always.parse_status != ParseStatus::NotParsed
        {
            held_back += 1;
        }
    }

    assert!(
        compared > 100,
        "only {compared} files of this repository were counted"
    );
    assert!(
        held_back > 0,
        "the filter parsed every one of the {compared} files it was handed, so \
         this comparison says nothing about the needles"
    );
}

#[test]
fn the_never_mode_reads_the_path_rule_alone() {
    let source = source("cfg_test_mod");
    let path = rust().as_counted("cfg_test_mod");
    let count_with = |mode| {
        counter_in(mode)
            .count_source(&path, &path, &source)
            .expect("Rust is a language the tool counts")
    };

    let never = count_with(TreeMode::Never);
    assert_eq!(
        never.test.total(),
        0,
        "this is what --no-tree reports for a file whose test code is a \
         #[cfg(test)] mod"
    );
    assert!(never.spans.is_empty());
    assert_eq!(never.parse_status, ParseStatus::NotParsed);
    assert_eq!(never.production, lines::count(&source, Language::Rust));

    assert!(
        count_with(TreeMode::Auto).test.total() > 0,
        "the default mode finds the module the fast mode cannot see"
    );

    // A counter that was handed no tree rule at all and one told never to read
    // the one it has must answer the same, or `--no-tree` would mean two
    // different things depending on how the counter was built.
    assert_eq!(
        never,
        Counter::new(PathRules::builtin())
            .count_source(&path, &path, &source)
            .expect("Rust is a language the tool counts")
    );
}

#[test]
fn a_file_whose_bytes_hold_no_needle_is_never_parsed() {
    assert!(
        Language::Rust
            .needles()
            .iter()
            .all(|needle| !NO_NEEDLE.contains(needle)),
        "the source of this test holds a needle after all, so it proves nothing"
    );

    let path = Path::new("src/add.rs");
    let count_with = |mode| {
        counter_in(mode)
            .count_source(path, path, NO_NEEDLE)
            .expect("Rust is a language the tool counts")
    };

    let auto = count_with(TreeMode::Auto);
    assert_eq!(
        auto.parse_status,
        ParseStatus::NotParsed,
        "the tool must not name a parse it never ran"
    );
    assert_eq!(
        count_with(TreeMode::Always).parse_status,
        ParseStatus::Clean,
        "the same file parses when the filter is off, so the answer above is \
         the filter and not a defect in the file"
    );
    assert_eq!(auto.test.total(), 0);
    assert_eq!(auto.production, lines::count(NO_NEEDLE, Language::Rust));
}

#[test]
fn a_needle_that_says_nothing_still_buys_a_parse_and_marks_nothing() {
    for (where_it_is, source) in GENEROUS {
        let path = Path::new("src/generous.rs");
        let auto = counter_in(TreeMode::Auto)
            .count_source(path, path, source)
            .expect("Rust is a language the tool counts");

        assert_eq!(
            auto.parse_status,
            ParseStatus::Clean,
            "a needle in {where_it_is} reaches the parser: the filter reads \
             bytes and knows nothing of syntax"
        );
        assert_eq!(
            auto.test.total(),
            0,
            "a needle in {where_it_is} is not a test node"
        );
        assert_eq!(
            auto,
            counter_in(TreeMode::Always)
                .count_source(path, path, source)
                .expect("Rust is a language the tool counts"),
            "a needle in {where_it_is}: the two modes read this file alike"
        );
    }
}

#[test]
fn a_language_with_no_tree_rule_carries_no_needle() {
    for &language in Language::all() {
        if language.tree_query().is_none() {
            assert!(
                language.needles().is_empty(),
                "{}: a needle filters a parse that never happens",
                language.name()
            );
        }
    }
}

//! The language table.
//!
//! One list declares every language the tool counts, and everything else in
//! this module is generated from that list: the [`Language`] enum, the display
//! name of each language, the stable order of [`Language::all`], the extensions
//! and file names that [`Language::from_path`] reads, and the comment and
//! string syntax that [`Language::comment_syntax`] hands to the line
//! classifier.
//!
//! The list is the single source of truth on purpose. A hand-written enum
//! beside a hand-written table drifts, and the drift is spelled as an absence —
//! a variant that no row mentions is a language the tool silently never
//! detects, or one whose comments it silently never reads. Here a variant
//! cannot exist without its row, because the macro writes both from the same
//! line.

use std::path::Path;

/// One row of the language table: a language, the extensions that name it, and
/// the whole file names that name it.
struct Entry {
    language: Language,
    extensions: &'static [&'static str],
    file_names: &'static [&'static str],
}

/// The comment and string syntax of one language, as the line classifier reads
/// it.
///
/// The classifier tries the three groups in one order at every position, and
/// the order is what makes Lua `--[[` a block comment rather than a line
/// comment: block openers first, then line comment tokens, then string openers.
/// Within a group the longer delimiter is listed first, so `"""` is tried
/// before `"`. A violation of either rule is a silent miscount, so a test
/// asserts both over the whole table.
pub struct CommentSyntax {
    /// The tokens that comment out the rest of their row.
    pub line: &'static [&'static str],
    /// The block comments, longest opener first.
    pub block: &'static [BlockSpec],
    /// Whether a block comment of this language nests, as one in Rust does.
    pub nested_block: bool,
    /// The string forms, longest opener first.
    pub strings: &'static [StringSpec],
    /// Whether this language spells a raw string as Rust does — `r"..."`,
    /// `r#"..."#`, `r##"..."##` — where the count of the hash marks varies and
    /// no fixed pair of delimiters describes it.
    pub raw_hash_strings: bool,
    /// Whether the single quote of this language opens a character literal
    /// that the classifier must tell from an *unpaired* single quote by
    /// looking ahead.
    ///
    /// Rust is the one language here that needs this, and it needs it because
    /// it spells both: `'"'` is a character literal, and `&'static str` is a
    /// lifetime. Neither reading of the quote can be the standing one. A
    /// [`StringSpec`] on the quote would open a string at every lifetime, and a
    /// Rust string spans rows, so that phantom string would run to the next
    /// quote anywhere in the file — the very bug this flag exists to end, in a
    /// far more common spelling.
    ///
    /// A language whose quote is unambiguous wants no flag and a plain
    /// [`StringSpec`] instead: Zig, Kotlin, Go, C, Java, and the rest of the
    /// table carry one. A language that spells a character literal *and* an
    /// unpaired quote carries neither today, and the rows below say why.
    pub char_literal_lookahead: bool,
}

/// One block comment form.
pub struct BlockSpec {
    /// The token that opens the comment.
    pub open: &'static str,
    /// The token that closes it.
    pub close: &'static str,
    /// Whether the two tokens are read only at the very start of a row, before
    /// any white space. Ruby `=begin` and Perl `=pod` are such tokens: an
    /// indented one is not a comment, and neither is one in the middle of a
    /// row.
    pub line_anchored: bool,
}

/// One string form.
pub struct StringSpec {
    /// The token that opens the string.
    pub open: &'static str,
    /// The token that closes it, which is often the same one.
    pub close: &'static str,
    /// The character that quotes the next one, where the language has one.
    pub escape: Option<char>,
    /// Whether the string may hold a row break.
    pub multiline: bool,
    /// Whether a string that opens its row counts as a comment rather than as
    /// code, as a Python docstring does. `cloc` counts a docstring as a
    /// comment, and the rule it uses is positional: the opener must be the
    /// first character of the row that is not white space. `s = """x"""` is a
    /// string, and therefore code.
    pub doc_when_line_leading: bool,
}

/// The chain of attributes that precedes an item, where the language spells an
/// attribute as a preceding *sibling* of the item it decorates rather than as a
/// child of it.
///
/// Rust is such a language: an `attribute_item` is a sibling of the
/// `mod_item` it applies to, so a query that captures the `mod_item` alone
/// loses the `#[cfg(test)]` row. The tree rule walks the whole contiguous chain
/// of siblings of this kind, and a match of `pattern` against the source text
/// of any one of them makes the item test code. Walking the whole chain rather
/// than the one adjacent sibling is what makes the stack `#[rstest]` over
/// `#[case(1)]` work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributeChain {
    /// The node kind that spells an attribute preceding an item.
    pub kind: &'static str,
    /// A match on the source text of the attribute makes the item a test.
    pub pattern: &'static str,
}

/// The tree rule of one language: the grammar, the query, and the two facts
/// about the shape of that language's tree that a query cannot state.
///
/// The grammar arrives behind a function because a `tree_sitter::Language` is
/// built at run time and a row of this table is built at compile time.
struct TreeRule {
    /// The tree-sitter grammar of the language.
    grammar: fn() -> tree_sitter::Language,
    /// The query naming the nodes whose rows are test rows.
    query: &'static str,
    /// The attribute chain, where the language spells an attribute as a sibling.
    attribute_chain: Option<AttributeChain>,
    /// The node kinds a `@test_scope` capture may climb to.
    scope_kinds: &'static [&'static str],
    /// The literals that must appear in a file before it is worth parsing. See
    /// [`Language::needles`].
    needles: &'static [&'static str],
}

/// The nodes of a Rust file whose rows may be test rows.
///
/// Both are candidates rather than tests, because in Rust nothing inside
/// either node says it is one: the `#[cfg(test)]` or `#[test]` that decides is
/// a *sibling* of the item, so the chain of attributes before the node has to
/// be read as well.
const RUST_QUERY: &str = "(mod_item) @candidate\n(function_item) @candidate\n";

/// The attributes that make a Rust item test code.
///
/// This one expression covers `#[cfg(test)] mod tests`, `#[cfg(test)] mod
/// other;`, `#[test] fn`, `#[tokio::test] async fn`, `#[cfg(all(test, feature =
/// "x"))] mod`, `#[bench]`, and the stack `#[rstest]` over `#[case(1)]`. It
/// leaves a `///` doc comment that holds a fenced example alone, which is what
/// keeps a doc test a comment and the total of this tool in agreement with
/// `cloc`.
const RUST_ATTRIBUTE: &str = r"^#\[\s*(cfg\s*\(.*\btest\b|cfg_attr\s*\(.*\btest\b|.*\btest\s*\]|.*::test\s*\]|rstest|bench|test_case|proptest)";

/// The literals a Rust file must hold before it is parsed.
///
/// Every branch of [`RUST_ATTRIBUTE`] but one holds `test`: `cfg(test)`,
/// `#[test]`, `#[tokio::test]`, `rstest`, `test_case`, and `proptest` all do,
/// and so does the `#[cfg(test)] mod <name>;` the module pass reads. The one
/// branch that does not is `bench`, which is why it is listed apart rather
/// than folded into the first.
const RUST_NEEDLES: &[&str] = &["test", "bench"];

/// The grammar of Rust.
fn rust_grammar() -> tree_sitter::Language {
    tree_sitter_rust::LANGUAGE.into()
}

/// The tree rule of Rust.
const RUST_TREE: Option<TreeRule> = Some(TreeRule {
    grammar: rust_grammar,
    query: RUST_QUERY,
    attribute_chain: Some(AttributeChain {
        kind: "attribute_item",
        pattern: RUST_ATTRIBUTE,
    }),
    scope_kinds: &[],
    needles: RUST_NEEDLES,
});

/// A tree rule that says everything it has to say in its query.
///
/// Most languages are this shape: the thing that makes a node a test — the
/// annotation, or the name — is a *child* of the node, so the query alone
/// reaches it and the two fields below have nothing to add. Only a language
/// that spells its annotation as a sibling wants an attribute chain, and only
/// one that marks an enclosing node wants a scope kind.
const fn plain(
    grammar: fn() -> tree_sitter::Language,
    query: &'static str,
    needles: &'static [&'static str],
) -> Option<TreeRule> {
    Some(TreeRule {
        grammar,
        query,
        attribute_chain: None,
        scope_kinds: &[],
        needles,
    })
}

/// The functions of a Go file whose rows are test rows.
///
/// The trailing `([A-Z_]|$)` is the whole of the rule that `go test` uses, and
/// it is what keeps `func Testify()` and `func TestingHelper()` out of the test
/// bucket: a test there is `Test` followed by the end of the name or by a
/// character that opens a new word.
const GO_QUERY: &str = r#"((function_declaration name: (identifier) @_n) @test
 (#match? @_n "^(Test|Benchmark|Fuzz|Example)([A-Z_]|$)"))
"#;

/// The literals a Go file must hold before it is parsed.
///
/// One per prefix the pattern of the query names. Each is written with its
/// capital letter, because `go test` reads the prefix of an exported name and
/// the query anchors on it.
const GO_NEEDLES: &[&str] = &["Test", "Benchmark", "Fuzz", "Example"];

/// The tests of a Zig file.
///
/// Zig is the clean case of the whole table. A test there is a language
/// construct beside `fn` and `struct`, so the grammar names it outright and no
/// heuristic over a name enters.
const ZIG_QUERY: &str = "(test_declaration) @test\n";

/// The literal a Zig file must hold before it is parsed. A `test_declaration`
/// opens with the keyword `test`, so the one needle is exact rather than
/// generous.
const ZIG_NEEDLES: &[&str] = &["test"];

/// The definitions of a Python file whose rows are test rows.
///
/// The second pattern is not a duplicate of the first. A decorated function is
/// a `decorated_definition` that *holds* a `function_definition`, so the first
/// pattern alone starts the span at the `def` and leaves the `@pytest.mark…`
/// rows above it in the production bucket. The second pattern captures the
/// outer node, and the union of the two spans is the whole thing.
const PYTHON_QUERY: &str = r#"((function_definition name: (identifier) @_n) @test (#match? @_n "^test_"))
((decorated_definition definition: (function_definition name: (identifier) @_n)) @test (#match? @_n "^test_"))
((decorated_definition (decorator) @_d) @test (#match? @_d "pytest"))
((class_definition name: (identifier) @_n) @test (#match? @_n "^Test"))
((class_definition superclasses: (argument_list) @_s) @test (#match? @_s "TestCase"))
"#;

/// The literals a Python file must hold before it is parsed.
///
/// `test` covers the `^test_` of the two function patterns and the `pytest` of
/// the decorator pattern, and `Test` covers the `^Test` of the class pattern
/// and the `TestCase` of the superclass pattern. The search is over bytes and
/// therefore case-sensitive, so the two spellings are two needles.
const PYTHON_NEEDLES: &[&str] = &["test", "Test"];

/// The calls of a JavaScript, TypeScript, or TSX file whose rows are test rows.
///
/// The three languages share one rule because they share one way of spelling a
/// test: a call to a function that a runner defined. Matching the *whole*
/// function expression rather than an identifier is what covers `it.each`,
/// `it.only`, `test.concurrent`, and `describe.skip` without a pattern each,
/// and the word boundary at the end is what keeps `testHelper()` out.
///
/// The doubled backslash is not a Rust escape — this is a raw string, and the
/// query language has an unescaping pass of its own. Tree-sitter reads `\\` as
/// one backslash and hands the regular expression the `\b` it wants.
const SCRIPT_QUERY: &str = r#"((call_expression function: (_) @_f) @test
 (#match? @_f "^(describe|it|test|suite|bench|context)\\b"))
"#;

/// The literals a JavaScript, TypeScript, or TSX file must hold before it is
/// parsed. They are the alternation of [`SCRIPT_QUERY`], word for word.
///
/// `it` appears in nearly every file of these languages — in `with`, `omit`,
/// `split`, and a hundred other words — so the filter buys little here. That is
/// the correct trade rather than an oversight: `it ('x', …)` with a space
/// between the name and the parenthesis is legal, so a needle of `it(` would
/// drop a real test, and a dropped test is a silent undercount while a needless
/// parse is merely slow.
const SCRIPT_NEEDLES: &[&str] = &["describe", "it", "test", "suite", "bench", "context"];

/// The methods and classes of a Java file whose rows are test rows.
///
/// The alternation is what reads both spellings of an annotation. A bare
/// `@Test` is a `marker_annotation`, and `@ValueSource(ints = {1, 2})` is an
/// `annotation`, so a query naming one of the two kinds would drop every test
/// whose deciding annotation is written the other way.
///
/// The annotation is a child of the item it decorates here, which is what makes
/// Java simpler than Rust: the span of the `method_declaration` already reaches
/// back over the whole stack of annotations, and no attribute chain is wanted.
///
/// The second pattern marks a whole class, because `@RunWith`, `@ExtendWith`,
/// and `@SpringBootTest` decorate the class rather than any one method of it.
const JAVA_QUERY: &str = r#"((method_declaration (modifiers [(marker_annotation name: (identifier) @_a) (annotation name: (identifier) @_a)])) @test
 (#match? @_a "^(Test|ParameterizedTest|RepeatedTest|BeforeEach|AfterEach|BeforeAll|AfterAll|Before|After)$"))
((class_declaration (modifiers [(marker_annotation name: (identifier) @_a) (annotation name: (identifier) @_a)])) @test
 (#match? @_a "^(RunWith|ExtendWith|SpringBootTest)$"))
"#;

/// The literals a Java file must hold before it is parsed.
///
/// Written without the `@`, because `@ Test` with a space between is legal Java
/// and the annotation the query reads is the identifier rather than the sign.
/// `Test` alone covers `ParameterizedTest`, `RepeatedTest`, and
/// `SpringBootTest`, and `Before` and `After` cover the four lifecycle hooks
/// between them.
const JAVA_NEEDLES: &[&str] = &["Test", "Before", "After", "RunWith", "ExtendWith"];

/// The functions of a Kotlin file whose rows are test rows.
///
/// Kotlin spells one annotation kind rather than two: an annotation there is an
/// `annotation` holding a `user_type`, whether or not it carries arguments.
const KOTLIN_QUERY: &str = r#"((function_declaration (modifiers (annotation (user_type (identifier) @_a)))) @test
 (#match? @_a "^(Test|ParameterizedTest|RepeatedTest|Before|After|BeforeEach|AfterEach)$"))
"#;

/// The literals a Kotlin file must hold before it is parsed. Three cover the
/// seven names of the query: `Test` takes the three tests, and `Before` and
/// `After` take the four hooks.
const KOTLIN_NEEDLES: &[&str] = &["Test", "Before", "After"];

/// The methods and classes of a C# file whose rows are test rows.
///
/// One name each is enough for the four runners in service: `Test` and
/// `TestCase` are NUnit, `Fact` and `Theory` are xUnit, `TestMethod` is MSTest,
/// and `SetUp`/`TearDown` are the fixtures around them. The second pattern
/// marks a whole class for the two runners that decorate one.
const CSHARP_QUERY: &str = r#"((method_declaration (attribute_list (attribute name: (identifier) @_a))) @test
 (#match? @_a "^(Test|Fact|Theory|TestMethod|TestCase|SetUp|TearDown)$"))
((class_declaration (attribute_list (attribute name: (identifier) @_a))) @test
 (#match? @_a "^(TestFixture|TestClass)$"))
"#;

/// The literals a C# file must hold before it is parsed. `Test` covers
/// `TestMethod`, `TestCase`, `TestFixture`, and `TestClass` as well as the bare
/// `Test` of NUnit.
const CSHARP_NEEDLES: &[&str] = &["Test", "Fact", "Theory", "SetUp", "TearDown"];

/// The blocks and classes of a Ruby file whose rows are test rows.
///
/// The receiver does not enter the first pattern, so it reads `RSpec.describe
/// "x" do` and a bare `describe "x" do` alike. The `do_block` is what keeps the
/// pattern off a call of a method that merely shares a name with a block of
/// RSpec: `ledger.describe("totals")` carries no block and marks nothing.
///
/// The second pattern is Minitest, whose tests are methods of a class that
/// inherits a case rather than blocks.
const RUBY_QUERY: &str = r#"((call method: (identifier) @_m block: (do_block)) @test
 (#match? @_m "^(describe|context|feature|it|specify|scenario)$"))
((class superclass: (superclass) @_s) @test
 (#match? @_s "Minitest::Test|Test::Unit::TestCase|ActiveSupport::TestCase"))
"#;

/// The literals a Ruby file must hold before it is parsed: the six block names
/// of the first pattern, and the two that cover the three superclasses of the
/// second — `Test::Unit::TestCase` and `ActiveSupport::TestCase` both hold
/// `TestCase`, and `Minitest::Test` holds `Minitest`.
const RUBY_NEEDLES: &[&str] = &[
    "describe", "context", "feature", "it", "specify", "scenario", "Minitest", "TestCase",
];

/// The classes and functions of a Swift file whose rows are test rows.
///
/// XCTest and Quick both spell a test suite as a class that inherits one, which
/// the first pattern reads. The second reads the Swift Testing library, whose
/// test is a free function carrying a `@Test` or `@Suite` attribute.
const SWIFT_QUERY: &str = r#"((class_declaration (inheritance_specifier inherits_from: (user_type (type_identifier) @_s))) @test
 (#match? @_s "^(XCTestCase|QuickSpec)$"))
((function_declaration (modifiers (attribute (user_type (type_identifier) @_a)))) @test
 (#match? @_a "^(Test|Suite)$"))
"#;

/// The literals a Swift file must hold before it is parsed: the two classes the
/// first pattern names, and the two attributes of the second. `Test` is listed
/// although `XCTestCase` holds it, because the attribute of the Swift Testing
/// library stands alone.
const SWIFT_NEEDLES: &[&str] = &["XCTestCase", "QuickSpec", "Test", "Suite"];

/// The calls of an Elixir file whose rows are test rows.
///
/// The first pattern is the block a test is written in, and the string in the
/// arguments is what tells `test "adds" do` from a `def test do` of production
/// code.
///
/// The second is the one use of `@test_scope` in the whole tool. `use
/// ExUnit.Case` says nothing about the rows around it and everything about the
/// module that holds it, so the capture climbs to the outermost enclosing
/// `call` — which is the `defmodule` — and marks all of it. A neighbouring
/// production module is a `call` of its own and is left alone.
const ELIXIR_QUERY: &str = r#"((call target: (identifier) @_t (arguments (string)) (do_block)) @test
 (#match? @_t "^(test|describe|property)$"))
((call target: (identifier) @_t (arguments (alias) @_a)) @test_scope
 (#eq? @_t "use") (#match? @_a "ExUnit"))
"#;

/// The literals an Elixir file must hold before it is parsed: the three block
/// names of the first pattern, and the `ExUnit` the second climbs from.
const ELIXIR_NEEDLES: &[&str] = &["test", "describe", "property", "ExUnit"];

/// The node kind an Elixir `@test_scope` capture climbs to.
///
/// `defmodule Foo do … end` is a `call`, exactly as the `use ExUnit.Case`
/// inside it is, so the outermost `call` above the `use` is the module it
/// belongs to.
const ELIXIR_SCOPE_KINDS: &[&str] = &["call"];

/// The grammar of Go.
fn go_grammar() -> tree_sitter::Language {
    tree_sitter_go::LANGUAGE.into()
}

/// The grammar of Zig.
fn zig_grammar() -> tree_sitter::Language {
    tree_sitter_zig::LANGUAGE.into()
}

/// The grammar of Python.
fn python_grammar() -> tree_sitter::Language {
    tree_sitter_python::LANGUAGE.into()
}

/// The grammar of JavaScript.
fn javascript_grammar() -> tree_sitter::Language {
    tree_sitter_javascript::LANGUAGE.into()
}

/// The grammar of TypeScript.
fn typescript_grammar() -> tree_sitter::Language {
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
}

/// The grammar of TSX, which is a grammar of its own and not a mode of the one
/// above. `<span>` is an element in TSX and a type assertion in TypeScript, so
/// a TSX file read under the TypeScript grammar fails to parse and counts
/// wholly as production code.
fn tsx_grammar() -> tree_sitter::Language {
    tree_sitter_typescript::LANGUAGE_TSX.into()
}

/// The grammar of Java.
fn java_grammar() -> tree_sitter::Language {
    tree_sitter_java::LANGUAGE.into()
}

/// The grammar of Kotlin, which arrives from `tree-sitter-kotlin-ng` rather
/// than from `tree-sitter-kotlin`. The two are separate crates over separate
/// grammars, and the node kinds the query above names are the ones of this one.
fn kotlin_grammar() -> tree_sitter::Language {
    tree_sitter_kotlin_ng::LANGUAGE.into()
}

/// The grammar of C#.
fn csharp_grammar() -> tree_sitter::Language {
    tree_sitter_c_sharp::LANGUAGE.into()
}

/// The grammar of Ruby.
fn ruby_grammar() -> tree_sitter::Language {
    tree_sitter_ruby::LANGUAGE.into()
}

/// The grammar of Swift.
fn swift_grammar() -> tree_sitter::Language {
    tree_sitter_swift::LANGUAGE.into()
}

/// The grammar of Elixir.
fn elixir_grammar() -> tree_sitter::Language {
    tree_sitter_elixir::LANGUAGE.into()
}

/// The tree rule of Go.
const GO_TREE: Option<TreeRule> = plain(go_grammar, GO_QUERY, GO_NEEDLES);

/// The tree rule of Zig.
const ZIG_TREE: Option<TreeRule> = plain(zig_grammar, ZIG_QUERY, ZIG_NEEDLES);

/// The tree rule of Python.
const PYTHON_TREE: Option<TreeRule> = plain(python_grammar, PYTHON_QUERY, PYTHON_NEEDLES);

/// The tree rule of JavaScript.
const JAVASCRIPT_TREE: Option<TreeRule> = plain(javascript_grammar, SCRIPT_QUERY, SCRIPT_NEEDLES);

/// The tree rule of TypeScript.
const TYPESCRIPT_TREE: Option<TreeRule> = plain(typescript_grammar, SCRIPT_QUERY, SCRIPT_NEEDLES);

/// The tree rule of TSX.
const TSX_TREE: Option<TreeRule> = plain(tsx_grammar, SCRIPT_QUERY, SCRIPT_NEEDLES);

/// The tree rule of Java.
const JAVA_TREE: Option<TreeRule> = plain(java_grammar, JAVA_QUERY, JAVA_NEEDLES);

/// The tree rule of Kotlin.
const KOTLIN_TREE: Option<TreeRule> = plain(kotlin_grammar, KOTLIN_QUERY, KOTLIN_NEEDLES);

/// The tree rule of C#.
const CSHARP_TREE: Option<TreeRule> = plain(csharp_grammar, CSHARP_QUERY, CSHARP_NEEDLES);

/// The tree rule of Ruby.
const RUBY_TREE: Option<TreeRule> = plain(ruby_grammar, RUBY_QUERY, RUBY_NEEDLES);

/// The tree rule of Swift.
const SWIFT_TREE: Option<TreeRule> = plain(swift_grammar, SWIFT_QUERY, SWIFT_NEEDLES);

/// The tree rule of Elixir, the one rule of the table that marks a node it
/// climbs to rather than one the query captured.
const ELIXIR_TREE: Option<TreeRule> = Some(TreeRule {
    grammar: elixir_grammar,
    query: ELIXIR_QUERY,
    attribute_chain: None,
    scope_kinds: ELIXIR_SCOPE_KINDS,
    needles: ELIXIR_NEEDLES,
});

/// A language whose test code the path rule finds on its own, for now.
///
/// A later slice turns one of these into a rule of its own, and the whole of
/// that change is this word and a fixture.
const NO_TREE: Option<TreeRule> = None;

/// A block comment that is read anywhere in a row.
const fn block(open: &'static str, close: &'static str) -> BlockSpec {
    BlockSpec {
        open,
        close,
        line_anchored: false,
    }
}

/// A block comment that is read only at the very start of a row.
const fn anchored(open: &'static str, close: &'static str) -> BlockSpec {
    BlockSpec {
        open,
        close,
        line_anchored: true,
    }
}

/// One string form, spelled out.
const fn quote(
    open: &'static str,
    close: &'static str,
    escape: Option<char>,
    multiline: bool,
    doc_when_line_leading: bool,
) -> StringSpec {
    StringSpec {
        open,
        close,
        escape,
        multiline,
        doc_when_line_leading,
    }
}

/// The character every language below spells an escape with.
const BACKSLASH: Option<char> = Some('\\');

/// `"…"` on one row, with a backslash escape.
const DQ_ESC: StringSpec = quote("\"", "\"", BACKSLASH, false, false);
/// `"…"` over many rows, with a backslash escape, as Rust has.
const DQ_ESC_ML: StringSpec = quote("\"", "\"", BACKSLASH, true, false);
/// `"…"` on one row, with no escape.
const DQ_PLAIN: StringSpec = quote("\"", "\"", None, false, false);
/// `'…'` on one row, with a backslash escape.
const SQ_ESC: StringSpec = quote("'", "'", BACKSLASH, false, false);
/// `'…'` on one row, with no escape, as a shell has.
const SQ_PLAIN: StringSpec = quote("'", "'", None, false, false);
/// `"""…"""` over many rows.
const TDQ: StringSpec = quote("\"\"\"", "\"\"\"", None, true, false);
/// `"""…"""` over many rows, a comment when it opens its row.
const TDQ_DOC: StringSpec = quote("\"\"\"", "\"\"\"", None, true, true);
/// `'''…'''` over many rows.
const TSQ: StringSpec = quote("'''", "'''", None, true, false);
/// `'''…'''` over many rows, a comment when it opens its row.
const TSQ_DOC: StringSpec = quote("'''", "'''", None, true, true);
/// A back-quoted string over many rows, with no escape, as Go has.
const BACKTICK: StringSpec = quote("`", "`", None, true, false);
/// A back-quoted template over many rows, with a backslash escape, as
/// JavaScript has.
const BACKTICK_ESC: StringSpec = quote("`", "`", BACKSLASH, true, false);
/// The Lua long bracket, `[[…]]`.
const LUA_LONG: StringSpec = quote("[[", "]]", None, true, false);
/// The Nix indented string, `''…''`.
const NIX_INDENTED: StringSpec = quote("''", "''", None, true, false);

/// `/*…*/`, which most of the C family spells the same way.
const C_BLOCK: BlockSpec = block("/*", "*/");
/// `<!--…-->`, of HTML and XML.
const MARKUP_BLOCK: BlockSpec = block("<!--", "-->");
/// `<#…#>`, of PowerShell.
const POWERSHELL_BLOCK: BlockSpec = block("<#", "#>");
/// `--[[…]]`, of Lua. It is listed as a block comment so that it is read
/// before the `--` line comment token.
const LUA_BLOCK: BlockSpec = block("--[[", "]]");
/// `{-…-}`, of Haskell, which nests.
const HASKELL_BLOCK: BlockSpec = block("{-", "-}");
/// `=begin`/`=end`, of Ruby, which is read only at the start of a row.
const RUBY_BLOCK: BlockSpec = anchored("=begin", "=end");
/// `=pod`/`=cut`, the POD of Perl, which is read only at the start of a row.
const PERL_BLOCK: BlockSpec = anchored("=pod", "=cut");

/// Declares the language table, and everything derived from it.
///
/// Each row reads
/// `Variant => "Display name", [extensions], [file names], line: [tokens],
/// block: [specs], nested: bool, strings: [specs], raw_hash: bool,
/// char_lit: bool, tree: rule;`.
///
/// Extensions are written in lower case, because [`Language::from_path`]
/// compares them without regard to case. File names are written exactly as they
/// appear on disk, because that comparison is exact. The block and string specs
/// are the named constants above, so a reader sees the shape of a string rather
/// than five fields of a literal.
macro_rules! language_table {
    ($(
        $variant:ident => $name:literal, [$($extension:literal),* $(,)?], [$($file_name:literal),* $(,)?],
            line: [$($line:literal),* $(,)?],
            block: [$($block:ident),* $(,)?],
            nested: $nested:literal,
            strings: [$($string:ident),* $(,)?],
            raw_hash: $raw_hash:literal,
            char_lit: $char_lit:literal,
            tree: $tree:ident;
    )*) => {
        /// A language that `cdva` counts.
        ///
        /// The order of the variants is the order of the table, which is the
        /// order [`Language::all`] reports and the order the report prints when
        /// it has no other order to follow.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Language {
            $($variant,)*
        }

        impl Language {
            /// The display name of the language, as the report prints it.
            #[must_use]
            pub fn name(self) -> &'static str {
                match self {
                    $(Language::$variant => $name,)*
                }
            }

            /// The comment and string syntax of the language, which the line
            /// classifier reads.
            #[must_use]
            pub fn comment_syntax(self) -> &'static CommentSyntax {
                match self {
                    $(Language::$variant => {
                        static SYNTAX: CommentSyntax = CommentSyntax {
                            line: &[$($line,)*],
                            block: &[$($block,)*],
                            nested_block: $nested,
                            strings: &[$($string,)*],
                            raw_hash_strings: $raw_hash,
                            char_literal_lookahead: $char_lit,
                        };
                        &SYNTAX
                    })*
                }
            }

            /// The tree rule of the language, where it has one.
            ///
            /// This is the one entrance to the column, and the four public
            /// methods below read it. A caller therefore cannot hold half a
            /// rule — a query with no grammar, or an attribute chain belonging
            /// to a language whose query never captures a candidate.
            fn tree_rule(self) -> Option<&'static TreeRule> {
                match self {
                    $(Language::$variant => {
                        static RULE: Option<TreeRule> = $tree;
                        RULE.as_ref()
                    })*
                }
            }
        }

        /// Every language, in the order of the table.
        static ALL: &[Language] = &[$(Language::$variant,)*];

        /// The table itself, in the order of the variants.
        static TABLE: &[Entry] = &[$(
            Entry {
                language: Language::$variant,
                extensions: &[$($extension,)*],
                file_names: &[$($file_name,)*],
            },
        )*];
    };
}

language_table! {
    Rust => "Rust", ["rs"], [],
        line: ["//"], block: [C_BLOCK], nested: true, strings: [DQ_ESC_ML], raw_hash: true, char_lit: true, tree: RUST_TREE;
    Go => "Go", ["go"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, BACKTICK, SQ_ESC], raw_hash: false, char_lit: false, tree: GO_TREE;
    Python => "Python", ["py", "pyi"], [],
        line: ["#"], block: [], nested: false, strings: [TDQ_DOC, TSQ_DOC, DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: PYTHON_TREE;
    JavaScript => "JavaScript", ["js", "jsx", "mjs", "cjs"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC, BACKTICK_ESC], raw_hash: false, char_lit: false, tree: JAVASCRIPT_TREE;
    TypeScript => "TypeScript", ["ts", "mts", "cts"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC, BACKTICK_ESC], raw_hash: false, char_lit: false, tree: TYPESCRIPT_TREE;
    Tsx => "TSX", ["tsx"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC, BACKTICK_ESC], raw_hash: false, char_lit: false, tree: TSX_TREE;
    Java => "Java", ["java"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [TDQ, DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: JAVA_TREE;
    // Kotlin spells a character literal `'a'` and carries no unpaired quote, so
    // the plain string form on the quote is right and no lookahead is wanted.
    Kotlin => "Kotlin", ["kt", "kts"], [],
        line: ["//"], block: [C_BLOCK], nested: true, strings: [TDQ, DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: KOTLIN_TREE;
    CSharp => "C#", ["cs"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [TDQ, DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: CSHARP_TREE;
    Ruby => "Ruby", ["rb", "rake", "gemspec"], ["Gemfile", "Rakefile"],
        line: ["#"], block: [RUBY_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: RUBY_TREE;
    // Swift has no character literal at all. A character there is a `"` string
    // of one character, which the form below already reads.
    Swift => "Swift", ["swift"], [],
        line: ["//"], block: [C_BLOCK], nested: true, strings: [TDQ, DQ_ESC], raw_hash: false, char_lit: false, tree: SWIFT_TREE;
    // Elixir gets neither rule, because its two spellings want opposite ones: a
    // charlist is `'abc'`, and a character is `?'`. A string form on the quote
    // would read the `?'` of `if c == ?' do` as the opening of a charlist. Its
    // quote therefore stays ordinary code until somebody measures the pair.
    Elixir => "Elixir", ["ex", "exs"], [],
        line: ["#"], block: [], nested: false, strings: [TDQ, DQ_ESC], raw_hash: false, char_lit: false, tree: ELIXIR_TREE;
    // Zig spells a character literal `'a'` and carries no unpaired quote, and a
    // test in Zig is a language construct rather than a name, so nothing there
    // wants a bare quote either.
    Zig => "Zig", ["zig"], [],
        line: ["//"], block: [], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: ZIG_TREE;
    C => "C", ["c"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    CHeader => "C/C++ Header", ["h"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    Cpp => "C++", ["cc", "cpp", "cxx"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    CppHeader => "C++ Header", ["hh", "hpp", "hxx"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    Php => "PHP", ["php"], [],
        line: ["//", "#"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    Shell => "Shell", ["sh", "bash", "zsh", "bats"], [],
        line: ["#"], block: [], nested: false, strings: [DQ_ESC, SQ_PLAIN], raw_hash: false, char_lit: false, tree: NO_TREE;
    PowerShell => "PowerShell", ["ps1", "psm1", "psd1"], [],
        line: ["#"], block: [POWERSHELL_BLOCK], nested: false, strings: [DQ_ESC, SQ_PLAIN], raw_hash: false, char_lit: false, tree: NO_TREE;
    Batch => "Batch", ["bat", "cmd"], [],
        line: ["::", "REM ", "rem "], block: [], nested: false, strings: [DQ_PLAIN], raw_hash: false, char_lit: false, tree: NO_TREE;
    Html => "HTML", ["html", "htm"], [],
        line: [], block: [MARKUP_BLOCK], nested: false, strings: [DQ_PLAIN, SQ_PLAIN], raw_hash: false, char_lit: false, tree: NO_TREE;
    Xml => "XML", ["xml", "xsd", "xsl"], [],
        line: [], block: [MARKUP_BLOCK], nested: false, strings: [DQ_PLAIN, SQ_PLAIN], raw_hash: false, char_lit: false, tree: NO_TREE;
    Css => "CSS", ["css"], [],
        line: [], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    Scss => "SCSS", ["scss", "sass"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    Json => "JSON", ["json"], [],
        line: [], block: [], nested: false, strings: [DQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    Yaml => "YAML", ["yaml", "yml"], [],
        line: ["#"], block: [], nested: false, strings: [DQ_ESC, SQ_PLAIN], raw_hash: false, char_lit: false, tree: NO_TREE;
    Toml => "TOML", ["toml"], [],
        line: ["#"], block: [], nested: false, strings: [TDQ, TSQ, DQ_ESC, SQ_PLAIN], raw_hash: false, char_lit: false, tree: NO_TREE;
    Ini => "INI", ["ini", "cfg"], [],
        line: ["#", ";"], block: [], nested: false, strings: [DQ_PLAIN, SQ_PLAIN], raw_hash: false, char_lit: false, tree: NO_TREE;
    Markdown => "Markdown", ["md", "markdown"], [],
        line: [], block: [], nested: false, strings: [], raw_hash: false, char_lit: false, tree: NO_TREE;
    Sql => "SQL", ["sql"], [],
        line: ["--"], block: [C_BLOCK], nested: false, strings: [SQ_ESC, DQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    Makefile => "Makefile", ["mk", "mak"], ["Makefile", "makefile", "GNUmakefile"],
        line: ["#"], block: [], nested: false, strings: [DQ_PLAIN, SQ_PLAIN], raw_hash: false, char_lit: false, tree: NO_TREE;
    Dockerfile => "Dockerfile", ["dockerfile"], ["Dockerfile", "Containerfile", "dockerfile", "containerfile"],
        line: ["#"], block: [], nested: false, strings: [DQ_PLAIN, SQ_PLAIN], raw_hash: false, char_lit: false, tree: NO_TREE;
    Lua => "Lua", ["lua"], [],
        line: ["--"], block: [LUA_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC, LUA_LONG], raw_hash: false, char_lit: false, tree: NO_TREE;
    // Scala and Haskell are the two rows that get neither rule, and they get
    // neither on purpose. Both spell a character literal `'a'` AND an unpaired
    // quote — Scala the symbol `'foo`, Haskell the primed identifier `x'` — so
    // a string form on the quote would open a string at every symbol and at
    // every primed name, which is the bug of Rust in a commoner spelling. DO
    // NOT ADD ONE. The lookahead of Rust does not fit Haskell either: a prime
    // ends a name, and `f x' 'a'` puts a quote, a space, and a quote in a row,
    // which no lookahead can tell from the literal `' '`. Their quote stays
    // ordinary code, which is right for every unpaired form and reads the
    // quotes of a character literal as code as well — the same answer for the
    // row. The one loss is `'"'`, whose quote opens a string that ends with its
    // row, and a row holding a character literal is code either way.
    Scala => "Scala", ["scala", "sc"], [],
        line: ["//"], block: [C_BLOCK], nested: true, strings: [TDQ, DQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    Haskell => "Haskell", ["hs"], [],
        line: ["--"], block: [HASKELL_BLOCK], nested: true, strings: [DQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    Nix => "Nix", ["nix"], [],
        line: ["#"], block: [C_BLOCK], nested: false, strings: [NIX_INDENTED, DQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    Protobuf => "Protocol Buffers", ["proto"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    GraphQL => "GraphQL", ["graphql", "gql"], [],
        line: ["#"], block: [], nested: false, strings: [TDQ_DOC, DQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    Perl => "Perl", ["pl", "pm"], [],
        line: ["#"], block: [PERL_BLOCK], nested: false, strings: [DQ_ESC, SQ_PLAIN], raw_hash: false, char_lit: false, tree: NO_TREE;
}

impl Language {
    /// Every language the tool knows, in a stable order.
    #[must_use]
    pub fn all() -> &'static [Language] {
        ALL
    }

    /// The language of a file, from its extension or its whole file name.
    ///
    /// The whole file name comes first, so `Makefile` is a makefile and not a
    /// file with no extension. That comparison is exact, because a file name is
    /// a name and `MAKEFILE` is a different one. The extension comes second,
    /// and that comparison ignores case, because `.RS` is Rust.
    ///
    /// Returns `None` for a file the tool does not count.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Language> {
        let file_name = path.file_name()?.to_str()?;

        if let Some(entry) = TABLE
            .iter()
            .find(|entry| entry.file_names.contains(&file_name))
        {
            return Some(entry.language);
        }

        let extension = path.extension()?.to_str()?;

        TABLE
            .iter()
            .find(|entry| {
                entry
                    .extensions
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(extension))
            })
            .map(|entry| entry.language)
    }

    /// The tree-sitter grammar, for a language the tree rule reads.
    ///
    /// Returns `None` for a language with no tree rule, which is every
    /// language whose test code is found by its path alone.
    #[must_use]
    pub fn grammar(self) -> Option<tree_sitter::Language> {
        self.tree_rule().map(|rule| (rule.grammar)())
    }

    /// The query naming the nodes whose rows are test rows.
    ///
    /// Returns `None` for a language with no tree rule.
    #[must_use]
    pub fn tree_query(self) -> Option<&'static str> {
        self.tree_rule().map(|rule| rule.query)
    }

    /// The attribute chain, for a language whose attribute is a sibling.
    ///
    /// Returns `None` for a language whose annotation is a child of the item it
    /// decorates, where the query alone gives the whole span.
    #[must_use]
    pub fn attribute_chain(self) -> Option<AttributeChain> {
        self.tree_rule().and_then(|rule| rule.attribute_chain)
    }

    /// The node kinds that a `@test_scope` capture may climb to.
    ///
    /// Empty for a language whose query never captures `@test_scope`.
    #[must_use]
    pub fn scope_kinds(self) -> &'static [&'static str] {
        self.tree_rule().map_or(&[], |rule| rule.scope_kinds)
    }

    /// Literal strings, any one of which must appear in a file before the tree
    /// rule will parse it.
    ///
    /// A parse costs far more than a scan of the rows, so a file that can hold
    /// no test is never handed to a parser: a Rust file whose bytes hold
    /// neither `test` nor `bench` can hold no test node, whatever else is in
    /// it.
    ///
    /// The set is therefore a *superset* of everything the query and the
    /// attribute chain of the language can match, and the asymmetry of the two
    /// mistakes is what settles every doubtful case in favour of the shorter,
    /// commoner needle. A needle that filters nothing is merely slow. A needle
    /// that filters too much is a silent undercount: the file is never parsed,
    /// its test rows are never found, and the tool reports a clean number that
    /// nobody can tell from a correct one.
    ///
    /// Empty for a language with no tree rule, and for one whose rule is to be
    /// parsed every time.
    #[must_use]
    pub fn needles(self) -> &'static [&'static str] {
        self.tree_rule().map_or(&[], |rule| rule.needles)
    }
}

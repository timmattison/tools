# cdva design

`cdva` — "count da various attributes" — counts the lines of code of a tree, as
`cloc` does, and reports the test code apart from the production code. It
answers issue #406.

This document is the brief. It records the decisions that the issue left open,
the module layout, the shared types, and the facts about `tree-sitter` that were
measured rather than assumed.

## The decisions

The issue closes with five open questions. Four are answered here. The fifth,
generated code, is a separate ask and stays out.

1. **The name is `cdva`.** The issue proposes `clot`.
2. **The tree rule covers every language in the issue table**: Rust, Zig,
   Python, JavaScript, TypeScript, TSX, Java, Kotlin, C#, Ruby, Swift, Go, and
   Elixir. That is 13 grammars.
3. **A Rust doc comment stays a comment**, even when it holds a fenced example
   that `cargo test` runs. Every other counter agrees, and the total of `cdva`
   thus agrees with the total of `cloc`. The README names the limit.
4. **A test data directory joins the test bucket.** `testdata/`,
   `__snapshots__/`, `fixtures/`, and `__mocks__/` are test material.
5. **No cargo feature gates the grammars.** Every grammar the tool ships is
   always on. `--no-tree` gives the fast path at run time.

## The invariant

> For every file, the production count plus the test count equals the count that
> the same tool reports with the split turned off.

The line classifier decides the *kind* of a line, which is blank, comment, or
code. The path rule and the tree rule decide the *bucket* of a line, which is
production or test. The two decisions are independent, so the invariant holds by
construction. A test asserts it over the whole fixture corpus.

## The module layout

```
src/cdva/
  Cargo.toml
  README.md
  src/
    lib.rs        the public API, and the modules below
    lang.rs       the language table: detection, comment syntax, needles, grammar
    lines.rs      LineIndex, and the blank/comment/code classifier
    pathrule.rs   the built-in globs, and the globs of the user
    treerule.rs   the parse, the query, the span, and the parse status
    file.rs       one file: the counts of both buckets, and the spans
    counts.rs     the totals, by language and over all
    report.rs     the table, --by-file, --json, --csv, and --explain
    main.rs       clap, the walk, rayon, and the exit code
  tests/
    fixtures/<lang>/...
```

## The shared types

```rust
/// The kind of one line. A line that holds both code and a comment is code.
pub enum LineKind { Blank, Comment, Code }

/// The count of one bucket of one file, or of many files.
pub struct Counts { pub blank: u64, pub comment: u64, pub code: u64 }

/// Which rule marked a span, for `--explain`.
pub enum Rule { PathGlob(String), TreeQuery(&'static str), ModDeclaration(String) }

/// One marked region of one file, in 1-based inclusive rows.
pub struct Span { pub first_row: u32, pub last_row: u32, pub rule: Rule }

/// The result of reading one file.
pub struct FileCount {
    pub path: PathBuf,
    pub language: Language,
    pub production: Counts,
    pub test: Counts,
    pub spans: Vec<Span>,
    pub parse_status: ParseStatus,
}

/// Tree-sitter recovers from a syntax error and still returns a tree, so a run
/// that throws no error proves nothing. The tool looks for an ERROR node and
/// for a missing node.
pub enum ParseStatus { NotParsed, Clean, Failed }
```

`FileCount::total()` returns `production + test`. The invariant test asserts
`total()` against the classifier alone.

## The line classifier

One pass of a state machine over the characters of the file. The states are
Normal, InBlockComment, and InString. Each language gives the classifier a
table:

```rust
pub struct CommentSyntax {
    pub line: &'static [&'static str],
    pub block: &'static [(&'static str, &'static str)],
    pub nested_block: bool,
    pub strings: &'static [StringSpec],
}

pub struct StringSpec {
    pub open: &'static str,
    pub close: &'static str,
    pub escape: Option<char>,
    pub multiline: bool,
}
```

The classifier tracks two flags for each row: the row held a character of code,
and the row held a character of a comment. A row with no character at all is
blank. A row with a code character is code. Every other row is a comment. This
is the rule of `cloc`.

A Rust raw string (`r#"..."#`) needs a rule of its own, because the count of the
hash marks varies. The classifier holds one special case for it. Python and
several other languages need a triple-quoted string, which the table expresses
as a `StringSpec` with `multiline: true`.

### Two measured rules, and where `cloc` differs

Both rules below come from running `cloc` 2.x over a sample and reading the
numbers back.

**A triple-quoted string that opens a line is a comment.** `cloc` counts a
Python docstring as a comment, and so does `cdva`. The rule is positional: when
the opener of a multi-line string is the first character of the row that is not
white space, the whole string counts as a comment. When something precedes it,
as in `s = """x"""`, the string is code. A `StringSpec` carries the flag:

```rust
pub struct StringSpec {
    pub open: &'static str,
    pub close: &'static str,
    pub escape: Option<char>,
    pub multiline: bool,
    /// A string that opens its row counts as a comment, as a docstring does.
    pub doc_when_line_leading: bool,
}
```

**A nested block comment nests.** `cloc` closes a Rust `/* a /* b */ c */` at
the first `*/` and counts the rest as code. Rust nests such a comment, so
`cloc` is wrong there and `cdva` is right. The number of `cdva` thus differs
from the number of `cloc` by design for a file that nests a block comment. The
invariant of this tool is internal, and it says the two buckets sum to the
unsplit count of the same tool. It does not say that the total agrees with
`cloc` line for line.

**A byte offset never indexes a string directly.** `LineIndex` holds the byte
offset of the start of each row, and a binary search converts an offset to a
row. `clippy::string_slice` is a warning in the workspace lint set.

## The path rule

The path rule marks a whole file. It runs first, and a file that it marks needs
no parse. The table of the issue is the built-in set, plus the test data
directories of decision 4.

`--test-glob` and `--production-glob` add to the built-in set.
`--production-glob` wins over `--test-glob`, and a user glob wins over the
built-in set. The order makes an override possible.

## The tree rule

Each language gets one tree-sitter query. A capture named `test` marks the span
of the node it captures.

### The measured facts

These were measured against `tree-sitter` 0.26 and the grammar versions in the
root manifest. Do not assume them; they differ between grammars.

- **`QueryCursor::matches` returns a streaming iterator.** The trait is
  `tree_sitter::StreamingIterator`, which the `tree-sitter` crate re-exports.
  No dependency on `streaming-iterator` is needed.
- **In `tree-sitter-rust` an `attribute_item` is a preceding sibling of the item
  it decorates, and not a child of it.** A query that captures the `mod_item`
  alone loses the `#[cfg(test)]` row. The tree rule walks `prev_sibling()` while
  the kind is `attribute_item`, so a stack such as `#[rstest]` over `#[case(1)]`
  extends the span to the first attribute.
- **In `tree-sitter-rust` the arguments of an attribute are tokens and not an
  expression.** An `attribute_item` holds one `attribute`, which holds a path —
  an `identifier` or a `scoped_identifier` whose `name` field is the last name —
  and an optional `arguments` field of kind `token_tree`. That group is flat:
  `cfg(all(not(windows), test))` gives `(token_tree (identifier "all")
  (token_tree (identifier "not") (token_tree (identifier "windows"))
  (identifier "test")))`. So a call is a name that a `token_tree` follows, and
  `feature = "x"` is a name, an anonymous `=`, and a `string_literal`.
- **In Java, Kotlin, C#, Python, and Swift the annotation is a child.** The
  query alone gives the whole span, and no walk is needed. The node paths are:
  - Java: `(method_declaration (modifiers (marker_annotation name: (identifier))))`
  - Kotlin: `(function_declaration (modifiers (annotation (user_type (identifier)))))`
  - C#: `(method_declaration (attribute_list (attribute name: (identifier))))`
  - Python: `(decorated_definition (decorator ...) definition: (function_definition))`
  - Swift: `(function_declaration (modifiers (attribute (user_type (type_identifier)))))`,
    and `(class_declaration (inheritance_specifier inherits_from: (user_type (type_identifier))))`
- **Zig gives a `test_declaration` node.** A test in Zig is a language
  construct, so the query is exact and no heuristic enters.
- **Elixir spells everything as a call.** `test "b" do` is
  `(call target: (identifier) (arguments (string ...)) (do_block ...))`, and
  `use ExUnit.Case` is `(call target: (identifier) (arguments (alias)))`.
- **Ruby spells `RSpec.describe` as** `(call receiver: (constant) method:
  (identifier) block: (do_block ...))`, a bare `describe` as the same node with
  no `receiver` field at all, and a Minitest class as
  `(class superclass: (superclass (scope_resolution ...)))`.

### The traps

Each one needs a fixture of its own.

- **An attribute is a sibling.** See above.
- **`mod tests;` moves the test code to another file.** A second pass resolves a
  `#[cfg(test)] mod <name>;` declaration and marks `<name>.rs` and
  `<name>/mod.rs` in the same directory as test files.
- **A parse error is silent.** The tool looks for an `ERROR` node and for a
  missing node. Such a file counts as production code, the footer names the
  count, and `--strict` makes the run fail.
- **Spans overlap.** A `#[test] fn` inside a `#[cfg(test)] mod` gives two spans
  over the same rows. The tool holds a set of row numbers and never adds the
  lengths of the spans.
- **A test helper outside a test node counts as production code.** Tree-sitter
  reads syntax and resolves no names. The README says so.

## The speed

1. The path rule runs first, and a file it marks is never parsed.
2. A literal search over the raw bytes runs next, with `memchr::memmem` and the
   needles of the language. A Rust file whose bytes hold no `test` never reaches
   the parser.
3. `rayon` parses the survivors in parallel.

`--no-tree` turns the tree rule off. `--tree` parses every file of a known
language and skips the needle filter. A test asserts that the two modes agree
over the fixture corpus, which is what proves the needle set complete.

## The command

```
cdva [PATH...]
     [--by-file] [--json] [--csv] [--sort <column>] [--top N]
     [--tests-only] [--production-only]
     [--explain <path>]
     [--test-glob <glob>] [--production-glob <glob>]
     [--no-tree | --tree] [--strict]
     [--hidden] [--no-ignore]
```

The default report is the table of the issue. `Test code` is a part of `Code`,
and not a column beside it. A file counts in `Test files` when the tool marked
at least one of its rows as a test row.

The output carries no color, so a pipe and a terminal read the same bytes.

## The rules of this repository

- The manifest carries `[lints]` and `workspace = true`. Every target root
  states a position on each lint that the crate raises.
- `--version` comes from `buildinfo::version_string!()`.
- The tool needs an entry in `README.md` and a row in `TLDR.md`.
  `repo_guards::tool_index` fails `cargo test` without both.
- The walk uses `ignore`, so `.gitignore` holds by default.
- A test that runs in parallel with a copy of itself must not name a shared
  resource. Every fixture tree that a test writes goes under `tempfile::tempdir()`.

## Appendix: the verified queries

Every query below was run against a sample of the language and the marked rows
were read back by hand. They are measured, not guessed. A capture name that
starts with `_` is a helper for a predicate and marks nothing.

Three capture names carry meaning:

- `@test` — the span of the captured node is test code.
- `@candidate` — the node is test code only when one attribute of the chain
  that precedes it says so. The span then reaches back to the first attribute of
  the chain. Rust alone needs this, because an attribute there is a sibling.
- `@test_scope` — the outermost enclosing node of a listed kind is test code.
  Elixir alone needs this, for `use ExUnit.Case`.

### Rust

```
(mod_item) @candidate
(function_item) @candidate
```

The attribute chain is `attribute_item`, and the rule reads the tree of the
attribute rather than the text of it. An attribute says one of two things, and
the two are read two ways.

A **name** makes the item test code on its own. The name of an attribute is a
path, so the rule reads the last name of that path and looks for it among
`test`, `rstest`, `bench`, `test_case`, and `proptest`. The arguments say
nothing here, so `#[tokio::test(flavor = "multi_thread")]` reads exactly as
`#[tokio::test]` does.

A **condition** — the argument of `#[cfg(…)]` — makes the item test code when
it names the option `test` where no `not` inverts it. The grammar gives that
argument as a flat list of tokens, so the rule walks it: a name that a group
follows is `not(…)`, `all(…)`, or `any(…)`; a name that an equals sign follows
is an option with a value, such as `feature = "x"`; a name that neither follows
is a bare option. `not` inverts the condition below it and `all` and `any`
invert nothing, so:

| Attribute | Verdict |
| --- | --- |
| `#[cfg(test)]` | test |
| `#[cfg(all(test, feature = "x"))]` | test |
| `#[cfg(all(not(windows), test))]` | test |
| `#[cfg(any(test, feature = "x"))]` | test |
| `#[cfg(not(test))]` | production |
| `#[cfg(not(not(test)))]` | test |
| `#[cfg(feature = "test-support")]` | production |
| `#[cfg_attr(test, allow(dead_code))]` | production |

A regular expression over the text cannot answer this. A `cfg` condition is a
nested boolean expression, so the question names a syntactic category, and a
word search reads `not(test)` — the code that is compiled when the tests are
OFF — as test code.

`cfg_attr` is left out for the same reason it is production code: it says which
*attributes* apply and never whether the item exists.

Together this marks `#[cfg(test)] mod tests`, `#[cfg(test)] mod other;`,
`#[test] fn`, `#[tokio::test] async fn`, `#[cfg(all(test, feature = "x"))]
mod`, `#[bench]`, and the stack `#[rstest]` over `#[case(1)]`. It leaves a `///`
doc comment that holds a fenced example as a comment, which decision 3
requires.

### Go

```
((function_declaration name: (identifier) @_n) @test
 (#match? @_n "^(Test|Benchmark|Fuzz|Example)([A-Z_]|$)"))
```

The trailing `([A-Z_]|$)` is what keeps `func Testify()` out.

### Zig

```
(test_declaration) @test
```

### Python

```
((function_definition name: (identifier) @_n) @test (#match? @_n "^test_"))
((decorated_definition definition: (function_definition name: (identifier) @_n)) @test (#match? @_n "^test_"))
((decorated_definition (decorator) @_d) @test (#match? @_d "pytest"))
((class_definition name: (identifier) @_n) @test (#match? @_n "^Test"))
((class_definition superclasses: (argument_list) @_s) @test (#match? @_s "TestCase"))
```

### JavaScript, TypeScript, and TSX

```
((call_expression function: (_) @_f) @test
 (#match? @_f "^(describe|it|test|suite|bench|context)($|(\\.(only|skip|skipIf|runIf|todo|todoIf|each|for|concurrent|sequential|shuffle|failing|fails|extend|if))+($|[(`]))"))
```

The match on the whole function expression covers `it.each`, `it.only`,
`test.concurrent`, and `describe.skip` in one rule. A mode that takes an
argument gives the runner back, so `it.each([[1, 2]])("doubles %i", fn)` calls
the runner twice and the outer call carries the inner one as its function
expression. The tail of the pattern reads that argument list, and a tagged
template beside it.

The name and the chain of modes are the whole of what the pattern accepts, and
that is what keeps production code out. `context` and `it` are common variable
names in these languages, so `testHelper()`, `context.fillRect(0, 0, w, h)`,
and `it.next()` are production code.

### Java

```
((method_declaration (modifiers [(marker_annotation name: (identifier) @_a) (annotation name: (identifier) @_a)])) @test
 (#match? @_a "^(Test|ParameterizedTest|RepeatedTest|BeforeEach|AfterEach|BeforeAll|AfterAll|Before|After)$"))
((class_declaration (modifiers [(marker_annotation name: (identifier) @_a) (annotation name: (identifier) @_a)])) @test
 (#match? @_a "^(RunWith|ExtendWith|SpringBootTest)$"))
```

A bare `@Test` is a `marker_annotation`, and `@ValueSource(ints = {1, 2})` is an
`annotation`. The query takes both.

### Kotlin

```
((function_declaration (modifiers (annotation (user_type (identifier) @_a)))) @test
 (#match? @_a "^(Test|ParameterizedTest|RepeatedTest|Before|After|BeforeEach|AfterEach)$"))
```

### C#

```
((method_declaration (attribute_list (attribute name: (identifier) @_a))) @test
 (#match? @_a "^(Test|Fact|Theory|TestMethod|TestCase|SetUp|TearDown)$"))
((class_declaration (attribute_list (attribute name: (identifier) @_a))) @test
 (#match? @_a "^(TestFixture|TestClass)$"))
```

### Ruby

```
((call !receiver method: (identifier) @_m block: (do_block)) @test
 (#match? @_m "^(describe|context|feature|it|specify|scenario)$"))
((call receiver: (constant) @_r method: (identifier) @_m block: (do_block)) @test
 (#eq? @_r "RSpec")
 (#match? @_m "^(describe|context|feature|it|specify|scenario)$"))
((class superclass: (superclass) @_s) @test
 (#match? @_s "Minitest::Test|Test::Unit::TestCase|ActiveSupport::TestCase"))
```

The first rule takes a bare `describe "x" do`, which the negated field
`!receiver` holds it to, and the second takes `RSpec.describe "x" do`. A call
on any other receiver is a method of that object, so
`logger.context(name) do |scope|` stays production code.

### Swift

```
((class_declaration (inheritance_specifier inherits_from: (user_type (type_identifier) @_s))) @test
 (#match? @_s "^(XCTestCase|QuickSpec)$"))
((function_declaration (modifiers (attribute (user_type (type_identifier) @_a)))) @test
 (#match? @_a "^(Test|Suite)$"))
```

### Elixir

```
((call target: (identifier) @_t (arguments (string)) (do_block)) @test
 (#match? @_t "^(test|describe|property)$"))
((call target: (identifier) @_t (arguments (alias) @_a)) @test_scope
 (#eq? @_t "use") (#match? @_a "ExUnit"))
```

The scope kind for `@test_scope` is `call`, so `use ExUnit.Case` marks the whole
`defmodule` that holds it, and leaves a neighboring production module alone.

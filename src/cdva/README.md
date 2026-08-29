# cdva

"count da various attributes". Counts the lines of a tree the way `cloc` does,
and reports the test code apart from the production code.

```console
$ cdva src/gsw
Language  Files  Blank  Comment    Code |  Test files  Test code  Test %
------------------------------------------------------------------------
Rust         12  1,222    4,621  11,675 |          11      8,541   73.2%
TOML          1      5        0      26 |           0          0    0.0%
------------------------------------------------------------------------
Total        13  1,227    4,621  11,701 |          11      8,541   73.0%
```

`Test code` is a part of `Code`, not a column beside it. Of the 11,701 rows of
code above, 8,541 are test code and 3,160 are production code.

Every other counter reports one number for a file. `cdva` reports two, because
a file is not always one thing: a Rust source with a `#[cfg(test)] mod tests`
at the bottom holds production code and test code in one file, and one number
hides that.

## The invariant

> For every file, the production count plus the test count equals the count
> that the same tool reports with the split turned off.

This holds field by field: blank, comment, and code each add up. It is what
makes the two numbers trustworthy, and it holds by construction — the line
classifier decides the *kind* of a row (blank, comment, or code) and the two
marking rules decide the *bucket* of a row (production or test), and neither
decision reads the other.

`src/cdva/tests/repository.rs` asserts it over every file of this repository,
which is a tree nobody wrote to suit the tool.

## The two rules

### The path rule

The path rule reads a name and marks the whole file. It runs first, and a file
it marks is never parsed. The built-in set holds 34 globs — `*_test.go`,
`tests/**`, `benches/**`, `*.spec.*`, `__tests__/**`, `test_*.py`,
`*Test.java`, `*_spec.rb`, `testdata/**`, `fixtures/**`, and the rest — and
every one of them matches at any depth.

`--test-glob` and `--production-glob` add to that set. A glob of the user wins
over a built-in one, and `--production-glob` wins over `--test-glob`, so a
directory the built-in set marks can be handed back to production. A glob that
begins with `/` is anchored to the root of the walk instead of matching at any
depth.

### The tree rule

The tree rule parses what the path rule left and marks the span of each test
node *inside* the file. Thirteen grammars have one, and the three script
dialects share a query, so the table holds eleven rows:

| Language | What the rule marks |
| --- | --- |
| Rust | A `mod` or `fn` whose attributes hold a name that ends in `test`, `rstest`, `bench`, `test_case`, or `proptest` — `#[test]`, `#[tokio::test]`, `#[rstest]` — or a `#[cfg(…)]` condition that names the option `test` where no `not` inverts it. So `#[cfg(test)]`, `#[cfg(all(test, feature = "x"))]`, and `#[cfg(all(not(windows), test))]` are test code, while `#[cfg(not(test))]`, `#[cfg(feature = "test-support")]`, and `#[cfg_attr(test, allow(dead_code))]` are production code. The span reaches back over the whole attribute stack. |
| Go | A `func` named `Test…`, `Benchmark…`, `Fuzz…`, or `Example…`. The name must break there, so `Testify` is production code. |
| Zig | A `test` declaration, which is a construct of the language and needs no heuristic. |
| Python | A `def test_…`, a `class Test…`, a class inheriting `TestCase`, and any definition a `pytest` decorator marks. |
| JavaScript, TypeScript, TSX | A call to `describe`, `it`, `test`, `suite`, `bench`, or `context`, alone or under a chain of runner modes: `.only`, `.skip`, `.each`, `.concurrent`, and the rest of what Jest, Vitest, Mocha, `node:test`, and Bun spell. Nothing else after the name counts, so `testHelper()`, `context.fillRect()`, and `it.next()` are production code. |
| Java | A method annotated `@Test`, `@ParameterizedTest`, `@RepeatedTest`, `@Before…`, or `@After…`; a class annotated `@RunWith`, `@ExtendWith`, or `@SpringBootTest`. |
| Kotlin | A function annotated `@Test`, `@ParameterizedTest`, `@RepeatedTest`, `@Before…`, or `@After…`. |
| C# | A method attributed `[Test]`, `[Fact]`, `[Theory]`, `[TestMethod]`, `[TestCase]`, `[SetUp]`, or `[TearDown]`; a class attributed `[TestFixture]` or `[TestClass]`. |
| Ruby | A `describe`, `context`, `feature`, `it`, `specify`, or `scenario` block that names no receiver, or one whose receiver is `RSpec`. Any other receiver is a method of that object, so `logger.context(name) do … end` is production code. Also a class inheriting `Minitest::Test`, `Test::Unit::TestCase`, or `ActiveSupport::TestCase`. |
| Swift | A class inheriting `XCTestCase` or `QuickSpec`; a function attributed `@Test` or `@Suite`. |
| Elixir | A `test`, `describe`, or `property` block; `use ExUnit.…`, which marks the whole module that holds it and leaves a neighbouring production module alone. |

Two spans over the same rows count those rows once, so a `#[test] fn` inside a
`#[cfg(test)] mod` is not counted twice.

One rule reads across files: a `#[cfg(test)] mod tests;` declaration marks the
file it names, which is `tests.rs` or `tests/mod.rs` of the module directory of
the declaring file. That pass runs after every file is counted, and it never
touches the filesystem — a candidate is matched against the paths the walk
already found.

`--explain <PATH>` prints what marked one file, and which rule did it:

```console
$ cdva --explain src/cwt/src/main.rs
./src/cwt/src/main.rs — Rust — parsed clean

  rows 369..=447    the tree rule matched a mod_item
  rows 378..=381    the tree rule matched a function_item
  rows 383..=389    the tree rule matched a function_item
  rows 391..=401    the tree rule matched a function_item
  rows 403..=411    the tree rule matched a function_item
  rows 413..=423    the tree rule matched a function_item
  rows 425..=439    the tree rule matched a function_item
  rows 441..=446    the tree rule matched a function_item

  test         79 rows:  10 blank,  13 comment,  56 code
  production  368 rows:  29 blank, 136 comment, 203 code
```

The whole walk still runs behind `--explain`, because of the cross-file rule
above: the answer is the explanation of the number the table printed, and a
file read on its own would answer a different question.

## The limits

Read this section before you trust a number.

### A test helper outside a test node counts as production code

Tree-sitter reads syntax and resolves no names. It sees `#[test]`, and it
cannot see that

```rust
fn sample_config() -> Config { … }
```

standing in a production module exists only to serve the tests below it. Such a
helper counts as production code, and so does every fixture constant, builder,
and mock that lives outside a marked span.

This is the largest source of error in the split, and it has no fix inside a
parser: knowing that a function is only ever called from a test needs a name
resolver or a call graph, which is a different tool. A helper that should count
as test code can be moved inside the test module, or its file can be named by
`--test-glob`.

### A Rust doc comment holding a fenced example counts as a comment

`cargo test` runs a doctest, so the fenced example under `///` is test code
that really executes. `cdva` counts it as a comment, because every other
counter does, and because a run of `cdva` and a run of `cloc` over the same
tree should report the same total. The doctest is invisible to the split rather
than mis-bucketed: it is a comment row, and comment rows are not code.

### A parse failure counts the whole file as production code

Tree-sitter recovers from a syntax error and still returns a tree, so a parse
that throws nothing proves nothing. `cdva` looks for an `ERROR` node and for a
missing node, and a file that holds either counts entirely as production code —
the safe reading of a tree nobody could read, and a silent one.

So the table names those files under it:

```text
2 files failed to parse and count as production code:
    src/thing/broken.ts
    src/thing/also-broken.ts
```

`--strict` puts the same news in the exit status, for a build that would rather
stop.

**`--no-tree` parses nothing, so no parse can fail and `--strict` under it
always passes.** That is the honest answer to the question the flag asks, and it
is not the answer a build wants. A check that means to catch a broken grammar
must not also ask for the fast mode.

### A scan that ends inside a string spoils the comment count

The line classifier runs one pass of a state machine over the bytes of a file.
A pass that ends inside a string or a block comment did not read that file the
way its language does: valid source almost never ends that way, so the
condition is a row of the language table reading a construct wrong. The
unmodelled regular expression below is one such construct. Every row behind it
carries the wrong label, and the classifier reports them all with numbers that
look like any other numbers.

So the table names those files under it as well:

```text
2 files ended inside a string or a block comment, so their comment and code counts are not to be trusted:
    src/thing/regex.js
    src/thing/verbatim.cs
```

**The two footers are two footers, and the two lists stay apart.** The faults
cost different things. A failed parse moves every row of its file into the
production bucket, which is the split this tool exists to report. A scan that
does not end moves rows between the comment count and the code count, and moves
no row between the buckets — the classifier decides the *kind* of a row, and the
rules decide its *bucket*. One list of both would say that something is wrong
and not what, and the two are fixed in different places.

**`--strict` therefore answers for the parse and not for the scan.** It guards
the split, and this fault does not touch it. A run under `--no-tree` parses
nothing and still scans everything, so this footer holds there while `--strict`
has nothing to fail over. `--json` and `--csv` carry no prose, and the JSON
carries both lists as data, under `failed_parses` and `unterminated_scans`.

A test of this repository asks the same question of every file it holds, under
the language of that file's own path. That is what catches the next row of the
table that reads a construct wrong, without anybody having to think of the
construct in advance.

### A NUL byte reaches the parser as a space

The lexer of a generated parser reads the value 0 as the end of the input,
because 0 is the value it gives a real end of input. A NUL byte inside a
literal is data that no language here objects to, and a grammar that met one
stopped there and called the rest of the file an error.

So the parser reads a copy in which every NUL byte is a space. A space is one
byte, as a NUL byte is, so every row the parser names is still the row of the
file. Nothing else reads the copy: the row classification counts the file as it
is, and a file whose parse fails for any other reason fails as it did.

A NUL byte between two tokens, rather than inside a literal, is a defect this
hides. No compiler of these languages reads such a file either. `cdva` counts
rows, and it does not rule on whether a file builds.

### `#[path = "…"] mod x;` is not resolved

The cross-file pass knows the two conventional spellings of a module file and
nothing else. A declaration that names its file directly marks the two rows of
itself and nothing more, so the file it points at is read on its own terms —
by the path rule and the tree rule, as any other file is.

### Four literal forms are unmodelled

The line classifier holds a string table for each language, and four literal
forms are missing from it. Each one can mis-count a row:

| Form | What happens |
| --- | --- |
| The C++ digit separator, `1'000'000` | The first `'` opens a character literal, which closes at the second. |
| A C++ raw string, `R"delim(…)"` | Read as an ordinary string, so it ends at the row it opened on. A `//` on a later row of it reads as a comment. |
| A C# verbatim string, `@"…"` | Read as an ordinary string, so the doubled `""` that a verbatim string escapes with reads as a close followed by an open. |
| A JavaScript, TypeScript, or TSX regular expression that holds a backtick, `` /`/ `` | The backtick opens a template string, which spans rows. Every row below it reads as code until the next backtick of the file, and each backtick after that flips the state again. |

None of the four moves a row between the production bucket and the test bucket.
The first three move a row between the comment count and the code count, on the
row they appear on. `cloc` gets the raw-string case wrong the same way.

The regular expression is the one that reaches past its own row, and it is the
one the tool reports: a file it runs to the end of ends inside a string, so the
footer above names that file. Telling a regular expression from a division needs
the tokens that stand before the slash, which this classifier does not hold. A
guess would trade one miscount for another, so `cdva` reports the condition
rather than modelling the form.

### Where `cdva` and `cloc` disagree

The totals of the two tools agree over almost every file. Run over the 306 Rust
files of this repository, `cloc` 2.10 and `cdva` differ on eight of them, and
`cdva` is right in all eight. Two classes are known:

**A comment token inside a string literal is not a comment.** `cloc` reads one
as a comment; `cdva` knows it is inside a string, and a string is code. This
class accounts for all eight files. It shows on a row of a multi-row string
that holds nothing else — a `// Initialize xterm.js` inside a block of
JavaScript embedded in a Rust source (`src/beta/src/export/web.rs`), or a
`https://claude.ai/code` on its own row of a multi-row error message
(`src/inscribe/src/main.rs`). A row that also holds real code is code to both
tools, which is why a `let url = "https://…";` agrees. The `/*` and `*/`
spellings do more damage: the glob `"tests/**"` opens a block comment for
`cloc`, and it stays open until a later `"**/"` closes it, so most of
`src/cdva/src/pathrule.rs` reads as comment to `cloc` and as code to `cdva`.

**A nested block comment nests.** `cloc` closes `/* a /* b */ c */` at the first
`*/` and reads `c */` as code. Rust nests a block comment, so those rows are a
comment to the end. No source of this repository holds one, so this class costs
nothing here; it is measured on the sample pinned in
`src/cdva/tests/lines.rs`, where `cloc` reports comment=3 code=7 and `cdva`
reports comment=4 code=6. Those numbers are not to be "fixed" toward `cloc`.

So a total from `cdva` will not always match `cloc` row for row. The invariant
above is internal: it says the two buckets of this tool sum to the unsplit
count of this tool.

## Speed

The default mode does the least work that is still correct:

1. The path rule marks what it can from the name alone. Such a file is never
   opened for a parse.
2. A literal search over the raw bytes (`memchr::memmem`) looks for the needles
   of the language. A Rust file whose bytes hold no `test` never reaches a
   parser.
3. `rayon` parses the survivors across the cores.

Over this repository — 522 files, about 180,000 rows, on 14 cores — the three
modes measure like this, as the median of three runs:

| Mode | Wall | CPU |
| --- | --- | --- |
| default | 0.068 s | 0.50 s |
| `--no-tree` | 0.024 s | 0.06 s |
| `--tree` | 0.070 s | 0.54 s |

`--no-tree` is the path rule alone. It runs in about a third of the wall time
and an eighth of the CPU, and it reports no test code inside a production file.

`--tree` parses every file of a language that has a rule and skips the literal
pre-filter. It costs about what the default costs here, because most files of
this tree hold the word `test` somewhere and reach a parser anyway. It exists
so a test can assert that the two modes agree over the fixture corpus, which is
what proves the needle set complete: a needle the table forgot would show up as
a file the default mode marked as production code and `--tree` marked as test
code.

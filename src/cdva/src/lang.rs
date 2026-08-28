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
});

/// A tree rule that says everything it has to say in its query.
///
/// Most languages are this shape: the thing that makes a node a test — the
/// annotation, or the name — is a *child* of the node, so the query alone
/// reaches it and the two fields below have nothing to add. Only a language
/// that spells its annotation as a sibling wants an attribute chain, and only
/// one that marks an enclosing node wants a scope kind.
const fn plain(grammar: fn() -> tree_sitter::Language, query: &'static str) -> Option<TreeRule> {
    Some(TreeRule {
        grammar,
        query,
        attribute_chain: None,
        scope_kinds: &[],
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

/// The tests of a Zig file.
///
/// Zig is the clean case of the whole table. A test there is a language
/// construct beside `fn` and `struct`, so the grammar names it outright and no
/// heuristic over a name enters.
const ZIG_QUERY: &str = "(test_declaration) @test\n";

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

/// The tree rule of Go.
const GO_TREE: Option<TreeRule> = plain(go_grammar, GO_QUERY);

/// The tree rule of Zig.
const ZIG_TREE: Option<TreeRule> = plain(zig_grammar, ZIG_QUERY);

/// The tree rule of Python.
const PYTHON_TREE: Option<TreeRule> = plain(python_grammar, PYTHON_QUERY);

/// The tree rule of JavaScript.
const JAVASCRIPT_TREE: Option<TreeRule> = plain(javascript_grammar, SCRIPT_QUERY);

/// The tree rule of TypeScript.
const TYPESCRIPT_TREE: Option<TreeRule> = plain(typescript_grammar, SCRIPT_QUERY);

/// The tree rule of TSX.
const TSX_TREE: Option<TreeRule> = plain(tsx_grammar, SCRIPT_QUERY);

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
        line: ["//"], block: [C_BLOCK], nested: false, strings: [TDQ, DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    // Kotlin spells a character literal `'a'` and carries no unpaired quote, so
    // the plain string form on the quote is right and no lookahead is wanted.
    Kotlin => "Kotlin", ["kt", "kts"], [],
        line: ["//"], block: [C_BLOCK], nested: true, strings: [TDQ, DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    CSharp => "C#", ["cs"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [TDQ, DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    Ruby => "Ruby", ["rb", "rake", "gemspec"], ["Gemfile", "Rakefile"],
        line: ["#"], block: [RUBY_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    // Swift has no character literal at all. A character there is a `"` string
    // of one character, which the form below already reads.
    Swift => "Swift", ["swift"], [],
        line: ["//"], block: [C_BLOCK], nested: true, strings: [TDQ, DQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
    // Elixir gets neither rule, because its two spellings want opposite ones: a
    // charlist is `'abc'`, and a character is `?'`. A string form on the quote
    // would read the `?'` of `if c == ?' do` as the opening of a charlist. Its
    // quote therefore stays ordinary code until somebody measures the pair.
    Elixir => "Elixir", ["ex", "exs"], [],
        line: ["#"], block: [], nested: false, strings: [TDQ, DQ_ESC], raw_hash: false, char_lit: false, tree: NO_TREE;
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
}

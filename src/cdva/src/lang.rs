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
/// tree: rule;`.
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
        line: ["//"], block: [C_BLOCK], nested: true, strings: [DQ_ESC_ML], raw_hash: true, tree: RUST_TREE;
    Go => "Go", ["go"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, BACKTICK, SQ_ESC], raw_hash: false, tree: NO_TREE;
    Python => "Python", ["py", "pyi"], [],
        line: ["#"], block: [], nested: false, strings: [TDQ_DOC, TSQ_DOC, DQ_ESC, SQ_ESC], raw_hash: false, tree: NO_TREE;
    JavaScript => "JavaScript", ["js", "jsx", "mjs", "cjs"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC, BACKTICK_ESC], raw_hash: false, tree: NO_TREE;
    TypeScript => "TypeScript", ["ts", "mts", "cts"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC, BACKTICK_ESC], raw_hash: false, tree: NO_TREE;
    Tsx => "TSX", ["tsx"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC, BACKTICK_ESC], raw_hash: false, tree: NO_TREE;
    Java => "Java", ["java"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [TDQ, DQ_ESC, SQ_ESC], raw_hash: false, tree: NO_TREE;
    Kotlin => "Kotlin", ["kt", "kts"], [],
        line: ["//"], block: [C_BLOCK], nested: true, strings: [TDQ, DQ_ESC], raw_hash: false, tree: NO_TREE;
    CSharp => "C#", ["cs"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [TDQ, DQ_ESC, SQ_ESC], raw_hash: false, tree: NO_TREE;
    Ruby => "Ruby", ["rb", "rake", "gemspec"], ["Gemfile", "Rakefile"],
        line: ["#"], block: [RUBY_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, tree: NO_TREE;
    Swift => "Swift", ["swift"], [],
        line: ["//"], block: [C_BLOCK], nested: true, strings: [TDQ, DQ_ESC], raw_hash: false, tree: NO_TREE;
    Elixir => "Elixir", ["ex", "exs"], [],
        line: ["#"], block: [], nested: false, strings: [TDQ, DQ_ESC], raw_hash: false, tree: NO_TREE;
    Zig => "Zig", ["zig"], [],
        line: ["//"], block: [], nested: false, strings: [DQ_ESC], raw_hash: false, tree: NO_TREE;
    C => "C", ["c"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, tree: NO_TREE;
    CHeader => "C/C++ Header", ["h"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, tree: NO_TREE;
    Cpp => "C++", ["cc", "cpp", "cxx"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, tree: NO_TREE;
    CppHeader => "C++ Header", ["hh", "hpp", "hxx"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, tree: NO_TREE;
    Php => "PHP", ["php"], [],
        line: ["//", "#"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, tree: NO_TREE;
    Shell => "Shell", ["sh", "bash", "zsh", "bats"], [],
        line: ["#"], block: [], nested: false, strings: [DQ_ESC, SQ_PLAIN], raw_hash: false, tree: NO_TREE;
    PowerShell => "PowerShell", ["ps1", "psm1", "psd1"], [],
        line: ["#"], block: [POWERSHELL_BLOCK], nested: false, strings: [DQ_ESC, SQ_PLAIN], raw_hash: false, tree: NO_TREE;
    Batch => "Batch", ["bat", "cmd"], [],
        line: ["::", "REM ", "rem "], block: [], nested: false, strings: [DQ_PLAIN], raw_hash: false, tree: NO_TREE;
    Html => "HTML", ["html", "htm"], [],
        line: [], block: [MARKUP_BLOCK], nested: false, strings: [DQ_PLAIN, SQ_PLAIN], raw_hash: false, tree: NO_TREE;
    Xml => "XML", ["xml", "xsd", "xsl"], [],
        line: [], block: [MARKUP_BLOCK], nested: false, strings: [DQ_PLAIN, SQ_PLAIN], raw_hash: false, tree: NO_TREE;
    Css => "CSS", ["css"], [],
        line: [], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, tree: NO_TREE;
    Scss => "SCSS", ["scss", "sass"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, tree: NO_TREE;
    Json => "JSON", ["json"], [],
        line: [], block: [], nested: false, strings: [DQ_ESC], raw_hash: false, tree: NO_TREE;
    Yaml => "YAML", ["yaml", "yml"], [],
        line: ["#"], block: [], nested: false, strings: [DQ_ESC, SQ_PLAIN], raw_hash: false, tree: NO_TREE;
    Toml => "TOML", ["toml"], [],
        line: ["#"], block: [], nested: false, strings: [TDQ, TSQ, DQ_ESC, SQ_PLAIN], raw_hash: false, tree: NO_TREE;
    Ini => "INI", ["ini", "cfg"], [],
        line: ["#", ";"], block: [], nested: false, strings: [DQ_PLAIN, SQ_PLAIN], raw_hash: false, tree: NO_TREE;
    Markdown => "Markdown", ["md", "markdown"], [],
        line: [], block: [], nested: false, strings: [], raw_hash: false, tree: NO_TREE;
    Sql => "SQL", ["sql"], [],
        line: ["--"], block: [C_BLOCK], nested: false, strings: [SQ_ESC, DQ_ESC], raw_hash: false, tree: NO_TREE;
    Makefile => "Makefile", ["mk", "mak"], ["Makefile", "makefile", "GNUmakefile"],
        line: ["#"], block: [], nested: false, strings: [DQ_PLAIN, SQ_PLAIN], raw_hash: false, tree: NO_TREE;
    Dockerfile => "Dockerfile", ["dockerfile"], ["Dockerfile", "Containerfile", "dockerfile", "containerfile"],
        line: ["#"], block: [], nested: false, strings: [DQ_PLAIN, SQ_PLAIN], raw_hash: false, tree: NO_TREE;
    Lua => "Lua", ["lua"], [],
        line: ["--"], block: [LUA_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC, LUA_LONG], raw_hash: false, tree: NO_TREE;
    Scala => "Scala", ["scala", "sc"], [],
        line: ["//"], block: [C_BLOCK], nested: true, strings: [TDQ, DQ_ESC], raw_hash: false, tree: NO_TREE;
    Haskell => "Haskell", ["hs"], [],
        line: ["--"], block: [HASKELL_BLOCK], nested: true, strings: [DQ_ESC], raw_hash: false, tree: NO_TREE;
    Nix => "Nix", ["nix"], [],
        line: ["#"], block: [C_BLOCK], nested: false, strings: [NIX_INDENTED, DQ_ESC], raw_hash: false, tree: NO_TREE;
    Protobuf => "Protocol Buffers", ["proto"], [],
        line: ["//"], block: [C_BLOCK], nested: false, strings: [DQ_ESC, SQ_ESC], raw_hash: false, tree: NO_TREE;
    GraphQL => "GraphQL", ["graphql", "gql"], [],
        line: ["#"], block: [], nested: false, strings: [TDQ_DOC, DQ_ESC], raw_hash: false, tree: NO_TREE;
    Perl => "Perl", ["pl", "pm"], [],
        line: ["#"], block: [PERL_BLOCK], nested: false, strings: [DQ_ESC, SQ_PLAIN], raw_hash: false, tree: NO_TREE;
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

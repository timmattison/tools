//! The line classifier, read through the public API.
//!
//! Every case here is a source and the three numbers it must produce. Two of
//! them were measured against `cloc` 2.x rather than reasoned about, and each
//! says so where it stands.

use cdva::{classify, count, ends_unterminated, Counts, Language, LineIndex, LineKind};
use std::path::{Path, PathBuf};

/// The three numbers of a bucket, which reads better in an assertion than the
/// four lines of a struct literal.
const fn counts(blank: u64, comment: u64, code: u64) -> Counts {
    Counts {
        blank,
        comment,
        code,
    }
}

/// Counts a source, and asserts on the way past that the classifier labelled
/// exactly as many rows as the index holds. Every case below goes through here,
/// so that invariant is asserted everywhere rather than once.
fn tally(source: &str, language: Language) -> Counts {
    let rows = u64::from(LineIndex::new(source).row_count());
    let labels = classify(source, language);
    assert_eq!(
        u64::try_from(labels.len()).expect("a row count fits a u64"),
        rows,
        "the classifier labelled a different number of rows than the index holds"
    );
    let counts = count(source, language);
    assert_eq!(
        counts.total(),
        rows,
        "the counts of a source must sum to its number of rows"
    );
    counts
}

// ---------------------------------------------------------------------------
// The two samples measured against `cloc`.
// ---------------------------------------------------------------------------

/// A Python source holding a docstring, a comment token inside a string, and a
/// trailing comment.
const PYTHON_DOCSTRING: &str = r##"# comment
def f():
    """A docstring.
    Second line."""
    s = "# not a comment"
    return s  # trailing

"##;

#[test]
fn a_python_docstring_counts_as_a_comment() {
    // Measured: `cloc` 2.x reports exactly these three numbers for this file.
    // The docstring is a comment because its opener is the first character of
    // its row that is not white space. `s = "# not a comment"` is code, and so
    // is `return s  # trailing`, because code wins over a comment on a row that
    // holds both.
    assert_eq!(tally(PYTHON_DOCSTRING, Language::Python), counts(1, 3, 3));
}

/// A Rust source holding a raw string, a block comment, and a nested block
/// comment.
const RUST_NESTED_BLOCK: &str = r##"// a line comment
fn main() {
    let s = "// not a comment";
    let r = r#"also /* not */ a comment"#;
    /* block
       comment */
    let x = 1; // trailing comment

    /* nested /* inner */ still comment */
    println!("{s}{r}{x}");
}
"##;

#[test]
fn a_nested_block_comment_in_rust_stays_a_comment_to_the_end() {
    // `cloc` 2.x reports comment=3 and code=7 for this file, because it closes
    // `/* nested /* inner */` at the first `*/` and reads the rest of that row
    // as code. Rust nests a block comment, so `cloc` is wrong there and this
    // tool is right. DO NOT "fix" these numbers toward `cloc`.
    assert_eq!(tally(RUST_NESTED_BLOCK, Language::Rust), counts(1, 4, 6));
}

// ---------------------------------------------------------------------------
// Strings that hold comment tokens.
// ---------------------------------------------------------------------------

#[test]
fn a_comment_token_inside_a_string_is_not_a_comment() {
    assert_eq!(
        tally("const a = \"// not a comment\";\n", Language::JavaScript),
        counts(0, 0, 1)
    );
    assert_eq!(
        tally("const b = '/* not a comment */';\n", Language::JavaScript),
        counts(0, 0, 1)
    );
    assert_eq!(
        tally("s = '# not a comment'\n", Language::Python),
        counts(0, 0, 1)
    );

    // A real comment after the string still loses to the code on its row.
    assert_eq!(
        tally(
            "const c = \"//\"; // a real comment\n",
            Language::JavaScript
        ),
        counts(0, 0, 1)
    );

    // The strong case: a block opener inside a string must not open a block. If
    // it did, the second row here would be a comment.
    assert_eq!(
        tally("const d = \"/*\";\nconst e = 1;\n", Language::JavaScript),
        counts(0, 0, 2)
    );

    // And a string that opens a row is a comment only where the table says so.
    // Python says so; JavaScript does not.
    assert_eq!(
        tally("s = \"\"\"x\"\"\"\n", Language::Python),
        counts(0, 0, 1)
    );
}

// ---------------------------------------------------------------------------
// Rust raw strings.
// ---------------------------------------------------------------------------

#[test]
fn a_rust_raw_string_holds_comment_tokens_and_quotes() {
    let source = r####"let a = r"a /* b // c";
let b = r#"a "quoted" /* still the string"#;
let c = r##"a "# is not the end"##;
let d = 1;
"####;
    assert_eq!(tally(source, Language::Rust), counts(0, 0, 4));
}

#[test]
fn a_plain_r_is_not_a_raw_string() {
    // If the `r` of the binding opened a raw string, it would swallow the
    // comment on the second row and report it as code.
    assert_eq!(
        tally("let r = 1;\n// a comment\n", Language::Rust),
        counts(0, 1, 1)
    );

    // A byte raw string is a `b` and then a raw string, and the raw string
    // still ends on its own row.
    assert_eq!(
        tally("let a = br\"x // y\";\n// a comment\n", Language::Rust),
        counts(0, 1, 1)
    );
}

// ---------------------------------------------------------------------------
// Character literals, and the quotes that only look like one.
// ---------------------------------------------------------------------------

/// The row of `src/krt/src/source.rs` that this whole section exists for.
///
/// `cdva` once read the `"` inside the character literal `'"'` as the opening
/// of a string. A Rust string spans rows, so that phantom string ran to the
/// next quote anywhere in the file: `cdva` reported comment=484 code=619 for
/// that file where `cloc` reported comment=540 code=563, and those 56 rows were
/// the only disagreement between the two tools over the whole of `src/krt`,
/// `src/cwt`, and `src/gsw`.
const RUST_FORBIDDEN_CHARS: &str = r#"const FORBIDDEN: [char; 9] = [':', '/', '\\', '?', '*', '<', '>', '"', '|'];
// this comment must stay a comment
fn f() {}
"#;

#[test]
fn a_rust_character_literal_of_a_quote_does_not_open_a_string() {
    assert_eq!(tally(RUST_FORBIDDEN_CHARS, Language::Rust), counts(0, 1, 2));
    assert!(!ends_unterminated(RUST_FORBIDDEN_CHARS, Language::Rust));
}

/// Every form a Rust character literal comes in.
///
/// The escapes are written through a raw string, so each one reaches the
/// classifier as the two characters a Rust source holds rather than as the one
/// character the compiler of *this* file would make of it.
const RUST_CHAR_LITERALS: &[&str] = &[
    "'a'",
    "'\"'",
    "' '",
    r"'\\'",
    r"'\''",
    r"'\n'",
    r"'\t'",
    r"'\0'",
    r"'\x41'",
    r"'\u{1F600}'",
    "'日'",
    "'é'",
    "'🎉'",
];

#[test]
fn every_rust_character_literal_is_code_and_opens_nothing() {
    for literal in RUST_CHAR_LITERALS {
        // A literal that opened a string would swallow the comment below it,
        // because a Rust string spans rows.
        let source = format!("let c = {literal};\n// a comment\nfn f() {{}}\n");
        assert_eq!(tally(&source, Language::Rust), counts(0, 1, 2), "{literal}");
        assert!(!ends_unterminated(&source, Language::Rust), "{literal}");

        // The same row with nothing after it. A phantom string has no row end
        // to close it here, so this is what the state of the scan reports.
        let alone = format!("let c = {literal};");
        assert!(!ends_unterminated(&alone, Language::Rust), "{literal}");
    }
}

#[test]
fn a_character_literal_of_many_bytes_is_read_whole() {
    // A lookahead that stepped one *byte* rather than one character would not
    // know 日 for a character, and would leave the closing quote of `'日'` for
    // the scanner to read again. That quote then pairs with the comma to make
    // `','`, and the `"` behind it opens the phantom string all over again.
    for literal in ["'日'", "'é'", "'🎉'", r"'\u{1F600}'"] {
        let source = format!(
            "let m = [{literal},'\"'];\n// this comment must stay a comment\nfn f() {{}}\n"
        );
        assert_eq!(tally(&source, Language::Rust), counts(0, 1, 2), "{literal}");
        assert!(!ends_unterminated(&source, Language::Rust), "{literal}");
    }
}

/// Rust rows that hold an unpaired quote, which is a lifetime and not the
/// opening of anything.
///
/// This is why the quote of Rust is read by a lookahead rather than by a string
/// form of the table: a form on the quote would open a string at every row
/// here, which is a worse spelling of the same bug.
const RUST_LIFETIMES: &[&str] = &[
    "let s: &'static str = \"text\";",
    "fn f<'a>(x: &'a str) -> &'a str { x }",
    "struct S<'a, 'b>(&'a str, &'b str);",
    "impl<'a> Trait for S<'a> {}",
    "fn g<'a>(x: &'a str) { println!(\"{x}\"); }",
    "fn h<T: 'static>(x: T) -> Box<dyn Any + 'static> { Box::new(x) }",
];

#[test]
fn a_rust_lifetime_is_code_and_opens_nothing() {
    for row in RUST_LIFETIMES {
        let source = format!("{row}\n// a comment\nfn f() {{}}\n");
        assert_eq!(tally(&source, Language::Rust), counts(0, 1, 2), "{row}");
        assert!(!ends_unterminated(&source, Language::Rust), "{row}");
    }
}

#[test]
fn a_quote_at_the_end_of_a_rust_source_reads_no_further() {
    // A lookahead that read past the end of the source would panic here rather
    // than count. Each of these is a quote with too little behind it to make a
    // character literal of.
    assert_eq!(tally("let c = '", Language::Rust), counts(0, 0, 1));
    assert_eq!(tally("let c = 'a", Language::Rust), counts(0, 0, 1));
    assert_eq!(tally("let c = '\\", Language::Rust), counts(0, 0, 1));
    assert_eq!(tally("let c = '\\n", Language::Rust), counts(0, 0, 1));
    assert_eq!(tally("let c = '日", Language::Rust), counts(0, 0, 1));
    assert_eq!(tally("'", Language::Rust), counts(0, 0, 1));
    assert_eq!(tally("''", Language::Rust), counts(0, 0, 1));

    // A quote that ends its row is a lifetime bound. No character literal
    // spans a row, so the lookahead stops at the row break and the comment
    // below stays a comment.
    assert_eq!(
        tally(
            "fn f<T>() where T: 'a\n// a comment\nfn g() {}\n",
            Language::Rust
        ),
        counts(0, 1, 2)
    );
    assert_eq!(
        tally("let c = '\n// a comment\nfn g() {}\n", Language::Rust),
        counts(0, 1, 2)
    );
}

#[test]
fn a_character_literal_of_a_quote_opens_no_string_in_zig_or_kotlin() {
    // Zig and Kotlin spell a character literal with a quote and carry no
    // unpaired quote, so each takes a plain string form on the quote rather
    // than the lookahead Rust needs. Without one, the `"` of `'"'` opens a
    // string that is still open when the source ends.
    assert!(!ends_unterminated("const q: u8 = '\"';", Language::Zig));
    assert!(!ends_unterminated("val q: Char = '\"'", Language::Kotlin));

    // In Kotlin the phantom string also swallows the block comment behind it,
    // and reports both of its rows as code.
    assert_eq!(
        tally("val q = '\"' /* a\nb */\nval x = 1\n", Language::Kotlin),
        counts(0, 2, 1)
    );

    // The other forms of both languages, which a string form on the quote
    // reads without any lookahead.
    for source in [
        "const a: u8 = 'a';\nconst b: u8 = '\\'';\nconst c: u8 = '\\\\';\n",
        "const d: u8 = '\\n';\n// a comment\nconst e = 1;\n",
    ] {
        assert!(!ends_unterminated(source, Language::Zig), "{source}");
    }
    assert_eq!(
        tally(
            "val a = 'a'\nval b = '\\''\n// a comment\n",
            Language::Kotlin
        ),
        counts(0, 1, 2)
    );
}

#[test]
fn a_haskell_prime_and_a_scala_symbol_stay_code_and_open_nothing() {
    // Haskell spells a character literal `'a'` *and* an identifier `x'`, and
    // Scala a character literal *and* the symbol `'foo`. Neither carries a
    // string form on the quote, and neither may gain one: it would open a
    // string at every primed name and at every symbol.
    assert_eq!(
        tally("let x' = 1\n-- a comment\ny = x'\n", Language::Haskell),
        counts(0, 1, 2)
    );
    assert!(!ends_unterminated("let x' = 1", Language::Haskell));
    assert_eq!(
        tally("val s = 'foo\n// a comment\nval t = 1\n", Language::Scala),
        counts(0, 1, 2)
    );
    assert!(!ends_unterminated("val s = 'foo", Language::Scala));
}

// ---------------------------------------------------------------------------
// The state the scan ended in.
// ---------------------------------------------------------------------------

#[test]
fn ends_unterminated_reads_the_state_the_scan_ended_in() {
    // Well-formed source ends outside every string and every comment.
    for source in [
        "fn f() {}\n",
        "let s = \"text\";\n",
        "let r = r#\"text\"#;\n",
        "/* a comment */\nfn f() {}\n",
        "// a comment\n",
        "",
        "   \n\n",
    ] {
        assert!(!ends_unterminated(source, Language::Rust), "{source:?}");
    }

    // A string, a raw string, and a block comment that never close.
    for source in [
        "let s = \"text;\n",
        "let r = r#\"text;\n",
        "/* a comment\nand more\n",
        "let s = \"text\\\n",
    ] {
        assert!(ends_unterminated(source, Language::Rust), "{source:?}");
    }

    // A string of one row closes at the end of its row, so a language whose
    // string holds no row break never ends unterminated for that reason. The
    // back-quoted string of Go does hold one.
    assert!(!ends_unterminated("s := \"text;\n", Language::Go));
    assert!(ends_unterminated("s := `text;\n", Language::Go));

    // And a docstring that never closes is still a string.
    assert!(ends_unterminated(
        "def f():\n    \"\"\"docs\n",
        Language::Python
    ));
}

// ---------------------------------------------------------------------------
// Every Rust source of this repository.
// ---------------------------------------------------------------------------

/// The root of this repository, found from the manifest directory of this
/// crate.
///
/// `CARGO_MANIFEST_DIR` is `<repository>/src/cdva`, so the root is two levels
/// above it. An absolute path written here would name one checkout, and this
/// repository is worked in worktrees.
fn repository_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .expect("the manifest of this crate lies two levels below the repository root")
        .to_path_buf()
}

/// Every `*.rs` file under a directory, read from the working tree.
///
/// The walk reads and never writes, so any number of copies of this test may
/// run at once. It follows no symbolic link, because the type of a directory
/// entry is the type of the link itself.
fn collect_rust_sources(directory: &Path, skip: &Path, found: &mut Vec<PathBuf>) {
    if directory == skip {
        return;
    }
    let entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("{} reads: {error}", directory.display()));
    for entry in entries {
        let entry = entry.expect("a directory entry reads");
        let path = entry.path();
        let kind = entry.file_type().expect("a directory entry has a type");
        if kind.is_dir() {
            // A build directory holds generated sources, which nobody here
            // committed and which say nothing about the language table.
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            collect_rust_sources(&path, skip, found);
        } else if kind.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn no_rust_source_of_this_repository_ends_unterminated() {
    // This is the guard the character literal of Rust needed. A file that ends
    // inside a string or a block comment is almost never a file somebody wrote
    // that way; it is the language table reading a construct wrong, and every
    // row of that file behind the construct is in the wrong bucket. The
    // classifier reports such a file with numbers that look like any other, so
    // nothing but this question tells the difference.
    let root = repository_root();
    // The one deliberately malformed source of the corpus. It is a fixture of
    // the tree rule, which needs a file that does not parse.
    let skip = root.join("src").join("cdva").join("tests").join("fixtures");

    let mut sources = Vec::new();
    collect_rust_sources(&root.join("src"), &skip, &mut sources);
    assert!(
        sources.len() > 100,
        "the walk found only {} Rust sources under {}, so it is reading the wrong tree and a clean \
         report here would mean nothing",
        sources.len(),
        root.display()
    );

    let mut unterminated = Vec::new();
    for path in &sources {
        let source = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("{} reads: {error}", path.display()));
        if ends_unterminated(&source, Language::Rust) {
            unterminated.push(path.display().to_string());
        }
    }
    unterminated.sort();

    assert!(
        unterminated.is_empty(),
        "these sources end inside a string or a block comment, which means the Rust row of the \
         language table reads one of their constructs wrong:\n{}",
        unterminated.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Strings that span rows.
// ---------------------------------------------------------------------------

#[test]
fn a_multiline_string_spans_its_rows_as_code() {
    // Go, back quotes, no escape.
    assert_eq!(
        tally(
            "s := `line one\n// not a comment\nline three`\n",
            Language::Go
        ),
        counts(0, 0, 3)
    );

    // JavaScript, a template literal.
    assert_eq!(
        tally(
            "const s = `a\n// not a comment\n${x}`;\n",
            Language::JavaScript
        ),
        counts(0, 0, 3)
    );

    // A row holding nothing but white space is blank even inside a string,
    // which is the rule `cloc` uses too.
    assert_eq!(tally("s := `a\n\nb`\n", Language::Go), counts(1, 0, 2));

    // A string of one row does not run past the end of its row.
    assert_eq!(
        tally("s := \"unterminated\n// a comment\n", Language::Go),
        counts(0, 1, 1)
    );
}

// ---------------------------------------------------------------------------
// Block comments, nested and not.
// ---------------------------------------------------------------------------

/// A language that nests a block comment, and a source whose inner `*/` must
/// not close the outer comment.
const NESTING: &[(Language, &str)] = &[
    (Language::Rust, "/* a /* b */\nlet x = 1;\n"),
    (Language::Kotlin, "/* a /* b */\nval x = 1\n"),
    (Language::Swift, "/* a /* b */\nlet x = 1\n"),
    (Language::Scala, "/* a /* b */\nval x = 1\n"),
    (Language::Haskell, "{- a {- b -}\nx = 1\n"),
];

#[test]
fn a_nested_block_comment_needs_one_terminator_for_each_opener() {
    for &(language, source) in NESTING {
        assert_eq!(
            tally(source, language),
            counts(0, 2, 0),
            "{} nests a block comment, so the second row is still inside it",
            language.name()
        );
    }
}

#[test]
fn a_nested_block_comment_closes_once_its_terminators_balance() {
    assert_eq!(
        tally("/* a /* b */ c */\nlet x = 1;\n", Language::Rust),
        counts(0, 1, 1)
    );
    assert_eq!(
        tally("{- a {- b -} c -}\nx = 1\n", Language::Haskell),
        counts(0, 1, 1)
    );
}

#[test]
fn a_block_comment_that_does_not_nest_closes_at_the_first_terminator() {
    // C reads `/* a /* b */` as one comment that ends at the only `*/`, so the
    // second row is code. This is the same source the nesting test uses.
    assert_eq!(
        tally("/* a /* b */\nint x = 1;\n", Language::C),
        counts(0, 1, 1)
    );

    // And on one row the code after the terminator wins.
    assert_eq!(
        tally("/* a /* b */ int x = 1;\n", Language::C),
        counts(0, 0, 1)
    );
}

// ---------------------------------------------------------------------------
// Delimiters that share a prefix.
// ---------------------------------------------------------------------------

#[test]
fn a_lua_long_comment_beats_the_line_comment_that_prefixes_it() {
    // `--[[` is a block opener and `--` is a line comment token. The block
    // group is read first, so the second row is still inside the comment.
    assert_eq!(
        tally(
            "--[[ a comment\nstill comment ]]\nlocal x = 1 -- trailing\n",
            Language::Lua
        ),
        counts(0, 2, 1)
    );

    // A bare `--` is still a line comment.
    assert_eq!(tally("-- just a comment\n", Language::Lua), counts(0, 1, 0));

    // And a Lua long bracket is a string, not a comment.
    assert_eq!(
        tally(
            "local s = [[ -- not a comment\nstill the string ]]\n",
            Language::Lua
        ),
        counts(0, 0, 2)
    );
}

#[test]
fn a_sql_line_comment_is_two_dashes() {
    assert_eq!(
        tally("-- a comment\nSELECT 1; -- trailing\n", Language::Sql),
        counts(0, 1, 1)
    );
    assert_eq!(
        tally("/* block */\nSELECT 'a -- b';\n", Language::Sql),
        counts(0, 1, 1)
    );
}

// ---------------------------------------------------------------------------
// Block comments read only at the start of a row.
// ---------------------------------------------------------------------------

#[test]
fn a_ruby_block_comment_is_read_only_at_the_start_of_a_row() {
    assert_eq!(
        tally("=begin\ncomment\n=end\nputs 1\n", Language::Ruby),
        counts(0, 3, 1)
    );

    // An indented `=begin` is not a comment. Ruby reads the token at column
    // zero and nowhere else, so every row below is code.
    assert_eq!(
        tally("  =begin\nputs 1\n=end\n", Language::Ruby),
        counts(0, 0, 3)
    );

    // Nor is one in the middle of a row.
    assert_eq!(
        tally("puts 1 =begin\nputs 2\n", Language::Ruby),
        counts(0, 0, 2)
    );
}

#[test]
fn perl_pod_is_read_only_at_the_start_of_a_row() {
    assert_eq!(
        tally("=pod\ndocs\n=cut\nprint 1;\n", Language::Perl),
        counts(0, 3, 1)
    );
    assert_eq!(tally("  =pod\nprint 1;\n", Language::Perl), counts(0, 0, 2));
}

// ---------------------------------------------------------------------------
// A wider sweep of the table.
// ---------------------------------------------------------------------------

/// One source per language, and the numbers it must produce.
const LANGUAGE_CASES: &[(Language, &str, Counts)] = &[
    (Language::Json, "{\n  \"a\": 1\n}\n", counts(0, 0, 3)),
    (Language::Yaml, "# c\na: 1\n", counts(0, 1, 1)),
    (
        Language::Toml,
        "# c\na = \"\"\"x\ny\"\"\"\n",
        counts(0, 1, 2),
    ),
    (Language::Ini, "; c\n# d\na=1\n", counts(0, 2, 1)),
    (Language::Shell, "# c\necho 'a # b'\n", counts(0, 1, 1)),
    (
        Language::Php,
        "# c\n// d\n/* e */\n$a = 1;\n",
        counts(0, 3, 1),
    ),
    (
        Language::Nix,
        "# c\ns = ''\n  a # b\n'';\n",
        counts(0, 1, 3),
    ),
    (
        Language::GraphQL,
        "\"\"\"\nDescribes it.\n\"\"\"\ntype Q { a: Int }\n",
        counts(0, 3, 1),
    ),
    (
        Language::Elixir,
        "# c\n@doc \"\"\"\nx\n\"\"\"\n",
        counts(0, 1, 3),
    ),
    (Language::Zig, "// c\nconst x = 1;\n", counts(0, 1, 1)),
    (
        Language::Css,
        "/* c */\na { color: red; }\n",
        counts(0, 1, 1),
    ),
    (Language::Scss, "// c\na { color: red; }\n", counts(0, 1, 1)),
    (
        Language::Makefile,
        "# c\nall:\n\techo hi\n",
        counts(0, 1, 2),
    ),
    (Language::Dockerfile, "# c\nFROM alpine\n", counts(0, 1, 1)),
    (Language::Protobuf, "// c\nmessage M {}\n", counts(0, 1, 1)),
    (
        Language::Batch,
        ":: c\nREM d\nrem e\nECHO hi\n",
        counts(0, 3, 1),
    ),
    (
        Language::Html,
        "<!-- a\nb -->\n<p class=\"x\">hi</p>\n",
        counts(0, 2, 1),
    ),
    (
        Language::Xml,
        "<!-- a\nb -->\n<a x='1'/>\n",
        counts(0, 2, 1),
    ),
    (
        Language::PowerShell,
        "<# a\nb #>\nWrite-Host 'hi' # done\n",
        counts(0, 2, 1),
    ),
    (
        Language::Java,
        "// c\nString s = \"\"\"\nx\"\"\";\n",
        counts(0, 1, 2),
    ),
    (
        Language::CSharp,
        "// c\nvar s = \"a\"; /* b */\n",
        counts(0, 1, 1),
    ),
    (Language::Tsx, "// c\nconst a = <p />;\n", counts(0, 1, 1)),
    (
        Language::TypeScript,
        "/* c */\nconst a: number = 1;\n",
        counts(0, 1, 1),
    ),
    (Language::CHeader, "// c\nint f(void);\n", counts(0, 1, 1)),
    (
        Language::Cpp,
        "/* c */\nint f() { return 1; }\n",
        counts(0, 1, 1),
    ),
    (Language::CppHeader, "// c\nclass A {};\n", counts(0, 1, 1)),
];

#[test]
fn one_source_of_each_language_lands_in_the_right_buckets() {
    for &(language, source, expected) in LANGUAGE_CASES {
        assert_eq!(tally(source, language), expected, "{}", language.name());
    }
}

#[test]
fn markdown_counts_every_row_that_is_not_blank_as_code() {
    // Markdown has no comment syntax at all, so a leading `#` is a heading and
    // not a comment.
    assert_eq!(
        tally("# Title\n\ntext\n", Language::Markdown),
        counts(1, 0, 2)
    );
}

// ---------------------------------------------------------------------------
// LineIndex.
// ---------------------------------------------------------------------------

#[test]
fn row_of_reads_the_row_that_holds_a_byte() {
    let index = LineIndex::new("ab\ncd\nef");
    assert_eq!(index.row_of(0), 0);
    assert_eq!(index.row_of(1), 0);
    // The newline closes the row it ends, so it belongs to that row.
    assert_eq!(index.row_of(2), 0);
    assert_eq!(index.row_of(3), 1);
    assert_eq!(index.row_of(5), 1);
    assert_eq!(index.row_of(6), 2);
    // The last byte of the source.
    assert_eq!(index.row_of(7), 2);
}

#[test]
fn row_of_saturates_at_the_last_row() {
    let index = LineIndex::new("ab\ncd\n");
    assert_eq!(index.row_count(), 2);
    assert_eq!(index.row_of(6), 1);
    assert_eq!(index.row_of(1_000_000), 1);

    // An empty source holds no row, and reports the first one rather than
    // panicking.
    let empty = LineIndex::new("");
    assert_eq!(empty.row_count(), 0);
    assert_eq!(empty.row_of(0), 0);
    assert_eq!(empty.row_of(99), 0);
}

#[test]
fn row_of_reads_a_byte_inside_a_character_of_many_bytes() {
    // Each of 日 and 本 takes three bytes, so the first row spans bytes 0..=6
    // and the newline is byte 6. 語 takes the last three.
    let source = "日本\n語";
    assert_eq!(source.len(), 10);
    let index = LineIndex::new(source);
    assert_eq!(index.row_count(), 2);
    for offset in 0..=6 {
        assert_eq!(index.row_of(offset), 0, "byte {offset} is on the first row");
    }
    for offset in 7..10 {
        assert_eq!(
            index.row_of(offset),
            1,
            "byte {offset} is on the second row"
        );
    }
}

#[test]
fn row_count_does_not_add_a_row_for_a_trailing_newline() {
    assert_eq!(LineIndex::new("").row_count(), 0);
    assert_eq!(LineIndex::new("a").row_count(), 1);
    assert_eq!(LineIndex::new("a\n").row_count(), 1);
    assert_eq!(LineIndex::new("a\nb").row_count(), 2);
    assert_eq!(LineIndex::new("a\nb\n").row_count(), 2);
    assert_eq!(LineIndex::new("\n").row_count(), 1);
    assert_eq!(LineIndex::new("\n\n").row_count(), 2);
}

// ---------------------------------------------------------------------------
// UTF-8, line endings, and the last row.
// ---------------------------------------------------------------------------

#[test]
fn a_source_of_many_byte_characters_classifies_and_never_panics() {
    let source = r#"// 日本語 コメント 🎉
let s = "café 🎉 日本語";
/* 日本語 café 🎉 */

"#;
    assert_eq!(tally(source, Language::Rust), counts(1, 2, 1));

    // The same characters inside a docstring, which the classifier reads to its
    // closing delimiter rather than by byte offset.
    assert_eq!(
        tally(
            "def f():\n    \"\"\"日本語 🎉\n    café\"\"\"\n    return 1\n",
            Language::Python
        ),
        counts(0, 2, 2)
    );

    // A row that is nothing but many-byte characters is code.
    assert_eq!(
        tally("日本語\n🎉\ncafé\n", Language::Markdown),
        counts(0, 0, 3)
    );
}

#[test]
fn crlf_line_endings_classify_as_lf_does() {
    let lf = "// c\nfn f() {}\n\n/* b\n c */\n";
    let crlf = lf.replace('\n', "\r\n");
    assert_eq!(
        classify(lf, Language::Rust),
        classify(&crlf, Language::Rust)
    );
    assert_eq!(count(&crlf, Language::Rust), counts(1, 3, 1));
}

#[test]
fn a_source_with_no_trailing_newline_counts_its_last_row() {
    assert_eq!(tally("let x = 1;", Language::Rust), counts(0, 0, 1));
    assert_eq!(tally("// a comment", Language::Rust), counts(0, 1, 0));
    assert_eq!(tally("   ", Language::Rust), counts(1, 0, 0));
    assert_eq!(tally("", Language::Rust), counts(0, 0, 0));
    assert_eq!(
        tally("fn f() {}\n// a comment", Language::Rust),
        counts(0, 1, 1)
    );
}

#[test]
fn classify_labels_each_row_in_order() {
    assert_eq!(
        classify("// c\nfn f() {}\n\n", Language::Rust),
        vec![LineKind::Comment, LineKind::Code, LineKind::Blank]
    );
    assert!(classify("", Language::Rust).is_empty());
}

// ---------------------------------------------------------------------------
// The table itself.
// ---------------------------------------------------------------------------

#[test]
fn every_language_carries_a_comment_syntax_with_no_empty_delimiter() {
    for &language in Language::all() {
        let syntax = language.comment_syntax();
        for token in syntax.line {
            assert!(
                !token.is_empty(),
                "{} lists an empty line comment token",
                language.name()
            );
        }
        for spec in syntax.block {
            assert!(
                !spec.open.is_empty(),
                "{} lists an empty block comment opener",
                language.name()
            );
            assert!(
                !spec.close.is_empty(),
                "{} lists an empty block comment terminator",
                language.name()
            );
        }
        for spec in syntax.strings {
            assert!(
                !spec.open.is_empty(),
                "{} lists an empty string opener",
                language.name()
            );
            assert!(
                !spec.close.is_empty(),
                "{} lists an empty string terminator",
                language.name()
            );
        }
    }
}

/// Asserts that no delimiter of a group is a prefix of one listed after it,
/// which is the order the scanner depends on. A violation is a silent
/// miscount: the shorter delimiter matches first and the longer one is never
/// reached.
fn assert_no_delimiter_shadows_a_later_one(
    language: Language,
    group: &str,
    delimiters: &[&'static str],
) {
    for (position, earlier) in delimiters.iter().enumerate() {
        for later in delimiters.iter().skip(position + 1) {
            assert!(
                !later.starts_with(*earlier),
                "{}: the {group} `{earlier}` shadows `{later}`, which is listed after it",
                language.name()
            );
        }
    }
}

#[test]
fn no_delimiter_is_a_prefix_of_one_listed_after_it() {
    for &language in Language::all() {
        let syntax = language.comment_syntax();
        assert_no_delimiter_shadows_a_later_one(language, "line comment token", syntax.line);

        let openers: Vec<&'static str> = syntax.block.iter().map(|spec| spec.open).collect();
        assert_no_delimiter_shadows_a_later_one(language, "block comment opener", &openers);

        let openers: Vec<&'static str> = syntax.strings.iter().map(|spec| spec.open).collect();
        assert_no_delimiter_shadows_a_later_one(language, "string opener", &openers);
    }
}

#[test]
fn every_language_classifies_a_source_of_delimiters_without_panicking() {
    // Every delimiter the table holds, on rows a scanner of any language must
    // walk to the end of.
    let source = "a \"b\" 'c' `d` /* e */ // f -- g # h ;i <!-- j --> <# k #>\n\n\tl\n";
    let rows = u64::from(LineIndex::new(source).row_count());
    for &language in Language::all() {
        let labels = classify(source, language);
        assert_eq!(
            u64::try_from(labels.len()).expect("a row count fits a u64"),
            rows,
            "{} labelled the wrong number of rows",
            language.name()
        );
        assert_eq!(count(source, language).total(), rows, "{}", language.name());
    }
}

// ---------------------------------------------------------------------------
// Counts.
// ---------------------------------------------------------------------------

#[test]
fn counts_add_field_by_field() {
    let a = counts(1, 2, 3);
    let b = counts(10, 20, 30);
    assert_eq!(a + b, counts(11, 22, 33));

    let mut sum = a;
    sum += b;
    assert_eq!(sum, a + b);

    assert_eq!(a + Counts::default(), a);
}

#[test]
fn counts_total_is_every_row() {
    assert_eq!(counts(1, 2, 3).total(), 6);
    assert_eq!(Counts::default().total(), 0);
    assert_eq!(Counts::default(), counts(0, 0, 0));
}

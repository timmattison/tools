//! Tests of the command words of one command line.

use wdid::command_words::{command_words, Aliases, CommandWord, Parsed};

/// Runs every row of a table. Each row is a line and the value that
/// `command_words` must give for it, with an empty alias table.
fn check(rows: Vec<(&str, Parsed)>) {
    let aliases = Aliases::new();
    for (line, expected) in rows {
        assert_eq!(command_words(line, &aliases), expected, "line: {line:?}");
    }
}

#[test]
fn a_line_that_the_parser_refuses_gives_unparsed() {
    check(vec![
        (";;", Parsed::Unparsed),
        ("a |", Parsed::Unparsed),
        ("echo $(", Parsed::Unparsed),
    ]);
}

/// Makes the expected value for a line that parses.
fn words(list: Vec<CommandWord>) -> Parsed {
    Parsed::Words(list)
}

/// Makes a command word that the person typed.
fn word<const N: usize>(name: &str, args: [&str; N]) -> CommandWord {
    CommandWord::new(name, args)
}

#[test]
fn the_walk_finds_the_command_words_of_every_list_and_group() {
    check(vec![
        ("prcp a b", words(vec![word("prcp", ["a", "b"])])),
        (
            "cat x | prhash",
            words(vec![word("cat", ["x"]), word("prhash", [])]),
        ),
        ("a && b", words(vec![word("a", []), word("b", [])])),
        ("a || b", words(vec![word("a", []), word("b", [])])),
        ("a ; b", words(vec![word("a", []), word("b", [])])),
        ("a & b", words(vec![word("a", []), word("b", [])])),
        ("(a; b)", words(vec![word("a", []), word("b", [])])),
        ("{ a; b; }", words(vec![word("a", []), word("b", [])])),
        (
            "! a | b && c",
            words(vec![word("a", []), word("b", []), word("c", [])]),
        ),
        (
            "if a; then b; elif c; then d; else e; fi",
            words(vec![
                word("a", []),
                word("b", []),
                word("c", []),
                word("d", []),
                word("e", []),
            ]),
        ),
        (
            "for f in 1 2; do prcp $f; done",
            words(vec![word("prcp", ["$f"])]),
        ),
        (
            "for ((i=0; i<3; i++)); do a; done",
            words(vec![word("a", [])]),
        ),
        (
            "while a; do b; done",
            words(vec![word("a", []), word("b", [])]),
        ),
        (
            "until a; do b; done",
            words(vec![word("a", []), word("b", [])]),
        ),
        (
            "case $x in p) a;; q) b; c;; esac",
            words(vec![word("a", []), word("b", []), word("c", [])]),
        ),
        ("time prcp x", words(vec![word("prcp", ["x"])])),
        ("f() { prcp; }", words(vec![])),
        ("prcp >out.txt 2>&1", words(vec![word("prcp", [])])),
        ("(( 1 + 2 ))", words(vec![])),
    ]);
}

#[test]
fn an_empty_line_and_a_comment_line_give_no_words() {
    check(vec![
        ("", words(vec![])),
        ("   \t ", words(vec![])),
        ("# prcp a b", words(vec![])),
        ("\n\n", words(vec![])),
    ]);
}

#[test]
fn the_walk_finds_the_command_words_inside_substitutions() {
    check(vec![
        (
            "echo $(prcp x)",
            words(vec![word("echo", ["$(prcp x)"]), word("prcp", ["x"])]),
        ),
        (
            "echo `prcp x`",
            words(vec![word("echo", ["`prcp x`"]), word("prcp", ["x"])]),
        ),
        (
            "echo \"$(prcp x)\"",
            words(vec![word("echo", ["\"$(prcp x)\""]), word("prcp", ["x"])]),
        ),
        (
            "echo \"a $(b \"$(c)\")\"",
            words(vec![
                word("echo", ["\"a $(b \"$(c)\")\""]),
                word("b", ["\"$(c)\""]),
                word("c", []),
            ]),
        ),
        (
            "eval $(dirc -paste)",
            words(vec![
                word("eval", ["$(dirc -paste)"]),
                word("dirc", ["-paste"]),
            ]),
        ),
        (
            "for f in $(ls); do prcp $f; done",
            words(vec![word("ls", []), word("prcp", ["$f"])]),
        ),
        (
            "case $(a) in p) b;; esac",
            words(vec![word("a", []), word("b", [])]),
        ),
        ("x=$(cwt)", words(vec![word("cwt", [])])),
        ("FOO=bar prcp", words(vec![word("prcp", [])])),
        (
            "FOO=$(a) b c=$(d)",
            words(vec![word("a", []), word("b", ["c=$(d)"]), word("d", [])]),
        ),
        (
            "diff <(a) <(b)",
            words(vec![word("diff", []), word("a", []), word("b", [])]),
        ),
        ("cat < <(a)", words(vec![word("cat", []), word("a", [])])),
        ("[[ $(a) == x ]]", words(vec![word("a", [])])),
        (
            "[[ -n x && ( $(a) != $(b) ) ]] >$(c)",
            words(vec![word("a", []), word("b", []), word("c", [])]),
        ),
        ("a >$(b)", words(vec![word("a", []), word("b", [])])),
        ("{ a; } >$(b)", words(vec![word("a", []), word("b", [])])),
        (
            "cat <<EOF\n$(prcp)\nEOF",
            words(vec![word("cat", []), word("prcp", [])]),
        ),
        ("cat <<'EOF'\nprcp\nEOF", words(vec![word("cat", [])])),
        ("cat <<EOF\nprcp $x\nEOF", words(vec![word("cat", [])])),
        (
            "cat <<<$(prcp)",
            words(vec![word("cat", []), word("prcp", [])]),
        ),
        ("f() { $(a); }", words(vec![])),
        ("echo $((1 + 2))", words(vec![word("echo", ["$((1 + 2))"])])),
    ]);
}

#[test]
fn a_substitution_that_the_parser_refuses_gives_unparsed() {
    check(vec![
        ("echo \"$(;;)\"", Parsed::Unparsed),
        ("echo $(a |)", Parsed::Unparsed),
        ("echo `a |`", Parsed::Unparsed),
        ("echo $(echo $(;;))", Parsed::Unparsed),
        ("cat <<EOF\n$(a |)\nEOF", Parsed::Unparsed),
    ]);
}

/// Makes a line that nests `levels` command substitutions around `prcp`.
fn nested(levels: usize) -> String {
    format!("{}prcp{}", "echo $(".repeat(levels), ")".repeat(levels))
}

#[test]
fn the_depth_limit_is_64_levels_of_substitutions() {
    let aliases = Aliases::new();
    let Parsed::Words(inside) = command_words(&nested(64), &aliases) else {
        panic!("64 levels must parse");
    };
    assert_eq!(inside.len(), 65);
    assert_eq!(inside.last().unwrap().word, "prcp");
    assert_eq!(command_words(&nested(65), &aliases), Parsed::Unparsed);
}

#[test]
fn quote_removal_applies_to_the_command_word_and_the_arguments() {
    check(vec![
        (
            "git commit -m \"fix prcp\"",
            words(vec![word("git", ["commit", "-m", "fix prcp"])]),
        ),
        ("echo 'prcp'", words(vec![word("echo", ["prcp"])])),
        (
            "echo '$x' \"$y\" $z",
            words(vec![word("echo", ["$x", "\"$y\"", "$z"])]),
        ),
        ("echo '' \"\"", words(vec![word("echo", ["", ""])])),
        ("\"prcp\" x", words(vec![word("prcp", ["x"])])),
        ("'prcp' x", words(vec![word("prcp", ["x"])])),
        ("pr\"c\"'p' x", words(vec![word("prcp", ["x"])])),
        ("\\prcp", words(vec![word("prcp", [])])),
        ("$EDITOR x", words(vec![])),
        ("pr$(x)cp y", words(vec![word("x", [])])),
        ("$(a) x", words(vec![word("a", [])])),
        (
            "~/.cargo/bin/prcp x",
            words(vec![word("~/.cargo/bin/prcp", ["x"])]),
        ),
        ("~tim/bin/prcp", words(vec![word("~tim/bin/prcp", [])])),
        ("~+/prcp", words(vec![])),
        (
            "export FOO=\"a b\" BAR=$(a)",
            words(vec![word("export", ["FOO=a b", "BAR=$(a)"]), word("a", [])]),
        ),
        ("cat <<'EOF'\nprcp\nEOF", words(vec![word("cat", [])])),
    ]);
}

#[test]
fn an_escape_sequence_gives_the_character_after_the_backslash() {
    check(vec![
        ("echo a\\ b", words(vec![word("echo", ["a b"])])),
        ("echo \\$HOME", words(vec![word("echo", ["$HOME"])])),
        ("echo x\\'y", words(vec![word("echo", ["x'y"])])),
        ("echo a\\\\b", words(vec![word("echo", ["a\\b"])])),
        ("echo a\\\nb", words(vec![word("echo", ["ab"])])),
        ("echo \"a\\\"b\"", words(vec![word("echo", ["a\"b"])])),
        ("echo \"a\\\\b\"", words(vec![word("echo", ["a\\b"])])),
        ("echo \"a\\$b\"", words(vec![word("echo", ["a$b"])])),
        ("echo \"a\\nb\"", words(vec![word("echo", ["a\\nb"])])),
        ("echo a\\ b\\ c", words(vec![word("echo", ["a b c"])])),
        ("a\\ b x", words(vec![word("a b", ["x"])])),
    ]);
}

#[test]
fn a_piece_that_is_not_text_keeps_the_raw_text_of_the_argument() {
    check(vec![
        (
            "echo ~ ~tim ~+",
            words(vec![word("echo", ["~", "~tim", "~+"])]),
        ),
        (
            "echo ${x} $'a\\tb'",
            words(vec![word("echo", ["${x}", "$'a\\tb'"])]),
        ),
        (
            "echo a${x}b \"a${x}b\"",
            words(vec![word("echo", ["a${x}b", "\"a${x}b\""])]),
        ),
        ("echo $((1 + 2))", words(vec![word("echo", ["$((1 + 2))"])])),
    ]);
}

#[test]
fn multibyte_text_survives_the_walk_and_the_quote_removal() {
    check(vec![
        (
            "prcp 日本語.txt 🎉/",
            words(vec![word("prcp", ["日本語.txt", "🎉/"])]),
        ),
        (
            "echo \"日本語 prcp\" 'café' 🎉",
            words(vec![word("echo", ["日本語 prcp", "café", "🎉"])]),
        ),
        (
            "echo 日本語$(prcp 🎉)",
            words(vec![
                word("echo", ["日本語$(prcp 🎉)"]),
                word("prcp", ["🎉"]),
            ]),
        ),
        ("日本語 x", words(vec![word("日本語", ["x"])])),
    ]);
}

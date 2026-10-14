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

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

//! Tests of the count rules for one command word.

use wdid::command_words::CommandWord;
use wdid::rules::{runs, Run};
use wdid::tool::{ToolName, ToolNames};

/// The tools of the fixture workspace.
fn tools() -> ToolNames {
    ["prcp", "cwt", "crap"]
        .into_iter()
        .map(ToolName::new)
        .collect()
}

/// Makes the expected `Use` of a tool.
fn used(name: &str) -> Run {
    Run::Use(ToolName::new(name))
}

/// Makes the expected `Development` run of a tool.
fn developed(name: &str) -> Run {
    Run::Development(ToolName::new(name))
}

/// Runs every row of a table. Each row is a command word, its arguments, and
/// the runs that the rules must give.
fn check(rows: Vec<(&str, &[&str], Vec<Run>)>) {
    let tools = tools();
    for (word, args, expected) in rows {
        let command = CommandWord::new(word, args.iter().copied());
        assert_eq!(runs(&command, &tools), expected, "word: {word:?} {args:?}");
    }
}

#[test]
fn a_word_that_names_a_tool_is_a_use() {
    check(vec![
        ("prcp", &["a", "b"], vec![used("prcp")]),
        ("cwt", &[], vec![used("cwt")]),
        ("日本語", &[], vec![]),
        ("ls", &["prcp"], vec![]),
        ("prcp2", &[], vec![]),
        ("PRCP", &[], vec![]),
        ("", &[], vec![]),
    ]);
}

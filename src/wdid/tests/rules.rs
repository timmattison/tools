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

#[test]
fn a_path_to_a_tool_is_a_use_or_a_development_run() {
    check(vec![
        ("~/.cargo/bin/prcp", &[], vec![used("prcp")]),
        ("/usr/local/bin/cwt", &["x"], vec![used("cwt")]),
        ("./prcp", &[], vec![used("prcp")]),
        ("./target/release/prcp", &[], vec![developed("prcp")]),
        ("target/debug/prcp", &[], vec![developed("prcp")]),
        (
            "~/code/tools/target/debug/prcp",
            &["a"],
            vec![developed("prcp")],
        ),
        ("/x/target-old/prcp", &[], vec![used("prcp")]),
        ("/x/mytarget/prcp", &[], vec![used("prcp")]),
        ("/x/target/", &[], vec![]),
        ("prcp/", &[], vec![]),
        ("./target/release/ls", &[], vec![]),
        ("/usr/bin/日本語", &[], vec![]),
        ("/target/prcp", &[], vec![developed("prcp")]),
        ("target/prcp/", &[], vec![]),
    ]);
}

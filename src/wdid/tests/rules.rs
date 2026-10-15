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

/// A path counts only when it names an installed binary or a build. A crate
/// directory has the name of its tool, and zsh changes into a directory that a
/// person types alone (`autocd`). Thus `src/occ` is a `cd`, not a use of `occ`.
#[test]
fn a_path_in_a_bin_directory_is_a_use_and_a_path_under_target_is_a_development_run() {
    check(vec![
        ("~/.cargo/bin/prcp", &[], vec![used("prcp")]),
        ("~tim/.cargo/bin/prcp", &[], vec![used("prcp")]),
        ("/usr/local/bin/cwt", &["x"], vec![used("cwt")]),
        ("bin/prcp", &[], vec![used("prcp")]),
        ("./target/release/prcp", &[], vec![developed("prcp")]),
        ("target/debug/prcp", &[], vec![developed("prcp")]),
        (
            "~/code/tools/target/debug/prcp",
            &["a"],
            vec![developed("prcp")],
        ),
        ("/target/prcp", &[], vec![developed("prcp")]),
        ("/x/target/bin/prcp", &[], vec![developed("prcp")]),
        ("src/prcp", &[], vec![]),
        ("/Volumes/x/tools-worktrees/cwt/src/cwt", &[], vec![]),
        ("./prcp", &[], vec![]),
        ("~/prcp", &[], vec![]),
        ("~/code/tools/cmd/prcp/prcp", &[], vec![]),
        ("/x/target-old/prcp", &[], vec![]),
        ("/x/mytarget/prcp", &[], vec![]),
        ("/x/bins/prcp", &[], vec![]),
        ("/x/target/", &[], vec![]),
        ("/x/bin/", &[], vec![]),
        ("prcp/", &[], vec![]),
        ("./target/release/ls", &[], vec![]),
        ("/usr/bin/日本語", &[], vec![]),
        ("target/prcp/", &[], vec![]),
    ]);
}

#[test]
fn cargo_run_gives_a_development_run_for_each_binary_it_names() {
    check(vec![
        ("cargo", &["run", "--bin", "prcp"], vec![developed("prcp")]),
        ("cargo", &["run", "--bin=prcp"], vec![developed("prcp")]),
        ("cargo", &["run", "-p", "cwt"], vec![developed("cwt")]),
        ("cargo", &["run", "-pcwt"], vec![developed("cwt")]),
        (
            "cargo",
            &["run", "--package", "cwt"],
            vec![developed("cwt")],
        ),
        ("cargo", &["run", "--package=cwt"], vec![developed("cwt")]),
        (
            "cargo",
            &["run", "--release", "--bin", "prcp", "--", "a.txt", "b/"],
            vec![developed("prcp")],
        ),
        ("cargo", &["r", "--bin", "prcp"], vec![developed("prcp")]),
        (
            "cargo",
            &["+nightly", "run", "--bin", "prcp"],
            vec![developed("prcp")],
        ),
        (
            "cargo",
            &["--quiet", "run", "-p", "cwt"],
            vec![developed("cwt")],
        ),
        // After `--` the words belong to the program that cargo runs.
        ("cargo", &["run", "--", "--bin", "prcp"], vec![]),
        (
            "cargo",
            &["run", "--bin", "prcp", "--", "--bin", "cwt"],
            vec![developed("prcp")],
        ),
        // Other subcommands do not run a binary.
        ("cargo", &["build", "--bin", "prcp"], vec![]),
        ("cargo", &["install", "--bin", "prcp"], vec![]),
        ("cargo", &["test", "-p", "cwt"], vec![]),
        ("cargo", &[], vec![]),
        ("cargo", &["run"], vec![]),
        // `--bin` values win over package values.
        (
            "cargo",
            &["run", "-p", "a", "--bin", "prcp"],
            vec![developed("prcp")],
        ),
        (
            "cargo",
            &["run", "--bin", "prcp", "-p", "cwt"],
            vec![developed("prcp")],
        ),
        // A package that is not a tool gives nothing.
        ("cargo", &["run", "-p", "a"], vec![]),
        ("cargo", &["run", "--bin", "ls"], vec![]),
        // Several values, one run for each tool, in order of first appearance.
        (
            "cargo",
            &[
                "run", "--bin", "prcp", "--bin", "ls", "--bin", "cwt", "--bin", "prcp",
            ],
            vec![developed("prcp"), developed("cwt")],
        ),
        (
            "cargo",
            &["run", "-p", "cwt", "--package=prcp", "-pcwt"],
            vec![developed("cwt"), developed("prcp")],
        ),
        // A value that is missing at the end of the arguments gives nothing.
        ("cargo", &["run", "--bin"], vec![]),
        // Only the exact word `cargo` is cargo.
        ("cargo2", &["run", "--bin", "prcp"], vec![]),
        ("/usr/bin/cargo", &["run", "--bin", "prcp"], vec![]),
        ("cargo", &["run", "--bin", "日本語"], vec![]),
    ]);
}

//! The `chf` binary.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::process::ExitCode;

use clap::Parser;

/// The command line of `chf`.
#[derive(Parser)]
#[command(
    name = "chf",
    version = buildinfo::version_string!(),
    about = "Find a Claude Code session by the words in its messages, and resume it",
    long_about = "Find a Claude Code session by the words in its messages, and resume it.\n\n\
Type words to search. Up, Down, Ctrl-P and Ctrl-N move the selection. \
PageUp and PageDown move one page. Shift-Up and Shift-Down scroll the preview. \
Ctrl-U clears the query. Enter resumes the session. Esc and Ctrl-C quit.\n\n\
For a session that Claude Code compacted, chf asks a question. Press c to resume \
with ch (the compacted version). Press e to resume with che (the expanded version)."
)]
struct Cli {}

fn main() -> ExitCode {
    let _cli = Cli::parse();
    ExitCode::SUCCESS
}

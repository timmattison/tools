//! `wn` — what is next.
//!
//! A plan of work is written as a chain: `#277 → #278 ∥ #279 → #280`. The
//! chain says the order, and GitHub says which of them are still open. Holding
//! the two together in your head means opening six tabs and reading six state
//! badges, which is how a chain gets walked out of order.
//!
//! `wn` puts the two together. It reads the chain, asks GitHub about every
//! number in it with one query, prints one row for each with its state and its
//! title, and names the first one that is still open.

mod chain;
mod github;
mod input;
mod render;
mod report;

use std::io::{IsTerminal, Read};
use std::process::ExitCode;

use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;
use termwindow::{effective_terminal_width, should_force_colors};

use crate::chain::parse_chain;
use crate::github::Repo;
use crate::report::Report;

/// The columns `wn` removes from the window on top of the one column
/// [`effective_terminal_width`] always keeps empty. `wn` draws nothing beside
/// its rows, so it removes none.
const WIDTH_OFFSET: usize = 0;

/// The exit status for a chain that names a number the repository does not
/// have. The rows still print, and the answer under them can still be right,
/// but the chain the reader typed does not match the repository.
const EXIT_MISSING_ISSUE: u8 = 1;

/// The exit status for a run that could not answer at all.
const EXIT_ERROR: u8 = 2;

/// The variable that names the command the answer prints.
const START_COMMAND_ENV: &str = "WN_START_COMMAND";

/// The command the answer names when the environment names none.
///
/// This repository ships no `si`. It is a shell function the reader supplies,
/// and it is the default here because it is the name the plans of this
/// repository are written with. [`START_COMMAND_ENV`] names a different one.
const DEFAULT_START_COMMAND: &str = "si";

/// The command that starts work on an issue.
///
/// A newtype rather than a `String`, because the value holds one rule every
/// reader of it depends on: it is never empty. The answer reads
/// `Start #278 next with 'si 278'`, and an empty command turns that into
/// `Start #278 next with ' 278'`, which names nothing at all.
struct StartCommand(String);

impl StartCommand {
    /// The command `value` names.
    ///
    /// `value` is the value of [`START_COMMAND_ENV`], which the caller reads.
    /// The environment is process-global state, and this function takes the
    /// value as an argument so a test of it touches no such state.
    ///
    /// An absent value gives [`DEFAULT_START_COMMAND`], and so does a value
    /// with nothing but whitespace in it: an exported but empty variable is a
    /// common accident, and the default is friendlier than an answer that
    /// names no command. The space around a command is dropped and the words
    /// inside it are kept, so `gh issue develop` goes in whole.
    fn new(value: Option<&str>) -> Self {
        let named = value.map(str::trim).filter(|command| !command.is_empty());
        Self(named.unwrap_or(DEFAULT_START_COMMAND).to_string())
    }

    /// The command, as the answer writes it.
    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "wn",
    version = version_string!(),
    about = "What's next — walks a chain of GitHub issues in order and names the one to start",
    long_about = "Reads a chain of issue numbers, such as \"#277 → #278 ∥ #279 → #280\", asks \
GitHub about every number in it, and names the first one that is still open.\n\n\
Every separator means the same thing: the issue on the left comes before the issue on the right. \
A double bar is read as an arrow, because the chain is a plan to walk in order.\n\n\
Quote the chain. A shell reads an unquoted `#` as the start of a comment.\n\n\
The answer names the command that starts the work: `si 278`. This tool ships no `si` — it is a \
shell function you supply. Set WN_START_COMMAND to name a different one, for example \
`export WN_START_COMMAND='gh issue develop'`."
)]
struct Cli {
    /// The chain, for example "#277 → #278 ∥ #279". Read from standard input
    /// when it is not given.
    #[arg(value_name = "CHAIN")]
    chain: Vec<String>,

    /// The repository to ask, as owner/name. Defaults to the repository of the
    /// current directory.
    #[arg(short = 'R', long, value_name = "OWNER/NAME")]
    repo: Option<String>,

    /// Write no color.
    #[arg(long)]
    no_color: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let stdout_is_tty = std::io::stdout().is_terminal();
    let columns_env: Option<usize> = std::env::var("COLUMNS").ok().and_then(|s| s.parse().ok());
    let no_color_env = std::env::var_os("NO_COLOR").is_some();

    if cli.no_color || no_color_env {
        #[allow(
            clippy::disallowed_methods,
            reason = "this process decides its own color output at startup; the ban covers the tests, which must go through testcolor::with_forced_ansi"
        )]
        colored::control::set_override(false);
    } else if should_force_colors(stdout_is_tty, columns_env.is_some(), no_color_env) {
        #[allow(
            clippy::disallowed_methods,
            reason = "this process decides its own color output at startup; the ban covers the tests, which must go through testcolor::with_forced_ansi"
        )]
        colored::control::set_override(true);
    }

    let width = effective_terminal_width(
        termsize::stdout_columns().map(usize::from),
        columns_env,
        stdout_is_tty,
        WIDTH_OFFSET,
    );

    let start = StartCommand::new(std::env::var(START_COMMAND_ENV).ok().as_deref());
    let clipboard_off =
        input::clipboard_is_off(std::env::var(input::NO_CLIPBOARD_ENV).ok().as_deref());

    match run(&cli, width, &start, clipboard_off) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{} {err:#}", "wn:".red().bold());
            ExitCode::from(EXIT_ERROR)
        }
    }
}

/// Read the chain, ask GitHub, print the answer.
///
/// `clipboard_off` is what [`input::clipboard_is_off`] said about the
/// environment, which `main` reads. The order of the inputs, and the rule that
/// only the input that answers is read, both live in [`input::Sources`].
fn run(cli: &Cli, width: usize, start: &StartCommand, clipboard_off: bool) -> Result<ExitCode> {
    // Each input is a function rather than its text, so an input that a nearer
    // input already answered for is never touched. This matters for the
    // clipboard, which is one shared resource of the whole machine.
    let piped: &dyn Fn() -> std::io::Result<String> = &|| {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        Ok(text)
    };
    let copied: &dyn Fn() -> input::ClipboardRead = &input::system_clipboard;

    let chain = input::Sources {
        argument: &cli.chain,
        // A terminal on standard input is a run with nothing piped into it.
        stdin: (!std::io::stdin().is_terminal()).then_some(piped),
        clipboard: (!clipboard_off).then_some(copied),
    }
    .chain()?;
    let numbers = parse_chain(chain.text()).map_err(|err| chain.blame(err))?;

    let repo = match &cli.repo {
        Some(spec) => Repo::parse(spec)?,
        None => github::current_repo()?,
    };

    let entries = github::fetch(&repo, &numbers)?;
    let report = Report::build(entries);
    println!(
        "{}",
        render::render(&report, &repo.to_string(), width, start)
    );

    Ok(if report.missing().is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_MISSING_ISSUE)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_environment_that_names_no_command_gives_the_default() {
        assert_eq!(StartCommand::new(None).as_str(), "si");
    }

    #[test]
    fn the_named_command_is_the_command() {
        assert_eq!(StartCommand::new(Some("start")).as_str(), "start");
    }

    #[test]
    fn a_command_of_more_than_one_word_keeps_every_word() {
        assert_eq!(
            StartCommand::new(Some("gh issue develop")).as_str(),
            "gh issue develop"
        );
    }

    #[test]
    fn an_empty_command_gives_the_default() {
        assert_eq!(StartCommand::new(Some("")).as_str(), "si");
        assert_eq!(StartCommand::new(Some("   ")).as_str(), "si");
        assert_eq!(StartCommand::new(Some("\t\n")).as_str(), "si");
    }

    #[test]
    fn the_space_around_a_command_is_dropped() {
        assert_eq!(StartCommand::new(Some("  start  ")).as_str(), "start");
    }
}

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
mod render;
mod report;

use std::io::{IsTerminal, Read};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
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

#[derive(Parser, Debug)]
#[command(
    name = "wn",
    version = version_string!(),
    about = "What's next — walks a chain of GitHub issues in order and names the one to start",
    long_about = "Reads a chain of issue numbers, such as \"#277 → #278 ∥ #279 → #280\", asks \
GitHub about every number in it, and names the first one that is still open.\n\n\
Every separator means the same thing: the issue on the left comes before the issue on the right. \
A double bar is read as an arrow, because the chain is a plan to walk in order.\n\n\
Quote the chain. A shell reads an unquoted `#` as the start of a comment."
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

    match run(&cli, width) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{} {err:#}", "wn:".red().bold());
            ExitCode::from(EXIT_ERROR)
        }
    }
}

/// Read the chain, ask GitHub, print the answer.
fn run(cli: &Cli, width: usize) -> Result<ExitCode> {
    let text = if cli.chain.is_empty() {
        read_stdin()?
    } else {
        chain_text(&cli.chain)
    };
    let numbers = parse_chain(&text)?;

    let repo = match &cli.repo {
        Some(spec) => Repo::parse(spec)?,
        None => github::current_repo()?,
    };

    let entries = github::fetch(&repo, &numbers)?;
    let report = Report::build(entries);
    println!("{}", render::render(&report, &repo.to_string(), width));

    Ok(if report.missing().is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_MISSING_ISSUE)
    })
}

/// The chain as one line of text.
///
/// A shell splits an unquoted chain into one argument for each word, and it
/// splits a quoted one into a single argument. Joining with a space gives the
/// same line either way, because the parser reads whitespace as a separator.
fn chain_text(args: &[String]) -> String {
    args.join(" ")
}

/// Read the chain from standard input, for a run that was given none.
fn read_stdin() -> Result<String> {
    if std::io::stdin().is_terminal() {
        bail!("no chain given. Pass it as an argument, in quotes: wn \"#277 → #278\"");
    }
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .context("could not read the chain from standard input")?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arguments_of_an_unquoted_chain_join_back_into_one_line() {
        let args = ["#277".to_string(), "→".to_string(), "#278".to_string()];
        assert_eq!(chain_text(&args), "#277 → #278");
        assert_eq!(chain_text(&["#277 → #278".to_string()]), "#277 → #278");
        assert_eq!(chain_text(&[]), "");
    }
}

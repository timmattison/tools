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

use crate::chain::parse_chain;
use crate::github::Repo;
use crate::report::Report;

/// The columns to lay the output out in when nothing says how wide the window
/// is. The classic terminal, and the same fallback `gsw` takes.
const DEFAULT_WIDTH: usize = 80;

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

    let width = effective_width(
        termsize::stdout_columns().map(usize::from),
        columns_env,
        stdout_is_tty,
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
    let _ = args;
    String::new()
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

/// The columns to lay the output out in.
///
/// A run whose output is a pipe has no window to measure, so a wrapper that
/// draws the output inside its own terminal says how wide that terminal is
/// through `COLUMNS`. A run that writes to a terminal measures the terminal
/// and ignores a stale `COLUMNS` left in the environment. `gsw` resolves the
/// same three inputs the same way.
fn effective_width(
    tty_width: Option<usize>,
    columns_env: Option<usize>,
    stdout_is_tty: bool,
) -> usize {
    let _ = (tty_width, columns_env, stdout_is_tty);
    DEFAULT_WIDTH
}

/// Must the color be forced on?
///
/// The `colored` crate writes no escape codes when standard output is not a
/// terminal, which is right for a pipe into a file and wrong for a wrapper
/// that paints the bytes into its own terminal. A wrapper says it is there by
/// exporting `COLUMNS`. `NO_COLOR` outranks all of it. This is `gsw`'s rule,
/// and the two tools are read side by side under the same wrapper.
fn should_force_colors(stdout_is_tty: bool, columns_env_present: bool, no_color_env: bool) -> bool {
    !stdout_is_tty && columns_env_present && !no_color_env
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

    #[test]
    fn a_terminal_is_measured_and_a_stale_columns_is_ignored() {
        assert_eq!(effective_width(Some(120), Some(40), true), 120);
    }

    #[test]
    fn a_wrapper_that_exports_columns_is_believed_through_a_pipe() {
        assert_eq!(effective_width(None, Some(40), false), 40);
    }

    #[test]
    fn no_signal_at_all_falls_back_to_the_classic_terminal() {
        assert_eq!(effective_width(None, None, false), DEFAULT_WIDTH);
        assert_eq!(effective_width(None, None, true), DEFAULT_WIDTH);
    }

    #[test]
    fn color_is_forced_only_for_a_wrapper_that_did_not_ask_for_none() {
        assert!(should_force_colors(false, true, false));
        assert!(
            !should_force_colors(true, true, false),
            "a terminal needs no forcing"
        );
        assert!(
            !should_force_colors(false, false, false),
            "a plain pipe keeps its plain text"
        );
        assert!(
            !should_force_colors(false, true, true),
            "NO_COLOR outranks the wrapper"
        );
    }
}

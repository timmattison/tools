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
//!
//! A plan of parallel work is a second shape of input. It holds several chains
//! side by side, one for each stream, and `wn` answers the whole page with one
//! query as well. The shape of the text says which reader takes it, so no flag
//! and no subcommand stands between the reader and the answer.
//!
//! A plan drawn as a picture is the third shape. It says the one thing a chain
//! and a table cannot: two streams that join. `wn` follows the wires from left
//! to right and names every issue that is ready, because two streams are two
//! people who work at the same time.
//!
//! A plan says that same thing in words with a `Waits for` column. A cell of
//! it names the work of other streams that comes before the stream, so a plan
//! that names one blocker or more is one graph and answers as a picture
//! answers. An empty cell and an absent column are the common case, and such a
//! plan answers as it always did.
//!
//! A plan written as JSON is the fifth shape, and it is the shape a program
//! hands back. The four written forms carry layout the reader has to undo, and
//! layout is lossy: a table re-wrapped by whatever pasted it can lose the
//! second line of an `Order` cell, which costs a step. A document carries no
//! layout, so `wn` reads it first and claims it on one character.

mod build;
mod chain;
mod github;
mod graph;
mod input;
mod json;
mod plan;
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

/// The exit status for a chain, or for a stream of a plan, that names a number
/// the repository does not have. The rows still print, and the answer under
/// them can still be right, but the text the reader typed does not match the
/// repository.
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
A plan of parallel work is a second shape of input. `wn` reads the plan the plan-parallel-work \
skill writes — the records it prints, the Markdown table it names, and the box-drawn table it \
arrives on the clipboard as — and names the issue to start in every stream of it. Only the Order \
field of a plan is read as a chain, because the Notes field is prose about code and prose about \
code is full of numbers.\n\n\
A pull request and the issue it closes are one step of a stream and not two, written PR#344 \
(#341) or the other way round as #4 (in flight, PR #15). A group in parentheses annotates the \
step to its left. Inside it, only a word carrying the # is a number, a PR in front of one marks \
that number as the work, and every other word is prose. The prose of a group holds a parenthesis \
as well, so #4 (a note (see the docs)) is one group and the parenthesis that closes it is the \
last one.\n\n\
A plan drawn as a picture is a third shape of input. Two streams that join are two people who \
work at the same time, and no chain and no table says that. `wn` follows the wires from left to \
right and names every issue that is ready to start now. A picture drawn from right to left is \
refused, because a guess at the order sends somebody to the wrong issue.\n\n\
A plan says that same thing in words, with a `Waits for` column beside its streams. A cell of it \
names the work of other streams that comes before the first step of that stream. The cell is a \
set and not a chain, so `#96, #91` names two blockers and says nothing about which of the two \
comes first. An empty cell and a plan with no such column are the common case and no error. A \
plan that names one blocker or more is one graph, so its answer is one row for each step, in the \
order of the work, and one start line for each issue somebody can begin now.\n\n\
A plan written as JSON is a fifth shape of input, and it is the shape a program hands back. `wn` \
reads the `streams` of it and nothing else: the order array of a stream is a chain, and the \
waitsFor of a step names the work that comes before that step. JSON is tried first and claimed on \
one character, because a text whose first character that is not a space is `{` is a JSON document \
and nothing else `wn` reads starts that way. A document that does not parse is an error and never \
a walk on to the next reader.\n\n\
Quote the chain. A shell reads an unquoted `#` as the start of a comment.\n\n\
The chain comes out of the first input that holds one: the argument, then standard input, then \
the system clipboard. So `wn` alone answers the chain you just copied, and a pipe still wins, \
because a pipe is explicit. Set WN_NO_CLIPBOARD to any value with a character in it to turn the \
clipboard off, which gives back the error a run with no chain printed before. An empty value \
leaves the clipboard on, because an exported but empty variable is a common accident.\n\n\
The answer names the command that starts the work: `si 278`. This tool ships no `si` — it is a \
shell function you supply. Set WN_START_COMMAND to name a different one, for example \
`export WN_START_COMMAND='gh issue develop'`."
)]
struct Cli {
    /// The chain, for example "#277 → #278 ∥ #279", or a whole plan of
    /// parallel work. Read from standard input when it is not given, and from
    /// the clipboard when neither gives one.
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

/// Read the text, ask GitHub, print the answer.
///
/// `clipboard_off` is what [`input::clipboard_is_off`] said about the
/// environment, which `main` reads. The order of the inputs, and the rule that
/// only the input that answers is read, both live in [`input::Sources`].
///
/// The shape of the text says which reader takes it. A text whose first
/// character that is not a space is `{` is a plan written as JSON, a page that
/// names a `Stream` field or an `Order` field is a plan of parallel work, a
/// text whose wires join steps on more than one line is a plan drawn as a
/// picture, and every other text is one chain. So a reader pipes or pastes
/// what they have, and no flag stands between them and the answer.
///
/// JSON is read first, and it is claimed on one character. Nothing else `wn`
/// reads starts with a brace, so the claim never costs a partial parse. A text
/// that starts with `{` and does not parse is an error and never a walk on to
/// the next reader: a document with one missing brace would otherwise reach
/// the chain reader, which would report `"version" is not an issue number`,
/// and that message names the wrong problem.
///
/// A plan is read twice: once as a set of streams, and once as the graph its
/// `Waits for` cells draw. A plan that draws one cross-stream edge or more
/// answers as a picture answers, because one step of one stream then blocks
/// another stream and a block for each stream says nothing about that. A plan
/// that draws none keeps the reader of streams, and that is every plan a
/// reader wrote before the column stood.
///
/// The picture is read after the plan and before the chain. A box-drawn table
/// of a plan reaches its own reader first, and a chain that holds an arrow on
/// one line reaches the chain reader, because a picture claims a text only
/// when its wires join steps on more than one line: one net that joins two
/// steps and spans the lines, or two box-drawn nets or more that each join two
/// steps and stand on lines of their own. [`graph::read`] states the whole
/// rule.
///
/// The repository is resolved after the text is read, in every path. A text
/// nobody can read is a mistake the reader made, and reporting it costs no
/// call to `gh`. A plan whose streams wait for each other is such a mistake,
/// so that refusal costs no call either.
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
        plan: None,
        refresh: false,
    }
    .chain()?;

    if let Some(document) = json::read(chain.text()) {
        let document = document.map_err(|err| chain.blame(err))?;
        let repo = repo_of(cli)?;
        return answer_graph(document.graph(), &repo, width, start);
    }

    if plan::looks_like_a_plan(chain.text()) {
        let plan = plan::parse(chain.text()).map_err(|err| chain.blame(err))?;
        let graph = graph::of_plan(&plan)
            .transpose()
            .map_err(|err| chain.blame(err))?;
        let repo = repo_of(cli)?;
        return match graph {
            Some(graph) => answer_graph(&graph, &repo, width, start),
            None => answer_plan(&plan, &repo, width, start),
        };
    }

    if let Some(graph) = graph::read(chain.text()) {
        let graph = graph.map_err(|err| chain.blame(err))?;
        let repo = repo_of(cli)?;
        return answer_graph(&graph, &repo, width, start);
    }

    let numbers = parse_chain(chain.text()).map_err(|err| chain.blame(err))?;
    let repo = repo_of(cli)?;

    let entries = github::fetch(&repo, &numbers)?;
    let report = Report::build(entries);
    println!(
        "{}",
        render::render(&report, &repo.to_string(), width, start)
    );

    Ok(exit_status(report.missing().is_empty()))
}

/// The repository the command line names, or the repository of the current
/// directory.
///
/// # Errors
///
/// Fails when the argument is not `owner/name`, and when `gh` can name no
/// repository for the current directory.
fn repo_of(cli: &Cli) -> Result<Repo> {
    match &cli.repo {
        Some(spec) => Repo::parse(spec),
        None => github::current_repo(),
    }
}

/// Ask GitHub about the whole plan, print one block for each stream, and give
/// the status the run exits with.
///
/// One query answers the plan. [`plan::Plan::numbers`] gives every number of
/// every stream once, so a number that stands in two streams costs one alias
/// of the query and is reported in both streams. A query for each stream would
/// spend one round trip and one unit of the rate limit for each of them, and
/// could give two answers for one number.
///
/// # Errors
///
/// Fails for the reasons [`github::fetch`] fails: `gh` is not installed, the
/// repository cannot be read, or GitHub could not answer for one number.
fn answer_plan(
    plan: &plan::Plan,
    repo: &Repo,
    width: usize,
    start: &StartCommand,
) -> Result<ExitCode> {
    let states = report::States::of(github::fetch(repo, &plan.numbers())?);
    let streams: Vec<render::StreamReport> = plan
        .streams()
        .iter()
        .map(|stream| render::StreamReport {
            label: stream.label().to_string(),
            report: Report::of_steps(stream.steps(), &states),
        })
        .collect();
    println!(
        "{}",
        render::render_plan(&streams, &repo.to_string(), width, start)
    );

    Ok(exit_status(
        streams
            .iter()
            .all(|stream| stream.report.missing().is_empty()),
    ))
}

/// Ask GitHub about the whole picture, print the rows, and give the status the
/// run exits with.
///
/// One query answers the picture, as one query answers a chain and a plan.
/// [`graph::Graph::numbers`] gives every number of the picture once, so a step
/// that stands in two places costs one alias of the query and is reported in
/// both of them.
///
/// The answer names every step somebody can start now, and not one of them.
/// Two streams that join are two people who work at the same time, which is
/// the whole reason somebody draws the picture.
///
/// # Errors
///
/// Fails for the reasons [`github::fetch`] fails: `gh` is not installed, the
/// repository cannot be read, or GitHub could not answer for one number.
fn answer_graph(
    graph: &graph::Graph,
    repo: &Repo,
    width: usize,
    start: &StartCommand,
) -> Result<ExitCode> {
    let states = report::States::of(github::fetch(repo, &graph.numbers())?);
    let report = Report::of_graph(graph, &states);
    println!(
        "{}",
        render::render_graph(&report, &repo.to_string(), width, start)
    );

    Ok(exit_status(report.missing().is_empty()))
}

/// The status a run that printed an answer exits with.
///
/// One rule for both shapes of input: a number the repository does not have is
/// text that does not match the repository, whether the reader wrote one chain
/// or a plan of many. The rows still print, and the answer under them can still
/// be right, so the status is what carries the fault to a script.
fn exit_status(every_number_is_known: bool) -> ExitCode {
    if every_number_is_known {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_MISSING_ISSUE)
    }
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

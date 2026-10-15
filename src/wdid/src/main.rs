//! `wdid` — "what do i do": counts which tools of this workspace the atuin
//! history uses.

use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use buildinfo::version_string;
use chrono::NaiveDate;
use clap::Parser;
use wdid::catalog::{self, CatalogError, Programs};
use wdid::history::{self, HistoryError};
use wdid::render;
use wdid::shell::{self, Definitions, Resolver, ShellError, ShellKind};
use wdid::tally::{tally, Window};

/// The one-line description that `--help` shows.
const ABOUT: &str = "What do I do: count which tools of this workspace the atuin history uses";

/// The format of the date that `--since` reads.
const DATE_FORMAT: &str = "%Y-%m-%d";

/// The default number of days in which a tool that never ran is still new.
const DEFAULT_NEW_DAYS: &str = "90";

/// The default number of days after which a tool without a use is idle.
const DEFAULT_IDLE_DAYS: &str = "180";

/// The variable that states the width of the terminal, in columns.
const WIDTH_VARIABLE: &str = "COLUMNS";

/// The variable that names the interactive shell of the person.
const SHELL_VARIABLE: &str = "SHELL";

/// The variable that names the cargo home directory.
const CARGO_HOME_VARIABLE: &str = "CARGO_HOME";

/// The variable that names the home directory.
const HOME_VARIABLE: &str = "HOME";

/// The variable that holds the search path of programs.
const PATH_VARIABLE: &str = "PATH";

/// The name that each message on the error output starts with.
const TOOL: &str = "wdid";

/// The exit code when `wdid` cannot give the report.
const EXIT_ERROR: u8 = 1;

/// The command line of `wdid`.
#[derive(Parser)]
#[command(version = version_string!(), about = ABOUT)]
struct Cli {
    /// The cargo workspace to report on. Without this flag, wdid uses the git
    /// work tree of the current directory.
    #[arg(long, value_name = "PATH")]
    repo: Option<PathBuf>,

    /// Count only the history rows from this date on. A row on this date
    /// counts.
    #[arg(long, value_name = "YYYY-MM-DD", value_parser = parse_date)]
    since: Option<NaiveDate>,

    /// Print the report as one JSON document.
    #[arg(long, conflicts_with = "unparsed")]
    json: bool,

    /// Print the commands that wdid could not parse, the functions that it
    /// could not parse, and the alias lines that it could not parse. Print no
    /// report.
    #[arg(long)]
    unparsed: bool,

    /// Do not start the shell. Aliases and functions then do not count.
    #[arg(long)]
    no_shell: bool,

    /// A tool that never ran is new when it is younger than this many days.
    #[arg(long, value_name = "DAYS", default_value = DEFAULT_NEW_DAYS)]
    new_days: u32,

    /// A tool is idle when its last use is older than this many days.
    #[arg(long, value_name = "DAYS", default_value = DEFAULT_IDLE_DAYS)]
    idle_days: u32,
}

/// Reads a date in the form `YYYY-MM-DD`.
fn parse_date(text: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(text, DATE_FORMAT)
        .map_err(|error| format!("{error} (use {DATE_FORMAT})"))
}

/// The reasons that `wdid` cannot give the report.
#[derive(Debug, thiserror::Error)]
enum Error {
    /// The system does not give the current directory.
    #[error("cannot read the current directory: {0}")]
    CurrentDirectory(#[source] std::io::Error),
    /// The workspace or its catalog cannot be read.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// The history cannot be read.
    #[error(transparent)]
    History(#[from] HistoryError),
    /// The aliases and the functions cannot be read.
    #[error(transparent)]
    Shell(#[from] ShellError),
}

/// Reads the aliases and the functions of the interactive shell.
fn definitions(shell: Option<OsString>) -> Result<Definitions, ShellError> {
    let shell = shell.ok_or(ShellError::NotSet)?;
    let kind = ShellKind::of(Path::new(&shell)).ok_or_else(|| ShellError::NotSupported {
        shell: shell.to_string_lossy().into_owned(),
    })?;
    shell::read(Command::new(&shell), kind, shell::TIMEOUT)
}

/// Runs the whole report and gives the text for standard output.
fn run(cli: &Cli) -> Result<String, Error> {
    let today = chrono::Utc::now().date_naive();
    let cwd = std::env::current_dir().map_err(Error::CurrentDirectory)?;
    let programs = Programs::default();
    let root = catalog::workspace_root(cli.repo.as_deref(), &cwd, &programs)?;
    let search_path = std::env::var_os(PATH_VARIABLE);
    let home = std::env::var_os(HOME_VARIABLE).map(PathBuf::from);
    let cargo_bin = catalog::cargo_bin(
        std::env::var_os(CARGO_HOME_VARIABLE).as_deref(),
        home.as_deref(),
    );
    let catalog = catalog::read(
        &root,
        &programs,
        search_path.as_deref().map(OsStr::new),
        cargo_bin.as_deref(),
    )?;
    let entries = history::read(OsStr::new(history::ATUIN))?;
    let definitions = if cli.no_shell {
        Definitions::default()
    } else {
        definitions(std::env::var_os(SHELL_VARIABLE))?
    };
    let resolver = Resolver::new(&definitions, &catalog.names());
    let window = Window {
        today,
        since: cli.since,
        new_days: cli.new_days,
        idle_days: cli.idle_days,
    };
    let tally = tally(&entries, &definitions, &resolver, &catalog, &window);
    Ok(if cli.json {
        render::json(&tally, &window)
    } else if cli.unparsed {
        render::unparsed(&tally)
    } else {
        let width = render::table_width(
            std::env::var(WIDTH_VARIABLE).ok().as_deref(),
            termsize::controlling_columns(),
        );
        render::text(&tally, &window, width)
    })
}

/// Writes the output to standard output.
///
/// A reader that closed the pipe (`wdid | head`, `wdid | true`) took what it
/// wanted, so a broken pipe is a success. `print!` panics on it instead.
fn write_output(output: &str) -> ExitCode {
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(output.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{TOOL}: cannot write the output: {error}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(output) => write_output(&output),
        Err(error) => {
            eprintln!("{TOOL}: {error}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

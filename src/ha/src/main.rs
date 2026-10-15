//! The `ha` binary.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use chrono::{Local, Utc};
use clap::{CommandFactory, FromArgMatches, Parser};
use ha::app::{App, TerminalEvents};
use ha::board::Board;
use ha::browser::BrowserOpener;
use ha::github::GhSource;
use ha::settings::{self, CommandLine, Settings};
use ha::status::{RateLimit, Status};
use ha::worker::{self, Worker};

/// The exit status of a run that `ha` refuses because of its settings.
const EXIT_USAGE: u8 = 2;

/// The exit status of a run that fails after the settings are good.
const EXIT_FAILURE: u8 = 1;

/// The command line of `ha`.
#[derive(Parser)]
#[command(
    name = "ha",
    version = buildinfo::version_string!(),
    about = "Hot action: a self-refreshing terminal view of GitHub Actions runs across repositories and owners"
)]
struct Cli {
    /// Repositories (owner/name) and owners (owner) to watch.
    ///
    /// Targets here replace all repos and owners of the config file.
    #[arg(value_name = "TARGET")]
    targets: Vec<String>,

    /// The config file to read. A missing file is an error.
    ///
    /// The default is ha/config.toml in the configuration directory.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// How far back finished runs show: a number and a unit (s, m, h, d).
    ///
    /// The default is 8h. The config key is `window`.
    #[arg(long, value_name = "DURATION", value_parser = settings::parse_duration)]
    window: Option<Duration>,

    /// How long to wait between two refreshes: a number and a unit (s, m, h, d).
    ///
    /// The default is 60s. The config key is `interval`.
    #[arg(long, value_name = "DURATION", value_parser = settings::parse_duration)]
    interval: Option<Duration>,
}

impl From<Cli> for CommandLine {
    fn from(cli: Cli) -> Self {
        CommandLine {
            targets: cli.targets,
            config: cli.config,
            window: cli.window,
            interval: cli.interval,
        }
    }
}

/// Restores the terminal when it drops, so every way out of `run` leaves
/// the terminal as it was.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

fn main() -> ExitCode {
    let command = Cli::command().after_help(settings::help_after(
        settings::default_config_path().as_deref(),
    ));
    let matches = command.get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };
    let settings = match settings::load(&cli.into()) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("ha: {error}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    if !std::io::stdout().is_terminal() {
        eprintln!(
            "ha: standard output is not a terminal. ha draws a full-screen view and has no plain-text mode."
        );
        return ExitCode::from(EXIT_USAGE);
    }
    let source = GhSource::new();
    let rate = match source.preflight() {
        Ok(rate) => rate,
        Err(error) => {
            eprintln!("ha: {error}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    // The worker thread is not joined: a `gh` child can run for 30 seconds.
    let worker = worker::spawn(source, &settings);
    match run_view(&settings, rate, &worker) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ha: {error}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

/// Draws the full-screen view until the user quits. The terminal is back
/// in its normal state when this function returns.
fn run_view(settings: &Settings, rate: RateLimit, worker: &Worker) -> std::io::Result<()> {
    // `ratatui::init` also installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    let _guard = TerminalGuard;
    let status = Status {
        rate: Some(rate),
        ..Status::default()
    };
    let tz = Local;
    let mut app = App::new(
        Board::new(settings.window),
        status,
        &BrowserOpener::from_environment(),
        worker,
        &Utc::now,
        &tz,
    );
    app.run(&mut terminal, &mut TerminalEvents)
}

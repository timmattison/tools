//! The `ha` binary.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{CommandFactory, FromArgMatches, Parser};
use ha::settings::{self, CommandLine};

/// The exit status of a run that `ha` refuses because of its settings.
const EXIT_USAGE: u8 = 2;

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

fn main() -> ExitCode {
    let command = Cli::command().after_help(settings::help_after(
        settings::default_config_path().as_deref(),
    ));
    let matches = command.get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };
    match settings::load(&cli.into()) {
        // Slice 8 adds the terminal check and the full-screen view here.
        Ok(_settings) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ha: {error}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

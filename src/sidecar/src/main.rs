#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
//! The `sidecar` command line.
//!
//! The subcommands are declared here. Each one reports that it is not implemented yet.

use std::process::ExitCode;

use buildinfo::version_string;
use clap::{Parser, Subcommand};
use sidecar::host::{Host, RegistryPolicy};
use sidecar::locate::locate;
use sidecar::setup::{ignore_refusal, is_ci, run_setup, CI_VARIABLE};
use sidecar::status::{render, rows, Row};

/// The command line of `sidecar`.
#[derive(Debug, Parser)]
#[command(
    name = "sidecar",
    version = version_string!(),
    about = "Make, clone, and link the private data repositories (sidecars) of a host repository",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The subcommands of `sidecar`.
#[derive(Debug, Subcommand)]
enum Command {
    /// Clone each sidecar of `sidecar.toml` and link it into the worktree
    Setup {
        /// Run as a git hook: never fail, and print warnings only
        #[arg(long)]
        hook: bool,
        /// The arguments that git gives to a `post-checkout` hook
        #[arg(requires = "hook", num_args = 0..=3)]
        hook_args: Vec<String>,
    },
    /// Make a new sidecar repository and register it in `sidecar.toml`
    Add {
        /// The name of the sidecar
        name: String,
        /// Do not ask for confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Show the state of each sidecar clone and link
    Status,
    /// Print the path of a sidecar
    Path {
        /// The name of the sidecar
        name: String,
    },
    /// Make a worktree of a sidecar for a branch
    Worktree {
        /// The name of the sidecar
        name: String,
        /// The branch of the new worktree
        branch: Option<String>,
    },
}

impl Command {
    /// The word that the user typed for this subcommand.
    fn word(&self) -> &'static str {
        match self {
            Command::Setup { .. } => "setup",
            Command::Add { .. } => "add",
            Command::Status => "status",
            Command::Path { .. } => "path",
            Command::Worktree { .. } => "worktree",
        }
    }
}

/// Runs the subcommand that the user chose.
fn run(cli: &Cli) -> anyhow::Result<ExitCode> {
    match &cli.command {
        Command::Setup { hook: false, .. } => run_setup_command(),
        Command::Setup {
            hook: true,
            hook_args,
        } => Ok(run_setup_hook(hook_args)),
        Command::Status => run_status_command(),
        Command::Path { name } => run_path_command(name),
        other => anyhow::bail!("{} is not implemented yet", other.word()),
    }
}

/// Runs `sidecar setup` without `--hook`. Actions go to standard output and warnings
/// to standard error. The exit code is 1 when a sidecar stays unlinked.
fn run_setup_command() -> anyhow::Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let host = Host::discover(&cwd, RegistryPolicy::Required)?;
    if let Some(refusal) = ignore_refusal(&host) {
        anyhow::bail!("{refusal}");
    }
    let report = run_setup(&host, None);
    for action in &report.actions {
        println!("{action}");
    }
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(if report.unlinked.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Runs `sidecar status`. The table goes to standard output. The exit code is 1 when a
/// row is not ready.
fn run_status_command() -> anyhow::Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let host = Host::discover(&cwd, RegistryPolicy::Required)?;
    let table = rows(&host);
    if table.is_empty() {
        println!("sidecar.toml names no sidecar.");
        return Ok(ExitCode::SUCCESS);
    }
    print!("{}", render(&table));
    Ok(if table.iter().all(Row::is_ready) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Runs `sidecar path <name>`. The path of the link goes to standard output. A sidecar
/// with no good link is an error.
fn run_path_command(name: &str) -> anyhow::Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let host = Host::discover(&cwd, RegistryPolicy::DefaultWhenMissing)?;
    let path = locate(&host, name)?;
    println!("{}", path.display());
    Ok(ExitCode::SUCCESS)
}

/// The third argument of a `post-checkout` hook when the checkout changed only a file.
const FILE_CHECKOUT_FLAG: &str = "0";

/// Runs `sidecar setup --hook`. The exit code is always 0. A checkout of a file and a
/// run in CI do nothing. Each error and each refusal becomes a warning.
fn run_setup_hook(hook_args: &[String]) -> ExitCode {
    let file_checkout = hook_args.len() == 3 && hook_args[2] == FILE_CHECKOUT_FLAG;
    let in_ci = is_ci(std::env::var(CI_VARIABLE).ok().as_deref());
    if file_checkout || in_ci {
        return ExitCode::SUCCESS;
    }
    if let Err(error) = run_setup_command() {
        eprintln!("warning: {error:#}");
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("sidecar: {error:#}");
            ExitCode::from(1)
        }
    }
}

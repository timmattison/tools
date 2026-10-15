//! `drun` runs a Docker container on the least busy of your Docker hosts, and
//! finds the container again later.
//!
//! A host is a Docker context. `drun run` asks every host how busy it is,
//! chooses the least busy host, and hands the arguments to
//! `docker --context <host> run`. The other commands find the host that has a
//! container, and hand their arguments to the docker command of that host.

#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod config;
mod docker_args;
mod fleet;
mod host;
mod survey;
mod table;

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::thread;

use buildinfo::version_string;

use crate::config::{Config, Environment, CONTEXTS_VARIABLE};
use crate::docker_args::RunArgs;
use crate::fleet::{Inventory, Scope, MANAGED_LABEL};
use crate::host::ContextName;
use crate::survey::{Survey, PROBE_IMAGE};

/// The exit status of a usage error.
const EXIT_USAGE: u8 = 2;

/// The exit status when `drun` stops before the docker command can run.
/// `docker run` gives the same status when it stops before the container
/// starts, so a script that reads the status of `docker run` can read the
/// status of `drun run` the same way.
const EXIT_NOT_STARTED: u8 = 125;

/// The commands of `drun`, for `drun <command> --help`.
const COMMANDS: &[&str] = &["run", "hosts", "pick"];

/// Why a command of `drun` stopped before it could hand over to docker.
#[derive(Debug)]
enum Failure {
    /// The arguments are wrong.
    Usage(String),
    /// The arguments are correct, but `drun` cannot continue.
    NotStarted(String),
}

impl Failure {
    /// Prints the failure to stderr and gives its exit status.
    fn report(self) -> ExitCode {
        match self {
            Self::Usage(message) => {
                eprintln!("drun: {message}");
                eprintln!("Run drun --help to see how to use drun.");
                ExitCode::from(EXIT_USAGE)
            }
            Self::NotStarted(message) => {
                eprintln!("drun: {message}");
                ExitCode::from(EXIT_NOT_STARTED)
            }
        }
    }
}

fn main() -> ExitCode {
    let environment = Environment::from_process();
    let mut args = std::env::args_os().skip(1);
    let Some(command) = args.next() else {
        eprint!("{}", usage(&environment));
        return ExitCode::from(EXIT_USAGE);
    };
    let rest: Vec<OsString> = args.collect();
    match dispatch(&environment, &command, &rest) {
        Ok(code) => code,
        Err(failure) => failure.report(),
    }
}

/// Runs the `drun` command `command` with the arguments `args`.
fn dispatch(
    environment: &Environment,
    command: &OsString,
    args: &[OsString],
) -> Result<ExitCode, Failure> {
    let name = command.to_str().unwrap_or_default();
    let wants_help = args.first().is_some_and(|first| first == "--help");
    if matches!(name, "-h" | "--help" | "help") || (wants_help && COMMANDS.contains(&name)) {
        print!("{}", usage(environment));
        return Ok(ExitCode::SUCCESS);
    }
    match name {
        "-V" | "--version" => {
            println!("drun {}", version_string!());
            Ok(ExitCode::SUCCESS)
        }
        "run" => run(environment, args),
        "hosts" => {
            no_arguments("hosts", args)?;
            hosts(environment)
        }
        "pick" => {
            no_arguments("pick", args)?;
            pick(environment)
        }
        _ => Err(Failure::Usage(format!(
            "\"{}\" is not a drun command",
            command.to_string_lossy()
        ))),
    }
}

/// `drun run`: runs a container on the least busy host.
fn run(environment: &Environment, args: &[OsString]) -> Result<ExitCode, Failure> {
    let parsed = docker_args::parse_run(args).map_err(|error| Failure::Usage(error.to_string()))?;
    let config = load(environment)?;
    let (survey, inventory) = thread::scope(|scope| {
        let survey = scope.spawn(|| survey::survey(&config));
        let inventory = fleet::inventory(&config, Scope::All);
        let survey = survey
            .join()
            .unwrap_or_else(|payload| std::panic::resume_unwind(payload));
        (survey, inventory)
    });
    let Some((host, load)) = survey.chosen() else {
        return Err(no_host_answered(&survey));
    };
    report_skipped(&survey);
    report_unchecked(&inventory, &survey);
    let name = match &parsed.name {
        Some(name) => {
            refuse_taken(&inventory, name)?;
            name.clone()
        }
        None => inventory.fresh_name().ok_or_else(|| {
            Failure::NotStarted("cannot find a container name that no host uses".to_owned())
        })?,
    };
    warn_about(&parsed, host);

    let mut command = host.docker();
    command.arg("run").arg("--label").arg(MANAGED_LABEL);
    if parsed.name.is_none() {
        command.arg("--name").arg(&name);
    }
    command.args(args);
    eprintln!("drun: {name} runs on {host} ({})", load.summary());
    if parsed.detach {
        start_detached(command, host, &name)
    } else {
        Ok(hand_over(command))
    }
}

/// `drun hosts`: shows how busy each host is.
fn hosts(environment: &Environment) -> Result<ExitCode, Failure> {
    let config = load(environment)?;
    let survey = survey::survey(&config);
    print!("{}", survey.table());
    if survey.chosen().is_some() {
        Ok(ExitCode::SUCCESS)
    } else {
        Err(Failure::NotStarted("no host answered".to_owned()))
    }
}

/// `drun pick`: prints the name of the least busy host.
fn pick(environment: &Environment) -> Result<ExitCode, Failure> {
    let config = load(environment)?;
    let survey = survey::survey(&config);
    let Some((host, _)) = survey.chosen() else {
        return Err(no_host_answered(&survey));
    };
    report_skipped(&survey);
    println!("{host}");
    Ok(ExitCode::SUCCESS)
}

/// Reads the configuration.
fn load(environment: &Environment) -> Result<Config, Failure> {
    Config::load(environment).map_err(|error| Failure::NotStarted(error.to_string()))
}

/// Refuses arguments for a command that takes none.
fn no_arguments(command: &str, args: &[OsString]) -> Result<(), Failure> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(Failure::Usage(format!("drun {command} takes no arguments")))
    }
}

/// The failure when no host answered the probe, with the reason of each host.
fn no_host_answered(survey: &Survey) -> Failure {
    let reasons: Vec<String> = survey
        .failures()
        .map(|(context, failure)| format!("\n  {context}: {failure}"))
        .collect();
    Failure::NotStarted(format!("no host answered:{}", reasons.concat()))
}

/// Prints a warning for each host that did not answer the probe.
fn report_skipped(survey: &Survey) {
    for (context, failure) in survey.failures() {
        eprintln!("drun: skipped {context}: {failure}");
    }
}

/// Prints a warning for each host that answered the probe but did not list
/// its containers, so `drun` cannot know if it uses a name. A host that did
/// not answer the probe has a warning already.
fn report_unchecked(inventory: &Inventory, survey: &Survey) {
    for (context, failure) in inventory.failures() {
        if survey.failures().all(|(skipped, _)| skipped != context) {
            eprintln!("drun: warning: cannot read the container names of {context}: {failure}");
        }
    }
}

/// Refuses `name` when a host has a container with that name. Docker makes a
/// name unique on one host only, and `drun` finds a container by its name on
/// all hosts.
fn refuse_taken(inventory: &Inventory, name: &str) -> Result<(), Failure> {
    let holders = inventory.hosts_with_name(name);
    if holders.is_empty() {
        return Ok(());
    }
    let holders: Vec<&str> = holders.into_iter().map(ContextName::as_str).collect();
    Err(Failure::NotStarted(format!(
        "a container named {name} is on {} already. Give another name, or remove that container first",
        holders.join(", ")
    )))
}

/// Prints the warnings for arguments that do not do on a remote host what
/// they do on this computer.
fn warn_about(parsed: &RunArgs, host: &ContextName) {
    if parsed.detach && parsed.remove {
        eprintln!(
            "drun: warning: with -d, --rm removes the container when it stops. After that, \
             drun status, drun logs, and drun wait find nothing. Remove the container \
             yourself after you have the result"
        );
    }
    for source in &parsed.bind_sources {
        if Path::new(source).exists() {
            eprintln!(
                "drun: warning: the bind mount {source} is a path on this computer, but the \
                 container runs on {host}, so Docker mounts {source} from {host}. To move \
                 files, use a named volume and docker cp"
            );
        }
    }
}

/// Runs a detached `docker run` and prints `<host> <name>` when it starts the
/// container. The ID that docker prints does not show, so stdout holds the
/// two words alone.
fn start_detached(
    mut command: Command,
    host: &ContextName,
    name: &str,
) -> Result<ExitCode, Failure> {
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| Failure::NotStarted(format!("cannot start docker: {error}")))?;
    if output.status.success() {
        println!("{host} {name}");
    }
    Ok(exit_code(output.status))
}

/// Replaces `drun` with `command`, so the terminal, the signals, and the exit
/// status go to and from docker directly. It returns only when docker cannot
/// start.
#[cfg(unix)]
fn hand_over(mut command: Command) -> ExitCode {
    use std::os::unix::process::CommandExt;
    let error = command.exec();
    eprintln!("drun: cannot start docker: {error}");
    ExitCode::from(EXIT_NOT_STARTED)
}

/// Runs `command` with the terminal of `drun`, and exits with its exit
/// status. A process cannot replace itself on this platform.
#[cfg(not(unix))]
fn hand_over(mut command: Command) -> ExitCode {
    match command.status() {
        Ok(status) => exit_code(status),
        Err(error) => {
            eprintln!("drun: cannot start docker: {error}");
            ExitCode::from(EXIT_NOT_STARTED)
        }
    }
}

/// The exit status that `drun` gives for a docker that stopped with
/// `status`. A docker that a signal stopped gives 128 plus the signal number,
/// as a shell does.
fn exit_code(status: ExitStatus) -> ExitCode {
    if let Some(code) = status.code() {
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return ExitCode::from(u8::try_from(128 + signal).unwrap_or(1));
        }
    }
    ExitCode::FAILURE
}

/// The help text.
fn usage(environment: &Environment) -> String {
    let config_path = environment.config_path().map_or_else(
        || "~/.config/drun/config.toml (HOME is not set)".to_owned(),
        |path| path.display().to_string(),
    );
    format!(
        "\
drun runs a Docker container on the least busy of your Docker hosts.

Usage:
  drun run [docker run options] IMAGE [COMMAND [ARG...]]
  drun hosts
  drun pick
  drun --help
  drun --version

Commands:
  run    Run a container on the least busy host. drun gives the arguments to
         docker run without a change. Without -d, drun gives the terminal to
         the container and exits with its exit status. With -d, drun prints
         \"<host> <name>\" when the container starts.
  hosts  Show how busy each host is, and which host drun chooses.
  pick   Print the name of the host that drun chooses, for example for
         docker --context \"$(drun pick)\" compose up -d

Names:
  drun gives each container a name that no host uses, unless you give a name
  with --name. drun refuses a --name that a host uses. Each container gets the
  label {MANAGED_LABEL}.

Hosts:
  A host is a Docker context. drun uses the contexts in {CONTEXTS_VARIABLE},
  separated by commas. When {CONTEXTS_VARIABLE} is not set, drun uses the
  contexts in {config_path}:

      contexts = [\"box-a\", \"box-b\"]
      host_timeout_secs = 10   # how long to wait for each host (optional)

  To make a context for a host, run:

      docker context create box-a --docker \"host=ssh://you@box-a\"

How drun chooses a host:
  drun starts a small {PROBE_IMAGE} container on each host, at the same time.
  The container reads the memory, the load average, and the CPU count of the
  host. A host is as busy as the larger of two fractions: the memory in use,
  and the one-minute load average for each CPU. drun chooses the least busy
  host.

Exit status:
  The exit status of docker, or of the container in the foreground. 2 when
  the arguments are wrong. 125 when drun stops before docker can run.
"
    )
}

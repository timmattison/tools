//! `drun` runs a Docker container on the least busy of your Docker hosts, and
//! finds the container again later.
//!
//! A host is a Docker context. `drun run` asks every host how busy it is,
//! chooses the least busy host, and hands the arguments to
//! `docker --context <host> run`. The other commands find the host that has a
//! container, and hand their arguments to the docker command of that host.
//! `drun` gives a command that it does not know to the docker of this
//! computer without a change, so a script can run `drun` in place of
//! `docker`.

#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod config;
mod docker_args;
mod flag_check;
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
use crate::fleet::{Inventory, Scope, MANAGED_LABEL, STATE_FORMAT};
use crate::host::ContextName;
use crate::survey::{Survey, PROBE_IMAGE};

/// The exit status of a usage error.
const EXIT_USAGE: u8 = 2;

/// The exit status when `drun` stops before the docker command can run.
/// `docker run` gives the same status when it stops before the container
/// starts, so a script that reads the status of `docker run` can read the
/// status of `drun run` the same way.
const EXIT_NOT_STARTED: u8 = 125;

/// The exit status when `drun check-flags` finds a difference.
const EXIT_DIFFERENCES: u8 = 1;

/// The commands of `drun`, for `drun <command> --help`.
const COMMANDS: &[&str] = &[
    "run",
    "ps",
    "status",
    "logs",
    "wait",
    "exec",
    "cp",
    "rm",
    "hosts",
    "pick",
    "check-flags",
];

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
        "ps" => {
            no_arguments("ps", args)?;
            ps(environment)
        }
        "status" => status(environment, args),
        "wait" => wait(environment, args),
        "logs" => {
            let container = docker_args::parse_logs(args).map_err(usage_error)?;
            hand_to_holder(environment, "logs", &container, args)
        }
        "exec" => {
            let container = docker_args::parse_exec(args).map_err(usage_error)?;
            hand_to_holder(environment, "exec", &container, args)
        }
        "cp" => {
            let container = docker_args::parse_cp(args).map_err(usage_error)?;
            hand_to_holder(environment, "cp", &container, args)
        }
        "rm" => rm(environment, args),
        "check-flags" => {
            no_arguments("check-flags", args)?;
            check_flags()
        }
        "hosts" => {
            no_arguments("hosts", args)?;
            hosts(environment)
        }
        "pick" => {
            no_arguments("pick", args)?;
            pick(environment)
        }
        _ => {
            let mut docker = host::this_computer();
            docker.arg(command).args(args);
            Ok(hand_over(docker))
        }
    }
}

/// `drun run`: runs a container on the least busy host.
fn run(environment: &Environment, args: &[OsString]) -> Result<ExitCode, Failure> {
    let parsed = docker_args::parse_run(args).map_err(usage_error)?;
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
            let name = fleet::bare_name(name);
            refuse_taken(&inventory, name)?;
            name.to_owned()
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

/// `drun ps`: lists the containers that `drun run` started, on every host.
fn ps(environment: &Environment) -> Result<ExitCode, Failure> {
    let config = load(environment)?;
    let inventory = fleet::inventory(&config, Scope::Managed);
    print!("{}", inventory.table());
    let mut complete = true;
    for (context, failure) in inventory.failures() {
        eprintln!("drun: {context} did not answer: {failure}");
        complete = false;
    }
    if complete {
        Ok(ExitCode::SUCCESS)
    } else {
        Err(Failure::NotStarted(
            "the list does not show the containers of the hosts that did not answer".to_owned(),
        ))
    }
}

/// `drun status NAME`: prints the host, the state, and the exit status of a
/// container.
fn status(environment: &Environment, args: &[OsString]) -> Result<ExitCode, Failure> {
    let name = docker_args::parse_name("status", args).map_err(usage_error)?;
    let config = load(environment)?;
    let host = locate(&config, &name)?;
    let inspect = [
        "inspect",
        "--type",
        "container",
        "--format",
        STATE_FORMAT,
        name.as_str(),
    ];
    let answer = host::ask(&host, &inspect, config.host_timeout())
        .map_err(|failure| Failure::NotStarted(format!("{host}: {failure}")))?;
    let state = fleet::describe_state(&answer).ok_or_else(|| {
        Failure::NotStarted(format!(
            "{host}: docker inspect printed \"{}\", which is not a state",
            answer.trim()
        ))
    })?;
    println!("{host} {state}");
    Ok(ExitCode::SUCCESS)
}

/// `drun wait NAME`: waits until a container stops, prints its exit status,
/// and exits with it.
fn wait(environment: &Environment, args: &[OsString]) -> Result<ExitCode, Failure> {
    let name = docker_args::parse_name("wait", args).map_err(usage_error)?;
    let config = load(environment)?;
    let host = locate(&config, &name)?;
    let output = host
        .docker()
        .args(["wait", name.as_str()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(cannot_start)?;
    if !output.status.success() {
        return Ok(exit_code(output.status));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let code: i64 = text.trim().parse().map_err(|_| {
        Failure::NotStarted(format!(
            "docker wait printed \"{}\", which is not an exit status",
            text.trim()
        ))
    })?;
    println!("{code}");
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}

/// Finds the host that has `container`, and replaces `drun` with
/// `docker <subcommand> <args>` on that host.
fn hand_to_holder(
    environment: &Environment,
    subcommand: &str,
    container: &str,
    args: &[OsString],
) -> Result<ExitCode, Failure> {
    let config = load(environment)?;
    let host = locate(&config, container)?;
    let mut command = host.docker();
    command.arg(subcommand).args(args);
    Ok(hand_over(command))
}

/// `drun rm NAME...`: removes each container on its own host.
///
/// It finds every container before it removes one, so it removes nothing when
/// it cannot find one of them. It tries every host, and exits with the first
/// exit status that is not zero.
fn rm(environment: &Environment, args: &[OsString]) -> Result<ExitCode, Failure> {
    let parsed = docker_args::parse_rm(args).map_err(usage_error)?;
    let config = load(environment)?;
    let inventory = fleet::inventory(&config, Scope::All);
    let mut plan: Vec<(ContextName, Vec<&str>)> = Vec::new();
    for container in &parsed.containers {
        let host = inventory
            .locate(container)
            .map_err(|error| Failure::NotStarted(error.to_string()))?;
        if let Some(index) = plan.iter().position(|(planned, _)| planned == host) {
            plan[index].1.push(container);
        } else {
            plan.push((host.clone(), vec![container]));
        }
    }
    let mut first_failure = None;
    for (host, containers) in &plan {
        let status = host
            .docker()
            .arg("rm")
            .args(&parsed.flags)
            .args(containers)
            .status()
            .map_err(cannot_start)?;
        if !status.success() && first_failure.is_none() {
            first_failure = Some(status);
        }
    }
    Ok(first_failure.map_or(ExitCode::SUCCESS, exit_code))
}

/// `drun check-flags`: compares the flag tables of `drun` with the help of
/// the docker CLI on this computer, and prints each difference.
fn check_flags() -> Result<ExitCode, Failure> {
    let report = flag_check::check().map_err(Failure::NotStarted)?;
    let mut count = 0_usize;
    for (command, differences) in &report {
        for difference in differences {
            println!("docker {command}: {difference}");
            count += 1;
        }
    }
    if count == 0 {
        println!("The flag tables of drun agree with the docker CLI on this computer.");
        return Ok(ExitCode::SUCCESS);
    }
    eprintln!(
        "drun: the flag tables and docker disagree in {count} places. Update the tables in \
         src/drun/src/docker_args.rs, and copy the new help into src/drun/tests/fixtures/help/"
    );
    Ok(ExitCode::from(EXIT_DIFFERENCES))
}

/// Finds the one host that has the container `reference`.
fn locate(config: &Config, reference: &str) -> Result<ContextName, Failure> {
    fleet::inventory(config, Scope::All)
        .locate(reference)
        .cloned()
        .map_err(|error| Failure::NotStarted(error.to_string()))
}

/// Reads the configuration.
fn load(environment: &Environment) -> Result<Config, Failure> {
    Config::load(environment).map_err(|error| Failure::NotStarted(error.to_string()))
}

/// The failure for arguments that `drun` cannot read.
#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err hands the error over by value"
)]
fn usage_error(error: docker_args::ArgsError) -> Failure {
    Failure::Usage(error.to_string())
}

/// The failure when the `docker` program does not start.
#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err hands the error over by value"
)]
fn cannot_start(error: std::io::Error) -> Failure {
    Failure::NotStarted(format!("cannot start docker: {error}"))
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
                "drun: warning: the bind mount {source} is a path on this computer, and the \
                 container runs on {host}. When {host} is another computer, Docker mounts \
                 {source} from {host} and not from this computer. To move files to or from \
                 another computer, use a named volume and drun cp"
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
        .map_err(cannot_start)?;
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
  drun ps
  drun status NAME
  drun logs [docker logs options] NAME
  drun wait NAME
  drun exec [docker exec options] NAME COMMAND [ARG...]
  drun cp [docker cp options] NAME:PATH LOCAL_PATH
  drun cp [docker cp options] LOCAL_PATH NAME:PATH
  drun rm [docker rm options] NAME [NAME...]
  drun hosts
  drun pick
  drun check-flags
  drun DOCKER-COMMAND [ARG...]
  drun --help
  drun --version

Commands:
  run     Run a container on the least busy host. drun gives the arguments
          to docker run without a change. Without -d, drun gives the
          terminal to the container and exits with its exit status. With -d,
          drun prints \"<host> <name>\" when the container starts.
  ps      List the containers that drun started, on every host.
  status  Print the host and the state of a container, and its exit status
          when it stopped, as in \"box-a exited 0\".
  logs    Show the logs of a container, with docker logs on its host.
  wait    Wait until a container stops. Print its exit status, and exit
          with it.
  exec    Run a command in a running container, with docker exec on its host.
  cp      Copy files out of a container or into it, with docker cp on its
          host.
  rm      Remove containers, with docker rm on the host of each. drun
          removes nothing when it cannot find one of the containers.
  hosts   Show how busy each host is, and which host drun chooses.
  pick    Print the name of the host that drun chooses, for example for
          docker --context \"$(drun pick)\" compose up -d
  check-flags
          Compare the flag tables of drun with the help of the docker CLI on
          this computer. Run it after each update of Docker. drun refuses a
          docker flag that is not in its tables, unless you write the flag
          as --flag=VALUE.
  DOCKER-COMMAND
          Any other command, or a docker flag before a command. drun gives
          it to the docker of this computer without a change, as in
          drun buildx ls or drun --context box-a ps. drun asks no host and
          adds no label or name, so a container that such a command starts
          does not show in drun ps.

  NAME is the name of a container, its ID, or the start of its ID. drun asks
  every host for its containers. NAME must match exactly one container on all
  hosts together, or drun stops and names each container that matches. As
  in docker, a NAME that starts with / is a name and never an ID.

Files:
  When the host is another computer, a bind mount, as in -v ./src:/app,
  mounts a path on that computer and not on this one. Write the results to
  a named volume, and get them with drun cp.

Names:
  drun gives each container a name that no host uses, unless you give a name
  with --name. drun refuses a --name that a host uses. As in docker, one
  leading / in the name is not part of it. Each container gets the label
  {MANAGED_LABEL}.

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
  The exit status of docker, or of the container in the foreground. 1 when
  drun check-flags finds a difference. 2 when the arguments are wrong. 125
  when drun stops before docker can run.
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lines of the `Usage:` block of the help, without their indent.
    fn usage_lines(help: &str) -> Vec<&str> {
        help.lines()
            .skip_while(|line| *line != "Usage:")
            .skip(1)
            .take_while(|line| line.starts_with("  "))
            .map(str::trim_start)
            .collect()
    }

    /// The words of the usage line `line` without its `[docker ... options]`
    /// markers, one space between each two words. The options of
    /// `docker buildx` come between `buildx` and `create`, so the words of
    /// that command show only without the markers.
    fn without_markers(line: &str) -> String {
        let mut text = line.to_owned();
        for (command, _) in docker_args::tables() {
            text = text.replace(&format!("[docker {command} options]"), " ");
        }
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// True when the usage line `line` is a line of the docker command
    /// `command`, which can have more than one word.
    fn is_line_of(line: &str, command: &str) -> bool {
        let head = format!("drun {command}");
        let words = without_markers(line);
        words == head || words.starts_with(&format!("{head} "))
    }

    /// Every usage line of a command that has a flag table shows the docker
    /// options, so a reader sees that the command takes them.
    #[test]
    fn each_usage_line_of_a_docker_command_shows_its_docker_options() {
        let help = usage(&Environment::default());
        let lines = usage_lines(&help);
        for (command, _) in docker_args::tables() {
            let marker = format!("[docker {command} options]");
            let own: Vec<&&str> = lines
                .iter()
                .filter(|line| is_line_of(line, command))
                .collect();
            assert!(!own.is_empty(), "no usage line for {command}: {help}");
            for line in own {
                assert!(line.contains(&marker), "{line:?} lacks {marker}");
            }
        }
    }

    /// The `Files:` section states its condition, because the container does
    /// not always run on another computer.
    #[test]
    fn the_files_section_of_the_help_starts_with_its_condition() {
        let help = usage(&Environment::default());
        let section: Vec<&str> = help
            .lines()
            .skip_while(|line| *line != "Files:")
            .skip(1)
            .take_while(|line| line.starts_with("  "))
            .map(str::trim_start)
            .collect();
        let text = section.join(" ");
        assert!(
            text.starts_with("When the host is another computer"),
            "{text}"
        );
    }
}

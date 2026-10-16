//! Reads the arguments that `drun` hands to a docker command.
//!
//! `drun` passes every argument to docker without a change, but it needs a
//! few facts from them first: if `docker run` detaches, which name the
//! container gets, and which bind mounts read from a path. To find those
//! facts, `drun` must know which flags take a value, as the docker CLI does:
//! in `docker run -h web alpine`, `web` is the value of `-h` and `alpine` is
//! the image. For `docker buildx create`, `drun` must find if the arguments
//! name an endpoint.
//!
//! The tables below list the flags of each docker command. `drun` refuses a
//! flag that is not in its table, because it cannot know if the next argument
//! is the value of that flag or the image. A flag that is written with `=`,
//! as in `--new-flag=value`, carries its own value, so `drun` accepts it.

use std::ffi::OsString;

/// One flag of a docker command.
#[derive(Debug, Clone, Copy)]
pub struct FlagSpec {
    /// The one-letter form, as in `-d`.
    pub short: Option<char>,
    /// The long form without the dashes, as in `detach`.
    pub long: &'static str,
    /// True when the flag takes a value, as in `--name web`.
    pub takes_value: bool,
    /// True when `docker <command> --help` lists the flag. Docker accepts a
    /// flag that it does not list, such as an old alias.
    pub listed: bool,
}

/// A flag with a one-letter form that takes a value.
const fn value(short: char, long: &'static str) -> FlagSpec {
    flag(Some(short), long, true)
}

/// A flag with a one-letter form that is on or off.
const fn switch(short: char, long: &'static str) -> FlagSpec {
    flag(Some(short), long, false)
}

/// A flag with only a long form that takes a value.
const fn long_value(long: &'static str) -> FlagSpec {
    flag(None, long, true)
}

/// A flag with only a long form that is on or off.
const fn long_switch(long: &'static str) -> FlagSpec {
    flag(None, long, false)
}

/// A flag that docker lists in its help.
const fn flag(short: Option<char>, long: &'static str, takes_value: bool) -> FlagSpec {
    FlagSpec {
        short,
        long,
        takes_value,
        listed: true,
    }
}

/// `spec`, for a flag that docker accepts but does not list in its help.
const fn hidden(spec: FlagSpec) -> FlagSpec {
    FlagSpec {
        listed: false,
        ..spec
    }
}

/// The docker commands whose flags `drun` keeps in a table, each with its
/// table.
pub fn tables() -> [(&'static str, &'static [FlagSpec]); 7] {
    [RUN, LOGS, EXEC, CP, RM, BUILDX, BUILDX_CREATE].map(|spec| (spec.name, spec.flags))
}

/// The driver of a builder that runs `BuildKit` in a container on a Docker
/// endpoint. `docker buildx create` uses it when the arguments name no
/// driver.
const CONTAINER_DRIVER: &str = "docker-container";

/// The buildx command that creates a builder.
const CREATE: &str = "create";

/// The grammar of the arguments of one docker command.
#[derive(Debug)]
struct CommandSpec {
    /// The name of the docker command, for error messages.
    name: &'static str,
    /// The flags of the command.
    flags: &'static [FlagSpec],
    /// True when flags can come after the first positional argument. For
    /// `docker run` and `docker exec` they cannot: the arguments after the
    /// image or the container are the command for the container.
    interspersed: bool,
}

/// The flags of `docker run`, from Docker 29.
const RUN: CommandSpec = CommandSpec {
    name: "run",
    interspersed: false,
    flags: &[
        switch('d', "detach"),
        switch('i', "interactive"),
        switch('t', "tty"),
        switch('P', "publish-all"),
        switch('q', "quiet"),
        value('a', "attach"),
        value('c', "cpu-shares"),
        value('e', "env"),
        value('h', "hostname"),
        value('l', "label"),
        value('m', "memory"),
        value('p', "publish"),
        value('u', "user"),
        value('v', "volume"),
        value('w', "workdir"),
        hidden(long_switch("disable-content-trust")),
        long_switch("help"),
        long_switch("init"),
        long_switch("no-healthcheck"),
        long_switch("oom-kill-disable"),
        long_switch("privileged"),
        long_switch("read-only"),
        long_switch("rm"),
        long_switch("sig-proxy"),
        long_switch("use-api-socket"),
        long_value("add-host"),
        long_value("annotation"),
        long_value("blkio-weight"),
        long_value("blkio-weight-device"),
        long_value("cap-add"),
        long_value("cap-drop"),
        long_value("cgroup-parent"),
        long_value("cgroupns"),
        long_value("cidfile"),
        long_value("cpu-count"),
        long_value("cpu-percent"),
        long_value("cpu-period"),
        long_value("cpu-quota"),
        long_value("cpu-rt-period"),
        long_value("cpu-rt-runtime"),
        long_value("cpus"),
        long_value("cpuset-cpus"),
        long_value("cpuset-mems"),
        long_value("detach-keys"),
        long_value("device"),
        long_value("device-cgroup-rule"),
        long_value("device-read-bps"),
        long_value("device-read-iops"),
        long_value("device-write-bps"),
        long_value("device-write-iops"),
        long_value("dns"),
        hidden(long_value("dns-opt")),
        long_value("dns-option"),
        long_value("dns-search"),
        long_value("domainname"),
        long_value("entrypoint"),
        long_value("env-file"),
        long_value("expose"),
        long_value("gpus"),
        long_value("group-add"),
        long_value("health-cmd"),
        long_value("health-interval"),
        long_value("health-retries"),
        long_value("health-start-interval"),
        long_value("health-start-period"),
        long_value("health-timeout"),
        long_value("io-maxbandwidth"),
        long_value("io-maxiops"),
        long_value("ip"),
        long_value("ip6"),
        long_value("ipc"),
        long_value("isolation"),
        hidden(long_value("kernel-memory")),
        long_value("label-file"),
        long_value("link"),
        long_value("link-local-ip"),
        long_value("log-driver"),
        long_value("log-opt"),
        long_value("mac-address"),
        long_value("memory-reservation"),
        long_value("memory-swap"),
        long_value("memory-swappiness"),
        long_value("mount"),
        long_value("name"),
        hidden(long_value("net")),
        hidden(long_value("net-alias")),
        long_value("network"),
        long_value("network-alias"),
        long_value("oom-score-adj"),
        long_value("pid"),
        long_value("pids-limit"),
        long_value("platform"),
        long_value("pull"),
        long_value("restart"),
        long_value("runtime"),
        long_value("security-opt"),
        long_value("shm-size"),
        long_value("stop-signal"),
        long_value("stop-timeout"),
        long_value("storage-opt"),
        long_value("sysctl"),
        long_value("tmpfs"),
        long_value("ulimit"),
        long_value("userns"),
        long_value("uts"),
        long_value("volume-driver"),
        long_value("volumes-from"),
    ],
};

/// The flags of `docker logs`, from Docker 29.
const LOGS: CommandSpec = CommandSpec {
    name: "logs",
    interspersed: true,
    flags: &[
        switch('f', "follow"),
        switch('t', "timestamps"),
        value('n', "tail"),
        long_switch("details"),
        hidden(long_switch("help")),
        long_value("since"),
        long_value("until"),
    ],
};

/// The flags of `docker exec`, from Docker 29.
const EXEC: CommandSpec = CommandSpec {
    name: "exec",
    interspersed: false,
    flags: &[
        switch('d', "detach"),
        switch('i', "interactive"),
        switch('t', "tty"),
        value('e', "env"),
        value('u', "user"),
        value('w', "workdir"),
        hidden(long_switch("help")),
        long_switch("privileged"),
        long_value("detach-keys"),
        long_value("env-file"),
    ],
};

/// The flags of `docker cp`, from Docker 29.
const CP: CommandSpec = CommandSpec {
    name: "cp",
    interspersed: true,
    flags: &[
        switch('a', "archive"),
        switch('L', "follow-link"),
        switch('q', "quiet"),
        hidden(long_switch("help")),
    ],
};

/// The flags of `docker rm`, from Docker 29.
const RM: CommandSpec = CommandSpec {
    name: "rm",
    interspersed: true,
    flags: &[
        switch('f', "force"),
        switch('l', "link"),
        switch('v', "volumes"),
        hidden(long_switch("help")),
    ],
};

/// The flags of `docker buildx` that come before its command, from buildx
/// 0.36. buildx accepts `-h`, but it calls `-h` deprecated and does not list
/// it.
const BUILDX: CommandSpec = CommandSpec {
    name: "buildx",
    interspersed: false,
    flags: &[
        switch('D', "debug"),
        hidden(switch('h', "help")),
        long_value("builder"),
    ],
};

/// The flags of `docker buildx create`, from buildx 0.36. create accepts
/// `--config`, the old name of `--buildkitd-config`, and `--builder`, the flag
/// of `docker buildx` that create inherits. It does not list either of them.
const BUILDX_CREATE: CommandSpec = CommandSpec {
    name: "buildx create",
    interspersed: true,
    flags: &[
        switch('D', "debug"),
        hidden(switch('h', "help")),
        long_switch("append"),
        long_switch("bootstrap"),
        long_switch("leave"),
        long_switch("use"),
        hidden(long_value("builder")),
        long_value("buildkitd-config"),
        long_value("buildkitd-flags"),
        hidden(long_value("config")),
        long_value("driver"),
        long_value("driver-opt"),
        long_value("name"),
        long_value("node"),
        long_value("platform"),
        long_value("timeout"),
    ],
};

/// Why `drun` cannot read the arguments.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ArgsError {
    /// A one-letter flag that the table of the command does not have.
    #[error("drun does not know the flag -{flag} of docker {command}")]
    UnknownShort {
        /// The docker command.
        command: &'static str,
        /// The letter.
        flag: char,
    },
    /// A long flag without `=` that the table of the command does not have.
    #[error(
        "drun does not know the flag --{flag} of docker {command}. Write it with its value \
         after \"=\", as in --{flag}=VALUE or --{flag}=true"
    )]
    UnknownLong {
        /// The docker command.
        command: &'static str,
        /// The flag without the dashes.
        flag: String,
    },
    /// A flag that takes a value is the last argument.
    #[error("the flag {flag} of docker {command} needs a value")]
    MissingValue {
        /// The docker command.
        command: &'static str,
        /// The flag as it was written.
        flag: String,
    },
    /// A switch has a value that is not true or false.
    #[error("the flag {flag} of docker {command} takes true or false, not \"{value}\"")]
    NotABoolean {
        /// The docker command.
        command: &'static str,
        /// The flag as it was written.
        flag: String,
        /// The value.
        value: String,
    },
    /// The last `--name` is empty, or is only a leading `/`.
    #[error("the flag --name of docker {command} needs a container name, not an empty one")]
    EmptyName {
        /// The docker command.
        command: &'static str,
    },
    /// The arguments that are not flags are not what the command needs.
    #[error("drun {command} needs {what}")]
    Shape {
        /// The docker command.
        command: &'static str,
        /// What the command needs, as in "an image".
        what: &'static str,
    },
}

/// The facts that `drun` needs from the arguments of `docker run`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunArgs {
    /// True when the container runs in the background (`-d`).
    pub detach: bool,
    /// True when Docker removes the container when it stops (`--rm`).
    pub remove: bool,
    /// The name from `--name`, when there is one.
    pub name: Option<String>,
    /// The source path of each bind mount, from `-v` and `--mount`.
    pub bind_sources: Vec<String>,
}

/// The arguments of `docker rm`, divided into the flags and the containers.
#[derive(Debug, PartialEq, Eq)]
pub struct RmArgs {
    /// The flags, as they were written.
    pub flags: Vec<OsString>,
    /// The names or IDs of the containers.
    pub containers: Vec<String>,
}

/// What `drun` does with the arguments that follow `docker buildx`.
#[derive(Debug, PartialEq, Eq)]
pub enum BuildxArgs {
    /// The arguments create a `docker-container` builder and name no
    /// endpoint. `drun` adds the least busy host as the endpoint, so `BuildKit`
    /// runs on that host.
    PlaceBuilder {
        /// The name from `--name`, when there is one. An empty `--name`
        /// counts as no name, as an empty `--driver` counts as no driver.
        name: Option<String>,
    },
    /// Any other arguments. `drun` gives them to docker without a change.
    Unchanged,
}

/// Reads the arguments that follow `docker buildx` or `docker builder`.
///
/// `drun` places the builder only when all of these are true:
///
/// - The command of buildx is `create`.
/// - The arguments name no endpoint, so docker would use the current context
///   of this computer.
/// - The driver is `docker-container`, which is the default. The endpoint of
///   another driver, as `remote` or `kubernetes`, is not a Docker host.
/// - The arguments do not ask for the help, and do not remove a node with
///   `--leave`.
///
/// `drun` reads the flags of buildx before the command, to find the command.
/// It does not read the arguments of any command other than `create`.
///
/// # Errors
///
/// Returns an [`ArgsError`] for a flag that `drun` does not know, or a flag
/// without its value.
pub fn parse_buildx(args: &[OsString]) -> Result<BuildxArgs, ArgsError> {
    let root = BUILDX.scan(args)?;
    let Some(&command) = root.positionals.first() else {
        return Ok(BuildxArgs::Unchanged);
    };
    if args[command] != CREATE || asks_for_help(&root) {
        return Ok(BuildxArgs::Unchanged);
    }
    let create = BUILDX_CREATE.scan(&args[command + 1..])?;
    if !create.positionals.is_empty() || asks_for_help(&create) {
        return Ok(BuildxArgs::Unchanged);
    }
    let mut name = None;
    let mut driver = None;
    for found in create.flags {
        match (found.long, found.setting) {
            ("leave", Setting::Switch(true)) => return Ok(BuildxArgs::Unchanged),
            ("name", Setting::Value(value)) => name = (!value.is_empty()).then_some(value),
            ("driver", Setting::Value(value)) => driver = Some(value),
            _ => {}
        }
    }
    // An empty driver is no driver, so buildx uses the default.
    if driver.is_some_and(|driver| !driver.is_empty() && driver != CONTAINER_DRIVER) {
        return Ok(BuildxArgs::Unchanged);
    }
    Ok(BuildxArgs::PlaceBuilder { name })
}

/// True when the flags of `scan` turn on the help. Docker then prints the
/// help and does nothing else.
fn asks_for_help(scan: &Scan) -> bool {
    scan.flags.iter().any(|found| {
        matches!(
            (found.long, &found.setting),
            ("help", Setting::Switch(true))
        )
    })
}

/// The container that the arguments of `docker logs` name.
///
/// # Errors
///
/// Returns an [`ArgsError`] for a flag that `drun` does not know, a flag
/// without its value, or arguments that do not name exactly one container.
pub fn parse_logs(args: &[OsString]) -> Result<String, ArgsError> {
    let scan = LOGS.scan(args)?;
    match scan.positionals.as_slice() {
        [container] => Ok(lossy(&args[*container])),
        _ => Err(ArgsError::Shape {
            command: LOGS.name,
            what: "one container",
        }),
    }
}

/// The container that the arguments of `docker exec` name. The arguments
/// after the container are the command to run in it.
///
/// # Errors
///
/// Returns an [`ArgsError`] for a flag that `drun` does not know, a flag
/// without its value, or arguments without a container and a command.
pub fn parse_exec(args: &[OsString]) -> Result<String, ArgsError> {
    let scan = EXEC.scan(args)?;
    match scan.positionals.as_slice() {
        [container, _command, ..] => Ok(lossy(&args[*container])),
        _ => Err(ArgsError::Shape {
            command: EXEC.name,
            what: "a container and a command",
        }),
    }
}

/// The container that the arguments of `docker cp` name, on the side
/// written as `CONTAINER:PATH`.
///
/// The rule is the rule of the docker CLI: an argument that starts with `/`
/// or `.` is a path on this computer, and so is an argument without `:`.
///
/// # Errors
///
/// Returns an [`ArgsError`] for a flag that `drun` does not know, or for
/// arguments that are not one container side and one side on this computer.
pub fn parse_cp(args: &[OsString]) -> Result<String, ArgsError> {
    let scan = CP.scan(args)?;
    let shape = ArgsError::Shape {
        command: CP.name,
        what: "CONTAINER:PATH on one side and a path on this computer on the other",
    };
    let [first, second] = scan.positionals.as_slice() else {
        return Err(shape);
    };
    let containers: Vec<String> = [first, second]
        .into_iter()
        .filter_map(|index| cp_container(&lossy(&args[*index])))
        .collect();
    match containers.as_slice() {
        [container] => Ok(container.clone()),
        _ => Err(shape),
    }
}

/// The container of the `docker cp` argument `arg`, by the rule of the docker
/// CLI. `None` means that `arg` is a path on this computer.
fn cp_container(arg: &str) -> Option<String> {
    if arg.starts_with('/') || arg.starts_with('.') {
        return None;
    }
    let (container, _) = arg.split_once(':')?;
    (!container.is_empty()).then(|| container.to_owned())
}

/// Divides the arguments of `docker rm` into flags and containers.
///
/// # Errors
///
/// Returns an [`ArgsError`] for a flag that `drun` does not know, or for
/// arguments without a container.
pub fn parse_rm(args: &[OsString]) -> Result<RmArgs, ArgsError> {
    let scan = RM.scan(args)?;
    if scan.positionals.is_empty() {
        return Err(ArgsError::Shape {
            command: RM.name,
            what: "at least one container",
        });
    }
    // Every flag of docker rm is a switch, so each argument that is not a
    // container is a flag.
    let flags = args
        .iter()
        .enumerate()
        .filter(|(index, _)| !scan.positionals.contains(index))
        .map(|(_, arg)| arg.clone())
        .collect();
    let containers = scan
        .positionals
        .iter()
        .map(|index| lossy(&args[*index]))
        .collect();
    Ok(RmArgs { flags, containers })
}

/// The one container name of a `drun` command that takes a name and no
/// flags, as `drun status` and `drun wait` do.
///
/// # Errors
///
/// Returns [`ArgsError::Shape`] unless `args` is one argument that does not
/// start with `-`.
pub fn parse_name(command: &'static str, args: &[OsString]) -> Result<String, ArgsError> {
    match args {
        [only] if !only.to_string_lossy().starts_with('-') => Ok(lossy(only)),
        _ => Err(ArgsError::Shape {
            command,
            what: "one container name",
        }),
    }
}

/// `arg` as text. A byte that is not UTF-8 becomes U+FFFD, and a name with
/// such a byte then matches no container.
fn lossy(arg: &OsString) -> String {
    arg.to_string_lossy().into_owned()
}

/// Reads the arguments that follow `docker run`.
///
/// # Errors
///
/// Returns an [`ArgsError`] for a flag that `drun` does not know, a flag
/// without its value, a switch with a value that is not true or false,
/// arguments without an image, or a last `--name` that is empty once its
/// leading `/` is gone.
pub fn parse_run(args: &[OsString]) -> Result<RunArgs, ArgsError> {
    let scan = RUN.scan(args)?;
    if scan.positionals.is_empty() {
        return Err(ArgsError::Shape {
            command: RUN.name,
            what: "an image",
        });
    }
    let mut parsed = RunArgs::default();
    for found in scan.flags {
        match (found.long, found.setting) {
            ("detach", Setting::Switch(on)) => parsed.detach = on,
            ("rm", Setting::Switch(on)) => parsed.remove = on,
            ("name", Setting::Value(name)) => parsed.name = Some(name),
            ("volume", Setting::Value(spec)) => {
                parsed.bind_sources.extend(volume_bind_source(&spec));
            }
            ("mount", Setting::Value(spec)) => {
                parsed.bind_sources.extend(mount_bind_source(&spec));
            }
            _ => {}
        }
    }
    // Docker takes the last `--name`, so only the last one counts.
    if parsed
        .name
        .as_deref()
        .is_some_and(|name| crate::fleet::bare_name(name).is_empty())
    {
        return Err(ArgsError::EmptyName { command: RUN.name });
    }
    Ok(parsed)
}

/// The source of the `-v` value `spec` when it is a bind mount.
///
/// `SOURCE:TARGET[:OPTIONS]` is a bind mount when `SOURCE` is a path, which
/// starts with `/` or `.`. Any other `SOURCE` is the name of a volume, and a
/// value without `:` is a volume without a name.
fn volume_bind_source(spec: &str) -> Option<String> {
    let (source, _) = spec.split_once(':')?;
    (source.starts_with('/') || source.starts_with('.')).then(|| source.to_owned())
}

/// The source of the `--mount` value `spec` when it is a bind mount, as in
/// `type=bind,source=/srv,target=/x`. The type is `volume` when the value
/// does not say.
fn mount_bind_source(spec: &str) -> Option<String> {
    let mut kind = "volume";
    let mut source = None;
    for field in spec.split(',') {
        match field.split_once('=') {
            Some(("type", value)) => kind = value,
            Some(("source" | "src", value)) => source = Some(value),
            _ => {}
        }
    }
    if kind == "bind" {
        source.map(str::to_owned)
    } else {
        None
    }
}

/// What one flag that the scan found sets.
#[derive(Debug)]
enum Setting {
    /// A switch, on or off.
    Switch(bool),
    /// The value of a flag that takes a value.
    Value(String),
}

/// One flag that the scan found.
#[derive(Debug)]
struct Found {
    /// The long form of the flag, without the dashes.
    long: &'static str,
    /// What the flag sets.
    setting: Setting,
}

/// The flags and the positional arguments of one command line.
#[derive(Debug, Default)]
struct Scan {
    /// The flags, in the order of the arguments.
    flags: Vec<Found>,
    /// The index of each positional argument.
    positionals: Vec<usize>,
}

impl CommandSpec {
    /// The flag with the one-letter form `letter`.
    fn by_short(&self, letter: char) -> Option<FlagSpec> {
        self.flags
            .iter()
            .copied()
            .find(|flag| flag.short == Some(letter))
    }

    /// The flag with the long form `name`.
    fn by_long(&self, name: &str) -> Option<FlagSpec> {
        self.flags.iter().copied().find(|flag| flag.long == name)
    }

    /// Divides `args` into flags and positional arguments, as the docker CLI
    /// does.
    ///
    /// `--` ends the flags. A long flag can hold its value after `=`. A group
    /// of one-letter flags, as in `-dit`, can end with a flag that takes a
    /// value, and the value is the rest of the group or the next argument.
    ///
    /// # Errors
    ///
    /// Returns an [`ArgsError`] for a flag that is not in the table, a flag
    /// without its value, or a switch with a value that is not true or false.
    fn scan(&self, args: &[OsString]) -> Result<Scan, ArgsError> {
        let mut scan = Scan::default();
        let mut index = 0;
        while index < args.len() {
            let text = args[index].to_string_lossy();
            if text == "--" {
                scan.positionals.extend(index + 1..args.len());
                break;
            }
            if let Some(long) = text.strip_prefix("--") {
                index = self.scan_long(args, index, long, &mut scan)?;
            } else if let Some(group) = text.strip_prefix('-').filter(|group| !group.is_empty()) {
                index = self.scan_short_group(args, index, group, &mut scan)?;
            } else {
                scan.positionals.push(index);
                if !self.interspersed {
                    scan.positionals.extend(index + 1..args.len());
                    break;
                }
            }
            index += 1;
        }
        Ok(scan)
    }

    /// Reads the long flag `long` (the text after `--`) at `args[index]`, and
    /// gives back the index of the last argument that it used.
    fn scan_long(
        &self,
        args: &[OsString],
        mut index: usize,
        long: &str,
        scan: &mut Scan,
    ) -> Result<usize, ArgsError> {
        let (name, inline) = match long.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (long, None),
        };
        let written = format!("--{name}");
        let Some(spec) = self.by_long(name) else {
            if inline.is_some() {
                return Ok(index);
            }
            return Err(ArgsError::UnknownLong {
                command: self.name,
                flag: name.to_owned(),
            });
        };
        let setting = if spec.takes_value {
            if let Some(value) = inline {
                Setting::Value(value.to_owned())
            } else {
                index += 1;
                Setting::Value(self.value_at(args, index, &written)?)
            }
        } else {
            Setting::Switch(self.switch_value(&written, inline)?)
        };
        scan.flags.push(Found {
            long: spec.long,
            setting,
        });
        Ok(index)
    }

    /// Reads the group of one-letter flags `group` (the text after `-`) at
    /// `args[index]`, and gives back the index of the last argument that it
    /// used.
    fn scan_short_group(
        &self,
        args: &[OsString],
        mut index: usize,
        group: &str,
        scan: &mut Scan,
    ) -> Result<usize, ArgsError> {
        let mut letters = group.chars();
        while let Some(letter) = letters.next() {
            let spec = self.by_short(letter).ok_or(ArgsError::UnknownShort {
                command: self.name,
                flag: letter,
            })?;
            let written = format!("-{letter}");
            let rest = letters.as_str();
            if spec.takes_value {
                let attached = rest.strip_prefix('=').unwrap_or(rest);
                let value = if attached.is_empty() {
                    index += 1;
                    self.value_at(args, index, &written)?
                } else {
                    attached.to_owned()
                };
                scan.flags.push(Found {
                    long: spec.long,
                    setting: Setting::Value(value),
                });
                return Ok(index);
            }
            if let Some(value) = rest.strip_prefix('=') {
                scan.flags.push(Found {
                    long: spec.long,
                    setting: Setting::Switch(self.switch_value(&written, Some(value))?),
                });
                return Ok(index);
            }
            scan.flags.push(Found {
                long: spec.long,
                setting: Setting::Switch(true),
            });
        }
        Ok(index)
    }

    /// The argument at `index`, which is the value of the flag `written`.
    fn value_at(
        &self,
        args: &[OsString],
        index: usize,
        written: &str,
    ) -> Result<String, ArgsError> {
        args.get(index)
            .map(|value| value.to_string_lossy().into_owned())
            .ok_or_else(|| ArgsError::MissingValue {
                command: self.name,
                flag: written.to_owned(),
            })
    }

    /// The setting of the switch `written` with the value `value`, with the
    /// words that Go's `strconv.ParseBool` accepts. A switch without a value
    /// is on.
    fn switch_value(&self, written: &str, value: Option<&str>) -> Result<bool, ArgsError> {
        match value {
            None | Some("1" | "t" | "T" | "true" | "TRUE" | "True") => Ok(true),
            Some("0" | "f" | "F" | "false" | "FALSE" | "False") => Ok(false),
            Some(other) => Err(ArgsError::NotABoolean {
                command: self.name,
                flag: written.to_owned(),
                value: other.to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    fn run(words: &[&str]) -> Result<RunArgs, ArgsError> {
        parse_run(&args(words))
    }

    #[test]
    fn an_empty_name_is_refused_when_it_is_the_last_one() {
        for words in [
            &["--name", "", "alpine"][..],
            &["--name=", "alpine"],
            &["--name", "/", "alpine"],
            &["--name", "job-1", "--name", "", "alpine"],
        ] {
            assert_eq!(
                run(words),
                Err(ArgsError::EmptyName { command: "run" }),
                "{words:?}"
            );
        }
    }

    #[test]
    fn an_empty_name_that_a_later_name_replaces_is_accepted() {
        let parsed = run(&["--name", "", "--name", "job-1", "alpine"]).unwrap();
        assert_eq!(parsed.name.as_deref(), Some("job-1"));
    }

    #[test]
    fn an_image_alone_runs_in_the_foreground() {
        assert_eq!(run(&["alpine"]), Ok(RunArgs::default()));
    }

    #[test]
    fn detach_is_found_in_every_spelling() {
        for words in [
            &["-d", "alpine"][..],
            &["-dit", "alpine", "sh"],
            &["-itd", "alpine"],
            &["--detach", "alpine"],
            &["--detach=true", "alpine"],
            &["-i", "-t", "-d", "alpine"],
        ] {
            assert!(run(words).unwrap().detach, "{words:?}");
        }
        assert!(!run(&["--detach=false", "alpine"]).unwrap().detach);
        assert!(!run(&["-it", "alpine"]).unwrap().detach);
    }

    #[test]
    fn the_arguments_after_the_image_are_the_command_and_not_flags() {
        let parsed = run(&["alpine", "echo", "-d", "--rm", "--name", "x", "-Z"]).unwrap();
        assert_eq!(parsed, RunArgs::default());
    }

    #[test]
    fn the_value_of_a_flag_is_not_the_image() {
        let parsed = run(&["-h", "web", "-e", "A=b", "--workdir", "/w", "alpine"]).unwrap();
        assert_eq!(parsed, RunArgs::default());
        let parsed = run(&["-h", "-d", "alpine"]).unwrap();
        assert!(!parsed.detach, "-d is the value of -h here");
    }

    #[test]
    fn a_short_flag_can_hold_its_value() {
        assert!(
            run(&["-p80:80", "-ePATH=/x", "-e=A=b", "-dp", "80:80", "alpine"])
                .unwrap()
                .detach
        );
    }

    #[test]
    fn the_last_name_wins() {
        assert_eq!(
            run(&["--name=first", "--name", "second", "alpine"])
                .unwrap()
                .name,
            Some("second".to_owned())
        );
    }

    #[test]
    fn a_name_keeps_its_characters() {
        assert_eq!(
            run(&["--name", "café-日本語-🎉", "alpine"]).unwrap().name,
            Some("café-日本語-🎉".to_owned())
        );
    }

    #[test]
    fn rm_is_found_and_can_be_turned_off() {
        assert!(run(&["--rm", "alpine"]).unwrap().remove);
        assert!(!run(&["--rm=false", "alpine"]).unwrap().remove);
        assert!(!run(&["alpine"]).unwrap().remove);
    }

    #[test]
    fn bind_mounts_give_their_source_and_volumes_do_not() {
        let parsed = run(&[
            "-v",
            "data:/data",
            "-v",
            "./src:/app",
            "-v",
            "/anonymous",
            "--volume=/home/me/out:/out:ro",
            "--mount",
            "type=bind,source=/srv,target=/x",
            "--mount",
            "type=bind,src=../up,dst=/y,readonly",
            "--mount",
            "type=volume,src=named,dst=/z",
            "--mount=source=/not-a-bind,target=/w",
            "alpine",
        ])
        .unwrap();
        assert_eq!(
            parsed.bind_sources,
            ["./src", "/home/me/out", "/srv", "../up"]
        );
    }

    #[test]
    fn a_double_dash_ends_the_flags() {
        assert!(run(&["-d", "--", "alpine"]).unwrap().detach);
        assert_eq!(
            run(&["--", "-d"]),
            Ok(RunArgs::default()),
            "after --, -d is the image"
        );
    }

    #[test]
    fn an_unknown_short_flag_is_refused() {
        assert_eq!(
            run(&["-Z", "alpine"]),
            Err(ArgsError::UnknownShort {
                command: "run",
                flag: 'Z'
            })
        );
        assert_eq!(
            run(&["-dZ", "alpine"]),
            Err(ArgsError::UnknownShort {
                command: "run",
                flag: 'Z'
            })
        );
    }

    #[test]
    fn an_unknown_long_flag_is_refused_unless_it_holds_its_value() {
        let error = run(&["--frobnicate", "alpine"]).unwrap_err();
        assert_eq!(
            error,
            ArgsError::UnknownLong {
                command: "run",
                flag: "frobnicate".to_owned()
            }
        );
        assert!(error.to_string().contains("--frobnicate=VALUE"), "{error}");
        assert_eq!(run(&["--frobnicate=1", "alpine"]), Ok(RunArgs::default()));
    }

    #[test]
    fn a_flag_without_its_value_is_refused() {
        assert_eq!(
            run(&["alpine", "--name"]).map(|parsed| parsed.name),
            Ok(None),
            "after the image, --name is a word of the command"
        );
        assert_eq!(
            run(&["--name"]),
            Err(ArgsError::MissingValue {
                command: "run",
                flag: "--name".to_owned()
            })
        );
        assert_eq!(
            run(&["-e"]),
            Err(ArgsError::MissingValue {
                command: "run",
                flag: "-e".to_owned()
            })
        );
    }

    #[test]
    fn a_switch_with_a_value_that_is_not_a_boolean_is_refused() {
        assert_eq!(
            run(&["--rm=maybe", "alpine"]),
            Err(ArgsError::NotABoolean {
                command: "run",
                flag: "--rm".to_owned(),
                value: "maybe".to_owned()
            })
        );
    }

    #[test]
    fn run_needs_an_image() {
        assert_eq!(
            run(&["-d", "--rm"]),
            Err(ArgsError::Shape {
                command: "run",
                what: "an image"
            })
        );
        assert_eq!(
            run(&[]),
            Err(ArgsError::Shape {
                command: "run",
                what: "an image"
            })
        );
    }

    #[test]
    fn logs_name_one_container_before_or_after_the_flags() {
        for words in [
            &["job"][..],
            &["-f", "job"],
            &["job", "-f"],
            &["--tail", "10", "job"],
            &["-n", "5", "-ft", "job"],
            &["--since=1h", "job", "--details"],
        ] {
            assert_eq!(parse_logs(&args(words)), Ok("job".to_owned()), "{words:?}");
        }
    }

    #[test]
    fn logs_without_exactly_one_container_are_refused() {
        let shape = Err(ArgsError::Shape {
            command: "logs",
            what: "one container",
        });
        assert_eq!(parse_logs(&args(&[])), shape);
        assert_eq!(parse_logs(&args(&["-f"])), shape);
        assert_eq!(parse_logs(&args(&["a", "b"])), shape);
        assert_eq!(
            parse_logs(&args(&["-x", "job"])),
            Err(ArgsError::UnknownShort {
                command: "logs",
                flag: 'x'
            })
        );
    }

    #[test]
    fn exec_names_the_container_before_the_command() {
        assert_eq!(
            parse_exec(&args(&["-it", "-e", "A=b", "job", "sh", "-c", "echo -d"])),
            Ok("job".to_owned())
        );
        assert_eq!(
            parse_exec(&args(&["-u", "root", "job", "ls", "-x"])),
            Ok("job".to_owned())
        );
        assert_eq!(
            parse_exec(&args(&["-u", "job", "ls"])),
            Err(ArgsError::Shape {
                command: "exec",
                what: "a container and a command"
            }),
            "job is the value of -u, so ls is the container and there is no command"
        );
    }

    #[test]
    fn cp_finds_the_container_on_either_side() {
        assert_eq!(
            parse_cp(&args(&["job:/out/result.json", "./result.json"])),
            Ok("job".to_owned())
        );
        assert_eq!(
            parse_cp(&args(&["./in.txt", "job:/in.txt"])),
            Ok("job".to_owned())
        );
        assert_eq!(
            parse_cp(&args(&["-a", "job:/x", "/tmp/x"])),
            Ok("job".to_owned())
        );
        assert_eq!(
            parse_cp(&args(&["/a:b", "job:/x"])),
            Ok("job".to_owned()),
            "an absolute path is on this computer, with or without a colon"
        );
        assert_eq!(
            parse_cp(&args(&["./a:b", "job:/x"])),
            Ok("job".to_owned()),
            "a path that starts with a dot is on this computer"
        );
        assert_eq!(
            parse_cp(&args(&["job:/x", "-"])),
            Ok("job".to_owned()),
            "- is stdout"
        );
    }

    #[test]
    fn cp_needs_one_container_side_and_one_local_side() {
        let shape = Err(ArgsError::Shape {
            command: "cp",
            what: "CONTAINER:PATH on one side and a path on this computer on the other",
        });
        assert_eq!(parse_cp(&args(&["a:/x", "b:/y"])), shape);
        assert_eq!(parse_cp(&args(&["./a", "./b"])), shape);
        assert_eq!(parse_cp(&args(&["job:/x"])), shape);
        assert_eq!(parse_cp(&args(&["job:/x", "./a", "./b"])), shape);
    }

    #[test]
    fn rm_divides_the_flags_from_the_containers() {
        assert_eq!(
            parse_rm(&args(&["-f", "a", "b", "-v"])),
            Ok(RmArgs {
                flags: args(&["-f", "-v"]),
                containers: vec!["a".to_owned(), "b".to_owned()],
            })
        );
        assert_eq!(
            parse_rm(&args(&["-f"])),
            Err(ArgsError::Shape {
                command: "rm",
                what: "at least one container"
            })
        );
    }

    fn buildx(words: &[&str]) -> Result<BuildxArgs, ArgsError> {
        parse_buildx(&args(words))
    }

    fn placed(name: Option<&str>) -> BuildxArgs {
        BuildxArgs::PlaceBuilder {
            name: name.map(str::to_owned),
        }
    }

    #[test]
    fn create_without_an_endpoint_places_the_builder_in_every_spelling() {
        for words in [
            &["create"][..],
            &["create", "--use"],
            &["create", "--driver", "docker-container"],
            &["create", "--driver=docker-container", "--bootstrap"],
            &["create", "--driver="],
            &["create", "--leave=false"],
            &["create", "-D", "--platform", "linux/arm64"],
            &["create", "--driver-opt", "network=host", "--timeout", "30s"],
            &["create", "--buildkitd-flags", "--debug"],
            &["--builder", "other", "create"],
            &["--builder=other", "-D", "create"],
            &["create", "--"],
        ] {
            assert_eq!(buildx(words), Ok(placed(None)), "{words:?}");
        }
    }

    #[test]
    fn the_last_name_of_the_builder_wins_and_keeps_its_characters() {
        assert_eq!(
            buildx(&["create", "--use", "--name", "misc-multiarch"]),
            Ok(placed(Some("misc-multiarch")))
        );
        assert_eq!(
            buildx(&["create", "--name", "first", "--name=café-日本語-🎉"]),
            Ok(placed(Some("café-日本語-🎉")))
        );
    }

    #[test]
    fn an_empty_name_of_the_builder_is_no_name() {
        for words in [
            &["create", "--name", ""][..],
            &["create", "--name="],
            &["create", "--name", "x", "--name", ""],
        ] {
            assert_eq!(buildx(words), Ok(placed(None)), "{words:?}");
        }
        assert_eq!(
            buildx(&["create", "--name", "", "--name", "x"]),
            Ok(placed(Some("x")))
        );
    }

    #[test]
    fn create_that_names_an_endpoint_another_driver_or_the_help_is_unchanged() {
        for words in [
            &["create", "box-a"][..],
            &["create", "box-a", "--name", "n"],
            &["create", "--name", "n", "--", "box-a"],
            &["create", "--driver", "kubernetes"],
            &["create", "--driver=remote", "tcp://buildkitd.example:1234"],
            &["create", "--driver", "cloud", "--name", "c"],
            &["create", "--leave", "--name", "n", "--node", "n0"],
            &["create", "--help"],
            &["create", "-h"],
            &["--help", "create"],
            &["-h", "create"],
        ] {
            assert_eq!(buildx(words), Ok(BuildxArgs::Unchanged), "{words:?}");
        }
    }

    #[test]
    fn create_accepts_the_flags_that_docker_hides() {
        assert_eq!(
            buildx(&["create", "--config", "./buildkitd.toml", "--name", "b"]),
            Ok(placed(Some("b")))
        );
        assert_eq!(buildx(&["create", "--builder", "x"]), Ok(placed(None)));
        // --config takes its value, so the endpoint after it is still the endpoint.
        assert_eq!(
            buildx(&["create", "--config", "./buildkitd.toml", "tcp://box-a:1234"]),
            Ok(BuildxArgs::Unchanged)
        );
    }

    #[test]
    fn buildx_commands_other_than_create_are_unchanged() {
        for words in [
            &[][..],
            &["--help"],
            &["ls"],
            &["inspect", "--bootstrap", "misc-multiarch"],
            &[
                "build",
                "--platform",
                "linux/amd64,linux/arm64",
                "--push",
                ".",
            ],
            &["build", "--flag-that-drun-does-not-know", "."],
            &["imagetools", "create", "--tag", "x", "y"],
            &["--builder", "create", "ls"],
            &["-D", "use", "create"],
        ] {
            assert_eq!(buildx(words), Ok(BuildxArgs::Unchanged), "{words:?}");
        }
    }

    #[test]
    fn an_unknown_flag_of_buildx_or_of_create_is_refused() {
        assert_eq!(
            buildx(&["--frobnicate", "create"]),
            Err(ArgsError::UnknownLong {
                command: "buildx",
                flag: "frobnicate".to_owned()
            })
        );
        assert_eq!(
            buildx(&["create", "--frobnicate", "x"]),
            Err(ArgsError::UnknownLong {
                command: "buildx create",
                flag: "frobnicate".to_owned()
            })
        );
        assert_eq!(
            buildx(&["create", "-Z"]),
            Err(ArgsError::UnknownShort {
                command: "buildx create",
                flag: 'Z'
            })
        );
        assert_eq!(
            buildx(&["create", "--name"]),
            Err(ArgsError::MissingValue {
                command: "buildx create",
                flag: "--name".to_owned()
            })
        );
        assert_eq!(
            buildx(&["create", "--frobnicate=x"]),
            Ok(placed(None)),
            "a flag that holds its value is accepted"
        );
    }

    #[test]
    fn a_command_that_takes_one_name_takes_only_one_name() {
        assert_eq!(parse_name("status", &args(&["job"])), Ok("job".to_owned()));
        assert_eq!(
            parse_name("status", &args(&["café-🎉"])),
            Ok("café-🎉".to_owned())
        );
        let shape = Err(ArgsError::Shape {
            command: "wait",
            what: "one container name",
        });
        assert_eq!(parse_name("wait", &args(&[])), shape);
        assert_eq!(parse_name("wait", &args(&["a", "b"])), shape);
        assert_eq!(parse_name("wait", &args(&["-f"])), shape);
    }
}

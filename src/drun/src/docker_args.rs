//! Reads the arguments that `drun` hands to a docker command.
//!
//! `drun` passes every argument to docker without a change, but it needs a
//! few facts from them first: if `docker run` detaches, which name the
//! container gets, and which bind mounts read from a path. To find those
//! facts, `drun` must know which flags take a value, as the docker CLI does:
//! in `docker run -h web alpine`, `web` is the value of `-h` and `alpine` is
//! the image.
//!
//! The tables below list the flags of each docker command. `drun` refuses a
//! flag that is not in its table, because it cannot know if the next argument
//! is the value of that flag or the image. A flag that is written with `=`,
//! as in `--new-flag=value`, carries its own value, so `drun` accepts it.

use std::ffi::OsString;

/// One flag of a docker command.
#[derive(Debug, Clone, Copy)]
struct FlagSpec {
    /// The one-letter form, as in `-d`.
    short: Option<char>,
    /// The long form without the dashes, as in `detach`.
    long: &'static str,
    /// True when the flag takes a value, as in `--name web`.
    takes_value: bool,
}

/// A flag with a one-letter form that takes a value.
const fn value(short: char, long: &'static str) -> FlagSpec {
    FlagSpec {
        short: Some(short),
        long,
        takes_value: true,
    }
}

/// A flag with a one-letter form that is on or off.
const fn switch(short: char, long: &'static str) -> FlagSpec {
    FlagSpec {
        short: Some(short),
        long,
        takes_value: false,
    }
}

/// A flag with only a long form that takes a value.
const fn long_value(long: &'static str) -> FlagSpec {
    FlagSpec {
        short: None,
        long,
        takes_value: true,
    }
}

/// A flag with only a long form that is on or off.
const fn long_switch(long: &'static str) -> FlagSpec {
    FlagSpec {
        short: None,
        long,
        takes_value: false,
    }
}

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
        long_switch("disable-content-trust"),
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
        long_value("dns-opt"),
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
        long_value("kernel-memory"),
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
        long_value("net"),
        long_value("net-alias"),
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

/// Reads the arguments that follow `docker run`.
///
/// # Errors
///
/// Returns an [`ArgsError`] for a flag that `drun` does not know, a flag
/// without its value, a switch with a value that is not true or false, or
/// arguments without an image.
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
}

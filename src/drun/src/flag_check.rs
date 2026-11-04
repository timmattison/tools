//! Compares the flag tables of `drun` with the flags that the docker CLI on
//! this computer lists in its help.
//!
//! `drun` refuses a flag that is not in its table, so a table that is older
//! than the docker CLI refuses new flags. This check finds that difference.
//! Run it with `drun check-flags` after each update of Docker.
//!
//! The help of each docker command has an `Options:` part with one line for
//! each flag:
//!
//! ```text
//!   -a, --attach list                      Attach to STDIN, STDOUT or STDERR
//!       --rm                               Automatically remove the
//!                                          container and its associated
//! ```
//!
//! A word after the long form, as `list` above, is the type of the value, so
//! the flag takes a value. A flag without such a word is a switch. A line
//! that continues a description starts further to the right. The check
//! refuses a line that it cannot read, so help text in a new layout gives an
//! error and never a report that all flags agree.

use std::fmt;
use std::time::Duration;

use crate::docker_args::{self, FlagSpec};
use crate::host;

/// How long to wait for `docker <command> --help`.
const HELP_TIMEOUT: Duration = Duration::from_secs(30);

/// The line that starts the list of flags in the help of a docker command.
const OPTIONS_HEADING: &str = "Options:";

/// The widest indent of a line that holds a flag. pflag puts a flag with a
/// one-letter form 2 columns in and a flag without one 6 columns in. The
/// lines that continue a description start further in.
const MAX_FLAG_INDENT: usize = 6;

/// One flag, as the help of a docker command lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpFlag {
    /// The one-letter form, as in `-d`.
    pub short: Option<char>,
    /// The long form without the dashes, as in `detach`.
    pub long: String,
    /// True when the help gives a type for the value of the flag.
    pub takes_value: bool,
}

/// One way in which a table of `drun` and the help of docker disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Difference {
    /// Docker lists the flag, and the table does not have it.
    NotInTable {
        /// The long form of the flag.
        long: String,
        /// True when docker says that the flag takes a value.
        takes_value: bool,
    },
    /// The table lists the flag, and docker does not.
    NotInDocker {
        /// The long form of the flag.
        long: &'static str,
    },
    /// Docker and the table disagree about whether the flag takes a value.
    Value {
        /// The long form of the flag.
        long: &'static str,
        /// True when docker says that the flag takes a value.
        docker_takes_value: bool,
    },
    /// Docker and the table disagree about the one-letter form of the flag.
    Short {
        /// The long form of the flag.
        long: &'static str,
        /// The one-letter form in the help of docker.
        docker: Option<char>,
        /// The one-letter form in the table.
        table: Option<char>,
    },
    /// The table marks the flag as hidden, and docker lists it.
    Listed {
        /// The long form of the flag.
        long: &'static str,
    },
}

/// Words for a one-letter form, as in `-d` or `none`.
fn describe_short(short: Option<char>) -> String {
    short.map_or_else(|| "none".to_owned(), |letter| format!("-{letter}"))
}

impl fmt::Display for Difference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInTable { long, takes_value } => {
                let kind = if *takes_value {
                    "a flag that takes a value"
                } else {
                    "a switch"
                };
                write!(
                    formatter,
                    "docker lists --{long}, {kind}, but the table of drun does not have it"
                )
            }
            Self::NotInDocker { long } => write!(
                formatter,
                "the table of drun lists --{long}, but docker does not. If docker still \
                 accepts it, mark it as hidden in the table"
            ),
            Self::Value {
                long,
                docker_takes_value: true,
            } => write!(
                formatter,
                "--{long} takes a value in docker, but not in the table of drun"
            ),
            Self::Value {
                long,
                docker_takes_value: false,
            } => write!(
                formatter,
                "--{long} takes no value in docker, but takes a value in the table of drun"
            ),
            Self::Short {
                long,
                docker,
                table,
            } => write!(
                formatter,
                "the one-letter form of --{long} is {} in docker and {} in the table of drun",
                describe_short(*docker),
                describe_short(*table)
            ),
            Self::Listed { long } => write!(
                formatter,
                "docker lists --{long}, which the table of drun marks as hidden"
            ),
        }
    }
}

/// Reads the flags from the `Options:` part of the help of a docker command.
///
/// # Errors
///
/// Returns words that say what is wrong when the text has no `Options:`
/// part, when the part has no flags, or when a line of the part is not a flag
/// and does not continue a description.
pub fn parse_help(text: &str) -> Result<Vec<HelpFlag>, String> {
    let mut lines = text
        .lines()
        .skip_while(|line| line.trim_end() != OPTIONS_HEADING);
    if lines.next().is_none() {
        return Err(format!("the help has no \"{OPTIONS_HEADING}\" part"));
    }
    let mut flags = Vec::new();
    for line in lines {
        // An empty line, or a line that does not start with a space, starts
        // the next part of the help.
        if line.trim().is_empty() || !line.starts_with(' ') {
            break;
        }
        let content = line.trim_start_matches(' ');
        if line.len() - content.len() > MAX_FLAG_INDENT {
            continue;
        }
        let flag =
            parse_flag_line(content).ok_or_else(|| format!("\"{}\" is not a flag", line.trim()))?;
        flags.push(flag);
    }
    if flags.is_empty() {
        return Err(format!("the \"{OPTIONS_HEADING}\" part lists no flags"));
    }
    Ok(flags)
}

/// Reads one flag line without its indent, as in
/// `-a, --attach list   Attach to STDIN` or `--rm   Automatically remove`.
///
/// It is `None` when the line is not in that form.
fn parse_flag_line(content: &str) -> Option<HelpFlag> {
    // Two spaces end the flag and its type, and start the description.
    let head = content
        .split_once("  ")
        .map_or(content, |(head, _)| head)
        .trim_end();
    let (short, rest) = if head.starts_with("--") {
        (None, head)
    } else {
        let (letter, rest) = head.strip_prefix('-')?.split_once(", ")?;
        let mut letters = letter.chars();
        let short = letters.next().filter(char::is_ascii_alphanumeric)?;
        if letters.next().is_some() {
            return None;
        }
        (Some(short), rest)
    };
    let name_and_type = rest.strip_prefix("--")?;
    let (long, value_type) = match name_and_type.split_once(' ') {
        Some((long, value_type)) => (long, Some(value_type)),
        None => (name_and_type, None),
    };
    let long_is_a_name = !long.is_empty()
        && long
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    let type_is_a_word =
        value_type.is_none_or(|value_type| !value_type.is_empty() && !value_type.contains(' '));
    (long_is_a_name && type_is_a_word).then(|| HelpFlag {
        short,
        long: long.to_owned(),
        takes_value: value_type.is_some(),
    })
}

/// The differences between the table `table` and the flags `help` that
/// docker lists, in the order of the help and then of the table.
pub fn compare(table: &[FlagSpec], help: &[HelpFlag]) -> Vec<Difference> {
    let mut differences = Vec::new();
    for flag in help {
        let Some(spec) = table.iter().find(|spec| spec.long == flag.long) else {
            differences.push(Difference::NotInTable {
                long: flag.long.clone(),
                takes_value: flag.takes_value,
            });
            continue;
        };
        if !spec.listed {
            differences.push(Difference::Listed { long: spec.long });
        }
        if spec.takes_value != flag.takes_value {
            differences.push(Difference::Value {
                long: spec.long,
                docker_takes_value: flag.takes_value,
            });
        }
        if spec.short != flag.short {
            differences.push(Difference::Short {
                long: spec.long,
                docker: flag.short,
                table: spec.short,
            });
        }
    }
    for spec in table.iter().filter(|spec| spec.listed) {
        if !help.iter().any(|flag| flag.long == spec.long) {
            differences.push(Difference::NotInDocker { long: spec.long });
        }
    }
    differences
}

/// Compares each table of `drun` with the help of the docker CLI on this
/// computer, and gives back the differences of each docker command.
///
/// # Errors
///
/// Returns words that say what went wrong when docker cannot give its help,
/// or when `drun` cannot read the help.
pub fn check() -> Result<Vec<(&'static str, Vec<Difference>)>, String> {
    docker_args::tables()
        .into_iter()
        .map(|(command, table)| {
            let text = host::docker_help(command, HELP_TIMEOUT)
                .map_err(|failure| format!("docker {command} --help: {failure}"))?;
            let help = parse_help(&text)
                .map_err(|why| format!("cannot read the help of docker {command}: {why}"))?;
            Ok((command, compare(table, &help)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The help of each docker command, from the Docker version in
    /// `tests/fixtures/help/VERSION`.
    fn committed_help(command: &str) -> &'static str {
        match command {
            "run" => include_str!("../tests/fixtures/help/run.txt"),
            "logs" => include_str!("../tests/fixtures/help/logs.txt"),
            "exec" => include_str!("../tests/fixtures/help/exec.txt"),
            "cp" => include_str!("../tests/fixtures/help/cp.txt"),
            "rm" => include_str!("../tests/fixtures/help/rm.txt"),
            "buildx" => include_str!("../tests/fixtures/help/buildx.txt"),
            "buildx create" => include_str!("../tests/fixtures/help/buildx-create.txt"),
            other => panic!("no committed help for docker {other}"),
        }
    }

    /// The table of the docker command `command`.
    fn table(command: &str) -> &'static [FlagSpec] {
        docker_args::tables()
            .into_iter()
            .find(|(name, _)| *name == command)
            .map(|(_, flags)| flags)
            .expect("drun has a table for the command")
    }

    /// The flag `long` in `flags`.
    fn find<'a>(flags: &'a [HelpFlag], long: &str) -> &'a HelpFlag {
        flags
            .iter()
            .find(|flag| flag.long == long)
            .unwrap_or_else(|| panic!("the help lists --{long}"))
    }

    /// `text` with each line that starts with `old` changed to start with
    /// `new`. Exactly one line must change, so a test cannot pass on an edit
    /// that did nothing.
    fn edit(text: &str, old: &str, new: &str) -> String {
        let changed = text.lines().filter(|line| line.starts_with(old)).count();
        assert_eq!(changed, 1, "{old:?} must start exactly one line");
        let lines: Vec<String> = text
            .lines()
            .map(|line| match line.strip_prefix(old) {
                Some(rest) => format!("{new}{rest}"),
                None => line.to_owned(),
            })
            .collect();
        lines.join("\n")
    }

    #[test]
    fn the_help_of_docker_run_gives_each_flag_and_its_value() {
        let flags = parse_help(committed_help("run")).unwrap();
        assert_eq!(flags.len(), 102);
        let expect = |long: &str, short: Option<char>, takes_value: bool| {
            assert_eq!(
                find(&flags, long),
                &HelpFlag {
                    short,
                    long: long.to_owned(),
                    takes_value
                }
            );
        };
        expect("add-host", None, true);
        expect("attach", Some('a'), true);
        expect("detach", Some('d'), false);
        expect("help", None, false);
        expect("hostname", Some('h'), true);
        expect("publish-all", Some('P'), false);
        expect("rm", None, false);
        expect("workdir", Some('w'), true);
    }

    #[test]
    fn the_help_of_docker_buildx_create_gives_each_flag_and_its_value() {
        let flags = parse_help(committed_help("buildx create")).unwrap();
        assert_eq!(flags.len(), 13);
        let expect = |long: &str, short: Option<char>, takes_value: bool| {
            assert_eq!(
                find(&flags, long),
                &HelpFlag {
                    short,
                    long: long.to_owned(),
                    takes_value
                }
            );
        };
        expect("debug", Some('D'), false);
        expect("driver", None, true);
        expect("driver-opt", None, true);
        expect("leave", None, false);
        expect("name", None, true);
        expect("timeout", None, true);
        expect("use", None, false);
    }

    #[test]
    fn the_help_of_docker_buildx_ends_its_flags_before_the_commands() {
        let flags = parse_help(committed_help("buildx")).unwrap();
        let longs: Vec<&str> = flags.iter().map(|flag| flag.long.as_str()).collect();
        assert_eq!(longs, ["builder", "debug"]);
    }

    #[test]
    fn the_tables_cover_the_flags_of_buildx_and_buildx_create() {
        let commands: Vec<&str> = docker_args::tables()
            .into_iter()
            .map(|(command, _)| command)
            .collect();
        for command in ["buildx", "buildx create"] {
            assert!(commands.contains(&command), "{command}: {commands:?}");
        }
    }

    #[test]
    fn a_continued_description_that_starts_with_a_dash_is_not_a_flag() {
        let text = committed_help("run");
        assert!(text.contains("\n                                         -1 for unlimited)"));
        let flags = parse_help(text).unwrap();
        assert!(flags
            .iter()
            .all(|flag| flag.long != "1" && !flag.long.is_empty()));
    }

    #[test]
    fn every_table_agrees_with_the_committed_help_of_docker() {
        for (command, flags) in docker_args::tables() {
            let help = parse_help(committed_help(command)).unwrap();
            assert_eq!(compare(flags, &help), [], "docker {command}");
        }
    }

    #[test]
    fn a_flag_that_docker_adds_is_a_difference() {
        let text = format!(
            "{}      --new-flag string                  Do something new\n",
            committed_help("run")
        );
        let differences = compare(table("run"), &parse_help(&text).unwrap());
        assert_eq!(
            differences,
            [Difference::NotInTable {
                long: "new-flag".to_owned(),
                takes_value: true
            }]
        );
        assert!(differences[0].to_string().contains("--new-flag"));
    }

    #[test]
    fn a_flag_that_docker_removes_is_a_difference() {
        let kept: Vec<&str> = committed_help("run")
            .lines()
            .filter(|line| !line.starts_with("      --rm "))
            .collect();
        let text = kept.join("\n");
        assert_eq!(
            compare(table("run"), &parse_help(&text).unwrap()),
            [Difference::NotInDocker { long: "rm" }]
        );
    }

    #[test]
    fn a_flag_that_starts_to_take_a_value_is_a_difference() {
        let text = edit(committed_help("run"), "      --rm    ", "      --rm string");
        assert_eq!(
            compare(table("run"), &parse_help(&text).unwrap()),
            [Difference::Value {
                long: "rm",
                docker_takes_value: true
            }]
        );
    }

    #[test]
    fn a_flag_that_stops_taking_a_value_is_a_difference() {
        let text = edit(
            committed_help("logs"),
            "      --since string",
            "      --since       ",
        );
        assert_eq!(
            compare(table("logs"), &parse_help(&text).unwrap()),
            [Difference::Value {
                long: "since",
                docker_takes_value: false
            }]
        );
    }

    #[test]
    fn a_new_one_letter_form_is_a_difference() {
        let text = edit(committed_help("run"), "      --init", "  -I, --init");
        assert_eq!(
            compare(table("run"), &parse_help(&text).unwrap()),
            [Difference::Short {
                long: "init",
                docker: Some('I'),
                table: None
            }]
        );
    }

    #[test]
    fn a_hidden_flag_that_docker_starts_to_list_is_a_difference() {
        let text = format!(
            "{}      --net network                      Connect a container to a network\n",
            committed_help("run")
        );
        assert_eq!(
            compare(table("run"), &parse_help(&text).unwrap()),
            [Difference::Listed { long: "net" }]
        );
    }

    #[test]
    fn help_text_that_cannot_be_read_is_an_error_and_not_agreement() {
        for text in [
            "",
            "Usage:  docker run [OPTIONS] IMAGE\n",
            "Options:\n",
            "Options:\n\n",
            "Options:\n  garbage here\n",
            "Options:\n  -ab, --x   two letters\n",
            "Options:\n  -a --attach list   no comma\n",
            "Options:\n      --attach list of things   two words\n",
            "Options:\n      --   no name\n",
        ] {
            assert!(parse_help(text).is_err(), "{text:?} must be refused");
        }
    }

    #[test]
    fn a_flag_without_a_description_is_read() {
        assert_eq!(
            parse_help("Options:\n      --help\n  -d, --detach\n"),
            Ok(vec![
                HelpFlag {
                    short: None,
                    long: "help".to_owned(),
                    takes_value: false
                },
                HelpFlag {
                    short: Some('d'),
                    long: "detach".to_owned(),
                    takes_value: false
                },
            ])
        );
    }

    #[test]
    fn the_options_end_at_the_next_part_of_the_help() {
        let flags =
            parse_help("Options:\n  -f, --force   Force\n\nRun 'docker COMMAND --help' for more\n")
                .unwrap();
        assert_eq!(flags.len(), 1);
    }
}

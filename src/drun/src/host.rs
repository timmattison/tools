//! How `drun` names a Docker host and asks it a question.
//!
//! A host is a Docker context. Every question goes to every host at the same
//! time, and each answer has a time limit, so one host that does not answer
//! delays `drun` by the limit and no more.

use std::fmt;
use std::process::Command;
use std::thread;
use std::time::Duration;

use runlimit::RunFailure;

/// The program that `drun` runs for every host.
const DOCKER: &str = "docker";

/// The prefix of every environment variable that the docker CLI reads.
const DOCKER_ENVIRONMENT_PREFIX: &str = "DOCKER_";

/// The `DOCKER_HOST` that `drun` gives `docker <subcommand> --help`.
/// `/dev/null` is not a directory, so no engine can listen at this path, and
/// docker prints the help of the CLI alone. `DOCKER_HOST` beats the current
/// context in the docker config.
const NO_ENGINE_HOST: &str = "unix:///dev/null/drun-no-engine.sock";

/// The most characters of an error from docker that a message shows.
const DETAIL_LIMIT: usize = 200;

/// The name of a Docker context, which is how `drun` names a host.
///
/// The value always obeys the rule that Docker applies to a context name. It
/// thus cannot start with `-`, and docker cannot read it as a flag.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContextName(String);

/// Why a text is not a Docker context name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "\"{0}\" is not a Docker context name. A name starts with a letter or a digit, and \
     continues with one or more letters, digits, \"_\", \".\", \"+\", or \"-\""
)]
pub struct InvalidContextName(pub String);

impl ContextName {
    /// Accepts `text` when Docker accepts it as a context name: a letter or a
    /// digit, then one or more letters, digits, `_`, `.`, `+`, or `-`.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidContextName`] for any other text.
    pub fn parse(text: &str) -> Result<Self, InvalidContextName> {
        let mut characters = text.chars();
        let starts_well = characters
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric());
        let rest = characters.as_str();
        let continues_well = !rest.is_empty()
            && rest.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '+' | '-')
            });
        if starts_well && continues_well {
            Ok(Self(text.to_owned()))
        } else {
            Err(InvalidContextName(text.to_owned()))
        }
    }

    /// The name as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Starts a `docker` command line that goes to this host.
    pub fn docker(&self) -> Command {
        let mut command = Command::new(DOCKER);
        command.arg("--context").arg(&self.0);
        command
    }
}

impl fmt::Display for ContextName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Why a host gave no usable answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostFailure {
    /// The `docker` program is not on this computer.
    #[error("the docker command is not installed on this computer")]
    DockerMissing,
    /// The `docker` program did not start, or the wait for it failed.
    #[error("cannot start docker: {0}")]
    Start(String),
    /// The host did not answer in the time limit.
    #[error("no answer in {} s", .0.as_secs())]
    TimedOut(Duration),
    /// `docker` stopped with an exit status that is not zero.
    #[error("docker failed ({}): {detail}", describe_code(*.code))]
    Failed {
        /// The exit status, or `None` when a signal stopped docker.
        code: Option<i32>,
        /// The last line that docker wrote to stderr.
        detail: String,
    },
    /// The host answered, but `drun` cannot read the answer.
    #[error("the answer is not readable: {0}")]
    Unreadable(String),
}

/// Words for an exit status, for an error message.
fn describe_code(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("exit status {code}"),
        None => "stopped by a signal".to_owned(),
    }
}

impl From<RunFailure> for HostFailure {
    fn from(failure: RunFailure) -> Self {
        match failure {
            RunFailure::NotInstalled => Self::DockerMissing,
            RunFailure::Spawn(error) | RunFailure::Wait(error) => Self::Start(error.to_string()),
            RunFailure::TimedOut(limit) => Self::TimedOut(limit),
        }
    }
}

/// The answer of one host.
#[derive(Debug)]
pub struct Answer<T> {
    /// The host that answered.
    pub context: ContextName,
    /// What the host said, or why it said nothing usable.
    pub result: Result<T, HostFailure>,
}

/// Runs `docker --context <context> <args>` and gives back its stdout.
///
/// stdin is closed, so docker cannot stop to ask a question. `limit` is the
/// longest time to wait.
///
/// # Errors
///
/// Returns a [`HostFailure`] when docker is missing, cannot start, does not
/// finish in `limit`, stops with an exit status that is not zero, or writes
/// stdout that is not UTF-8.
pub fn ask(context: &ContextName, args: &[&str], limit: Duration) -> Result<String, HostFailure> {
    let mut command = context.docker();
    command.args(args);
    stdout_of(&mut command, limit)
}

/// Runs `docker <subcommand> --help` on this computer, without a context,
/// and gives back its stdout. The docker CLI hides each flag that the engine
/// or `DOCKER_API_VERSION` does not support. So `drun` removes the `DOCKER_`
/// environment and points docker at no engine, and docker lists every flag
/// that the CLI knows.
///
/// # Errors
///
/// Returns a [`HostFailure`] for the same reasons as [`ask`].
pub fn docker_help(subcommand: &str, limit: Duration) -> Result<String, HostFailure> {
    let mut command = Command::new(DOCKER);
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .starts_with(DOCKER_ENVIRONMENT_PREFIX)
        {
            command.env_remove(name);
        }
    }
    command.env("DOCKER_HOST", NO_ENGINE_HOST);
    command.args([subcommand, "--help"]);
    stdout_of(&mut command, limit)
}

/// Runs `command`, which is a docker command, and gives back its stdout.
///
/// # Errors
///
/// Returns a [`HostFailure`] for the same reasons as [`ask`].
fn stdout_of(command: &mut Command, limit: Duration) -> Result<String, HostFailure> {
    let output = runlimit::run_with_limit(command, limit)?;
    if !output.status.success() {
        return Err(HostFailure::Failed {
            code: output.status.code(),
            detail: last_line(&String::from_utf8_lossy(&output.stderr), DETAIL_LIMIT),
        });
    }
    String::from_utf8(output.stdout)
        .map_err(|_| HostFailure::Unreadable("the output is not UTF-8".to_owned()))
}

/// Asks every host in `contexts` the same `question`, all at the same time.
///
/// The answers come back in the order of `contexts`.
pub fn ask_all<T, Q>(contexts: &[ContextName], question: Q) -> Vec<Answer<T>>
where
    T: Send,
    Q: Fn(&ContextName) -> Result<T, HostFailure> + Sync,
{
    let question = &question;
    thread::scope(|scope| {
        let handles: Vec<_> = contexts
            .iter()
            .map(|context| scope.spawn(move || question(context)))
            .collect();
        contexts
            .iter()
            .zip(handles)
            .map(|(context, handle)| Answer {
                context: context.clone(),
                result: handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload)),
            })
            .collect()
    })
}

/// The last line of `text` that is not empty, cut to `max_characters`.
///
/// The cut counts characters, not bytes, so a multi-byte character is never
/// split.
pub fn last_line(text: &str, max_characters: usize) -> String {
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if line.chars().count() <= max_characters {
        return line.to_owned();
    }
    let kept: String = line
        .chars()
        .take(max_characters.saturating_sub(3))
        .collect();
    format!("{kept}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_context_name_obeys_the_rule_of_docker() {
        for valid in ["box-a", "default", "desktop-linux", "my.ctx_1+x", "B2"] {
            assert_eq!(
                ContextName::parse(valid).map(|name| name.as_str().to_owned()),
                Ok(valid.to_owned())
            );
        }
        for invalid in [
            "",
            "a",
            "-x",
            ".x",
            "_x",
            "box a",
            "café",
            "日本語",
            "🎉🎉",
            "a/b",
        ] {
            assert_eq!(
                ContextName::parse(invalid),
                Err(InvalidContextName(invalid.to_owned())),
                "{invalid:?} must be refused"
            );
        }
    }

    #[test]
    fn last_line_skips_empty_lines_at_the_end() {
        assert_eq!(last_line("first\nsecond\n\n  \n", 80), "second");
        assert_eq!(last_line("", 80), "");
    }

    #[test]
    fn last_line_cuts_at_a_character_and_not_at_a_byte() {
        assert_eq!(last_line("日本語テスト", 5), "日本...");
        assert_eq!(last_line("🎉🎊🎁🎈🎂", 4), "🎉...");
        assert_eq!(last_line("café au lait", 8), "café ...");
        assert_eq!(last_line("short", 5), "short");
    }

    #[test]
    fn a_failure_names_the_exit_status_and_the_error() {
        let failure = HostFailure::Failed {
            code: Some(1),
            detail: "Cannot connect to the Docker daemon".to_owned(),
        };
        assert_eq!(
            failure.to_string(),
            "docker failed (exit status 1): Cannot connect to the Docker daemon"
        );
        assert_eq!(
            HostFailure::TimedOut(Duration::from_secs(10)).to_string(),
            "no answer in 10 s"
        );
    }
}

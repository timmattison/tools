//! The `ssh` connection to a second Mac.
//!
//! The local `glance` opens one `ssh` connection. The remote Mac runs
//! `glance --stream`, and this module reads one JSON line for each sample.

use crate::interval::Interval;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

/// The first delay before a new connection attempt.
pub const FIRST_RETRY_DELAY: Duration = Duration::from_secs(5);

/// The longest delay before a new connection attempt.
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

/// The path of the remote `glance` when the person gives none.
pub const DEFAULT_REMOTE_BINARY: &str = "~/.cargo/bin/glance";

/// The reason that a text is not a destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationError {
    /// The text is empty.
    Empty,
    /// The text starts with a dash, so `ssh` reads it as an option.
    LeadingDash {
        /// The text that the person gave.
        text: String,
    },
}

impl fmt::Display for DestinationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DestinationError::Empty => write!(f, "the destination must not be empty"),
            DestinationError::LeadingDash { text } => {
                write!(
                    f,
                    "the destination must not start with \"-\", and \"{text}\" does"
                )
            }
        }
    }
}

impl std::error::Error for DestinationError {}

/// The place that `ssh` connects to: an alias, `user@host`, or an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination(String);

impl Destination {
    /// The destination as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Destination {
    type Err = DestinationError;

    /// Reads a destination from text.
    ///
    /// # Errors
    ///
    /// Returns an error if the text is empty or starts with `-`.
    fn from_str(text: &str) -> Result<Destination, DestinationError> {
        if text.is_empty() {
            return Err(DestinationError::Empty);
        }
        if text.starts_with('-') {
            return Err(DestinationError::LeadingDash {
                text: text.to_owned(),
            });
        }
        Ok(Destination(text.to_owned()))
    }
}

/// The reason that a text is not a remote binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteBinaryError {
    /// The text is empty.
    Empty,
}

impl fmt::Display for RemoteBinaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemoteBinaryError::Empty => write!(f, "the remote binary must not be empty"),
        }
    }
}

impl std::error::Error for RemoteBinaryError {}

/// The path of `glance` on the remote Mac.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBinary(String);

impl RemoteBinary {
    /// The path as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RemoteBinary {
    fn default() -> RemoteBinary {
        RemoteBinary(DEFAULT_REMOTE_BINARY.to_owned())
    }
}

impl FromStr for RemoteBinary {
    type Err = RemoteBinaryError;

    /// Reads a remote binary path from text.
    ///
    /// # Errors
    ///
    /// Returns an error if the text is empty.
    fn from_str(text: &str) -> Result<RemoteBinary, RemoteBinaryError> {
        if text.is_empty() {
            return Err(RemoteBinaryError::Empty);
        }
        Ok(RemoteBinary(text.to_owned()))
    }
}

/// The delay before the next connection attempt.
///
/// The delay doubles after each end of the connection, up to
/// [`MAX_RETRY_DELAY`]. A sample sets it back to [`FIRST_RETRY_DELAY`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backoff {
    /// The delay that the next end of the connection gets.
    next: Duration,
}

impl Backoff {
    /// A delay that starts at [`FIRST_RETRY_DELAY`].
    #[must_use]
    pub fn new() -> Backoff {
        Backoff {
            next: FIRST_RETRY_DELAY,
        }
    }

    /// Gives the delay for the end of a connection, then doubles it.
    pub fn after_end(&mut self) -> Duration {
        let delay = self.next;
        self.next = (self.next * 2).min(MAX_RETRY_DELAY);
        delay
    }

    /// Sets the delay back to [`FIRST_RETRY_DELAY`].
    pub fn after_sample(&mut self) {
        self.next = FIRST_RETRY_DELAY;
    }
}

impl Default for Backoff {
    fn default() -> Backoff {
        Backoff::new()
    }
}

/// The start of a path that the remote shell expands to the home directory.
const HOME_PREFIX: &str = "~/";

/// The words after the binary in the remote command.
const STREAM_FLAGS: &str = "--stream --interval";

/// The options that `ssh` gets before the destination. Each option has its own argument.
const SSH_OPTIONS: [&str; 10] = [
    "-T",
    "-o",
    "BatchMode=yes",
    "-o",
    "ConnectTimeout=10",
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=2",
    // Ends the options, so a destination can never act as one.
    "--",
];

/// The command that the remote shell runs.
///
/// A leading `~/` stays outside the quotes, so the remote shell expands it. The
/// rest of the path is quoted, so a space or a quote in it stays one word.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the connection that starts the child comes in the next commit"
    )
)]
fn remote_command(binary: &RemoteBinary, interval: Interval) -> String {
    let path = binary.as_str();
    let quoted = match path.strip_prefix(HOME_PREFIX) {
        Some(rest) => format!("{HOME_PREFIX}{}", shellquote::shell_quote(rest)),
        None => shellquote::shell_quote(path),
    };
    format!("{quoted} {STREAM_FLAGS} {interval}")
}

/// The arguments of `ssh`.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the connection that starts the child comes in the next commit"
    )
)]
fn ssh_arguments(
    destination: &Destination,
    interval: Interval,
    binary: &RemoteBinary,
) -> Vec<String> {
    let mut arguments: Vec<String> = SSH_OPTIONS
        .iter()
        .map(|&option| option.to_owned())
        .collect();
    arguments.push(destination.as_str().to_owned());
    arguments.push(remote_command(binary, interval));
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interval(text: &str) -> Interval {
        text.parse().expect("an interval")
    }

    #[test]
    fn a_destination_refuses_an_empty_text() {
        assert_eq!("".parse::<Destination>(), Err(DestinationError::Empty));
    }

    #[test]
    fn a_destination_refuses_a_leading_dash() {
        let error = "-oProxyCommand=x".parse::<Destination>().unwrap_err();
        assert_eq!(
            error,
            DestinationError::LeadingDash {
                text: "-oProxyCommand=x".to_owned()
            }
        );
        assert_eq!(
            error.to_string(),
            "the destination must not start with \"-\", and \"-oProxyCommand=x\" does"
        );
        assert_eq!(
            DestinationError::Empty.to_string(),
            "the destination must not be empty"
        );
    }

    #[test]
    fn a_destination_keeps_each_other_text() {
        for text in ["m1-ts", "tim@m1.local", "192.168.1.5"] {
            let destination = text.parse::<Destination>().expect("a destination");
            assert_eq!(destination.as_str(), text);
            assert_eq!(destination.to_string(), text);
        }
    }

    #[test]
    fn a_remote_binary_refuses_an_empty_text() {
        assert_eq!("".parse::<RemoteBinary>(), Err(RemoteBinaryError::Empty));
    }

    #[test]
    fn a_remote_binary_keeps_each_other_text() {
        let binary = "~/my bin/glance".parse::<RemoteBinary>().expect("a binary");
        assert_eq!(binary.as_str(), "~/my bin/glance");
        assert_eq!(RemoteBinary::default().as_str(), DEFAULT_REMOTE_BINARY);
    }

    fn command_for(binary: &str) -> String {
        remote_command(&binary.parse().expect("a binary"), interval("1"))
    }

    #[test]
    fn the_default_binary_keeps_its_tilde_outside_the_quotes() {
        assert_eq!(
            remote_command(&RemoteBinary::default(), interval("1")),
            "~/'.cargo/bin/glance' --stream --interval 1"
        );
    }

    #[test]
    fn a_tilde_path_with_a_space_quotes_only_the_rest() {
        assert_eq!(
            command_for("~/my bin/glance"),
            "~/'my bin/glance' --stream --interval 1"
        );
    }

    #[test]
    fn only_a_leading_tilde_slash_gets_the_rule() {
        assert_eq!(
            command_for("~user/glance"),
            "'~user/glance' --stream --interval 1"
        );
        assert_eq!(
            command_for("/opt/a~b/glance"),
            "'/opt/a~b/glance' --stream --interval 1"
        );
        assert_eq!(command_for("glance"), "'glance' --stream --interval 1");
    }

    #[test]
    fn the_command_carries_the_interval() {
        assert_eq!(
            remote_command(&RemoteBinary::default(), interval("0.25")),
            "~/'.cargo/bin/glance' --stream --interval 0.25"
        );
    }

    #[test]
    fn ssh_gets_the_options_then_the_destination_then_the_command() {
        let destination = "m1-ts".parse::<Destination>().expect("a destination");
        let arguments = ssh_arguments(&destination, interval("1"), &RemoteBinary::default());
        assert_eq!(
            arguments,
            [
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=2",
                "--",
                "m1-ts",
                "~/'.cargo/bin/glance' --stream --interval 1",
            ]
        );
    }

    #[test]
    fn the_delay_doubles_up_to_the_maximum() {
        let mut backoff = Backoff::new();
        let delays: Vec<u64> = (0..6).map(|_| backoff.after_end().as_secs()).collect();
        assert_eq!(delays, [5, 10, 20, 40, 60, 60]);
    }

    #[test]
    fn a_sample_sets_the_delay_back() {
        let mut backoff = Backoff::default();
        backoff.after_end();
        backoff.after_end();
        backoff.after_sample();
        assert_eq!(backoff, Backoff::new());
        assert_eq!(backoff.after_end(), FIRST_RETRY_DELAY);
    }
}

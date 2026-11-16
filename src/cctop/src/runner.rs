//! Runs external commands. Each command goes through the [`Runner`] trait, so
//! a test can answer a command without it running.

use std::time::Duration;

/// What a finished command gave back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// The exit code. `None` when a signal ended the command.
    pub code: Option<i32>,
    /// The text on standard output.
    pub stdout: String,
    /// The text on standard error.
    pub stderr: String,
}

impl CommandOutput {
    /// Returns true when the command exited with code 0.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }

    /// Makes the output of a command that exited with code 0.
    #[must_use]
    pub fn ok(stdout: &str) -> Self {
        Self {
            code: Some(0),
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    /// Makes the output of a command that exited with a non-zero `code`.
    #[must_use]
    pub fn failed(code: i32, stderr: &str) -> Self {
        Self {
            code: Some(code),
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }
}

/// Why a command gave no output.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    /// The program does not exist.
    #[error("{program} is not installed")]
    NotInstalled {
        /// The name of the program.
        program: String,
    },
    /// The command did not finish in the time limit.
    #[error("{program} did not finish in {seconds} s")]
    TimedOut {
        /// The name of the program.
        program: String,
        /// The time limit in seconds.
        seconds: u64,
    },
    /// The command did not start.
    #[error("{program} did not start: {message}")]
    Start {
        /// The name of the program.
        program: String,
        /// The reason that the system gave.
        message: String,
    },
}

/// The time limit of each external command.
pub const COMMAND_LIMIT: Duration = Duration::from_secs(5);

/// Runs a command and waits for it at most `limit`.
pub trait Runner: Send + Sync {
    /// Runs `argv`. The first item is the program.
    ///
    /// # Errors
    ///
    /// Returns a [`RunError`] when the program is not installed, does not
    /// start, or does not finish in `limit`.
    fn run(&self, argv: &[String], limit: Duration) -> Result<CommandOutput, RunError>;
}

/// The runner that starts real processes.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, argv: &[String], limit: Duration) -> Result<CommandOutput, RunError> {
        let Some((program, arguments)) = argv.split_first() else {
            return Err(RunError::Start {
                program: String::new(),
                message: "the command is empty".to_string(),
            });
        };
        let mut command = childenv::command(program);
        command.args(arguments);
        match runlimit::run_with_limit(&mut command, limit) {
            Ok(output) => Ok(CommandOutput {
                code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }),
            Err(runlimit::RunFailure::NotInstalled) => Err(RunError::NotInstalled {
                program: program.clone(),
            }),
            Err(runlimit::RunFailure::TimedOut(waited)) => Err(RunError::TimedOut {
                program: program.clone(),
                seconds: waited.as_secs(),
            }),
            Err(runlimit::RunFailure::Spawn(error) | runlimit::RunFailure::Wait(error)) => {
                Err(RunError::Start {
                    program: program.clone(),
                    message: error.to_string(),
                })
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod fake;

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn system_runner_returns_code_and_output() {
        let output = SystemRunner
            .run(
                &argv(&["/bin/sh", "-c", "echo out; echo err >&2; exit 3"]),
                COMMAND_LIMIT,
            )
            .unwrap();
        assert_eq!(output.code, Some(3));
        assert_eq!(output.stdout, "out\n");
        assert_eq!(output.stderr, "err\n");
    }

    #[test]
    fn system_runner_names_a_missing_program() {
        let error = SystemRunner
            .run(&argv(&["/no/such/program-cctop"]), COMMAND_LIMIT)
            .unwrap_err();
        assert_eq!(
            error,
            RunError::NotInstalled {
                program: "/no/such/program-cctop".to_string()
            }
        );
    }

    #[test]
    fn system_runner_times_out() {
        let error = SystemRunner
            .run(&argv(&["/bin/sleep", "5"]), Duration::from_millis(200))
            .unwrap_err();
        assert!(matches!(error, RunError::TimedOut { .. }), "{error:?}");
    }

    #[test]
    fn system_runner_refuses_an_empty_argv() {
        let error = SystemRunner.run(&[], COMMAND_LIMIT).unwrap_err();
        assert!(matches!(error, RunError::Start { .. }), "{error:?}");
    }
}

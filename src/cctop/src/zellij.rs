//! Focus step 1: asks Zellij to focus the pane of a session.

use crate::runner::{CommandOutput, RunError, Runner, COMMAND_LIMIT};
use crate::state::ZellijLocation;

/// Makes the argv that lists the Zellij sessions.
///
/// The argv has no `--short`, because `--short` does not mark an exited
/// session.
#[must_use]
pub fn list_sessions_argv() -> Vec<String> {
    ["zellij", "list-sessions", "--no-formatting"]
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// Reads the names of the live sessions from the output of `list-sessions`.
///
/// A live line has no `(EXITED`. The name is the text before ` [Created`.
#[must_use]
pub fn parse_live_sessions(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.contains("(EXITED"))
        .map(|line| {
            let name = line.split_once(" [Created").map_or(line, |(name, _)| name);
            name.trim().to_string()
        })
        .collect()
}

/// Makes the argv that focuses the pane at `location`.
#[must_use]
pub fn focus_argv(location: &ZellijLocation) -> Vec<String> {
    [
        "zellij",
        "--session",
        &location.session,
        "action",
        "focus-pane-id",
    ]
    .iter()
    .map(ToString::to_string)
    .chain(std::iter::once(location.pane.terminal_target()))
    .collect()
}

/// Why the focus request of step 1 failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ZellijFocusError {
    /// The list of the sessions did not run.
    #[error("zellij list-sessions failed: {0}")]
    List(RunError),
    /// The session is not in the list of live sessions.
    #[error("the Zellij session {0} is gone")]
    SessionGone(String),
    /// The focus command did not run.
    #[error("zellij action focus-pane-id failed: {0}")]
    Focus(RunError),
    /// The focus command ran and refused.
    #[error("zellij action focus-pane-id exited with {code}: {message}")]
    Refused {
        /// The exit code as text, or `a signal`.
        code: String,
        /// The text that the command wrote.
        message: String,
    },
}

/// Asks Zellij to focus the pane at `location`.
///
/// # Errors
///
/// Returns a [`ZellijFocusError`] when the list of sessions fails, when the
/// session is not live, or when the focus command fails or refuses.
pub fn focus_pane(runner: &dyn Runner, location: &ZellijLocation) -> Result<(), ZellijFocusError> {
    let listing = runner
        .run(&list_sessions_argv(), COMMAND_LIMIT)
        .map_err(ZellijFocusError::List)?;
    let live = if listing.succeeded() {
        parse_live_sessions(&listing.stdout)
    } else {
        Vec::new()
    };
    if !live.contains(&location.session) {
        return Err(ZellijFocusError::SessionGone(location.session.clone()));
    }
    let output = runner
        .run(&focus_argv(location), COMMAND_LIMIT)
        .map_err(ZellijFocusError::Focus)?;
    if output.succeeded() || is_already_focused(&output) {
        return Ok(());
    }
    let stderr = output.stderr.trim();
    let message = if stderr.is_empty() {
        output.stdout.trim()
    } else {
        stderr
    };
    Err(ZellijFocusError::Refused {
        code: output
            .code
            .map_or_else(|| "a signal".to_string(), |code| code.to_string()),
        message: message.to_string(),
    })
}

/// Returns true when Zellij exited with 1 to say that the pane has the focus.
fn is_already_focused(output: &CommandOutput) -> bool {
    output.code == Some(1)
        && [&output.stdout, &output.stderr]
            .iter()
            .any(|text| text.to_lowercase().contains("already focused"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;
    use crate::state::ZellijPaneId;

    fn location(session: &str) -> ZellijLocation {
        ZellijLocation {
            session: session.to_string(),
            pane: ZellijPaneId::parse("310").unwrap(),
        }
    }

    fn words(argv: &[String]) -> Vec<&str> {
        argv.iter().map(String::as_str).collect()
    }

    /// A runner that lists the fixture sessions and answers the focus
    /// command of `target` with `focus`.
    fn runner_for(target: &ZellijLocation, focus: Result<CommandOutput, RunError>) -> FakeRunner {
        let list = include_str!("../fixtures/zellij-list-sessions.txt");
        FakeRunner::new()
            .answer(&words(&list_sessions_argv()), Ok(CommandOutput::ok(list)))
            .answer(&words(&focus_argv(target)), focus)
    }

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_live_sessions_of_the_real_output_are_the_four_live_names() {
        let stdout = include_str!("../fixtures/zellij-list-sessions.txt");
        assert_eq!(
            parse_live_sessions(stdout),
            argv(&["nvr-tools", "tools", "megarepo", "dbt"])
        );
    }

    #[test]
    fn blank_lines_are_skipped_and_a_name_keeps_its_inner_space() {
        let stdout = "\n  \nmy session [Created 1h ago] \n\nold [Created 2d ago] (EXITED - attach to resurrect)\n";
        assert_eq!(parse_live_sessions(stdout), argv(&["my session"]));
    }

    #[test]
    fn the_list_argv_has_no_short_flag() {
        assert_eq!(
            list_sessions_argv(),
            argv(&["zellij", "list-sessions", "--no-formatting"])
        );
    }

    #[test]
    fn the_focus_argv_names_the_session_and_the_terminal_pane() {
        let location = ZellijLocation {
            session: "my tools".to_string(),
            pane: ZellijPaneId::parse("310").unwrap(),
        };
        assert_eq!(
            focus_argv(&location),
            argv(&[
                "zellij",
                "--session",
                "my tools",
                "action",
                "focus-pane-id",
                "terminal_310"
            ])
        );
    }

    #[test]
    fn a_live_session_is_listed_then_focused() {
        let target = location("tools");
        let runner = runner_for(&target, Ok(CommandOutput::ok("")));
        assert_eq!(focus_pane(&runner, &target), Ok(()));
        assert_eq!(
            runner.calls(),
            vec![list_sessions_argv(), focus_argv(&target)]
        );
    }

    #[test]
    fn an_exited_or_absent_session_is_gone_and_the_focus_command_never_runs() {
        for name in ["brainrot", "scraper", "no-such-session"] {
            let target = location(name);
            let runner = runner_for(&target, Ok(CommandOutput::ok("")));
            assert_eq!(
                focus_pane(&runner, &target),
                Err(ZellijFocusError::SessionGone(name.to_string()))
            );
            assert_eq!(runner.calls(), vec![list_sessions_argv()], "{name}");
        }
    }

    #[test]
    fn a_failed_list_command_means_the_session_is_gone() {
        let target = location("tools");
        let runner = FakeRunner::new().answer(
            &words(&list_sessions_argv()),
            Ok(CommandOutput::failed(1, "no server")),
        );
        assert_eq!(
            focus_pane(&runner, &target),
            Err(ZellijFocusError::SessionGone("tools".to_string()))
        );
    }

    #[test]
    fn a_list_command_that_does_not_run_is_a_list_error() {
        let error = RunError::NotInstalled {
            program: "zellij".to_string(),
        };
        let runner = FakeRunner::new().answer(&words(&list_sessions_argv()), Err(error.clone()));
        assert_eq!(
            focus_pane(&runner, &location("tools")),
            Err(ZellijFocusError::List(error))
        );
    }

    fn output(code: Option<i32>, stdout: &str, stderr: &str) -> CommandOutput {
        CommandOutput {
            code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn exit_1_with_already_focused_text_is_success() {
        let target = location("tools");
        for answer in [
            output(Some(1), "", "Error: pane is already focused\n"),
            output(Some(1), "Already Focused\n", ""),
        ] {
            let runner = runner_for(&target, Ok(answer));
            assert_eq!(focus_pane(&runner, &target), Ok(()));
        }
    }

    #[test]
    fn exit_1_with_other_text_is_refused() {
        let target = location("tools");
        let runner = runner_for(&target, Ok(output(Some(1), "", " no such pane \n")));
        assert_eq!(
            focus_pane(&runner, &target),
            Err(ZellijFocusError::Refused {
                code: "1".to_string(),
                message: "no such pane".to_string()
            })
        );
    }

    #[test]
    fn only_exit_1_counts_as_already_focused() {
        let target = location("tools");
        let runner = runner_for(&target, Ok(output(Some(2), "", "already focused")));
        assert_eq!(
            focus_pane(&runner, &target),
            Err(ZellijFocusError::Refused {
                code: "2".to_string(),
                message: "already focused".to_string()
            })
        );
    }

    #[test]
    fn a_refusal_without_stderr_gives_stdout_and_a_signal_has_no_code() {
        let target = location("tools");
        let runner = runner_for(&target, Ok(output(None, " from stdout \n", "")));
        assert_eq!(
            focus_pane(&runner, &target),
            Err(ZellijFocusError::Refused {
                code: "a signal".to_string(),
                message: "from stdout".to_string()
            })
        );
    }

    #[test]
    fn a_focus_command_that_does_not_run_is_a_focus_error() {
        let target = location("tools");
        let error = RunError::TimedOut {
            program: "zellij".to_string(),
            seconds: 5,
        };
        let runner = runner_for(&target, Err(error.clone()));
        assert_eq!(
            focus_pane(&runner, &target),
            Err(ZellijFocusError::Focus(error))
        );
    }
}

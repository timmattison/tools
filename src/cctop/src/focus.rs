//! Focus of a session in two steps. Step 1 asks Zellij to focus the pane.
//! Step 2 asks WezTerm to focus the window and the pane of the terminal.

use std::sync::Arc;

use proctree::Pid;

use crate::board::Session;
use crate::runner::Runner;
use crate::state::{ZellijLocation, ZellijPaneId};
use crate::wezterm::TtyWriter;

/// The session to focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusTarget {
    /// The process id of the Claude Code process.
    pub pid: Pid,
    /// The text that names the session on the screen.
    pub label: String,
    /// The Zellij session and pane of the session. `None` when the state file
    /// does not name both.
    pub zellij: Option<ZellijLocation>,
}

impl FocusTarget {
    /// Makes the target of `session`.
    #[must_use]
    pub fn of(session: &Session) -> Self {
        Self {
            pid: session.file.pid,
            label: session.label(),
            zellij: session.file.zellij_location(),
        }
    }
}

/// The things that a focus needs from the system.
#[derive(Clone)]
pub struct FocusTools {
    /// Runs the external commands.
    pub runner: Arc<dyn Runner>,
    /// Writes the focus request to a terminal device.
    pub writer: Arc<dyn TtyWriter>,
}

/// The result of a focus. Each step has its own result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusReport {
    /// The label of the session.
    pub label: String,
    /// The result of step 1. `None` when step 1 did not run. `Ok` holds the
    /// pane that Zellij focused.
    pub zellij: Option<Result<ZellijPaneId, String>>,
    /// The result of step 2. `Ok` holds the WezTerm pane that got the focus
    /// request.
    pub wezterm: Result<u64, String>,
}

impl FocusReport {
    /// Tells if each step that ran did succeed.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.zellij.as_ref().is_none_or(Result::is_ok) && self.wezterm.is_ok()
    }

    /// Makes the status line. Its parts are separated by ` · `.
    #[must_use]
    pub fn status_line(&self) -> String {
        let mut parts = Vec::new();
        match &self.zellij {
            None => {}
            Some(Ok(pane)) => parts.push(format!(
                "Zellij focused pane {}",
                pane.terminal_target()
            )),
            Some(Err(error)) => parts.push(format!("Zellij failed: {error}")),
        }
        match &self.wezterm {
            Ok(pane) => parts.push(format!("WezTerm focused pane {pane}")),
            Err(error) => parts.push(format!("WezTerm failed: {error}")),
        }
        format!("{}: {}", self.label, parts.join(" · "))
    }
}

/// Focuses the session of `target`. Both steps always run.
///
/// Step 1 runs only when the target has a Zellij session and pane. Step 2
/// finds the WezTerm pane of the terminal and sends the focus request to the
/// terminal device of that pane.
#[must_use]
pub fn focus(target: &FocusTarget, _tools: &FocusTools) -> FocusReport {
    FocusReport {
        label: target.label.clone(),
        zellij: None,
        wezterm: Err(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use crate::ancestry::ps_argv;
    use crate::lsof::{tty_argv, unix_sockets_argv};
    use crate::runner::fake::FakeRunner;
    use crate::runner::CommandOutput;
    use crate::state::parse_state_file;
    use crate::wezterm::{focus_request, list_argv};
    use crate::zellij::{focus_argv, list_sessions_argv};

    pub(super) fn session(pid: u32, name: Option<&str>, zellij: Option<(&str, &str)>) -> Session {
        let (zellij_session, zellij_pane) = zellij.map_or(
            ("null".to_string(), "null".to_string()),
            |(session, pane)| (format!("\"{session}\""), format!("\"{pane}\"")),
        );
        let json = format!(
            r#"{{"version":1,"pid":{pid},"sessionId":"s","startedAt":1,"state":"needs-you","stamp":2,"cwd":"/work/tools","zellijSession":{zellij_session},"zellijPane":{zellij_pane}}}"#
        );
        let file = parse_state_file(&json).unwrap();
        Session {
            file,
            name: name.map(ToString::to_string),
            since: crate::state::EpochMillis::new(2),
        }
    }

    #[test]
    fn the_target_of_a_session_in_zellij_has_its_pid_label_and_location() {
        let target = FocusTarget::of(&session(29130, Some("docs"), Some(("tools", "310"))));
        assert_eq!(
            target,
            FocusTarget {
                pid: Pid::new(29130),
                label: "docs".to_string(),
                zellij: Some(ZellijLocation {
                    session: "tools".to_string(),
                    pane: ZellijPaneId::parse("310").unwrap(),
                }),
            }
        );
    }

    #[test]
    fn the_target_of_a_session_outside_zellij_has_no_location_and_the_directory_label() {
        let target = FocusTarget::of(&session(51234, None, None));
        assert_eq!(target.pid, Pid::new(51234));
        assert_eq!(target.label, "tools");
        assert_eq!(target.zellij, None);
    }

    fn report(
        zellij: Option<Result<ZellijPaneId, String>>,
        wezterm: Result<u64, String>,
    ) -> FocusReport {
        FocusReport {
            label: "docs".to_string(),
            zellij,
            wezterm,
        }
    }

    fn pane(text: &str) -> ZellijPaneId {
        ZellijPaneId::parse(text).unwrap()
    }

    #[test]
    fn the_status_line_of_two_good_steps_names_both_panes() {
        assert_eq!(
            report(Some(Ok(pane("310"))), Ok(239)).status_line(),
            "docs: Zellij focused pane terminal_310 · WezTerm focused pane 239"
        );
    }

    #[test]
    fn the_status_line_outside_zellij_has_only_the_wezterm_part() {
        assert_eq!(
            report(None, Ok(239)).status_line(),
            "docs: WezTerm focused pane 239"
        );
    }

    #[test]
    fn the_status_line_names_each_failed_step_with_its_error() {
        assert_eq!(
            report(Some(Err("the Zellij session tools is gone".to_string())), Ok(239)).status_line(),
            "docs: Zellij failed: the Zellij session tools is gone · WezTerm focused pane 239"
        );
        assert_eq!(
            report(Some(Ok(pane("310"))), Err("no pane".to_string())).status_line(),
            "docs: Zellij focused pane terminal_310 · WezTerm failed: no pane"
        );
        assert_eq!(
            report(None, Err("no pane".to_string())).status_line(),
            "docs: WezTerm failed: no pane"
        );
    }

    #[test]
    fn a_report_succeeds_only_when_each_step_that_ran_succeeded() {
        assert!(report(Some(Ok(pane("310"))), Ok(239)).succeeded());
        assert!(report(None, Ok(239)).succeeded());
        assert!(!report(Some(Err("x".to_string())), Ok(239)).succeeded());
        assert!(!report(Some(Ok(pane("310"))), Err("x".to_string())).succeeded());
        assert!(!report(None, Err("x".to_string())).succeeded());
    }

    const PS: &str = include_str!("../fixtures/ps-tree.txt");
    const SOCKETS: &str = include_str!("../fixtures/lsof-unix-sockets.txt");
    const TTY: &str = include_str!("../fixtures/lsof-tty.txt");
    const PANES: &str = include_str!("../fixtures/wezterm-list.json");
    const SESSIONS: &str = include_str!("../fixtures/zellij-list-sessions.txt");

    /// A writer that records each write. It fails when `fail` is set.
    #[derive(Default)]
    struct RecordingWriter {
        writes: Mutex<Vec<(PathBuf, Vec<u8>)>>,
        fail: bool,
    }

    impl RecordingWriter {
        fn writes(&self) -> Vec<(PathBuf, Vec<u8>)> {
            self.writes.lock().unwrap().clone()
        }
    }

    impl TtyWriter for RecordingWriter {
        fn write(&self, tty: &Path, bytes: &[u8]) -> std::io::Result<()> {
            self.writes
                .lock()
                .unwrap()
                .push((tty.to_path_buf(), bytes.to_vec()));
            if self.fail {
                Err(std::io::Error::other("device is busy"))
            } else {
                Ok(())
            }
        }
    }

    fn words(argv: &[String]) -> Vec<&str> {
        argv.iter().map(String::as_str).collect()
    }

    fn ok(stdout: &str) -> Result<CommandOutput, crate::runner::RunError> {
        Ok(CommandOutput::ok(stdout))
    }

    fn zellij_target() -> FocusTarget {
        FocusTarget::of(&session(29130, Some("docs"), Some(("tools", "310"))))
    }

    /// A runner that answers each command of a focus in Zellij.
    fn zellij_runner() -> FakeRunner {
        FakeRunner::new()
            .answer(&words(&list_sessions_argv()), ok(SESSIONS))
            .answer(
                &words(&focus_argv(&zellij_target().zellij.unwrap())),
                ok(""),
            )
            .answer(&words(&ps_argv()), ok(PS))
            .answer(&words(&unix_sockets_argv()), ok(SOCKETS))
            .answer(&words(&tty_argv(Pid::new(19800))), ok(TTY))
            .answer(&words(&list_argv()), ok(PANES))
    }

    fn tools(runner: &Arc<FakeRunner>, writer: &Arc<RecordingWriter>) -> FocusTools {
        FocusTools {
            runner: runner.clone(),
            writer: writer.clone(),
        }
    }

    #[test]
    fn a_focus_in_zellij_runs_both_steps_in_order_and_writes_the_request() {
        let runner = Arc::new(zellij_runner());
        let writer = Arc::new(RecordingWriter::default());
        let target = zellij_target();
        let report = focus(&target, &tools(&runner, &writer));
        assert_eq!(
            runner.calls(),
            vec![
                list_sessions_argv(),
                focus_argv(&target.zellij.clone().unwrap()),
                ps_argv(),
                unix_sockets_argv(),
                tty_argv(Pid::new(19800)),
                list_argv(),
            ]
        );
        assert_eq!(
            writer.writes(),
            vec![(
                PathBuf::from("/dev/ttys231"),
                focus_request(239).into_bytes()
            )]
        );
        assert_eq!(report.zellij, Some(Ok(pane("310"))));
        assert_eq!(report.wezterm, Ok(239));
        assert!(report.succeeded());
        assert_eq!(
            report.status_line(),
            "docs: Zellij focused pane terminal_310 · WezTerm focused pane 239"
        );
    }
}

//! Focus of a session in two steps. Step 1 asks Zellij to focus the pane.
//! Step 2 asks `WezTerm` to focus the window and the pane of the terminal.

use std::path::Path;
use std::sync::Arc;

use proctree::{Pid, ProcessTree};

use crate::board::Session;
use crate::runner::{Runner, COMMAND_LIMIT};
use crate::state::{ZellijLocation, ZellijPaneId};
use crate::wezterm::TtyWriter;
use crate::{ancestry, lsof, wezterm, zellij};

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
    /// The result of step 2. `Ok` holds the `WezTerm` pane that got the focus
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
            Some(Ok(pane)) => parts.push(format!("Zellij focused pane {}", pane.terminal_target())),
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
/// finds the `WezTerm` pane of the terminal and sends the focus request to the
/// terminal device of that pane.
#[must_use]
pub fn focus(target: &FocusTarget, tools: &FocusTools) -> FocusReport {
    let zellij = target.zellij.as_ref().map(|location| {
        zellij::focus_pane(&*tools.runner, location)
            .map(|()| location.pane)
            .map_err(|error| error.to_string())
    });
    FocusReport {
        label: target.label.clone(),
        zellij,
        wezterm: focus_wezterm(target, tools),
    }
}

/// Runs `argv` and gives its output when the command ran and exited with 0.
///
/// `what` names the command in the error text.
fn run_ok(runner: &dyn Runner, argv: &[String], what: &str) -> Result<String, String> {
    let output = runner
        .run(argv, COMMAND_LIMIT)
        .map_err(|error| format!("{what} failed: {error}"))?;
    if output.succeeded() {
        Ok(output.stdout)
    } else {
        let code = output
            .code
            .map_or_else(|| "a signal".to_string(), |code| code.to_string());
        Err(format!(
            "{what} failed: it exited with {code}: {}",
            output.stderr.trim()
        ))
    }
}

/// Gives the tty of the process `pid`. A failed `lsof` gives `None`, like a
/// process with no tty.
fn tty_of(runner: &dyn Runner, pid: Pid) -> Option<String> {
    run_ok(runner, &lsof::tty_argv(pid), "lsof")
        .ok()
        .and_then(|text| lsof::parse_tty(&text))
}

/// Gives the terminal devices where the focus request can go, in the order
/// of use.
fn candidate_ttys(target: &FocusTarget, runner: &dyn Runner) -> Result<Vec<String>, String> {
    let tree = ProcessTree::parse(&run_ok(runner, &ancestry::ps_argv(), "ps")?);
    if let Some(server) = ancestry::zellij_server_of(&tree, target.pid) {
        let sockets =
            lsof::parse_unix_sockets(&run_ok(runner, &lsof::unix_sockets_argv(), "lsof")?);
        let ttys: Vec<String> = lsof::client_pids(server, &sockets)
            .into_iter()
            .filter_map(|client| tty_of(runner, client))
            .collect();
        if ttys.is_empty() {
            return Err("no Zellij client shows the session".to_string());
        }
        Ok(ttys)
    } else {
        tty_of(runner, target.pid)
            .map(|tty| vec![tty])
            .ok_or_else(|| format!("cannot find the terminal of pid {}", target.pid.as_u32()))
    }
}

/// Step 2: sends the focus request to the first terminal that a `WezTerm` pane
/// shows.
fn focus_wezterm(target: &FocusTarget, tools: &FocusTools) -> Result<u64, String> {
    let ttys = candidate_ttys(target, &*tools.runner)?;
    let listing = run_ok(&*tools.runner, &wezterm::list_argv(), "wezterm cli list")?;
    let panes = wezterm::parse_panes(&listing)
        .map_err(|error| format!("wezterm cli list gave bad JSON: {error}"))?;
    for tty in &ttys {
        let Some(pane) = wezterm::pane_for_tty(&panes, tty) else {
            continue;
        };
        let request = wezterm::focus_request(pane);
        return tools
            .writer
            .write(Path::new(tty), request.as_bytes())
            .map(|()| pane)
            .map_err(|error| format!("cannot write to {tty}: {error}"));
    }
    Err(format!("no WezTerm pane shows {}", ttys.join(", ")))
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
            handled: None,
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
            report(
                Some(Err("the Zellij session tools is gone".to_string())),
                Ok(239)
            )
            .status_line(),
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

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the fake runner answers with a Result"
    )]
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
                focus_argv(&target.zellij.unwrap()),
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

    #[test]
    fn a_focus_outside_zellij_reads_the_tty_of_the_target_and_skips_step_1() {
        let target = FocusTarget::of(&session(51234, Some("docs"), None));
        let runner = Arc::new(
            FakeRunner::new()
                .answer(&words(&ps_argv()), ok(PS))
                .answer(
                    &words(&tty_argv(Pid::new(51234))),
                    ok("p51234\nf0\nn/dev/ttys097\n"),
                )
                .answer(&words(&list_argv()), ok(PANES)),
        );
        let writer = Arc::new(RecordingWriter::default());
        let report = focus(&target, &tools(&runner, &writer));
        assert_eq!(
            runner.calls(),
            vec![ps_argv(), tty_argv(Pid::new(51234)), list_argv()]
        );
        assert_eq!(
            writer.writes(),
            vec![(
                PathBuf::from("/dev/ttys097"),
                focus_request(235).into_bytes()
            )]
        );
        assert_eq!(report.zellij, None);
        assert_eq!(report.wezterm, Ok(235));
        assert_eq!(report.status_line(), "docs: WezTerm focused pane 235");
    }

    #[test]
    fn the_first_client_with_a_wezterm_pane_gets_the_request() {
        // A second client (pid 1000) with no tty, and a third (pid 2000)
        // with a tty that no pane shows, stand before the client 19800.
        let sockets = format!(
            "{SOCKETS}p1000\nf5\nd0xa1\nn->0x692bb8b027c5345a\np2000\nf5\nd0xa2\nn->0x692bb8b027c5345a\n"
        );
        let runner = Arc::new(
            FakeRunner::new()
                .answer(&words(&list_sessions_argv()), ok(SESSIONS))
                .answer(
                    &words(&focus_argv(&zellij_target().zellij.unwrap())),
                    ok(""),
                )
                .answer(&words(&ps_argv()), ok(PS))
                .answer(&words(&unix_sockets_argv()), ok(&sockets))
                .answer(&words(&tty_argv(Pid::new(19800))), ok(TTY))
                .answer(&words(&tty_argv(Pid::new(1000))), ok("p1000\nf0\n"))
                .answer(
                    &words(&tty_argv(Pid::new(2000))),
                    ok("p2000\nf0\nn/dev/ttys999\n"),
                )
                .answer(&words(&list_argv()), ok(PANES)),
        );
        let writer = Arc::new(RecordingWriter::default());
        let report = focus(&zellij_target(), &tools(&runner, &writer));
        assert_eq!(report.wezterm, Ok(239));
        assert_eq!(
            writer.writes(),
            vec![(
                PathBuf::from("/dev/ttys231"),
                focus_request(239).into_bytes()
            )]
        );
    }

    #[test]
    fn a_failed_step_1_does_not_stop_step_2() {
        let runner = Arc::new(
            FakeRunner::new()
                .answer(
                    &words(&list_sessions_argv()),
                    ok("other [Created 1h ago]\n"),
                )
                .answer(&words(&ps_argv()), ok(PS))
                .answer(&words(&unix_sockets_argv()), ok(SOCKETS))
                .answer(&words(&tty_argv(Pid::new(19800))), ok(TTY))
                .answer(&words(&list_argv()), ok(PANES)),
        );
        let writer = Arc::new(RecordingWriter::default());
        let report = focus(&zellij_target(), &tools(&runner, &writer));
        assert_eq!(
            report.zellij,
            Some(Err("the Zellij session tools is gone".to_string()))
        );
        assert_eq!(report.wezterm, Ok(239));
        assert_eq!(writer.writes().len(), 1);
        assert!(!report.succeeded());
        assert_eq!(
            report.status_line(),
            "docs: Zellij failed: the Zellij session tools is gone · WezTerm focused pane 239"
        );
    }

    fn wezterm_error(
        target: &FocusTarget,
        runner: FakeRunner,
    ) -> (String, Vec<(PathBuf, Vec<u8>)>) {
        let runner = Arc::new(runner);
        let writer = Arc::new(RecordingWriter::default());
        let report = focus(target, &tools(&runner, &writer));
        (report.wezterm.unwrap_err(), writer.writes())
    }

    #[test]
    fn a_failed_ps_is_a_step_2_error() {
        let ps_missing = FakeRunner::new()
            .answer(&words(&list_sessions_argv()), ok(SESSIONS))
            .answer(
                &words(&focus_argv(&zellij_target().zellij.unwrap())),
                ok(""),
            )
            .answer(
                &words(&ps_argv()),
                Err(crate::runner::RunError::NotInstalled {
                    program: "ps".to_string(),
                }),
            );
        let (error, writes) = wezterm_error(&zellij_target(), ps_missing);
        assert_eq!(error, "ps failed: ps is not installed");
        assert!(writes.is_empty());
    }

    #[test]
    fn a_server_with_no_client_is_a_step_2_error() {
        let runner = FakeRunner::new()
            .answer(&words(&list_sessions_argv()), ok(SESSIONS))
            .answer(
                &words(&focus_argv(&zellij_target().zellij.unwrap())),
                ok(""),
            )
            .answer(&words(&ps_argv()), ok(PS))
            .answer(
                &words(&unix_sockets_argv()),
                ok("p42617\nf5\nd0x1\nn/path\n"),
            );
        let (error, writes) = wezterm_error(&zellij_target(), runner);
        assert_eq!(error, "no Zellij client shows the session");
        assert!(writes.is_empty());
    }

    #[test]
    fn a_target_with_no_tty_outside_zellij_is_a_step_2_error() {
        let target = FocusTarget::of(&session(51234, Some("docs"), None));
        let runner = FakeRunner::new()
            .answer(&words(&ps_argv()), ok(PS))
            .answer(&words(&tty_argv(Pid::new(51234))), ok("p51234\nf0\n"));
        let (error, writes) = wezterm_error(&target, runner);
        assert_eq!(error, "cannot find the terminal of pid 51234");
        assert!(writes.is_empty());
    }

    /// A runner for a focus in Zellij, with `list` as the answer of
    /// `wezterm cli list`.
    fn runner_with_list(list: Result<CommandOutput, crate::runner::RunError>) -> FakeRunner {
        FakeRunner::new()
            .answer(&words(&list_sessions_argv()), ok(SESSIONS))
            .answer(
                &words(&focus_argv(&zellij_target().zellij.unwrap())),
                ok(""),
            )
            .answer(&words(&ps_argv()), ok(PS))
            .answer(&words(&unix_sockets_argv()), ok(SOCKETS))
            .answer(&words(&tty_argv(Pid::new(19800))), ok(TTY))
            .answer(&words(&list_argv()), list)
    }

    #[test]
    fn a_wezterm_that_is_not_installed_is_a_step_2_error() {
        let runner = runner_with_list(Err(crate::runner::RunError::NotInstalled {
            program: "wezterm".to_string(),
        }));
        let (error, writes) = wezterm_error(&zellij_target(), runner);
        assert_eq!(error, "wezterm cli list failed: wezterm is not installed");
        assert!(writes.is_empty());
    }

    #[test]
    fn a_wezterm_that_exits_with_an_error_is_a_step_2_error() {
        let runner = runner_with_list(Ok(CommandOutput::failed(1, "no mux running\n")));
        let (error, writes) = wezterm_error(&zellij_target(), runner);
        assert_eq!(
            error,
            "wezterm cli list failed: it exited with 1: no mux running"
        );
        assert!(writes.is_empty());
    }

    #[test]
    fn a_wezterm_list_with_bad_json_is_a_step_2_error() {
        let (error, writes) = wezterm_error(&zellij_target(), runner_with_list(ok("not json")));
        assert!(
            error.starts_with("wezterm cli list gave bad JSON: "),
            "{error}"
        );
        assert!(writes.is_empty());
    }

    #[test]
    fn a_tty_that_no_pane_shows_is_a_step_2_error_that_names_the_tty() {
        let runner = FakeRunner::new()
            .answer(&words(&list_sessions_argv()), ok(SESSIONS))
            .answer(
                &words(&focus_argv(&zellij_target().zellij.unwrap())),
                ok(""),
            )
            .answer(&words(&ps_argv()), ok(PS))
            .answer(&words(&unix_sockets_argv()), ok(SOCKETS))
            .answer(
                &words(&tty_argv(Pid::new(19800))),
                ok("p19800\nf0\nn/dev/ttys999\n"),
            )
            .answer(&words(&list_argv()), ok(PANES));
        let (error, writes) = wezterm_error(&zellij_target(), runner);
        assert_eq!(error, "no WezTerm pane shows /dev/ttys999");
        assert!(writes.is_empty());
    }

    #[test]
    fn a_failed_write_is_a_step_2_error_that_names_the_tty() {
        let runner = Arc::new(zellij_runner());
        let writer = Arc::new(RecordingWriter {
            fail: true,
            ..RecordingWriter::default()
        });
        let report = focus(&zellij_target(), &tools(&runner, &writer));
        assert_eq!(
            report.wezterm,
            Err("cannot write to /dev/ttys231: device is busy".to_string())
        );
        assert_eq!(writer.writes().len(), 1);
        assert!(!report.succeeded());
    }
}

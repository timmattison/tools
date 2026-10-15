//! Opens the address of a run in a browser.
//!
//! A user who sets `BROWSER` names the command that opens a web address, and
//! `ha` runs that command. A forwarder such as the opener of mosh-rs sets
//! `BROWSER`, so that the address opens on the computer of the user and not on
//! the host that runs `ha`. When `BROWSER` is not set or is empty, `ha` uses the
//! opener of the system, which is `/usr/bin/open` on macOS.
//!
//! The opener of the system is not sufficient for a forwarder. The `open`
//! crate runs `/usr/bin/open` by its full path, so a shim early in `PATH`
//! never gets the address, and the crate ignores `BROWSER`.
//!
//! `ha` reads `BROWSER` with one rule of `gh` and one rule of `xdg-open`.
//! Neither tool obeys both rules:
//!
//! - The value is one command line, split into words by the rules of a POSIX
//!   shell, as `gh` splits it. A path that holds a space needs quotes.
//!   `xdg-open` splits the value at white space and has no rules for quotes.
//! - Each `%s` in an argument takes the address, as in `xdg-open`. `gh` does
//!   not replace `%s`. When no argument holds `%s`, the address is the last
//!   argument, as in both tools.
//! - A list of commands with `:` between them is not supported.

use std::ffi::OsString;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::app::{OpenFailure, Opener};

/// The environment variable that names the browser command.
pub const BROWSER_VARIABLE: &str = "BROWSER";

/// The text in an argument of the browser command that the address replaces.
const ADDRESS_PLACEHOLDER: &str = "%s";

/// How long after its start a failure of the browser command still shows in
/// the status line.
///
/// A forwarder can need some seconds to report a failure. `mosh-open` of
/// mosh-rs waits up to 15 s, its `ANSWER_DEADLINE`, for the answer of the
/// client, so this window is longer than that. A browser that stays open and
/// fails later says nothing about the open, so `ha` does not report it.
const FAILURE_WINDOW: Duration = Duration::from_secs(20);

/// Opens a web address with the opener of the system.
type SystemOpener = Box<dyn Fn(&str) -> std::io::Result<()>>;

/// The opener that runs the command of `BROWSER`, or else the opener of the
/// system.
pub struct BrowserOpener {
    launcher: Launcher,
    system: SystemOpener,
    failure_window: Duration,
    /// The end that each new browser command gets a copy of.
    failure_sender: Sender<OpenFailure>,
    /// The failures that the browser commands sent.
    failures: Receiver<OpenFailure>,
}

/// What the value of `BROWSER` asks for.
#[derive(Debug)]
enum Launcher {
    /// `BROWSER` is not set, or it holds only white space.
    System,
    /// The browser command, split into words.
    Command {
        program: String,
        arguments: Vec<String>,
    },
    /// `BROWSER` is set, but it is not a command line. The text is the reason.
    Unusable(String),
}

impl Launcher {
    /// Reads the value of `BROWSER`.
    fn from_browser_variable(value: Option<OsString>) -> Self {
        let Some(value) = value else {
            return Launcher::System;
        };
        let Ok(text) = value.into_string() else {
            return Launcher::Unusable(format!("{BROWSER_VARIABLE} is not valid UTF-8"));
        };
        let Some(words) = shlex::split(&text) else {
            return Launcher::Unusable(format!(
                "{BROWSER_VARIABLE} is not a valid command line: {text}"
            ));
        };
        let mut words = words.into_iter();
        match words.next() {
            None => Launcher::System,
            Some(program) => Launcher::Command {
                program,
                arguments: words.collect(),
            },
        }
    }
}

/// Gives the arguments of the browser command for `url`.
fn with_address(arguments: &[String], url: &str) -> Vec<String> {
    if arguments
        .iter()
        .any(|argument| argument.contains(ADDRESS_PLACEHOLDER))
    {
        arguments
            .iter()
            .map(|argument| argument.replace(ADDRESS_PLACEHOLDER, url))
            .collect()
    } else {
        let mut all = arguments.to_vec();
        all.push(url.to_owned());
        all
    }
}

/// Starts the browser command for `url`, and returns without a wait for it
/// to stop.
///
/// The command gets no terminal. Its output goes to the null device, because
/// text that it writes on the terminal breaks the view. A thread waits for
/// the command to stop. When the command fails inside `window`, that thread
/// sends the failure to `failures`.
///
/// # Errors
/// Returns the reason when the command does not start.
fn start(
    program: &str,
    arguments: &[String],
    url: &str,
    window: Duration,
    failures: Sender<OpenFailure>,
) -> Result<(), String> {
    let mut command = Command::new(program);
    command
        .args(with_address(arguments, url))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // A process group of its own takes the command out of the job of `ha`.
    // Thus the shell does not send it the hangup signal when the terminal
    // closes. The `open` crate gets the same result with `setsid` in a double
    // fork, which needs unsafe code.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("{BROWSER_VARIABLE} command {program} did not start: {error}"))?;
    let started = Instant::now();
    let program = program.to_owned();
    let url = url.to_owned();
    // This thread also reaps a browser that stays open, so that it does not
    // become a zombie. A send after the opener is gone has no receiver, and
    // the failure then has nobody to tell.
    thread::spawn(move || {
        let exit = child.wait();
        if let Some(reason) = failure_reason(&program, exit, started.elapsed(), window) {
            let _ = failures.send(OpenFailure { url, reason });
        }
    });
    Ok(())
}

/// Gives the reason to report for the browser command `program`, which
/// stopped with `exit` after it ran for `ran_for`.
///
/// Gives `None` when the command succeeded, and when it ran longer than
/// `window`, because a later failure says nothing about the open.
fn failure_reason(
    program: &str,
    exit: std::io::Result<ExitStatus>,
    ran_for: Duration,
    window: Duration,
) -> Option<String> {
    if ran_for > window {
        return None;
    }
    match exit {
        Ok(status) if status.success() => None,
        Ok(status) => Some(format!(
            "{BROWSER_VARIABLE} command {program} failed: {status}"
        )),
        Err(error) => Some(format!(
            "ha cannot wait for {BROWSER_VARIABLE} command {program}: {error}"
        )),
    }
}

impl BrowserOpener {
    /// Builds the opener from the `BROWSER` variable of this process.
    #[must_use]
    pub fn from_environment() -> Self {
        Self::new(
            std::env::var_os(BROWSER_VARIABLE),
            Box::new(|url| open::that_detached(url)),
            FAILURE_WINDOW,
        )
    }

    /// Builds the opener from the value of `BROWSER`, the opener of the
    /// system, and the time after its start in which a failure of the
    /// browser command is reported.
    fn new(browser: Option<OsString>, system: SystemOpener, failure_window: Duration) -> Self {
        let (failure_sender, failures) = mpsc::channel();
        BrowserOpener {
            launcher: Launcher::from_browser_variable(browser),
            system,
            failure_window,
            failure_sender,
            failures,
        }
    }
}

impl Opener for BrowserOpener {
    fn open(&self, url: &str) -> Result<(), String> {
        match &self.launcher {
            Launcher::System => (self.system)(url).map_err(|error| error.to_string()),
            Launcher::Command { program, arguments } => start(
                program,
                arguments,
                url,
                self.failure_window,
                self.failure_sender.clone(),
            ),
            Launcher::Unusable(reason) => Err(reason.clone()),
        }
    }

    fn late_failure(&self) -> Option<OpenFailure> {
        self.failures.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::path::Path;
    use std::rc::Rc;
    use std::thread;
    use std::time::Instant;

    use super::*;

    /// An address with the characters that a shell treats as special.
    const URL: &str = "https://github.com/o/r/actions/runs/1?check_suite_focus=true&page=2";

    /// A failure window that a short command always stops inside. Thus a
    /// test that waits for the failure of a command never races the clock.
    const WINDOW: Duration = Duration::from_secs(60);

    /// How long a test waits for a command to write a file or to fail.
    const WAIT_LIMIT: Duration = Duration::from_secs(60);

    /// Builds an opener from the value of `BROWSER`. Its system opener
    /// records each address and never starts a real browser.
    fn opener_for(
        browser: Option<OsString>,
        window: Duration,
    ) -> (BrowserOpener, Rc<RefCell<Vec<String>>>) {
        let urls = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&urls);
        let system: SystemOpener = Box::new(move |url| {
            recorded.borrow_mut().push(url.to_owned());
            Ok(())
        });
        (BrowserOpener::new(browser, system, window), urls)
    }

    /// Builds an opener whose `BROWSER` holds `words`, as [`opener_for`] does.
    fn opener(words: &str, window: Duration) -> (BrowserOpener, Rc<RefCell<Vec<String>>>) {
        opener_for(Some(OsString::from(words)), window)
    }

    /// Gives the quoted form of `path` for a shell command line.
    fn quoted(path: &Path) -> String {
        shellquote::shell_quote(&path.to_string_lossy())
    }

    /// Gives a shell command that writes the output of `producer` into
    /// `output` in one step.
    ///
    /// `open` does not wait for the browser command, so a test reads the file
    /// while the command can still run. The command writes a file next to
    /// `output` and then renames it, thus a test that sees `output` sees all
    /// of it.
    fn write_whole(producer: &str, output: &Path) -> String {
        let partial = output.with_extension("partial");
        format!(
            "{producer} > {partial} && mv {partial} {output}\n",
            partial = quoted(&partial),
            output = quoted(output),
        )
    }

    /// Writes a script into `directory` that writes each of its arguments on
    /// one line of `args.txt`. Gives the `BROWSER` words that run the script
    /// and the path of `args.txt`.
    ///
    /// The script runs as an argument of `/bin/sh` and is never run as a
    /// program itself. Thus no other thread of the test binary can hold the
    /// new file open for writing when it starts.
    fn recorder(directory: &Path) -> (String, std::path::PathBuf) {
        let output = directory.join("args.txt");
        let script = directory.join("record.sh");
        fs::write(&script, write_whole("printf '%s\\n' \"$@\"", &output)).unwrap();
        (format!("/bin/sh {}", quoted(&script)), output)
    }

    /// Waits for the arguments that a recorder writes, and reads them.
    fn recorded_arguments(output: &Path) -> Vec<String> {
        assert!(wait_for(output), "the browser command did not run");
        fs::read_to_string(output)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Writes a script into `directory` that waits until `release` exists,
    /// and then runs `last`. Gives the `BROWSER` words that run the script.
    ///
    /// The wait stops after 60 s even when the test never writes `release`,
    /// so a failed test leaves no process that runs forever.
    fn waiting_command(directory: &Path, release: &Path, last: &str) -> String {
        let script = directory.join("wait.sh");
        fs::write(
            &script,
            format!(
                "i=0\n\
                 while [ ! -e {release} ] && [ \"$i\" -lt 600 ]; do sleep 0.1; i=$((i + 1)); done\n\
                 {last}\n",
                release = quoted(release),
            ),
        )
        .unwrap();
        format!("/bin/sh {}", quoted(&script))
    }

    /// Waits for the next late failure of `opener`, for at most
    /// [`WAIT_LIMIT`].
    fn wait_for_failure(opener: &BrowserOpener) -> Option<OpenFailure> {
        let deadline = Instant::now() + WAIT_LIMIT;
        while Instant::now() < deadline {
            if let Some(failure) = opener.late_failure() {
                return Some(failure);
            }
            thread::sleep(Duration::from_millis(20));
        }
        opener.late_failure()
    }

    /// Makes sure that `reason` names `BROWSER` as the cause, and holds
    /// `detail`. A user who reads the status line then knows which setting
    /// to change.
    fn assert_names_browser_and(reason: &str, detail: &str) {
        assert!(reason.contains(BROWSER_VARIABLE), "{reason:?}");
        assert!(reason.contains(detail), "{reason:?}");
    }

    /// Waits until `path` exists, for at most [`WAIT_LIMIT`].
    fn wait_for(path: &Path) -> bool {
        let deadline = Instant::now() + WAIT_LIMIT;
        while Instant::now() < deadline {
            if path.exists() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        path.exists()
    }

    /// Gives the status of a command that exited with `code`.
    #[cfg(unix)]
    fn exit_status(code: i32) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt as _;

        ExitStatus::from_raw(code << 8)
    }

    #[cfg(unix)]
    #[test]
    fn a_browser_command_gets_the_address_as_its_last_argument() {
        let directory = tempfile::tempdir().unwrap();
        let (words, output) = recorder(directory.path());
        let (opener, urls) = opener(&words, WINDOW);

        assert_eq!(opener.open(URL), Ok(()));

        assert_eq!(recorded_arguments(&output), [URL]);
        assert!(urls.borrow().is_empty(), "the system opener also ran");
    }

    #[cfg(unix)]
    #[test]
    fn the_browser_command_is_split_into_words_as_a_shell_splits_it() {
        let directory = tempfile::tempdir().unwrap();
        let (words, output) = recorder(directory.path());
        let (opener, _) = opener(&format!("{words} --new-window 'two words'"), WINDOW);

        assert_eq!(opener.open(URL), Ok(()));

        assert_eq!(
            recorded_arguments(&output),
            ["--new-window", "two words", URL]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_percent_s_in_the_browser_command_takes_the_address() {
        let directory = tempfile::tempdir().unwrap();
        let (words, output) = recorder(directory.path());
        let (opener, _) = opener(&format!("{words} --url=%s"), WINDOW);

        assert_eq!(opener.open(URL), Ok(()));

        assert_eq!(recorded_arguments(&output), [format!("--url={URL}")]);
    }

    #[cfg(unix)]
    #[test]
    fn a_browser_command_that_fails_at_once_is_a_late_failure() {
        let (opener, urls) = opener("/bin/sh -c 'exit 3'", WINDOW);

        assert_eq!(opener.open(URL), Ok(()));

        let failure = wait_for_failure(&opener).expect("no failure arrived");
        assert_eq!(failure.url, URL);
        assert_names_browser_and(&failure.reason, "exit status: 3");
        assert!(urls.borrow().is_empty(), "the system opener also ran");
    }

    #[cfg(unix)]
    #[test]
    fn a_browser_command_that_fails_after_open_returns_is_a_late_failure() {
        let directory = tempfile::tempdir().unwrap();
        let release = directory.path().join("release");
        // As the opener of mosh-rs does, the command waits for an answer, and
        // then it fails.
        let words = waiting_command(directory.path(), &release, "exit 4");
        let (opener, _) = opener(&words, WINDOW);

        assert_eq!(opener.open(URL), Ok(()));

        assert_eq!(
            opener.late_failure(),
            None,
            "a failure arrived before the command stopped"
        );
        fs::write(&release, "").unwrap();
        let failure = wait_for_failure(&opener).expect("no failure arrived");
        assert_eq!(failure.url, URL);
        assert_names_browser_and(&failure.reason, "exit status: 4");
    }

    #[test]
    fn a_browser_command_that_cannot_start_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("no-such-browser");
        let (opener, urls) = opener(&quoted(&missing), WINDOW);

        let reason = opener.open(URL).unwrap_err();

        assert_names_browser_and(&reason, &missing.to_string_lossy());
        assert!(urls.borrow().is_empty(), "the system opener also ran");
    }

    #[test]
    fn a_browser_command_with_an_open_quotation_is_an_error() {
        let (opener, urls) = opener("/bin/sh 'no closing quote", WINDOW);

        let reason = opener.open(URL).unwrap_err();

        assert_names_browser_and(&reason, "'no closing quote");
        assert!(urls.borrow().is_empty(), "the system opener also ran");
    }

    #[cfg(unix)]
    #[test]
    fn a_browser_variable_that_is_not_utf8_is_an_error() {
        use std::os::unix::ffi::OsStringExt;

        let (opener, urls) = opener_for(Some(OsString::from_vec(vec![b'/', 0xff])), WINDOW);

        let reason = opener.open(URL).unwrap_err();

        assert_names_browser_and(&reason, "UTF-8");
        assert!(urls.borrow().is_empty(), "the system opener also ran");
    }

    #[cfg(unix)]
    #[test]
    fn a_browser_command_that_stays_open_does_not_stop_the_view() {
        let directory = tempfile::tempdir().unwrap();
        let release = directory.path().join("release");
        let finished = directory.path().join("finished");
        let words = waiting_command(
            directory.path(),
            &release,
            &format!(": > {}", quoted(&finished)),
        );
        let (opener, urls) = opener(&words, WINDOW);

        assert_eq!(opener.open(URL), Ok(()));

        assert!(urls.borrow().is_empty(), "the system opener also ran");
        assert!(
            !finished.exists(),
            "open waited for the browser command to stop"
        );
        fs::write(&release, "").unwrap();
        assert!(wait_for(&finished), "the browser command did not run");
    }

    #[cfg(unix)]
    #[test]
    fn the_browser_command_runs_in_a_process_group_of_its_own() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("group.txt");
        let script = directory.path().join("group.sh");
        fs::write(
            &script,
            write_whole("echo \"$$ $(ps -o pgid= -p $$)\"", &output),
        )
        .unwrap();
        let (opener, _) = opener(&format!("/bin/sh {}", quoted(&script)), WINDOW);

        assert_eq!(opener.open(URL), Ok(()));

        assert!(wait_for(&output), "the browser command did not run");
        let text = fs::read_to_string(&output).unwrap();
        let ids: Vec<&str> = text.split_whitespace().collect();
        assert_eq!(ids.len(), 2, "unexpected output: {text:?}");
        assert_eq!(ids[0], ids[1], "the process is not the leader of its group");
    }

    #[test]
    fn no_browser_variable_uses_the_opener_of_the_system() {
        let (opener, urls) = opener_for(None, WINDOW);

        assert_eq!(opener.open(URL), Ok(()));

        assert_eq!(*urls.borrow(), [URL]);
    }

    #[test]
    fn an_empty_browser_variable_uses_the_opener_of_the_system() {
        for value in ["", " \t "] {
            let (opener, urls) = opener(value, WINDOW);

            assert_eq!(opener.open(URL), Ok(()), "BROWSER={value:?}");

            assert_eq!(*urls.borrow(), [URL], "BROWSER={value:?}");
        }
    }

    #[test]
    fn a_failure_of_the_system_opener_is_an_error() {
        let system: SystemOpener = Box::new(|_| Err(std::io::Error::other("no browser")));
        let opener = BrowserOpener::new(None, system, WINDOW);

        assert!(opener.open(URL).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_command_that_fails_inside_the_window_gives_a_reason() {
        for ran_for in [Duration::ZERO, WINDOW] {
            let reason = failure_reason("mosh-open", Ok(exit_status(4)), ran_for, WINDOW);

            assert_names_browser_and(
                &reason.expect("no reason"),
                "mosh-open failed: exit status: 4",
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_command_that_fails_after_the_window_gives_no_reason() {
        let late = WINDOW + Duration::from_millis(1);

        assert_eq!(
            failure_reason("firefox", Ok(exit_status(1)), late, WINDOW),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_command_that_succeeds_gives_no_reason() {
        assert_eq!(
            failure_reason("mosh-open", Ok(exit_status(0)), Duration::ZERO, WINDOW),
            None
        );
    }

    #[test]
    fn a_command_that_ha_cannot_wait_for_gives_a_reason() {
        let exit = Err(std::io::Error::other("no child process"));

        let reason = failure_reason("mosh-open", exit, Duration::ZERO, WINDOW);

        assert_names_browser_and(&reason.expect("no reason"), "no child process");
    }
}

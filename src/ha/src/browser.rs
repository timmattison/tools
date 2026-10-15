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
//! `ha` reads `BROWSER` as `gh` and `xdg-open` do:
//!
//! - The value is one command line, split into words by the rules of a POSIX
//!   shell. A path that holds a space needs quotes.
//! - Each `%s` in an argument takes the address. When no argument holds `%s`,
//!   the address is the last argument.
//! - A list of commands with `:` between them is not supported.

use std::ffi::OsString;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use crate::app::Opener;

/// The environment variable that names the browser command.
pub const BROWSER_VARIABLE: &str = "BROWSER";

/// The text in an argument of the browser command that the address replaces.
const ADDRESS_PLACEHOLDER: &str = "%s";

/// How long [`BrowserOpener::open`] waits for the browser command to stop.
///
/// A forwarder stops in a few milliseconds, so its failure shows in the
/// status line. A browser that stays open does not stop, and the view then
/// continues after this time.
const EXIT_GRACE: Duration = Duration::from_millis(500);

/// Opens a web address with the opener of the system.
type SystemOpener = Box<dyn Fn(&str) -> std::io::Result<()>>;

/// The opener that runs the command of `BROWSER`, or else the opener of the
/// system.
pub struct BrowserOpener {
    launcher: Launcher,
    system: SystemOpener,
    exit_grace: Duration,
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

/// Starts the browser command for `url`, and waits up to `exit_grace` for it
/// to stop.
///
/// The command gets no terminal. Its output goes to the null device, because
/// text that it writes on the terminal breaks the view.
///
/// # Errors
/// Returns the reason when the command does not start, or when it stops
/// inside `exit_grace` with a failure.
fn start(
    program: &str,
    arguments: &[String],
    url: &str,
    exit_grace: Duration,
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
    let (sender, receiver) = mpsc::channel();
    // This thread reaps the command also after `start` stops waiting. Thus a
    // browser that stays open does not become a zombie.
    thread::spawn(move || {
        let _ = sender.send(child.wait());
    });
    match receiver.recv_timeout(exit_grace) {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(format!(
            "{BROWSER_VARIABLE} command {program} failed: {status}"
        )),
        Ok(Err(error)) => Err(format!(
            "ha cannot wait for {BROWSER_VARIABLE} command {program}: {error}"
        )),
        Err(RecvTimeoutError::Timeout) => Ok(()),
        Err(RecvTimeoutError::Disconnected) => Err(format!(
            "ha cannot wait for {BROWSER_VARIABLE} command {program}"
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
            EXIT_GRACE,
        )
    }

    /// Builds the opener from the value of `BROWSER`, the opener of the
    /// system, and the time to wait for the browser command to stop.
    fn new(browser: Option<OsString>, system: SystemOpener, exit_grace: Duration) -> Self {
        BrowserOpener {
            launcher: Launcher::from_browser_variable(browser),
            system,
            exit_grace,
        }
    }
}

impl Opener for BrowserOpener {
    fn open(&self, url: &str) -> Result<(), String> {
        match &self.launcher {
            Launcher::System => (self.system)(url).map_err(|error| error.to_string()),
            Launcher::Command { program, arguments } => {
                start(program, arguments, url, self.exit_grace)
            }
            Launcher::Unusable(reason) => Err(reason.clone()),
        }
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

    /// A time to wait that a short command always stops inside. Thus a test
    /// that waits for the end of a command never races the clock.
    const LONG_GRACE: Duration = Duration::from_secs(60);

    /// A time to wait that ends before a blocked command stops.
    const SHORT_GRACE: Duration = Duration::from_millis(100);

    /// How long a test waits for a file that a command writes.
    const FILE_WAIT: Duration = Duration::from_secs(60);

    /// Builds an opener from the value of `BROWSER`. Its system opener
    /// records each address and never starts a real browser.
    fn opener_for(
        browser: Option<OsString>,
        exit_grace: Duration,
    ) -> (BrowserOpener, Rc<RefCell<Vec<String>>>) {
        let urls = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&urls);
        let system: SystemOpener = Box::new(move |url| {
            recorded.borrow_mut().push(url.to_owned());
            Ok(())
        });
        (BrowserOpener::new(browser, system, exit_grace), urls)
    }

    /// Builds an opener whose `BROWSER` holds `words`, as [`opener_for`] does.
    fn opener(words: &str, exit_grace: Duration) -> (BrowserOpener, Rc<RefCell<Vec<String>>>) {
        opener_for(Some(OsString::from(words)), exit_grace)
    }

    /// Gives the quoted form of `path` for a shell command line.
    fn quoted(path: &Path) -> String {
        shellquote::shell_quote(&path.to_string_lossy())
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
        fs::write(
            &script,
            format!("printf '%s\\n' \"$@\" > {}\n", quoted(&output)),
        )
        .unwrap();
        (format!("/bin/sh {}", quoted(&script)), output)
    }

    /// Reads the arguments that a recorder wrote.
    fn recorded_arguments(output: &Path) -> Vec<String> {
        fs::read_to_string(output)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Makes sure that `reason` names `BROWSER` as the cause, and holds
    /// `detail`. A user who reads the status line then knows which setting
    /// to change.
    fn assert_names_browser_and(reason: &str, detail: &str) {
        assert!(reason.contains(BROWSER_VARIABLE), "{reason:?}");
        assert!(reason.contains(detail), "{reason:?}");
    }

    /// Waits until `path` exists, for at most [`FILE_WAIT`].
    fn wait_for(path: &Path) -> bool {
        let deadline = Instant::now() + FILE_WAIT;
        while Instant::now() < deadline {
            if path.exists() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        path.exists()
    }

    #[cfg(unix)]
    #[test]
    fn a_browser_command_gets_the_address_as_its_last_argument() {
        let directory = tempfile::tempdir().unwrap();
        let (words, output) = recorder(directory.path());
        let (opener, urls) = opener(&words, LONG_GRACE);

        assert_eq!(opener.open(URL), Ok(()));

        assert_eq!(recorded_arguments(&output), [URL]);
        assert!(urls.borrow().is_empty(), "the system opener also ran");
    }

    #[cfg(unix)]
    #[test]
    fn the_browser_command_is_split_into_words_as_a_shell_splits_it() {
        let directory = tempfile::tempdir().unwrap();
        let (words, output) = recorder(directory.path());
        let (opener, _) = opener(&format!("{words} --new-window 'two words'"), LONG_GRACE);

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
        let (opener, _) = opener(&format!("{words} --url=%s"), LONG_GRACE);

        assert_eq!(opener.open(URL), Ok(()));

        assert_eq!(recorded_arguments(&output), [format!("--url={URL}")]);
    }

    #[cfg(unix)]
    #[test]
    fn a_browser_command_that_fails_is_an_error() {
        let (opener, urls) = opener("/bin/sh -c 'exit 3'", LONG_GRACE);

        assert!(opener.open(URL).is_err());

        assert!(urls.borrow().is_empty(), "the system opener also ran");
    }

    #[test]
    fn a_browser_command_that_cannot_start_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("no-such-browser");
        let (opener, urls) = opener(&quoted(&missing), LONG_GRACE);

        let reason = opener.open(URL).unwrap_err();

        assert_names_browser_and(&reason, &missing.to_string_lossy());
        assert!(urls.borrow().is_empty(), "the system opener also ran");
    }

    #[test]
    fn a_browser_command_with_an_open_quotation_is_an_error() {
        let (opener, urls) = opener("/bin/sh 'no closing quote", LONG_GRACE);

        let reason = opener.open(URL).unwrap_err();

        assert_names_browser_and(&reason, "'no closing quote");
        assert!(urls.borrow().is_empty(), "the system opener also ran");
    }

    #[cfg(unix)]
    #[test]
    fn a_browser_variable_that_is_not_utf8_is_an_error() {
        use std::os::unix::ffi::OsStringExt;

        let (opener, urls) = opener_for(Some(OsString::from_vec(vec![b'/', 0xff])), LONG_GRACE);

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
        let script = directory.path().join("stay.sh");
        // The loop stops after 60 s even when the test never releases it, so
        // a failed test leaves no process that runs forever.
        fs::write(
            &script,
            format!(
                "i=0\n\
                 while [ ! -e {release} ] && [ \"$i\" -lt 600 ]; do sleep 0.1; i=$((i + 1)); done\n\
                 : > {finished}\n",
                release = quoted(&release),
                finished = quoted(&finished),
            ),
        )
        .unwrap();
        let (opener, urls) = opener(&format!("/bin/sh {}", quoted(&script)), SHORT_GRACE);

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
            format!("echo \"$$ $(ps -o pgid= -p $$)\" > {}\n", quoted(&output)),
        )
        .unwrap();
        let (opener, _) = opener(&format!("/bin/sh {}", quoted(&script)), LONG_GRACE);

        assert_eq!(opener.open(URL), Ok(()));

        let text = fs::read_to_string(&output).unwrap();
        let ids: Vec<&str> = text.split_whitespace().collect();
        assert_eq!(ids.len(), 2, "unexpected output: {text:?}");
        assert_eq!(ids[0], ids[1], "the process is not the leader of its group");
    }

    #[test]
    fn no_browser_variable_uses_the_opener_of_the_system() {
        let (opener, urls) = opener_for(None, LONG_GRACE);

        assert_eq!(opener.open(URL), Ok(()));

        assert_eq!(*urls.borrow(), [URL]);
    }

    #[test]
    fn an_empty_browser_variable_uses_the_opener_of_the_system() {
        for value in ["", " \t "] {
            let (opener, urls) = opener(value, LONG_GRACE);

            assert_eq!(opener.open(URL), Ok(()), "BROWSER={value:?}");

            assert_eq!(*urls.borrow(), [URL], "BROWSER={value:?}");
        }
    }

    #[test]
    fn a_failure_of_the_system_opener_is_an_error() {
        let system: SystemOpener = Box::new(|_| Err(std::io::Error::other("no browser")));
        let opener = BrowserOpener::new(None, system, LONG_GRACE);

        assert!(opener.open(URL).is_err());
    }
}

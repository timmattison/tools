//! Opens the address of a run in a browser.
//!
//! A user who sets `BROWSER` names the command that opens a web address, and
//! `ha` runs that command. A forwarder such as the opener of mosh-rs sets
//! `BROWSER`, so that the address opens on the computer of the user and not on
//! the host that runs `ha`. When `BROWSER` is not set or is empty, `ha` uses the
//! opener of the system, which is `/usr/bin/open` on macOS.

use std::ffi::OsString;
use std::time::Duration;

use crate::app::Opener;

/// The environment variable that names the browser command.
pub const BROWSER_VARIABLE: &str = "BROWSER";

/// How long [`BrowserOpener::open`] waits for the browser command to stop.
const EXIT_GRACE: Duration = Duration::from_millis(500);

/// Opens a web address with the opener of the system.
type SystemOpener = Box<dyn Fn(&str) -> std::io::Result<()>>;

/// The opener that runs the command of `BROWSER`, or else the opener of the
/// system.
pub struct BrowserOpener {
    browser: Option<OsString>,
    system: SystemOpener,
    exit_grace: Duration,
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
            browser,
            system,
            exit_grace,
        }
    }
}

impl Opener for BrowserOpener {
    fn open(&self, url: &str) -> Result<(), String> {
        (self.system)(url).map_err(|error| error.to_string())
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

    /// Builds an opener from `browser`. Its system opener records each
    /// address and never starts a real browser.
    fn opener(
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

    /// Gives the `BROWSER` value of `words`.
    fn browser(words: &str) -> Option<OsString> {
        Some(OsString::from(words))
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
        let (opener, urls) = opener(browser(&words), LONG_GRACE);

        assert_eq!(opener.open(URL), Ok(()));

        assert_eq!(recorded_arguments(&output), [URL]);
        assert!(urls.borrow().is_empty(), "the system opener also ran");
    }

    #[cfg(unix)]
    #[test]
    fn the_browser_command_is_split_into_words_as_a_shell_splits_it() {
        let directory = tempfile::tempdir().unwrap();
        let (words, output) = recorder(directory.path());
        let (opener, _) = opener(
            browser(&format!("{words} --new-window 'two words'")),
            LONG_GRACE,
        );

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
        let (opener, _) = opener(browser(&format!("{words} --url=%s")), LONG_GRACE);

        assert_eq!(opener.open(URL), Ok(()));

        assert_eq!(recorded_arguments(&output), [format!("--url={URL}")]);
    }

    #[cfg(unix)]
    #[test]
    fn a_browser_command_that_fails_is_an_error() {
        let (opener, urls) = opener(browser("/bin/sh -c 'exit 3'"), LONG_GRACE);

        assert!(opener.open(URL).is_err());

        assert!(urls.borrow().is_empty(), "the system opener also ran");
    }

    #[test]
    fn a_browser_command_that_cannot_start_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("no-such-browser");
        let (opener, urls) = opener(browser(&quoted(&missing)), LONG_GRACE);

        assert!(opener.open(URL).is_err());

        assert!(urls.borrow().is_empty(), "the system opener also ran");
    }

    #[test]
    fn a_browser_command_with_an_open_quotation_is_an_error() {
        let (opener, urls) = opener(browser("/bin/sh 'no closing quote"), LONG_GRACE);

        assert!(opener.open(URL).is_err());

        assert!(urls.borrow().is_empty(), "the system opener also ran");
    }

    #[cfg(unix)]
    #[test]
    fn a_browser_variable_that_is_not_utf8_is_an_error() {
        use std::os::unix::ffi::OsStringExt;

        let (opener, urls) = opener(Some(OsString::from_vec(vec![b'/', 0xff])), LONG_GRACE);

        assert!(opener.open(URL).is_err());

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
        let (opener, urls) = opener(browser(&format!("/bin/sh {}", quoted(&script))), SHORT_GRACE);

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
        let (opener, _) = opener(browser(&format!("/bin/sh {}", quoted(&script))), LONG_GRACE);

        assert_eq!(opener.open(URL), Ok(()));

        let text = fs::read_to_string(&output).unwrap();
        let ids: Vec<&str> = text.split_whitespace().collect();
        assert_eq!(ids.len(), 2, "unexpected output: {text:?}");
        assert_eq!(ids[0], ids[1], "the process is not the leader of its group");
    }

    #[test]
    fn no_browser_variable_uses_the_opener_of_the_system() {
        let (opener, urls) = opener(None, LONG_GRACE);

        assert_eq!(opener.open(URL), Ok(()));

        assert_eq!(*urls.borrow(), [URL]);
    }

    #[test]
    fn an_empty_browser_variable_uses_the_opener_of_the_system() {
        for value in ["", " \t "] {
            let (opener, urls) = opener(browser(value), LONG_GRACE);

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

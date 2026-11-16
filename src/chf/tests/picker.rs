//! End-to-end tests of the picker of `chf`, under a pseudo-terminal.
//!
//! ratatui writes only the cells that changed since the last frame, and it
//! skips a cell that equals the cell under it. A text with a space, or a text
//! over equal characters, thus does not arrive as one run of bytes. So each
//! title here has no space, each wait is for one word, and a word that the
//! screen writes a second time is found with `count_of`.
#![cfg(unix)]
// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde_json::json;

/// How long a wait lasts. It bounds a hang. No test asserts a duration.
const PATIENCE: Duration = Duration::from_secs(60);
/// The pause between two looks at the screen.
const GLANCE: Duration = Duration::from_millis(20);
/// The seconds after which the kernel stops the child, whatever the test does.
const ALARM_SECONDS: &str = "120";
/// The Perl program that sets the alarm and then replaces itself with chf.
const ALARM_WRAPPER: &str = "alarm(shift @ARGV); exec { $ARGV[0] } @ARGV or die \"exec: $!\";";
/// The sequence that leaves the alternate screen.
const LEAVE_SCREEN: &str = "\x1b[?1049l";
/// The text that the fake `crap` prints.
const FAKE_RAN: &str = "FAKE-CRAP-RAN";
/// The id of the session of each fixture.
const ID: &str = "11111111-2222-4333-8444-555555555555";
/// The exit status of the fake `crap`.
const FAKE_STATUS: u32 = 7;
/// The file in which the fake `crap` of the environment test writes what it got.
const CRAP_ENV_FILE: &str = "crap-env";
/// What the fake `crap` writes for a variable that its environment does not hold.
const UNSET: &str = "unset";
/// A variable that git exports into each hook. It carries the intent of the hook.
const HOOK_VARIABLE: &str = "GIT_DIR";
/// A variable that a person exports. It carries the intent of the user.
const USER_VARIABLE: &str = "GIT_TERMINAL_PROMPT";
/// The value that the test gives to [`USER_VARIABLE`].
const USER_VALUE: &str = "0";

/// Writes one session into `home`. A compacted session adds a boundary.
fn write_session(home: &Path, title: &str, compacted: bool) {
    let folder = home.join(".claude/projects/-tmp-proj");
    std::fs::create_dir_all(&folder).expect("the project folder is made");
    let mut records = vec![
        json!({"type": "ai-title", "aiTitle": title}),
        json!({"type": "user", "cwd": "/tmp/proj", "timestamp": "2026-01-01T00:00:00Z",
               "message": {"role": "user", "content": "hello"}}),
        json!({"type": "assistant", "timestamp": "2026-01-01T00:00:01Z",
               "message": {"role": "assistant", "content": [{"type": "text", "text": "world"}]}}),
    ];
    if compacted {
        records.push(
            json!({"type": "system", "subtype": "compact_boundary", "uuid": "b1",
            "timestamp": "2026-01-01T00:00:02Z", "compactMetadata": {"preTokens": 150_000}}),
        );
        records.push(json!({"type": "user", "isCompactSummary": true,
            "timestamp": "2026-01-01T00:00:03Z",
            "message": {"role": "user", "content": "summary"}}));
        records.push(json!({"type": "user", "timestamp": "2026-01-01T00:00:04Z",
            "message": {"role": "user", "content": "after"}}));
    }
    let text = records
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(folder.join(format!("{ID}.jsonl")), text).expect("the transcript is written");
    std::fs::write(
        home.join(".bashrc"),
        format!(
            "crap() {{ printf '%s\\n' \"$@\" > \"$HOME/crap-args\"; printf '{FAKE_RAN}\\n'; return {FAKE_STATUS}; }}\n"
        ),
    )
    .expect("the bashrc is written");
}

/// One run of `chf` under a pseudo-terminal.
struct Picker {
    _master: Box<dyn MasterPty + Send>,
    keys: Box<dyn Write + Send>,
    shown: Arc<Mutex<Vec<u8>>>,
    closed: Arc<AtomicBool>,
    child: Box<dyn Child + Send + Sync>,
}

impl Picker {
    /// Starts `chf` with `shell` as `$SHELL` and `home` as the home directory.
    fn open(home: &Path, shell: &str) -> Self {
        Self::open_with(home, shell, &[])
    }

    /// Starts `chf` as [`Picker::open`] does, with each pair of `extra_env`
    /// added to the environment of `chf`.
    fn open_with(home: &Path, shell: &str, extra_env: &[(&str, &OsStr)]) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("the pseudo-terminal opens");
        let mut command = CommandBuilder::new("perl");
        command.args([
            "-e",
            ALARM_WRAPPER,
            ALARM_SECONDS,
            env!("CARGO_BIN_EXE_chf"),
        ]);
        command.env_clear();
        command.env("HOME", home);
        command.env("PATH", "/usr/bin:/bin");
        command.env("TERM", "xterm-256color");
        command.env("SHELL", shell);
        command.env("XDG_CACHE_HOME", home.join("cache"));
        command.env("BASH_SILENCE_DEPRECATION_WARNING", "1");
        for (key, value) in extra_env {
            command.env(key, value);
        }
        command.cwd(home);
        let child = pair
            .slave
            .spawn_command(command)
            .expect("chf starts under the pseudo-terminal");
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("the pseudo-terminal gives a reader");
        let keys = pair
            .master
            .take_writer()
            .expect("the pseudo-terminal gives a writer");
        let shown = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let filled = Arc::clone(&shown);
        let ended = Arc::clone(&closed);
        thread::spawn(move || {
            let mut chunk = [0_u8; 4096];
            while let Ok(read) = reader.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                filled
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend_from_slice(&chunk[..read]);
            }
            ended.store(true, Ordering::SeqCst);
        });
        Self {
            _master: pair.master,
            keys,
            shown,
            closed,
            child,
        }
    }

    /// Every byte that the terminal showed so far, as text.
    fn shown(&self) -> String {
        String::from_utf8_lossy(&self.shown.lock().unwrap_or_else(PoisonError::into_inner))
            .into_owned()
    }

    /// How many times `text` occurs in what the terminal showed.
    fn count_of(&self, text: &str) -> usize {
        self.shown().matches(text).count()
    }

    /// Waits until `text` occurs at least `times` times.
    fn wait_for_count(&self, text: &str, times: usize) {
        let deadline = Instant::now() + PATIENCE;
        while self.count_of(text) < times {
            assert!(
                Instant::now() < deadline,
                "the screen must show {text:?} {times} times, and it showed {:?}",
                self.shown()
            );
            thread::sleep(GLANCE);
        }
    }

    /// Waits until the terminal shows `text`.
    fn wait_for(&self, text: &str) {
        self.wait_for_count(text, 1);
    }

    /// Writes bytes into the terminal as keys.
    fn press(&mut self, bytes: &[u8]) {
        self.keys
            .write_all(bytes)
            .expect("the pseudo-terminal takes the keys");
        self.keys.flush().expect("the keys reach chf");
    }

    /// Waits until the child ends, then until the terminal closes.
    fn wait_for_exit(&mut self) -> u32 {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                while !self.closed.load(Ordering::SeqCst) && Instant::now() < deadline {
                    thread::sleep(GLANCE);
                }
                return status.exit_code();
            }
            assert!(
                Instant::now() < deadline,
                "chf must stop, and the screen showed {:?}",
                self.shown()
            );
            thread::sleep(GLANCE);
        }
    }
}

impl Drop for Picker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The lines of the file that the fake `crap` wrote.
fn crap_args(home: &Path) -> Vec<String> {
    std::fs::read_to_string(home.join("crap-args"))
        .expect("the fake crap wrote its arguments")
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn enter_on_a_plain_session_runs_ch() {
    let home = tempfile::tempdir().expect("a temporary directory");
    write_session(home.path(), "Zanzibar-trip-notes", false);
    let mut picker = Picker::open(home.path(), "/bin/bash");
    picker.wait_for("Zanzibar-trip-notes");
    picker.press(b"\r");
    assert_eq!(picker.wait_for_exit(), FAKE_STATUS);
    assert_eq!(crap_args(home.path()), ["--here", ID]);
    let shown = picker.shown();
    let left = shown
        .find(LEAVE_SCREEN)
        .expect("chf leaves the alternate screen");
    let ran = shown.find(FAKE_RAN).expect("the fake crap ran");
    assert!(
        left < ran,
        "chf must restore the terminal before the resume"
    );
}

#[test]
fn enter_on_a_compacted_session_asks_and_e_runs_che() {
    let home = tempfile::tempdir().expect("a temporary directory");
    write_session(home.path(), "Quokka-notes", true);
    let mut picker = Picker::open(home.path(), "/bin/bash");
    picker.wait_for("Quokka-notes");
    picker.press(b"\r");
    picker.wait_for("expanded:");
    picker.press(b"e");
    assert_eq!(picker.wait_for_exit(), FAKE_STATUS);
    assert_eq!(crap_args(home.path()), ["--here", "--expand", ID]);
}

#[test]
fn escape_exits_130_and_resumes_nothing() {
    let home = tempfile::tempdir().expect("a temporary directory");
    write_session(home.path(), "Zanzibar-trip-notes", false);
    let mut picker = Picker::open(home.path(), "/bin/bash");
    picker.wait_for("Zanzibar-trip-notes");
    picker.press(b"\x1b");
    assert_eq!(picker.wait_for_exit(), 130);
    assert!(picker.shown().contains(LEAVE_SCREEN));
    assert!(!home.path().join("crap-args").exists());
}

#[test]
fn a_word_that_matches_nothing_empties_the_list_and_ctrl_u_restores_it() {
    let home = tempfile::tempdir().expect("a temporary directory");
    write_session(home.path(), "Zanzibar-trip-notes", false);
    let mut picker = Picker::open(home.path(), "/bin/bash");
    picker.wait_for("Zanzibar-trip-notes");
    // The wait is for one more than the screen showed so far. Text that an
    // earlier frame drew cannot satisfy it.
    let empty_before = picker.count_of("matches");
    let title_before = picker.count_of("Zanzibar-trip-notes");
    picker.press(b"nomatchword");
    picker.wait_for_count("matches", empty_before + 1);
    picker.press(b"\x15");
    picker.wait_for_count("Zanzibar-trip-notes", title_before + 1);
    picker.press(b"\r");
    assert_eq!(picker.wait_for_exit(), FAKE_STATUS);
    assert_eq!(crap_args(home.path()), ["--here", ID]);
}

#[test]
fn the_shell_sheds_the_git_variables_of_a_hook_and_keeps_those_of_the_user() {
    let home = tempfile::tempdir().expect("a temporary directory");
    write_session(home.path(), "Zanzibar-trip-notes", false);
    std::fs::write(
        home.path().join(".bashrc"),
        format!(
            "crap() {{ printf '%s\\n' \"${{{HOOK_VARIABLE}-{UNSET}}}\" \"${{{USER_VARIABLE}-{UNSET}}}\" > \"$HOME/{CRAP_ENV_FILE}\"; return {FAKE_STATUS}; }}\n"
        ),
    )
    .expect("the bashrc is written");
    let decoy = home.path().join("decoy.git");
    let mut picker = Picker::open_with(
        home.path(),
        "/bin/bash",
        &[
            (HOOK_VARIABLE, decoy.as_os_str()),
            (USER_VARIABLE, OsStr::new(USER_VALUE)),
        ],
    );
    picker.wait_for("Zanzibar-trip-notes");
    picker.press(b"\r");
    assert_eq!(picker.wait_for_exit(), FAKE_STATUS);
    let seen = std::fs::read_to_string(home.path().join(CRAP_ENV_FILE))
        .expect("the fake crap wrote its environment");
    assert_eq!(
        seen.lines().collect::<Vec<_>>(),
        [UNSET, USER_VALUE],
        "crap must not get {HOOK_VARIABLE}, and must get {USER_VARIABLE}"
    );
}

#[test]
fn an_unsupported_shell_prints_the_command_to_paste() {
    let home = tempfile::tempdir().expect("a temporary directory");
    write_session(home.path(), "Zanzibar-trip-notes", false);
    let mut picker = Picker::open(home.path(), "/bin/sh");
    picker.wait_for("Zanzibar-trip-notes");
    picker.press(b"\r");
    assert_eq!(picker.wait_for_exit(), 1);
    let shown = picker.shown();
    assert!(shown.contains("Paste"), "shown: {shown:?}");
    assert!(
        shown.contains(&format!("crap --here '{ID}'")),
        "shown: {shown:?}"
    );
}

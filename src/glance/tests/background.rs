#![cfg(target_os = "macos")]
#![deny(unsafe_code)]
//! Tests of the status item that one run of `glance` starts and the next run
//! stops.
//!
//! Each test gives `glance` its own home directory. Thus the lock file of a
//! test is never the lock file of a `glance` that a person runs, and a test
//! never stops that `glance`. Each start also gets `--exit-after`, so a status
//! item that a failed test leaves behind ends by itself.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// The life of each status item that a test starts, in seconds after it is
/// up. A test stops its status item long before this bound. The bound only
/// ends a status item that a failed test left behind.
const LIFE_BOUND: &str = "120";

/// The short life that the test of the bound gives, in seconds.
const SHORT_LIFE: &str = "1";

/// The longest time that the test of the bound waits for the end.
const SHORT_LIFE_DEADLINE: Duration = Duration::from_secs(60);

/// The time between two looks at a process that must end.
const POLL: Duration = Duration::from_millis(100);

/// The path of the lock file below the home directory.
const LOCK_FILE: &str = "Library/Application Support/glance/glance.lock";

/// The text before the PID in the line that a status item writes when it is
/// up.
const READY_PREFIX: &str = "glance: the status item runs as PID ";

/// The text after the PID in the line that a status item writes when it is
/// up.
const READY_SUFFIX: &str = ". Run glance again to stop it.\n";

/// The text before the PID in the line that a stop writes.
const STOPPED_PREFIX: &str = "glance: stopped the status item that ran as PID ";

/// The text after the PID in the line that a stop writes.
const STOPPED_SUFFIX: &str = ".\n";

/// The exit code that clap gives for a usage error.
const USAGE_ERROR: i32 = 2;

/// A home directory for one test, and the status items that the test started.
///
/// A drop stops each status item that still runs, so a test that fails early
/// leaves no status item in the menu bar.
struct Home {
    dir: TempDir,
    started: Vec<u32>,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("glance-home-")
            .tempdir()
            .expect("make a home directory");
        Self {
            dir,
            started: Vec::new(),
        }
    }

    /// A command that runs `glance` with `args` in this home directory.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_glance"));
        command
            .env("HOME", self.dir.path())
            .args(args)
            .stdin(Stdio::null());
        command
    }

    /// Runs `glance` with `args` and the life bound of a test.
    fn run(&self, args: &[&str]) -> Output {
        self.run_bounded(args, LIFE_BOUND)
    }

    /// Runs `glance` with `args` and a life bound of `seconds`.
    fn run_bounded(&self, args: &[&str], seconds: &str) -> Output {
        self.command(args)
            .args(["--exit-after", seconds])
            .output()
            .expect("the glance binary starts")
    }

    /// Starts a status item in the background with a life bound of
    /// `seconds`, and returns its PID.
    fn start_in_background_bounded(&mut self, seconds: &str) -> u32 {
        // `output` waits until the start closes its stdout and its stderr.
        // A status item that kept them open would keep the test here until
        // its bound, and the test would then find it gone.
        let output = self.run_bounded(&["--background"], seconds);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
        let pid = ready_pid(&stdout);
        self.started.push(pid);
        pid
    }

    /// Starts a status item in the background and returns its PID.
    fn start_in_background(&mut self) -> u32 {
        self.start_in_background_bounded(LIFE_BOUND)
    }

    /// The lock file of `glance` in this home directory.
    fn lock_file(&self) -> PathBuf {
        self.dir.path().join(LOCK_FILE)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        for pid in &self.started {
            if alive(*pid) {
                let _ = childenv::fixture_command("kill")
                    .args(["-TERM", &pid.to_string()])
                    .status();
            }
        }
    }
}

/// The PID in the line that a status item writes when it is up.
fn ready_pid(line: &str) -> u32 {
    line.strip_prefix(READY_PREFIX)
        .and_then(|rest| rest.strip_suffix(READY_SUFFIX))
        .and_then(|pid| pid.parse().ok())
        .unwrap_or_else(|| panic!("not the line of a status item that is up: {line:?}"))
}

/// The line that a stop of the status item with `pid` writes.
fn stopped_line(pid: u32) -> String {
    format!("{STOPPED_PREFIX}{pid}{STOPPED_SUFFIX}")
}

/// The fields `keyword` of the process `pid`, or `None` when `ps` finds no
/// such process.
fn ps_field(pid: u32, keyword: &str) -> Option<String> {
    let output = childenv::fixture_command("ps")
        .args(["-o", &format!("{keyword}="), "-p", &pid.to_string()])
        .output()
        .expect("ps starts");
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !text.is_empty()).then_some(text)
}

/// Whether the process `pid` runs. A zombie does not run: it ended, and its
/// parent did not collect its status yet.
fn alive(pid: u32) -> bool {
    ps_field(pid, "stat").is_some_and(|state| !state.starts_with('Z'))
}

#[test]
fn a_background_start_returns_and_leaves_the_status_item_running() {
    let mut home = Home::new();
    let pid = home.start_in_background();
    assert!(alive(pid), "the status item {pid} does not run");
    assert!(
        home.lock_file().is_file(),
        "no lock file at {}",
        home.lock_file().display()
    );
}

#[test]
fn the_background_status_item_leads_its_own_process_group() {
    let mut home = Home::new();
    let pid = home.start_in_background();
    let group = ps_field(pid, "pgid").expect("the status item runs");
    assert_eq!(group, pid.to_string(), "the process group of {pid}");
}

#[test]
fn a_second_background_start_stops_the_status_item() {
    let mut home = Home::new();
    let pid = home.start_in_background();
    let output = home.run(&["--background"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), stopped_line(pid));
    assert!(!alive(pid), "the status item {pid} still runs");
}

#[test]
fn a_plain_start_stops_the_background_status_item() {
    let mut home = Home::new();
    let pid = home.start_in_background();
    let output = home.run(&[]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), stopped_line(pid));
    assert!(!alive(pid), "the status item {pid} still runs");
}

#[test]
fn a_start_after_a_stop_starts_a_new_status_item() {
    let mut home = Home::new();
    let first = home.start_in_background();
    let stop = home.run(&["--background"]);
    assert_eq!(String::from_utf8_lossy(&stop.stdout), stopped_line(first));
    let second = home.start_in_background();
    assert_ne!(first, second);
    assert!(alive(second), "the status item {second} does not run");
}

#[test]
fn a_sample_leaves_the_status_item_running() {
    let mut home = Home::new();
    let pid = home.start_in_background();
    let output = home.run(&["--sample", "--interval", "0.25"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    serde_json::from_str::<serde_json::Value>(&stdout).expect("stdout is JSON");
    assert!(alive(pid), "the sample stopped the status item {pid}");
}

#[test]
fn background_and_sample_together_are_a_usage_error() {
    let home = Home::new();
    let output = home.run(&["--background", "--sample"]);
    assert_eq!(output.status.code(), Some(USAGE_ERROR), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

#[test]
fn a_stop_ends_a_foreground_status_item_with_status_zero() {
    let mut home = Home::new();
    let mut child = home
        .command(&["--exit-after", LIFE_BOUND])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the glance binary starts");
    home.started.push(child.id());
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("the stdout of the status item"))
        .read_line(&mut line)
        .expect("read the stdout of the status item");
    assert_eq!(ready_pid(&line), child.id());

    let output = home.run(&[]);
    assert_eq!(String::from_utf8_lossy(&output.stdout), stopped_line(child.id()));
    let status = child.wait().expect("wait for the status item");
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[test]
fn the_life_bound_ends_a_background_status_item() {
    let mut home = Home::new();
    let pid = home.start_in_background_bounded(SHORT_LIFE);
    let deadline = Instant::now() + SHORT_LIFE_DEADLINE;
    while alive(pid) && Instant::now() < deadline {
        thread::sleep(POLL);
    }
    assert!(!alive(pid), "the status item {pid} outlived its bound");
}

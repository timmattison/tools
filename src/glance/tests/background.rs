#![cfg(target_os = "macos")]
#![deny(unsafe_code)]
//! Tests of the status item that one run of `glance` starts and the next run
//! stops.
//!
//! Each target has its own status item: the local Mac, and each destination
//! of `--host`. A run stops the status item of its own target only.
//!
//! Each test gives `glance` its own home directory. Thus the lock files of a
//! test are never the lock files of a `glance` that a person runs, and a test
//! never stops that `glance`. Each test also puts a fake `ssh` first on the
//! `PATH` of `glance`, so a status item of a host never starts the real
//! `ssh`. Each start gets `--exit-after`, so a status item that a failed test
//! leaves behind ends by itself.

use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
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

/// The directory of the lock files below the home directory.
const LOCK_DIRECTORY: &str = "Library/Application Support/glance";

/// The name of the lock file of the local status item.
const LOCAL_LOCK_FILE: &str = "local.lock";

/// The name of the directory of the fake `ssh` in the home directory.
const FAKE_BIN: &str = "fake-bin";

/// A fake `ssh` that fails at once. The status item then shows the error
/// line, and it tries again after its backoff.
const FAKE_SSH: &str = "#!/bin/sh\nexit 255\n";

/// The directories after the fake `ssh` on the `PATH` of `glance`.
const SYSTEM_PATH: &str = "/usr/bin:/bin";

/// The text before the PID in each line that names a status item.
const PID_MARK: &str = " as PID ";

/// The exit code that clap gives for a usage error.
const USAGE_ERROR: i32 = 2;

/// The first two destinations of `--host` in these tests.
const ALPHA: &str = "alpha";
const BETA: &str = "tim@beta";

/// The status item that a run starts or stops.
#[derive(Debug, Clone, Copy)]
enum Target {
    /// The status item of this Mac.
    Local,
    /// The status item of the `--host` destination.
    Host(&'static str),
}

impl Target {
    /// The arguments that name this target.
    fn args(self) -> Vec<&'static str> {
        match self {
            Target::Local => Vec::new(),
            Target::Host(destination) => vec!["--host", destination],
        }
    }

    /// The line that the status item of this target writes when it is up.
    fn ready_line(self, pid: u32) -> String {
        match self {
            Target::Local => {
                format!("glance: the status item runs as PID {pid}. Run glance again to stop it.\n")
            }
            Target::Host(destination) => format!(
                "glance: the status item of the host {destination} runs as PID {pid}. \
                 Run glance again with the same --host to stop it.\n"
            ),
        }
    }

    /// The line that a stop of the status item of this target writes.
    fn stopped_line(self, pid: u32) -> String {
        match self {
            Target::Local => format!("glance: stopped the status item that ran as PID {pid}.\n"),
            Target::Host(destination) => format!(
                "glance: stopped the status item of the host {destination} that ran as PID {pid}.\n"
            ),
        }
    }
}

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
        let bin = dir.path().join(FAKE_BIN);
        fs::create_dir(&bin).expect("make the directory of the fake ssh");
        let ssh = bin.join("ssh");
        fs::write(&ssh, FAKE_SSH).expect("write the fake ssh");
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755))
            .expect("make the fake ssh executable");
        Self {
            dir,
            started: Vec::new(),
        }
    }

    /// A command that runs `glance` with `args` in this home directory.
    fn command(&self, args: &[&str]) -> Command {
        let path = format!("{}:{SYSTEM_PATH}", self.dir.path().join(FAKE_BIN).display());
        let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_glance"));
        command
            .env("HOME", self.dir.path())
            .env("PATH", path)
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

    /// Runs `glance` for `target` with `extra` arguments after the target.
    fn run_target(&self, target: Target, extra: &[&str]) -> Output {
        let mut args = target.args();
        args.extend_from_slice(extra);
        self.run(&args)
    }

    /// Starts the status item of `target` in the background with a life
    /// bound of `seconds`, and returns its PID.
    fn start_bounded(&mut self, target: Target, seconds: &str) -> u32 {
        let mut args = target.args();
        args.push("--background");
        // `output` waits until the start closes its stdout and its stderr.
        // A status item that kept them open would keep the test here until
        // its bound, and the test would then find it gone.
        let output = self.run_bounded(&args, seconds);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
        let pid = named_pid(&stdout);
        self.started.push(pid);
        assert_eq!(stdout, target.ready_line(pid));
        pid
    }

    /// Starts the status item of `target` in the background and returns its
    /// PID.
    fn start(&mut self, target: Target) -> u32 {
        self.start_bounded(target, LIFE_BOUND)
    }

    /// Runs `glance` for `target` with `extra` arguments, and asserts that
    /// the run stopped the status item `pid` of that target.
    fn assert_stops(&self, target: Target, extra: &[&str], pid: u32) {
        let output = self.run_target(target, extra);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            target.stopped_line(pid)
        );
        assert!(!alive(pid), "the status item {pid} still runs");
    }

    /// The path of the lock file `name` in this home directory.
    fn lock_file(&self, name: &str) -> PathBuf {
        self.dir.path().join(LOCK_DIRECTORY).join(name)
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

/// The PID in a line that names a status item.
fn named_pid(line: &str) -> u32 {
    line.split_once(PID_MARK)
        .and_then(|(_, rest)| rest.split_once('.'))
        .and_then(|(pid, _)| pid.parse().ok())
        .unwrap_or_else(|| panic!("the line names no status item: {line:?}"))
}

/// The field `keyword` of the process `pid`, or `None` when `ps` finds no
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
    let pid = home.start(Target::Local);
    assert!(alive(pid), "the status item {pid} does not run");
    let lock = home.lock_file(LOCAL_LOCK_FILE);
    assert!(lock.is_file(), "no lock file at {}", lock.display());
}

#[test]
fn the_background_status_item_leads_its_own_process_group() {
    let mut home = Home::new();
    let pid = home.start(Target::Local);
    let group = ps_field(pid, "pgid").expect("the status item runs");
    assert_eq!(group, pid.to_string(), "the process group of {pid}");
}

#[test]
fn a_second_background_start_stops_the_status_item() {
    let mut home = Home::new();
    let pid = home.start(Target::Local);
    home.assert_stops(Target::Local, &["--background"], pid);
}

#[test]
fn a_plain_start_stops_the_background_status_item() {
    let mut home = Home::new();
    let pid = home.start(Target::Local);
    home.assert_stops(Target::Local, &[], pid);
}

#[test]
fn a_start_after_a_stop_starts_a_new_status_item() {
    let mut home = Home::new();
    let first = home.start(Target::Local);
    home.assert_stops(Target::Local, &["--background"], first);
    let second = home.start(Target::Local);
    assert_ne!(first, second);
    assert!(alive(second), "the status item {second} does not run");
}

#[test]
fn a_sample_leaves_the_status_item_running() {
    let mut home = Home::new();
    let pid = home.start(Target::Local);
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
    assert_eq!(line, Target::Local.ready_line(child.id()));

    let output = home.run(&[]);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        Target::Local.stopped_line(child.id())
    );
    let status = child.wait().expect("wait for the status item");
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[test]
fn the_life_bound_ends_a_background_status_item() {
    let mut home = Home::new();
    let pid = home.start_bounded(Target::Local, SHORT_LIFE);
    let deadline = Instant::now() + SHORT_LIFE_DEADLINE;
    while alive(pid) && Instant::now() < deadline {
        thread::sleep(POLL);
    }
    assert!(!alive(pid), "the status item {pid} outlived its bound");
}

#[test]
fn a_host_status_item_runs_beside_the_local_status_item() {
    let mut home = Home::new();
    let local = home.start(Target::Local);
    let host = home.start(Target::Host(ALPHA));
    assert_ne!(local, host);
    assert!(alive(local), "the local status item {local} does not run");
    assert!(alive(host), "the status item {host} of {ALPHA} does not run");
}

#[test]
fn a_second_start_of_a_host_stops_only_the_status_item_of_that_host() {
    let mut home = Home::new();
    let local = home.start(Target::Local);
    let alpha = home.start(Target::Host(ALPHA));
    let beta = home.start(Target::Host(BETA));
    home.assert_stops(Target::Host(ALPHA), &["--background"], alpha);
    assert!(alive(local), "the stop of {ALPHA} stopped the local status item");
    assert!(alive(beta), "the stop of {ALPHA} stopped the status item of {BETA}");
}

#[test]
fn a_plain_start_of_a_host_stops_the_status_item_of_that_host() {
    let mut home = Home::new();
    let beta = home.start(Target::Host(BETA));
    home.assert_stops(Target::Host(BETA), &[], beta);
}

#[test]
fn a_local_start_stops_only_the_local_status_item() {
    let mut home = Home::new();
    let alpha = home.start(Target::Host(ALPHA));
    let local = home.start(Target::Local);
    home.assert_stops(Target::Local, &["--background"], local);
    assert!(alive(alpha), "the local stop stopped the status item of {ALPHA}");
}

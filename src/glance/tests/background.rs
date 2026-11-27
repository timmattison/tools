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
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::Barrier;
use std::thread;
use std::time::{Duration, Instant};

use gitscratch::testing::executable::write_executable;
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

/// The first part of the line that a copy writes when another status item
/// of its target holds the lock.
const ANOTHER_STARTED_MARK: &str =
    "glance: another glance started the same status item at the same time, as PID ";

/// The first part of the last line that a start writes when its copy ends
/// before the status item is up.
const COPY_STOPPED_MARK: &str =
    "glance: the background copy stopped before its status item was up (";

/// The exit code that clap gives for a usage error.
const USAGE_ERROR: i32 = 2;

/// The first two destinations of `--host` in these tests.
const ALPHA: &str = "alpha";
const BETA: &str = "tim@beta";

/// The flag that shows the name that the second Mac reports.
const RESOLVE: &str = "--resolve";

/// The flag that keeps the status item in the process of the run.
const FOREGROUND: &str = "--foreground";

/// The short form of [`FOREGROUND`].
const FOREGROUND_SHORT: &str = "-f";

/// The flags that asked for a background start before it was the default.
const REMOVED_BACKGROUND_FLAGS: [&str; 2] = ["-b", "--background"];

/// Near misses of [`REMOVED_BACKGROUND_FLAGS`]. Clap suggests a flag for a
/// typo that is close to it, so a refusal of such a typo must not name the
/// hidden flag of the copy either.
const NEAR_MISSES_OF_BACKGROUND: [&str; 1] = ["--backgroun"];

/// The variable that makes clap paint its usage errors.
const FORCE_PAINT_VARIABLE: &str = "CLICOLOR_FORCE";

/// The value that turns the paint on.
const FORCE_PAINT_ON: &str = "1";

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
        write_executable(&ssh, FAKE_SSH).expect("write the fake ssh");
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

    /// Runs `glance` with `args`, asserts that clap refuses them, and gives
    /// the visible glyphs of the refusal.
    ///
    /// The paint is always on, so an assertion that skips the glyphs fails
    /// in every run. The life bound is short, so a run that starts a status
    /// item by mistake does not leave it in the menu bar for long.
    fn usage_error(&self, args: &[&str]) -> String {
        let output = self
            .command(args)
            .env(FORCE_PAINT_VARIABLE, FORCE_PAINT_ON)
            .args(["--exit-after", SHORT_LIFE])
            .output()
            .expect("the glance binary starts");
        assert_eq!(
            output.status.code(),
            Some(USAGE_ERROR),
            "{args:?} {output:?}"
        );
        testcolor::strip_ansi(&String::from_utf8_lossy(&output.stderr))
    }

    /// Runs `glance` for `target` with `extra` arguments after the target.
    fn run_target(&self, target: Target, extra: &[&str]) -> Output {
        let mut args = target.args();
        args.extend_from_slice(extra);
        self.run(&args)
    }

    /// Starts the status item of `target` in the background with the
    /// arguments `extra` and a life bound of `seconds`, and returns its PID.
    /// A run with no `--foreground` starts the status item in the background.
    fn start_bounded(&mut self, target: Target, extra: &[&str], seconds: &str) -> u32 {
        let mut args = target.args();
        args.extend_from_slice(extra);
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
        self.start_bounded(target, &[], LIFE_BOUND)
    }

    /// Starts the status item of `target` in the background with the
    /// arguments `extra` after the target, and returns its PID.
    fn start_with(&mut self, target: Target, extra: &[&str]) -> u32 {
        self.start_bounded(target, extra, LIFE_BOUND)
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

/// Whether the process `pid` runs.
///
/// A process that exits does not run. The kernel marks it `E` before it
/// closes its files, and a close of the lock file is what ends a stop. Thus
/// `ps` can still find the process when the stop returns. A zombie (`Z`)
/// does not run either: it ended, and its parent did not collect its status
/// yet.
fn alive(pid: u32) -> bool {
    ps_field(pid, "stat").is_some_and(|state| !state.starts_with('Z') && !state.contains('E'))
}

/// A run with no flag starts the status item in a copy, prints the ready line
/// of the copy, and returns.
///
/// The test reads the ready line before it waits for the run. Thus a run that
/// keeps the status item in its own process fails at once, and does not hold
/// the test until its life bound.
#[test]
fn a_plain_start_returns_and_leaves_the_status_item_running_in_a_copy() {
    let mut home = Home::new();
    let mut run = home
        .command(&["--exit-after", LIFE_BOUND])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the glance binary starts");
    let mut stdout = BufReader::new(run.stdout.take().expect("the stdout of the run"));
    let mut line = String::new();
    stdout
        .read_line(&mut line)
        .expect("read the stdout of the run");
    // The named process is the status item, in the copy or in the run.
    let pid = named_pid(&line);
    home.started.push(pid);
    assert_ne!(
        pid,
        run.id(),
        "the run kept the status item in its own process"
    );
    assert_eq!(line, Target::Local.ready_line(pid));
    let mut rest = String::new();
    stdout
        .read_to_string(&mut rest)
        .expect("read the rest of the stdout of the run");
    assert_eq!(rest, "", "the run wrote more than the ready line");
    let status = run.wait().expect("wait for the run");
    assert_eq!(status.code(), Some(0), "{status:?}");
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

/// The flag `--foreground` decides where a new status item runs, not which
/// status item a run stops.
#[test]
fn a_foreground_run_stops_the_background_status_item() {
    let mut home = Home::new();
    let pid = home.start(Target::Local);
    home.assert_stops(Target::Local, &[FOREGROUND], pid);
}

#[test]
fn a_second_plain_run_stops_the_status_item() {
    let mut home = Home::new();
    let pid = home.start(Target::Local);
    home.assert_stops(Target::Local, &[], pid);
}

#[test]
fn a_start_after_a_stop_starts_a_new_status_item() {
    let mut home = Home::new();
    let first = home.start(Target::Local);
    home.assert_stops(Target::Local, &[], first);
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

/// `--sample` and `--stream` start no status item, so a flag that says where
/// the status item runs has no meaning with them.
#[test]
fn foreground_with_sample_or_stream_is_a_usage_error() {
    let home = Home::new();
    for (flag, mode) in [(FOREGROUND, "--sample"), (FOREGROUND_SHORT, "--stream")] {
        let stderr = home.usage_error(&[flag, mode]);
        assert!(
            stderr.contains("cannot be used with"),
            "{flag} {mode}: {stderr}"
        );
    }
}

/// `-b` and `--background` asked for what is now the default. They are gone,
/// so a script that still gives one of them learns of the change at once.
/// The refusal of each of them, and of a near miss, names no hidden flag, so
/// a person does not follow a tip to the flag of the internal copy.
#[test]
fn the_removed_background_flags_are_usage_errors() {
    let home = Home::new();
    let hidden = glance::flags::flag(glance::flags::SPAWNED_COPY);
    for flag in REMOVED_BACKGROUND_FLAGS
        .into_iter()
        .chain(NEAR_MISSES_OF_BACKGROUND)
    {
        let stderr = home.usage_error(&[flag]);
        assert!(
            stderr.contains(&format!("unexpected argument '{flag}'")),
            "{flag}: {stderr}"
        );
        assert!(
            !stderr.contains(&hidden),
            "the refusal of {flag} names the hidden flag: {stderr}"
        );
    }
}

#[test]
fn a_stop_ends_a_foreground_status_item_with_status_zero() {
    assert_a_stop_ends_the_foreground_status_item(FOREGROUND);
}

#[test]
fn the_short_flag_f_starts_a_foreground_status_item() {
    assert_a_stop_ends_the_foreground_status_item(FOREGROUND_SHORT);
}

/// Starts the local status item with the flag `flag`, and asserts that the
/// status item runs in the process of the run until a plain run stops it.
/// The run then exits with status 0.
fn assert_a_stop_ends_the_foreground_status_item(flag: &str) {
    let mut home = Home::new();
    let mut child = home
        .command(&[flag, "--exit-after", LIFE_BOUND])
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
    let pid = home.start_bounded(Target::Local, &[], SHORT_LIFE);
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
    assert!(
        alive(host),
        "the status item {host} of {ALPHA} does not run"
    );
}

#[test]
fn a_second_start_of_a_host_stops_only_the_status_item_of_that_host() {
    let mut home = Home::new();
    let local = home.start(Target::Local);
    let alpha = home.start(Target::Host(ALPHA));
    let beta = home.start(Target::Host(BETA));
    home.assert_stops(Target::Host(ALPHA), &[], alpha);
    assert!(
        alive(local),
        "the stop of {ALPHA} stopped the local status item"
    );
    assert!(
        alive(beta),
        "the stop of {ALPHA} stopped the status item of {BETA}"
    );
}

#[test]
fn a_foreground_run_of_a_host_stops_the_status_item_of_that_host() {
    let mut home = Home::new();
    let beta = home.start(Target::Host(BETA));
    home.assert_stops(Target::Host(BETA), &[FOREGROUND_SHORT], beta);
}

/// The flag `--resolve` changes the name on the status item, not the target.
/// A run with it and a run without it stop the same status item.
#[test]
fn resolve_does_not_change_which_status_item_a_run_stops() {
    let mut home = Home::new();
    let resolved = home.start_with(Target::Host(ALPHA), &[RESOLVE]);
    home.assert_stops(Target::Host(ALPHA), &[], resolved);
    let plain = home.start(Target::Host(ALPHA));
    home.assert_stops(Target::Host(ALPHA), &[RESOLVE], plain);
}

#[test]
fn a_local_start_stops_only_the_local_status_item() {
    let mut home = Home::new();
    let alpha = home.start(Target::Host(ALPHA));
    let local = home.start(Target::Local);
    home.assert_stops(Target::Local, &[], local);
    assert!(
        alive(alpha),
        "the local stop stopped the status item of {ALPHA}"
    );
}

#[test]
fn the_background_status_item_writes_its_output_nowhere() {
    let mut home = Home::new();
    let pid = home.start(Target::Local);
    let output = childenv::fixture_command("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "1,2", "-F", "n"])
        .output()
        .expect("lsof starts");
    let text = String::from_utf8_lossy(&output.stdout);
    let names: Vec<&str> = text.lines().filter(|line| line.starts_with('n')).collect();
    assert_eq!(names, ["n/dev/null", "n/dev/null"], "{output:?}");
}

#[test]
fn a_copy_that_finds_the_lock_held_says_who_holds_it_and_starts_nothing() {
    let mut home = Home::new();
    let pid = home.start(Target::Local);
    let output = home.run(&["--spawned-copy"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("{ANOTHER_STARTED_MARK}{pid}\n")
    );
    assert!(alive(pid), "the copy stopped the status item {pid}");
}

/// What one start of the race printed, and how it ended.
#[derive(Debug)]
struct Start {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Start {
    fn new(output: &Output) -> Self {
        Self {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// The PID of the ready line of this start, if it printed one.
    fn ready_pid(&self, target: Target) -> Option<u32> {
        self.line_pid(|pid| target.ready_line(pid))
    }

    /// The PID of the stopped line of this start, if it printed one.
    fn stopped_pid(&self, target: Target) -> Option<u32> {
        self.line_pid(|pid| target.stopped_line(pid))
    }

    /// The PID that `stdout` names, when `stdout` is exactly the line that
    /// `line` builds for that PID.
    fn line_pid(&self, line: impl Fn(u32) -> String) -> Option<u32> {
        let pid = self
            .stdout
            .contains(PID_MARK)
            .then(|| named_pid(&self.stdout))?;
        (self.stdout == line(pid)).then_some(pid)
    }
}

/// Two plain runs of `glance` for one target at the same moment. Each one
/// starts the status item in the background.
///
/// The race has several valid orders. One start can win the claim, and the
/// other copy can lose it and exit 1 with the another-started line. One start
/// can also find the other status item and stop it, so the first start ends
/// with a stopped line and the other with a ready line or a stopped line. The
/// test accepts each order and asserts the rules that hold in all of them.
#[test]
fn two_plain_starts_at_the_same_moment_give_at_most_one_status_item() {
    let mut home = Home::new();
    let target = Target::Local;
    let barrier = Barrier::new(2);
    let outputs: Vec<Output> = thread::scope(|scope| {
        let workers: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    home.run(&[])
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a start does not panic"))
            .collect()
    });
    let starts: Vec<Start> = outputs.iter().map(Start::new).collect();
    let report = format!("{starts:#?}");
    let ready: Vec<u32> = starts.iter().filter_map(|s| s.ready_pid(target)).collect();
    home.started.extend(&ready);

    for (index, start) in starts.iter().enumerate() {
        let other = &starts[1 - index];
        match start.code {
            Some(0) => assert!(
                start.ready_pid(target).is_some() || start.stopped_pid(target).is_some(),
                "a start that exits 0 prints one ready line or one stopped line: {report}"
            ),
            Some(1) => {
                assert!(
                    start.stdout.is_empty(),
                    "a failed start prints to stdout: {report}"
                );
                let last = start.stderr.lines().last().unwrap_or_default();
                assert!(
                    last.starts_with(COPY_STOPPED_MARK),
                    "a failed start ends with the copy-stopped line: {report}"
                );
                let first = start.stderr.lines().next().unwrap_or_default();
                let explained_by_race = first
                    .strip_prefix(ANOTHER_STARTED_MARK)
                    .and_then(|pid| pid.parse::<u32>().ok())
                    .is_some_and(|holder| other.ready_pid(target) == Some(holder) && alive(holder));
                assert!(
                    explained_by_race || other.stopped_pid(target).is_some(),
                    "a failed start has no explanation: {report}"
                );
            }
            code => panic!("a start exits 0 or 1, not {code:?}: {report}"),
        }
        if let Some(pid) = start.stopped_pid(target) {
            assert!(!alive(pid), "the status item {pid} still runs: {report}");
        }
    }
    assert!(
        ready.len() <= 1,
        "two starts printed a ready line: {report}"
    );
    let running = ready.iter().filter(|pid| alive(**pid)).count();
    assert!(running <= 1, "two status items run: {report}");
}

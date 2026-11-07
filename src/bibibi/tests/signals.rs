//! Item 8 of the plan: signals.
//!
//! Each test starts `bibibi` in its own process group and sends one signal to
//! the `bibibi` process alone, while a test command runs. The test command
//! starts a `sleep 20`, so a run that does not stop its children ends within
//! 20 s, also in a build where `bibibi` handles no signal.
#![cfg(unix)]

mod support;

use std::fs::{self, File};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use gitscratch::testing::TestRepo;
use support::{add_test_commit_with, bibibi_command, linear_history, snapshot};
use tempfile::TempDir;

/// The ceiling on every wait for `bibibi`, so a wedged run fails the test.
const DEADLINE: Duration = Duration::from_secs(60);

/// How long a wait for a process to die lasts.
const DEATH_DEADLINE: Duration = Duration::from_secs(10);

/// How often a wait looks again.
const POLL: Duration = Duration::from_millis(10);

const VALUES: [&str; 10] = [
    "good", "good", "good", "good", "good", "bad", "bad", "bad", "bad", "bad",
];

/// The test script. It answers at once at commits 1 and 10 (the pre-flight)
/// and when the marker exists. Elsewhere, the first time, it starts a
/// `sleep 20`, writes its own pid and the pid of the sleep to the pids file,
/// creates the marker, and waits for the sleep.
const STALLING_SCRIPT: &str = "echo 't.sh ran'
case \"$(cat n.txt)\" in 1|10) grep -qx good value.txt; exit $?;; esac
if [ -e \"$1\" ]; then grep -qx good value.txt; exit $?; fi
sleep 20 &
echo \"$$ $!\" > \"$2\"
: > \"$1\"
wait
grep -qx good value.txt
";

/// Owns the `bibibi` process of one test. Its drop kills what the test
/// started, also when an assertion failed.
struct Run {
    child: Child,
    pids: PathBuf,
}

impl Run {
    /// The process ids that the test script wrote, if it wrote them.
    fn script_pids(&self) -> Vec<i32> {
        fs::read_to_string(&self.pids)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|word| word.parse().ok())
            .collect()
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let group = i32::try_from(self.child.id()).expect("pid fits in i32");
        // SAFETY: `kill` takes two integers and touches no memory. A negative
        // pid names the process group that `process_group(0)` made.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
        for pid in self.script_pids() {
            // SAFETY: as above. A pid that is gone gives an error that is not
            // used here.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

/// Wait until `path` exists. Fails the test at the deadline.
fn wait_for_file(path: &Path) {
    let start = Instant::now();
    while !path.exists() {
        assert!(
            start.elapsed() < DEADLINE,
            "the marker {} did not appear",
            path.display()
        );
        thread::sleep(POLL);
    }
}

/// Wait until `bibibi` ends. Fails the test at the deadline.
fn wait_for_exit(run: &mut Run) -> ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = run.child.try_wait().expect("wait for bibibi") {
            return status;
        }
        assert!(start.elapsed() < DEADLINE, "bibibi did not end");
        thread::sleep(POLL);
    }
}

/// Whether `pid` is a live process. A zombie is a process that died.
fn is_alive(pid: i32) -> bool {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    let state = String::from_utf8_lossy(&output.stdout);
    let state = state.trim();
    !state.is_empty() && !state.starts_with('Z')
}

/// Run the scenario with `signal` and check the result.
fn interrupted_run_cleans_up(signal: i32, expected_code: i32) {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    add_test_commit_with(&repo, STALLING_SCRIPT);
    let before = snapshot(&repo);

    let scratch = TempDir::new().expect("temporary directory");
    let marker = scratch.path().join("marker");
    let pids = scratch.path().join("pids");
    let stdout = File::create(scratch.path().join("stdout")).expect("stdout file");
    let stderr_path = scratch.path().join("stderr");
    let stderr = File::create(&stderr_path).expect("stderr file");

    let mut command = bibibi_command(
        repo.path(),
        &[
            "--good",
            &ids[0],
            "--",
            "sh",
            "tests/t.sh",
            marker.to_str().expect("UTF-8 path"),
            pids.to_str().expect("UTF-8 path"),
        ],
    );
    command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    let child = command.spawn().expect("start bibibi");
    let mut run = Run { child, pids };

    wait_for_file(&marker);
    let bibibi_pid = i32::try_from(run.child.id()).expect("pid fits in i32");
    // SAFETY: `kill` takes two integers and touches no memory. The pid is the
    // child that this test started and has not yet waited for.
    let sent = unsafe { libc::kill(bibibi_pid, signal) };
    assert_eq!(sent, 0, "could not send the signal");

    let status = wait_for_exit(&mut run);
    let stderr = fs::read_to_string(&stderr_path).expect("read stderr");

    assert_eq!(
        status.code(),
        Some(expected_code),
        "status: {status:?} (signal {:?}), stderr: {stderr}",
        status.signal()
    );
    assert!(
        stderr.contains(&format!("bibibi: a signal ({signal}) stopped the run")),
        "stderr: {stderr}"
    );
    assert_eq!(snapshot(&repo), before, "the checkout of the user changed");

    let script_pids = run.script_pids();
    assert_eq!(script_pids.len(), 2, "pids: {script_pids:?}");
    for pid in script_pids {
        let start = Instant::now();
        while is_alive(pid) {
            assert!(
                start.elapsed() < DEATH_DEADLINE,
                "process {pid} is still alive"
            );
            thread::sleep(POLL);
        }
    }
}

#[test]
fn sigint_stops_the_run_with_130_and_cleans_up() {
    interrupted_run_cleans_up(libc::SIGINT, 130);
}

#[test]
fn sigterm_stops_the_run_with_143_and_cleans_up() {
    interrupted_run_cleans_up(libc::SIGTERM, 143);
}

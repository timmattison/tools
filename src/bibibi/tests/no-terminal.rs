//! A build or a test that touches the terminal must not stop the run.
//!
//! A child in a background process group of the controlling terminal gets
//! SIGTTOU when it changes the terminal settings, and the kernel stops it. The
//! wait of `bibibi` then never returns. Only a real pseudo-terminal that is the
//! controlling terminal of `bibibi` can show this, thus the test uses one.
#![cfg(unix)]

mod support;

use std::fs::{self, File};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use gitscratch::testing::pty::Pty;
use gitscratch::testing::TestRepo;
use support::{add_test_commit_with, bibibi_command, linear_history, snapshot};
use tempfile::TempDir;

/// The ceiling on the wait for `bibibi`, so a stopped child fails the test.
const DEADLINE: Duration = Duration::from_secs(60);

/// How often the wait looks again.
const POLL: Duration = Duration::from_millis(10);

const VALUES: [&str; 10] = [
    "good", "good", "good", "good", "good", "bad", "bad", "bad", "bad", "bad",
];

/// The test script. It records its pid, changes the terminal settings, and
/// then decides from `value.txt`. The `2>/dev/null` comes first, because the
/// shell handles redirections from left to right and the open of `/dev/tty`
/// can fail.
const TERMINAL_SCRIPT: &str = "echo $$ >> \"$1\"
stty sane 2>/dev/null </dev/tty
grep -qx good value.txt
";

/// Owns the `bibibi` process of the test. Its drop kills what the test
/// started, also when an assertion failed.
struct Run {
    child: Child,
    pids: PathBuf,
}

impl Drop for Run {
    fn drop(&mut self) {
        let group = i32::try_from(self.child.id()).expect("pid fits in i32");
        // SAFETY: `kill` takes two integers and touches no memory. A negative
        // pid names the process group that `bibibi` leads.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
        let written = fs::read_to_string(&self.pids).unwrap_or_default();
        for pid in written
            .split_whitespace()
            .filter_map(|word| word.parse::<i32>().ok())
        {
            // SAFETY: as above. Each test script leads a group of its own. A
            // pid that is gone gives an error that is not used here.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

#[test]
fn a_test_that_touches_the_terminal_does_not_stop_the_run() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    add_test_commit_with(&repo, TERMINAL_SCRIPT);
    let before = snapshot(&repo);

    let scratch = TempDir::new().expect("temporary directory");
    let pids = scratch.path().join("pids");
    let stdout_path = scratch.path().join("stdout");
    let stderr_path = scratch.path().join("stderr");

    let pty = Pty::open(80);
    let mut command = bibibi_command(
        repo.path(),
        &[
            "--good",
            &ids[0],
            "--",
            "sh",
            "tests/t.sh",
            pids.to_str().expect("UTF-8 path"),
        ],
    );
    pty.give_as_controlling_terminal(&mut command);
    command
        .stdin(Stdio::null())
        .stdout(File::create(&stdout_path).expect("stdout file"))
        .stderr(File::create(&stderr_path).expect("stderr file"));
    let child = command.spawn().expect("start bibibi");
    let mut run = Run { child, pids };

    let start = Instant::now();
    let status = loop {
        if let Some(status) = run.child.try_wait().expect("wait for bibibi") {
            break status;
        }
        assert!(
            start.elapsed() < DEADLINE,
            "bibibi did not end; a child probably stopped on SIGTTOU. stderr: {}",
            fs::read_to_string(&stderr_path).unwrap_or_default()
        );
        thread::sleep(POLL);
    };
    drop(pty);

    let stderr = fs::read_to_string(&stderr_path).unwrap_or_default();
    assert_eq!(status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(
        fs::read_to_string(&stdout_path).expect("stdout text"),
        format!("{} commit 6\n", ids[5])
    );
    assert_eq!(snapshot(&repo), before, "the checkout of the user changed");
}

//! The binary prints its version for `--version` and `-V`, then exits.
//!
//! Without the check the binary opens its window and never exits, so each run
//! has its own deadline and the child never outlives the test.

use std::io::Read;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Longest time one run of the binary may take.
const DEADLINE: Duration = Duration::from_secs(60);
/// Time between two checks of the child.
const POLL: Duration = Duration::from_millis(50);

/// Run the binary with `flag`. Returns the status and standard output.
fn run_with(flag: &str) -> (std::process::ExitStatus, String) {
    let mut child = childenv::fixture_command(env!("CARGO_BIN_EXE_msgs"))
        .arg(flag)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start msgs");
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll msgs") {
            break status;
        }
        if start.elapsed() >= DEADLINE {
            child.kill().expect("kill msgs");
            child.wait().expect("reap msgs");
            panic!("msgs {flag} did not exit; it opened the app");
        }
        std::thread::sleep(POLL);
    };
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("piped stdout")
        .read_to_string(&mut stdout)
        .expect("read stdout");
    (status, stdout)
}

fn assert_version_line(flag: &str) {
    let (status, stdout) = run_with(flag);
    assert!(status.success(), "msgs {flag} exited with {status}");
    let prefix = format!("msgs {} (", env!("CARGO_PKG_VERSION"));
    assert!(stdout.starts_with(&prefix), "stdout was {stdout:?}");
    assert!(stdout.ends_with(")\n"), "stdout was {stdout:?}");
    assert!(
        stdout.contains(", clean)") || stdout.contains(", dirty)") || stdout.contains("unknown"),
        "stdout was {stdout:?}"
    );
}

#[test]
fn long_flag_prints_the_version() {
    assert_version_line("--version");
}

#[test]
fn short_flag_prints_the_version() {
    assert_version_line("-V");
}

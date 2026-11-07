//! End-to-end test: wu reports a process that holds a file open.
//!
//! wu had no test. The Linux path was rewritten onto the procfs 0.17 API, and
//! nothing ran it. This test starts a child shell that holds a temporary file
//! open. It then runs the `wu` binary with `--json` on that file. It checks
//! that wu reports the pid of the child, the descriptor, and the path.
//!
//! The test runs on Linux (procfs) and on macOS (lsof). The holder needs
//! `/bin/sh`, so no other platform is in scope.

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Script of the holder. The shell opens the file named by its first argument
/// on descriptor 3. It then prints `ready`. Then it replaces itself with
/// `sleep`, which keeps the same pid and the same open descriptor. The 120
/// second sleep is the time bound: the holder stops on its own if the test
/// process dies. The path is an argument, never part of the script.
const HOLDER_SCRIPT: &str = r#"exec 3<"$1"; echo ready; exec sleep 120"#;

/// Line that the holder prints after it opened the file.
const READY_LINE: &str = "ready";

/// Descriptor that wu reports on Linux. procfs gives the descriptor number.
#[cfg(target_os = "linux")]
const EXPECTED_DESCRIPTOR: &str = "3";

/// Descriptor that wu reports on macOS. lsof gives the descriptor number and
/// the access mode. The mode is `r` for a read-only descriptor.
#[cfg(target_os = "macos")]
const EXPECTED_DESCRIPTOR: &str = "3r";

/// A running holder process. The `Drop` implementation kills the child and
/// waits for it. A failed assertion thus does not leave the child running.
struct Holder {
    child: Child,
}

impl Holder {
    /// Starts a shell that holds `path` open on descriptor 3. Returns after
    /// the shell printed `ready`, which means the descriptor is open.
    fn start(path: &Path) -> Self {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(HOLDER_SCRIPT)
            .arg("wu-holder")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .expect("the holder must start");
        let stdout = child.stdout.take().expect("the holder stdout is piped");
        let holder = Self { child };
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .expect("the holder must print one line");
        assert_eq!(line.trim_end(), READY_LINE, "the holder must report ready");
        holder
    }

    /// Process id of the holder.
    fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        // The holder may be dead already. Both calls can fail without harm.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn wu_reports_the_process_that_holds_a_file_open() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let held = dir.path().join("held");
    std::fs::write(&held, b"some bytes").expect("the held file must be written");

    let holder = Holder::start(&held);

    let output = Command::new(env!("CARGO_BIN_EXE_wu"))
        .arg("--json")
        .arg(&held)
        .output()
        .expect("wu must run");
    assert!(
        output.status.success(),
        "wu failed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("wu must print JSON");
    let entry = report
        .as_array()
        .expect("the report is a JSON array")
        .iter()
        .find(|entry| entry["pid"].as_u64() == Some(u64::from(holder.pid())))
        .unwrap_or_else(|| {
            panic!(
                "wu did not report pid {}, stdout: {}",
                holder.pid(),
                String::from_utf8_lossy(&output.stdout)
            )
        });

    // Compare the file name only. lsof prints /private/var/... for /var/...
    let reported_path = entry["file_path"].as_str().expect("file_path is a string");
    assert_eq!(
        Path::new(reported_path).file_name(),
        held.file_name(),
        "wu reported another path: {entry}"
    );
    assert_eq!(
        entry["file_descriptor"].as_str(),
        Some(EXPECTED_DESCRIPTOR),
        "wu reported another descriptor: {entry}"
    );
}

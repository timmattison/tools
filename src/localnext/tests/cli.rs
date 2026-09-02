//! Integration tests for the `localnext` binary's CLI surface.
//!
//! These spawn the real binary via `CARGO_BIN_EXE_localnext`. They are
//! parallel-safe: every fixture is a unique `tempfile::TempDir`, no port is
//! hardcoded (the bind-failure test asks the operating system for one and the
//! banner tests pass `--port 0`), and the working directory is set per child
//! through `Command::current_dir` rather than by `std::env::set_current_dir`,
//! which is process-global and would race the other tests in this binary.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// How long to wait for a line of the child's banner before failing.
///
/// This budget covers a HUNG child and nothing else. Every cost that is not the
/// code — chiefly the scan macOS runs over a freshly built binary the first time
/// it executes, measured here at ten seconds and more — is paid by
/// [`warm_the_binary`] before the clock starts. So the wait no longer measures
/// the machine's load, which is how a timed test starts failing commits that
/// have nothing to do with it.
const BANNER_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the wait looks up from the channel to ask whether the child died.
///
/// A child that exits without printing must fail at once with the reason, rather
/// than burn [`BANNER_TIMEOUT`] and then report a timeout — the wrong diagnosis
/// for the commonest fault.
const LIVENESS_POLL: Duration = Duration::from_millis(100);

/// The prefix of the banner line that names the served root.
const SERVING_PREFIX: &str = "Serving ";

/// The separator between the served root and its URL on that same line.
const ON_URL: &str = " on http://";

/// Kills the child on drop.
///
/// The banner tests start a real, blocking server. A trailing `child.kill()`
/// would be skipped by a panicking assertion, leaking a running server for the
/// rest of the session — a reaper that only runs on the happy path is not a
/// reaper. `Drop` runs on the panic unwind too.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Builds a minimal static export: a `TempDir` holding an `out/index.html`.
///
/// The `TempDir` itself is the project directory. The caller must hold it for as
/// long as the child runs, because dropping it deletes the tree.
fn export_fixture() -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    let out = dir.path().join(localnext::ROOT_DIRECTORY);
    std::fs::create_dir(&out).expect("create out dir");
    std::fs::write(
        out.join("index.html"),
        b"<!doctype html><title>home</title>",
    )
    .expect("write index.html");
    dir
}

/// Runs the binary once and waits for it to exit, so the operating system has
/// already examined the file before any timed spawn.
///
/// macOS scans a freshly built, unsigned binary the first time it runs. That scan
/// took over ten seconds per spawn when this crate was written, and several tests
/// in this file spawn the same fresh binary at the same moment. Paying that cost
/// inside [`serving_root`]'s bounded wait made the wait a measurement of the
/// machine rather than of the code, and it failed once for exactly that reason.
/// `--version` exits at once and `Command::output` carries no deadline, so the
/// cost lands here, where nothing is timed.
fn warm_the_binary() {
    let _ = Command::new(env!("CARGO_BIN_EXE_localnext"))
        .arg("--version")
        .output();
}

/// Starts `localnext --port 0` with its working directory at `cwd` and returns
/// the export root its banner names.
///
/// The child's stdout is drained on a separate thread that forwards each line
/// over a channel, so the test thread can bound its wait. A direct
/// `lines().next()` on a child that never prints would block forever. Rust's
/// `Stdout` is line-buffered even when it is a pipe, so `println!` flushes the
/// banner on its own newline and no handshake is needed.
///
/// The wait ends for one of three reasons, and each names its own fault: the
/// banner arrives, the child closes stdout or exits, or [`BANNER_TIMEOUT`] runs
/// out on a child that is still alive and still silent.
fn serving_root(cwd: &Path) -> String {
    warm_the_binary();

    let mut child = Command::new(env!("CARGO_BIN_EXE_localnext"))
        .args(["--port", "0"])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawning localnext should succeed");

    let stdout = child.stdout.take().expect("stdout is piped");
    // The guard owns the child from here on, so every exit from this function —
    // including a panicking assertion below — kills the server.
    let mut guard = ChildGuard(child);

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + BANNER_TIMEOUT;
    loop {
        match receiver.recv_timeout(LIVENESS_POLL) {
            Ok(line) => {
                if let Some(rest) = line.strip_prefix(SERVING_PREFIX) {
                    let (root, _url) = rest
                        .rsplit_once(ON_URL)
                        .unwrap_or_else(|| panic!("the banner should name a URL, got: {line}"));
                    return root.to_string();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("localnext closed its stdout without printing a banner");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(Some(status)) = guard.0.try_wait() {
                    panic!("localnext exited before printing a banner: {status}");
                }
                assert!(
                    Instant::now() < deadline,
                    "localnext stayed alive and silent for {BANNER_TIMEOUT:?} without a banner"
                );
            }
        }
    }
}

/// `localnext --version` exits successfully and prints the buildinfo version
/// string, as every tool in this workspace must.
#[test]
fn the_version_flag_prints_the_buildinfo_string() {
    let output = Command::new(env!("CARGO_BIN_EXE_localnext"))
        .arg("--version")
        .output()
        .expect("spawning localnext --version should succeed");

    assert!(
        output.status.success(),
        "localnext --version should exit 0, got {:?}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let version_line = stdout
        .lines()
        .find(|line| line.starts_with("localnext "))
        .unwrap_or_else(|| panic!("version output should name the binary, got: {stdout}"));
    assert!(
        version_line.contains("0.1.0"),
        "version output should include the crate version, got: {version_line}"
    );
}

/// `localnext --help` exits successfully and documents the `--port` flag, the
/// escape hatch from the derived default.
#[test]
fn the_help_documents_the_port_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_localnext"))
        .arg("--help")
        .output()
        .expect("spawning localnext --help should succeed");

    assert!(
        output.status.success(),
        "localnext --help should exit 0, got {:?}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--port"),
        "help should document the --port flag, got: {stdout}"
    );
}

/// A directory with no `out` under it aborts startup with a non-zero exit and an
/// error on stderr that names the directory it looked for — never a hung server.
#[test]
fn a_directory_with_no_export_aborts_startup() {
    let dir = TempDir::new().expect("temp dir");

    let output = Command::new(env!("CARGO_BIN_EXE_localnext"))
        .current_dir(dir.path())
        .output()
        .expect("spawning localnext outside an export should succeed");

    assert!(
        !output.status.success(),
        "a missing export root should make localnext exit non-zero, got {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("'{}'", localnext::ROOT_DIRECTORY)),
        "stderr should name the missing '{}' directory, got: {stderr}",
        localnext::ROOT_DIRECTORY
    );
}

/// A port that is already taken aborts startup with a non-zero exit and a message
/// naming the port.
///
/// This is the regression test for the defect this port fixes: the Go tool
/// discarded the error from `http.ListenAndServe`, so a taken port exited 0 and
/// printed nothing at all. The listener below stays bound for the whole test, so
/// the child's bind cannot succeed.
#[test]
fn a_taken_port_aborts_startup() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
    let dir = export_fixture();

    let output = Command::new(env!("CARGO_BIN_EXE_localnext"))
        .args(["--port", &port.to_string()])
        .current_dir(dir.path())
        .output()
        .expect("spawning localnext on a taken port should succeed");

    // Held until here so the port stays taken for the child's whole lifetime.
    drop(listener);

    assert!(
        !output.status.success(),
        "a taken port should make localnext exit non-zero, got {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("bind"),
        "stderr should report the bind failure, got: {stderr}"
    );
    assert!(
        stderr.contains(&port.to_string()),
        "stderr should name the port it could not bind ({port}), got: {stderr}"
    );
}

/// Running from the project directory and running from inside `out` serve the
/// same canonical export root, and the banner says so.
///
/// The comparison is against the fixture's CANONICALIZED `out` path because
/// `find_root` canonicalizes: on macOS a `TempDir` lives under `/var`, a symlink
/// to `/private/var`.
#[test]
fn both_working_directories_serve_the_same_canonical_root() {
    let dir = export_fixture();
    let out = dir.path().join(localnext::ROOT_DIRECTORY);
    let expected = out.canonicalize().expect("canonicalize the export root");

    let from_project = serving_root(dir.path());
    let from_out = serving_root(&out);

    assert_eq!(
        from_project,
        expected.display().to_string(),
        "running from the project directory should serve the canonical export root"
    );
    assert_eq!(
        from_out, from_project,
        "running from inside out should serve the same root as running from the project directory"
    );
}

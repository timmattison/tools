//! Integration tests for the `zth` binary.
//!
//! These spawn the real binary via `CARGO_BIN_EXE_zth` against per-test
//! [`TempDir`] fixtures, so the suite is safe to run concurrently with another
//! copy of itself. Because the child's stderr is a pipe rather than a terminal,
//! the progress bar draws nothing - which is what lets these tests assert that
//! stderr stays completely empty.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

/// Matches in the closed-pipe fixture. Chosen with [`PIPE_NAME_PADDING`] so the
/// printed list runs to a few hundred kilobytes - several times a pipe's 64 KiB
/// buffer - which is what makes the closed read end break a write rather than
/// letting the whole list slip into the kernel and the test pass by accident.
const PIPE_FIXTURE_MATCHES: u32 = 1_200;

/// Padding characters in each closed-pipe fixture name. Long names buy output
/// bytes far more cheaply than more files do.
const PIPE_NAME_PADDING: usize = 100;

/// Writes `contents` to `dir/name`, creating any parent directories.
fn write(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("creating fixture directories should succeed");
    }
    fs::write(&path, contents).expect("writing the fixture should succeed");
    path
}

/// A temp dir plus its canonical path, which is what `zth` reports.
fn fixture() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("creating a temp dir should succeed");
    let canonical =
        fs::canonicalize(dir.path()).expect("canonicalizing the temp dir should succeed");
    (dir, canonical)
}

/// Runs `zth` with the given arguments and captures its output.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zth"))
        .args(args)
        .output()
        .expect("spawning zth should succeed")
}

/// The lines `zth` printed to stdout.
fn stdout_lines(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Renders paths the way stdout would show them.
fn as_lines(paths: &[&Path]) -> Vec<String> {
    let mut lines: Vec<String> = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    lines.sort();
    lines
}

/// Asserts an empty stderr, which requirement four demands even when files fail
/// to open.
fn assert_silent(output: &Output) {
    assert!(
        output.stderr.is_empty(),
        "zth must never print errors, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn prints_matching_absolute_paths_to_stdout() {
    let (_dir, root) = fixture();
    let top = write(&root, "top.bin", &[0_u8; 512]);
    let nested = write(&root, "a/b/nested.bin", &[0_u8; 1]);
    write(&root, "a/data.bin", &[0_u8, 0, 7, 0]);
    write(&root, "a/b/empty.bin", &[]);

    let output = run(&[&root.to_string_lossy()]);

    assert!(
        output.status.success(),
        "zth should exit 0, got {:?}",
        output.status
    );
    assert_eq!(
        stdout_lines(&output),
        as_lines(&[&top, &nested]),
        "stdout should list exactly the non-empty all-zero files, one per line"
    );
    assert_silent(&output);
}

#[test]
fn a_missing_path_prints_nothing_and_succeeds() {
    let (_dir, root) = fixture();

    let output = run(&[&root.join("nowhere").to_string_lossy()]);

    assert!(
        output.status.success(),
        "a missing path is an error to skip, got {:?}",
        output.status
    );
    assert!(
        output.stdout.is_empty(),
        "nothing matched, so nothing should be printed"
    );
    assert_silent(&output);
}

#[test]
fn a_tree_with_no_matches_prints_nothing() {
    let (_dir, root) = fixture();
    write(&root, "data.bin", &[1_u8; 64]);
    write(&root, "empty.bin", &[]);

    let output = run(&[&root.to_string_lossy()]);

    assert!(
        output.status.success(),
        "zth should exit 0, got {:?}",
        output.status
    );
    assert!(
        output.stdout.is_empty(),
        "no file qualifies, so stdout should be empty"
    );
    assert_silent(&output);
}

#[test]
fn the_jobs_flag_does_not_change_the_results() {
    let (_dir, root) = fixture();
    for index in 0_u8..8 {
        write(&root, &format!("zero-{index}.bin"), &[0_u8; 64]);
        write(&root, &format!("data-{index}.bin"), &[index + 1; 64]);
    }

    let one = run(&["--jobs", "1", &root.to_string_lossy()]);
    let many = run(&["-j", "8", &root.to_string_lossy()]);

    assert_eq!(
        stdout_lines(&one).len(),
        8,
        "all eight all-zero files should be reported"
    );
    assert_eq!(
        stdout_lines(&one),
        stdout_lines(&many),
        "the worker count must not change the output"
    );
    assert_silent(&one);
    assert_silent(&many);
}

#[cfg(unix)]
#[test]
fn unreadable_files_produce_no_error_output() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, root) = fixture();
    let readable = write(&root, "readable.bin", &[0_u8; 64]);
    let locked = write(&root, "locked.bin", &[0_u8; 64]);
    let locked_dir = root.join("locked-dir");
    write(&locked_dir, "hidden.bin", &[0_u8; 64]);

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
        .expect("chmod on a temp file should succeed");
    fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o000))
        .expect("chmod on a temp dir should succeed");

    let output = run(&[&root.to_string_lossy()]);

    // Restore access so the TempDir can clean itself up.
    let _ = fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o700));

    // Running as root defeats the permission bits entirely.
    if fs::read(&locked).is_ok() {
        return;
    }

    assert_silent(&output);
    assert_eq!(
        stdout_lines(&output),
        as_lines(&[&readable]),
        "the readable match should still be reported around the failures"
    );
}

#[cfg(unix)]
#[test]
fn a_failed_stdout_write_is_reported_through_the_exit_status() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixDatagram;

    let (_dir, root) = fixture();
    write(&root, "zero.bin", &[0_u8; 64]);

    // An unconnected datagram socket standing in for stdout. Writes to it fail
    // with `EDESTADDRREQ`: an ordinary I/O error, not a broken pipe. The errno
    // that matters in practice is `ENOSPC` from a full disk, which no test can
    // conjure cheaply; `EBADF` cannot stand in for it either, because `std`
    // deliberately reports a write that fails that way as having succeeded.
    let unwritable = UnixDatagram::unbound().expect("creating a datagram socket should succeed");

    let output = Command::new(env!("CARGO_BIN_EXE_zth"))
        .arg(root.as_os_str())
        .stdout(Stdio::from(OwnedFd::from(unwritable)))
        .stderr(Stdio::piped())
        .output()
        .expect("spawning zth should succeed");

    assert!(
        !output.status.success(),
        "a list that could not be written must not exit like a complete run, got {:?}",
        output.status
    );
    assert_silent(&output);
}

#[cfg(unix)]
#[test]
fn a_reader_that_stops_early_still_exits_zero() {
    let (_dir, root) = fixture();
    let padding = "x".repeat(PIPE_NAME_PADDING);
    for index in 0..PIPE_FIXTURE_MATCHES {
        write(&root, &format!("zero-{index:04}-{padding}.bin"), &[0_u8; 1]);
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_zth"))
        .arg(root.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning zth should succeed");

    // Closing the read end is the `zth /data | head` case: the writes fail with
    // `EPIPE`, which says the caller had seen enough, not that anything broke.
    drop(child.stdout.take());

    let output = child
        .wait_with_output()
        .expect("waiting for zth should succeed");

    assert!(
        output.status.success(),
        "a caller that stops reading must not turn the run into a failure, got {:?}",
        output.status
    );
    assert_silent(&output);
}

#[test]
fn version_flag_prints_the_buildinfo_string() {
    let output = run(&["--version"]);

    assert!(
        output.status.success(),
        "zth --version should exit 0, got {:?}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("zth "),
        "version output should name the binary, got: {stdout}"
    );
    assert!(
        stdout.contains("0.1.0"),
        "version output should include the crate version, got: {stdout}"
    );
}

#[test]
fn help_documents_the_path_argument_and_the_jobs_flag() {
    let output = run(&["--help"]);

    assert!(
        output.status.success(),
        "zth --help should exit 0, got {:?}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("PATH"),
        "help should document the PATH argument, got: {stdout}"
    );
    assert!(
        stdout.contains("--jobs"),
        "help should document the --jobs flag, got: {stdout}"
    );
}

#[test]
fn a_missing_path_argument_is_a_usage_error() {
    let output = run(&[]);

    assert!(
        !output.status.success(),
        "zth with no path should fail rather than scan something unasked for"
    );
}

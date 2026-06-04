//! Integration tests for `tsm record`.
//!
//! Each test runs the `tsm` binary as a subprocess via `assert_cmd`, using a
//! fresh temporary directory for `$XDG_DATA_HOME` / `$XDG_STATE_HOME` so test
//! runs cannot pollute each other or the developer's real session log.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use tempfile::TempDir;
use tsm_jsonl::{Header, HeaderKind, PrecmdKind, PrecmdRecord};

/// Known-good 32-char lowercase hex session id used across tests.
const TEST_SESSION_ID: &str = "0123456789abcdef0123456789abcdef";

/// Path to the session log file inside a tempdir root.
fn session_log_path(data: &TempDir, session_id: &str) -> PathBuf {
    data.path()
        .join("tsm")
        .join("sessions")
        .join(format!("{session_id}.jsonl"))
}

/// Path to the error log inside the state tempdir.
fn error_log_path(state: &TempDir) -> PathBuf {
    state.path().join("tsm").join("errors.log")
}

/// Path to the fail-counter for the *current* process (which is the parent of
/// the spawned `tsm record`).
fn fail_count_path(state: &TempDir) -> PathBuf {
    state
        .path()
        .join("tsm")
        .join(format!("fail-count.{}", std::process::id()))
}

/// Build a Command for `tsm` with `env_clear` plus the minimum env the
/// recorder needs to run.
fn tsm_command(data: &TempDir, state: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("tsm").expect("tsm binary");
    cmd.env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
        )
        .env("HOME", data.path())
        .env("XDG_DATA_HOME", data.path())
        .env("XDG_STATE_HOME", state.path());
    cmd
}

#[test]
fn record_first_call_writes_header() {
    let data = TempDir::new().expect("tempdir");
    let state = TempDir::new().expect("tempdir");
    tsm_command(&data, &state)
        .env("TSM_SESSION_ID", TEST_SESSION_ID)
        .args(["record", "--exit-code", "0", "--last-command", "ls"])
        .assert()
        .success();

    let log_path = session_log_path(&data, TEST_SESSION_ID);
    assert!(
        log_path.exists(),
        "session log file must exist at {log_path:?}"
    );

    let contents = fs::read_to_string(&log_path).expect("read log");
    let first_line = contents.lines().next().expect("at least one line");
    let header: Header = serde_json::from_str(first_line).expect("line 1 must parse as Header");
    assert!(matches!(header.kind, HeaderKind::Header));
    assert_eq!(header.schema_version, 1);
    assert!(
        !header.tsm_version.is_empty(),
        "tsm_version should be populated"
    );
}

#[test]
fn record_second_call_appends_record_after_header() {
    let data = TempDir::new().expect("tempdir");
    let state = TempDir::new().expect("tempdir");

    tsm_command(&data, &state)
        .env("TSM_SESSION_ID", TEST_SESSION_ID)
        .args(["record", "--exit-code", "0", "--last-command", "first"])
        .assert()
        .success();
    tsm_command(&data, &state)
        .env("TSM_SESSION_ID", TEST_SESSION_ID)
        .args(["record", "--exit-code", "7", "--last-command", "second"])
        .assert()
        .success();

    let log_path = session_log_path(&data, TEST_SESSION_ID);
    let contents = fs::read_to_string(&log_path).expect("read log");
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "expected exactly 2 lines, got: {contents:?}"
    );

    let header: Header = serde_json::from_str(lines[0]).expect("line 1 must parse as Header");
    assert!(matches!(header.kind, HeaderKind::Header));

    let record: PrecmdRecord =
        serde_json::from_str(lines[1]).expect("line 2 must parse as PrecmdRecord");
    assert!(matches!(record.kind, PrecmdKind::Precmd));
    assert_eq!(record.exit_code, 7);
    assert_eq!(record.last_command, "second");
}

#[test]
fn record_missing_session_id_writes_to_error_log_not_stderr() {
    let data = TempDir::new().expect("tempdir");
    let state = TempDir::new().expect("tempdir");

    let output = tsm_command(&data, &state)
        .args(["record", "--exit-code", "0", "--last-command", "ls"])
        .output()
        .expect("run tsm");

    assert!(
        output.stderr.is_empty(),
        "stderr must be empty, got: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );

    let err_path = error_log_path(&state);
    assert!(err_path.exists(), "error log must exist at {err_path:?}");
    let err_log = fs::read_to_string(&err_path).expect("read err log");
    assert!(
        err_log.to_ascii_lowercase().contains("session"),
        "error log should mention session id: {err_log:?}"
    );
}

#[test]
fn record_invalid_session_id_writes_to_error_log() {
    let data = TempDir::new().expect("tempdir");
    let state = TempDir::new().expect("tempdir");

    tsm_command(&data, &state)
        .env("TSM_SESSION_ID", "not-hex")
        .args(["record", "--exit-code", "0", "--last-command", "ls"])
        .assert()
        .success();

    let err_path = error_log_path(&state);
    assert!(err_path.exists(), "error log must exist");
    let err_log = fs::read_to_string(&err_path).expect("read err log");
    assert!(
        err_log.to_ascii_lowercase().contains("session"),
        "error log should mention invalid session id: {err_log:?}"
    );
}

#[test]
fn record_failure_increments_fail_counter() {
    let data = TempDir::new().expect("tempdir");
    let state = TempDir::new().expect("tempdir");

    tsm_command(&data, &state)
        .args(["record", "--exit-code", "0", "--last-command", "ls"])
        .assert()
        .success();

    let counter = fail_count_path(&state);
    assert!(
        counter.exists(),
        "fail-count file must exist at {counter:?}"
    );
    let v = fs::read_to_string(&counter).expect("read counter");
    assert_eq!(v.trim(), "1");

    tsm_command(&data, &state)
        .args(["record", "--exit-code", "0", "--last-command", "ls"])
        .assert()
        .success();
    let v = fs::read_to_string(&counter).expect("read counter");
    assert_eq!(v.trim(), "2");
}

#[test]
fn record_success_resets_fail_counter() {
    let data = TempDir::new().expect("tempdir");
    let state = TempDir::new().expect("tempdir");

    // Pre-create a fail-count file at 2.
    let counter = fail_count_path(&state);
    fs::create_dir_all(counter.parent().unwrap()).expect("mkdir state/tsm");
    fs::write(&counter, "2\n").expect("seed counter");

    tsm_command(&data, &state)
        .env("TSM_SESSION_ID", TEST_SESSION_ID)
        .args(["record", "--exit-code", "0", "--last-command", "ls"])
        .assert()
        .success();

    // After success, counter must be "0" (or absent — both are acceptable).
    if counter.exists() {
        let v = fs::read_to_string(&counter).expect("read counter");
        assert_eq!(
            v.trim(),
            "0",
            "after success counter must be reset to 0, got {v:?}"
        );
    }
}

#[test]
fn record_redacts_aws_session_token() {
    let data = TempDir::new().expect("tempdir");
    let state = TempDir::new().expect("tempdir");

    tsm_command(&data, &state)
        .env("TSM_SESSION_ID", TEST_SESSION_ID)
        .env("AWS_SESSION_TOKEN", "abc-secret-do-not-leak")
        .env("MY_API_KEY", "another-secret")
        .args(["record", "--exit-code", "0", "--last-command", "ls"])
        .assert()
        .success();
    // Second invocation produces a PrecmdRecord (line 2).
    tsm_command(&data, &state)
        .env("TSM_SESSION_ID", TEST_SESSION_ID)
        .env("AWS_SESSION_TOKEN", "abc-secret-do-not-leak")
        .env("MY_API_KEY", "another-secret")
        .args(["record", "--exit-code", "0", "--last-command", "ls"])
        .assert()
        .success();

    let log_path = session_log_path(&data, TEST_SESSION_ID);
    let contents = fs::read_to_string(&log_path).expect("read log");
    let lines: Vec<&str> = contents.lines().collect();
    assert!(lines.len() >= 2, "expected header + record");
    let record: PrecmdRecord = serde_json::from_str(lines[1]).expect("line 2 is PrecmdRecord");
    assert!(
        !record.env.contains_key("AWS_SESSION_TOKEN"),
        "AWS_SESSION_TOKEN must be redacted out of env"
    );
    assert!(
        !record.env.contains_key("MY_API_KEY"),
        "MY_API_KEY must be redacted out of env"
    );
    assert!(
        record
            .redacted_keys
            .contains(&"AWS_SESSION_TOKEN".to_string()),
        "AWS_SESSION_TOKEN must be in redacted_keys"
    );
    assert!(
        record.redacted_keys.contains(&"MY_API_KEY".to_string()),
        "MY_API_KEY must be in redacted_keys"
    );
    // The secret value must never appear in the file.
    assert!(
        !contents.contains("abc-secret-do-not-leak"),
        "secret value leaked into log: {contents}"
    );
}

#[test]
fn record_probe_subprocess_kills_long_running_child() {
    let data = TempDir::new().expect("tempdir");
    let state = TempDir::new().expect("tempdir");

    let start = Instant::now();
    tsm_command(&data, &state)
        .env("TSM_SESSION_ID", TEST_SESSION_ID)
        .args([
            "record",
            "--exit-code",
            "0",
            "--last-command",
            "probe",
            "--probe-subprocess",
            "/bin/sleep 5",
        ])
        .assert()
        .success();
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(3500),
        "probe must time out around 2s, took {elapsed:?}"
    );
    assert!(
        elapsed > Duration::from_millis(1500),
        "probe completed suspiciously fast ({elapsed:?}); is the watchdog firing?"
    );
}

#[test]
fn record_writes_no_stderr_in_happy_path() {
    let data = TempDir::new().expect("tempdir");
    let state = TempDir::new().expect("tempdir");
    let output = tsm_command(&data, &state)
        .env("TSM_SESSION_ID", TEST_SESSION_ID)
        .args(["record", "--exit-code", "0", "--last-command", "ls"])
        .output()
        .expect("run tsm");
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty in happy path, got: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

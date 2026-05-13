//! Integration tests for `tsm doctor`.
//!
//! Each test invokes the `tsm` binary as a subprocess with a controlled HOME
//! and `XDG_CACHE_HOME` so test runs cannot pollute each other or read the
//! developer's real Zellij cache.

use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use tempfile::TempDir;
use tsm_id::session_id_from_tuple;
use tsm_tuple::{Env, LayoutText, derive_tuple};

/// Build a `Command` for `tsm doctor` with `env_clear` and a controlled HOME +
/// `XDG_CACHE_HOME` so it can never read the user's real Zellij cache.
fn tsm_doctor(home: &TempDir, cache: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("tsm").expect("tsm binary");
    cmd.env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
        )
        .env("HOME", home.path())
        .env("XDG_CACHE_HOME", cache.path());
    cmd
}

/// Path inside `cache` where Zellij would write `session-layout.kdl` for
/// `session`. Creates the parent directory if missing.
fn write_layout(cache: &TempDir, session: &str, body: &str) -> PathBuf {
    let dir = cache.path().join(session);
    fs::create_dir_all(&dir).expect("mkdir session dir");
    let path = dir.join("session-layout.kdl");
    fs::write(&path, body).expect("write layout fixture");
    path
}

#[test]
fn doctor_runs_with_no_zellij_env_and_emits_structured_sections() {
    let home = TempDir::new().expect("tempdir");
    let cache = TempDir::new().expect("tempdir");

    let output = tsm_doctor(&home, &cache)
        .env_remove("ZELLIJ_SESSION_NAME")
        .env_remove("ZELLIJ_PANE_ID")
        .env_remove("ZELLIJ_CACHE_DIR")
        .arg("doctor")
        .output()
        .expect("run tsm doctor");
    assert!(
        output.status.success(),
        "tsm doctor must exit 0, got {:?}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(stdout.contains("tsm doctor"), "header line missing: {stdout}");
    assert!(stdout.contains("Zellij environment:"), "Zellij section missing: {stdout}");
    assert!(stdout.contains("ZELLIJ_SESSION_NAME = (unset)"), "expected unset session name marker: {stdout}");
    assert!(stdout.contains("ZELLIJ_PANE_ID      = (unset)"), "expected unset pane id marker: {stdout}");
    assert!(stdout.contains("Layout file:"), "Layout file section missing: {stdout}");
    assert!(stdout.contains("Tuple derivation:"), "Tuple derivation section missing: {stdout}");
    assert!(stdout.contains("Session id:"), "Session id section missing: {stdout}");
    assert!(stdout.contains("source   = random"), "expected source = random for outside-Zellij: {stdout}");
    assert!(stdout.contains("Warnings:"), "Warnings section missing: {stdout}");
}

#[test]
fn doctor_warns_about_lopsided_zellij_env() {
    let home = TempDir::new().expect("tempdir");
    let cache = TempDir::new().expect("tempdir");

    let output = tsm_doctor(&home, &cache)
        .env("ZELLIJ_SESSION_NAME", "lopsided")
        .env_remove("ZELLIJ_PANE_ID")
        .env_remove("ZELLIJ_CACHE_DIR")
        .arg("doctor")
        .output()
        .expect("run tsm doctor");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(
        stdout.contains("ZELLIJ_SESSION_NAME is set but ZELLIJ_PANE_ID is not"),
        "expected lopsided-env warning, got:\n{stdout}"
    );
}

#[test]
fn doctor_warns_when_layout_file_missing() {
    let home = TempDir::new().expect("tempdir");
    let cache = TempDir::new().expect("tempdir");

    let output = tsm_doctor(&home, &cache)
        .env("ZELLIJ_SESSION_NAME", "ghost")
        .env("ZELLIJ_PANE_ID", "1")
        .env_remove("ZELLIJ_CACHE_DIR")
        // ZELLIJ_CACHE_DIR is unset, so doctor will resolve via XDG_CACHE_HOME.
        .arg("doctor")
        .output()
        .expect("run tsm doctor");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(
        stdout.contains("layout file not found"),
        "expected layout-not-found warning, got:\n{stdout}"
    );
    assert!(
        stdout.contains("exists   = no"),
        "expected exists = no, got:\n{stdout}"
    );
}

#[test]
fn doctor_derives_tuple_and_session_id_with_real_layout() {
    let home = TempDir::new().expect("tempdir");
    let cache = TempDir::new().expect("tempdir");

    let layout_body = r#"layout {
    tab name="editor" focus=true {
        pane id=1 command="nvim"
        pane id=2 cwd="/tmp"
    }
    tab name="logs" {
        pane id=3
        pane id=4
    }
}
"#;
    let layout_path = write_layout(&cache, "my-session", layout_body);

    // Compute expected session id via the same pure path.
    let env_inputs = Env {
        zellij_session_name: "my-session".to_string(),
        zellij_pane_id: "2".to_string(),
    };
    let tuple = derive_tuple(&env_inputs, &LayoutText(layout_body.to_string()))
        .expect("fixture must derive");
    let expected_id = session_id_from_tuple(&tuple).as_hex().to_string();

    let output = tsm_doctor(&home, &cache)
        .env("ZELLIJ_SESSION_NAME", "my-session")
        .env("ZELLIJ_PANE_ID", "2")
        .env("ZELLIJ_CACHE_DIR", cache.path())
        .arg("doctor")
        .output()
        .expect("run tsm doctor");
    assert!(
        output.status.success(),
        "tsm doctor exit status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(
        stdout.contains(&format!("path     = {}", layout_path.display())),
        "expected layout path line for {layout_path:?}, got:\n{stdout}"
    );
    assert!(stdout.contains("exists   = yes"), "expected exists = yes: {stdout}");
    assert!(stdout.contains("status   = ok"), "expected status = ok: {stdout}");
    assert!(
        stdout.contains("(session=my-session, tab=editor, ordinal=1)"),
        "expected pretty tuple, got:\n{stdout}"
    );
    assert!(
        stdout.contains("source   = deterministic"),
        "expected source = deterministic, got:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("value    = {expected_id}")),
        "expected session id {expected_id}, got:\n{stdout}"
    );
    // Happy path should emit no warnings.
    assert!(
        stdout.contains("Warnings:\n  none"),
        "expected no warnings, got:\n{stdout}"
    );
}

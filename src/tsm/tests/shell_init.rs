//! Integration tests for the `tsm shell-init zsh` Zellij branch.
//!
//! When invoked inside a Zellij pane (with `ZELLIJ_SESSION_NAME` and
//! `ZELLIJ_PANE_ID` set and a readable `session-layout.kdl` on disk),
//! `tsm shell-init zsh` must inline a **deterministic** SessionId derived
//! from the Zellij tuple — not the random fallback. Outside Zellij, the
//! random fallback path is used and the deterministic id from the same
//! tuple must not appear in the output.

use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use tempfile::TempDir;
use tsm_id::session_id_from_tuple;
use tsm_tuple::{Env, LayoutText, derive_tuple};

const LAYOUT_BODY: &str = r#"layout {
    tab name="editor" focus=true {
        pane id=1 command="nvim"
        pane id=42 cwd="/tmp"
    }
    tab name="logs" {
        pane id=99
    }
}
"#;

/// Build a `tsm shell-init zsh` command with `env_clear` plus a controlled
/// HOME and `XDG_CACHE_HOME`.
fn shell_init(home: &TempDir, cache: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("tsm").expect("tsm binary");
    cmd.env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
        )
        .env("HOME", home.path())
        .env("XDG_CACHE_HOME", cache.path())
        .args(["shell-init", "zsh"]);
    cmd
}

/// Write a Zellij layout fixture into `<cache>/<session>/session-layout.kdl`.
fn write_layout_fixture(cache: &TempDir, session: &str, body: &str) -> PathBuf {
    let dir = cache.path().join(session);
    fs::create_dir_all(&dir).expect("mkdir session dir");
    let path = dir.join("session-layout.kdl");
    fs::write(&path, body).expect("write layout fixture");
    path
}

/// Compute the deterministic session id for the supplied tuple via the pure
/// `tsm-id` API — this is what the binary should embed in the snippet.
fn expected_deterministic_id(session: &str, pane: &str, layout: &str) -> String {
    let env = Env {
        zellij_session_name: session.to_string(),
        zellij_pane_id: pane.to_string(),
    };
    let tuple = derive_tuple(&env, &LayoutText(layout.to_string()))
        .expect("fixture must derive");
    session_id_from_tuple(&tuple).as_hex().to_string()
}

#[test]
fn shell_init_uses_deterministic_id_when_inside_zellij() {
    let home = TempDir::new().expect("tempdir");
    let cache = TempDir::new().expect("tempdir");
    write_layout_fixture(&cache, "live-session", LAYOUT_BODY);

    let expected = expected_deterministic_id("live-session", "42", LAYOUT_BODY);

    let output = shell_init(&home, &cache)
        .env("ZELLIJ_SESSION_NAME", "live-session")
        .env("ZELLIJ_PANE_ID", "42")
        .env("ZELLIJ_CACHE_DIR", cache.path())
        .output()
        .expect("run tsm shell-init zsh");
    assert!(
        output.status.success(),
        "shell-init must succeed: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(
        stdout.contains(&expected),
        "emitted snippet must contain deterministic id {expected}:\n{stdout}"
    );
    // It should appear exactly once (in the `TSM_SESSION_ID=` line).
    assert_eq!(
        stdout.matches(&expected).count(),
        1,
        "deterministic id should appear exactly once in the snippet:\n{stdout}"
    );
}

#[test]
fn shell_init_uses_random_id_outside_zellij() {
    let home = TempDir::new().expect("tempdir");
    let cache = TempDir::new().expect("tempdir");
    // Still write the fixture so we can compute its deterministic id and
    // assert it is NOT present in the outside-Zellij snippet.
    write_layout_fixture(&cache, "live-session", LAYOUT_BODY);
    let deterministic = expected_deterministic_id("live-session", "42", LAYOUT_BODY);

    let output = shell_init(&home, &cache)
        .env_remove("ZELLIJ_SESSION_NAME")
        .env_remove("ZELLIJ_PANE_ID")
        .env_remove("ZELLIJ_CACHE_DIR")
        .output()
        .expect("run tsm shell-init zsh");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(
        !stdout.contains(&deterministic),
        "outside Zellij the deterministic id {deterministic} must not appear (we should use a random id):\n{stdout}"
    );
}

#[test]
fn shell_init_falls_back_to_random_when_layout_file_missing() {
    let home = TempDir::new().expect("tempdir");
    let cache = TempDir::new().expect("tempdir");
    // ZELLIJ_SESSION_NAME / ZELLIJ_PANE_ID are set but no fixture is written.
    let deterministic = expected_deterministic_id("ghost", "1", LAYOUT_BODY);

    let output = shell_init(&home, &cache)
        .env("ZELLIJ_SESSION_NAME", "ghost")
        .env("ZELLIJ_PANE_ID", "1")
        .env("ZELLIJ_CACHE_DIR", cache.path())
        .output()
        .expect("run tsm shell-init zsh");
    assert!(
        output.status.success(),
        "shell-init must remain silent and exit 0 on failure to derive: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    // Failure to derive MUST NOT write to stderr (would break the user's shell).
    assert!(
        output.stderr.is_empty(),
        "shell-init must not write to stderr on derive failure: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(
        !stdout.contains(&deterministic),
        "deterministic id must not appear when layout file is missing: {stdout}"
    );
}

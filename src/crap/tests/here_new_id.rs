//! End-to-end tests for `crap --here <session> [<new-id>]` driving the real
//! binary, verifying the optional forced new session id rides through to the
//! here-mode protocol the shell function consumes.
//!
//! These run the compiled `crap` against a throwaway `HOME`, so they exercise
//! CLI parsing, validation, and output formatting together — the glue that the
//! in-crate unit tests cannot reach because `run_here` calls `exit`.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

/// A resolvable original session id, created in every throwaway `HOME`.
const ORIG: &str = "11111111-2222-3333-4444-555555555555";
/// A well-formed UUID used as the caller-supplied forked-session id.
const NEW: &str = "99999999-8888-7777-6666-555555555555";

// These mirror the binary's `--here` wire protocol (see `format_here_output`
// in `main.rs`); an integration test deliberately re-states the contract it is
// pinning rather than reaching into private constants.
const HERE_SENTINEL: &str = "__CRAP_HERE__";
const NO_NEW_ID_SENTINEL: &str = "__CRAP_NO_NEW_ID__";

/// A process-unique temp directory, keyed on pid + nanoseconds so concurrent
/// runs of this test never share state.
fn unique_root(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("crap-it-{tag}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Sets up a throwaway `HOME` containing the resolvable session [`ORIG`], then
/// runs the real `crap --here ORIG <extra_args>` binary from a fresh working
/// directory. Returns the captured process output.
fn run_here(tag: &str, extra_args: &[&str]) -> Output {
    run_here_planting(tag, extra_args, &[])
}

/// Like [`run_here`], but also plants a transcript for each id in `also_plant`,
/// so collisions with an existing session can be exercised.
fn run_here_planting(tag: &str, extra_args: &[&str], also_plant: &[&str]) -> Output {
    let root = unique_root(tag);
    let home = root.join("home");
    let projects = home.join(".claude").join("projects");

    let session_folder = projects.join("orig-project");
    fs::create_dir_all(&session_folder).unwrap();
    fs::write(
        session_folder.join(format!("{ORIG}.jsonl")),
        "{\"cwd\":\"/x\"}\n",
    )
    .unwrap();

    let extra_folder = projects.join("other-project");
    fs::create_dir_all(&extra_folder).unwrap();
    for id in also_plant {
        fs::write(extra_folder.join(format!("{id}.jsonl")), "{}\n").unwrap();
    }

    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();

    let mut args = vec!["--here", ORIG];
    args.extend_from_slice(extra_args);

    let output = Command::new(env!("CARGO_BIN_EXE_crap"))
        .env("HOME", &home)
        .current_dir(&work)
        .args(&args)
        .output()
        .expect("crap binary should run");

    let _ = fs::remove_dir_all(&root);
    output
}

#[test]
fn here_pins_supplied_new_session_id() {
    let out = run_here("pins", &[NEW]);
    assert!(
        out.status.success(),
        "exit status was {:?}, stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.first().copied(), Some(HERE_SENTINEL));
    assert_eq!(lines.get(1).copied(), Some(ORIG));
    // The forced id rides as the third field for the shell's `--session-id`.
    assert_eq!(lines.get(2).copied(), Some(NEW));
}

#[test]
fn here_without_new_id_uses_sentinel() {
    let out = run_here("plain", &[]);
    assert!(
        out.status.success(),
        "exit status was {:?}, stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    // Without a forced id the third field is the sentinel, so the shell lets
    // Claude mint a fresh random id.
    assert_eq!(lines.get(2).copied(), Some(NO_NEW_ID_SENTINEL));
}

#[test]
fn here_rejects_a_new_id_that_already_exists() {
    // Pinning the fork to an id that already names a transcript would overwrite
    // it, so `--here` must refuse before launching Claude.
    let out = run_here_planting("collide", &[NEW], &[NEW]);
    assert!(
        !out.status.success(),
        "a colliding new id must abort the resume"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("already exists"), "stderr: {stderr}");
}

#[test]
fn here_rejects_invalid_new_id() {
    let out = run_here("bad", &["not-a-uuid"]);
    assert!(
        !out.status.success(),
        "a malformed new id must abort the resume"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not a valid session id"),
        "stderr: {stderr}"
    );
}

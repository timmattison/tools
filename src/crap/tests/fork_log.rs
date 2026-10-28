//! End-to-end tests for the fork log: each fork that `crap` makes adds one
//! JSON line to `forks.jsonl` under the data directory of the user.
//!
//! The tests run the compiled `crap` against a throwaway `HOME` (see
//! `common::crap_command`), so the log lands in a temporary directory and
//! never in the real data directory.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

/// A resolvable original session id, planted in every throwaway `HOME`.
const ORIG: &str = "11111111-2222-3333-4444-555555555555";
/// The sentinel of the `--here` wire format (see `format_here_output`).
const HERE_SENTINEL: &str = "__CRAP_HERE__";

/// A temp directory of its own for one run. It removes itself on drop.
fn unique_root(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("crap-fl-{tag}-"))
        .tempdir()
        .unwrap()
}

/// Where the binary puts the fork log for `home` (see `dirs::data_dir`).
fn fork_log_path(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library/Application Support/crap/forks.jsonl")
    }
    #[cfg(not(target_os = "macos"))]
    {
        home.join(".local/share/crap/forks.jsonl")
    }
}

/// Plants a transcript for `id` under `projects_dir/<folder>` that records
/// `cwd` as its working directory.
fn plant(projects_dir: &Path, folder: &str, id: &str, cwd: &str) {
    let dir = projects_dir.join(folder);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{id}.jsonl")),
        format!("{{\"cwd\":\"{cwd}\"}}\n"),
    )
    .unwrap();
}

/// Makes `root/home` with [`ORIG`] planted (recorded cwd `/x/orig`) and
/// `root/work`. Returns `(home, work)`.
fn setup_here(root: &Path) -> (PathBuf, PathBuf) {
    let home = root.join("home");
    plant(
        &home.join(".claude/projects"),
        "orig-project",
        ORIG,
        "/x/orig",
    );
    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();
    (home, work)
}

/// Runs `crap --here ORIG` from `work` with `HOME` set to `home`.
fn run_here(home: &Path, work: &Path) -> Output {
    common::crap_command(home)
        .current_dir(work)
        .args(["--here", ORIG])
        .output()
        .expect("crap binary should run")
}

/// Returns the lines of the fork log, each parsed as JSON.
fn log_lines(log: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(log)
        .unwrap_or_else(|err| panic!("fork log {} unreadable: {err}", log.display()))
        .lines()
        .map(|line| serde_json::from_str(line).expect("each log line is JSON"))
        .collect()
}

/// Returns the third line of the wire output: the fork id.
fn wire_fork_id(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .nth(2)
        .expect("the wire output has a third line")
        .to_string()
}

#[test]
fn here_adds_one_line_with_every_field() {
    let tmp = unique_root("here");
    let (home, work) = setup_here(tmp.path());

    let output = run_here(&home, &work);
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with(HERE_SENTINEL),
        "{output:?}"
    );

    let lines = log_lines(&fork_log_path(&home));
    assert_eq!(lines.len(), 1, "exactly one line: {lines:?}");
    let line = &lines[0];
    assert_eq!(line.as_object().unwrap().len(), 7, "seven keys: {line}");
    assert_eq!(line["source_id"], ORIG);
    assert_eq!(line["fork_id"], wire_fork_id(&output));
    assert!(common::is_generated_fork_id(
        line["fork_id"].as_str().unwrap()
    ));
    assert_eq!(line["source_user"], "home");
    assert_eq!(line["source_dir"], "/x/orig");
    assert_eq!(
        line["fork_dir"],
        fs::canonicalize(&work).unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(line["kind"], "here");
    let time = line["time"].as_str().unwrap();
    let bytes = time.as_bytes();
    assert!(
        time.len() == 20
            && time.ends_with('Z')
            && bytes[4] == b'-'
            && bytes[10] == b'T'
            && bytes[13] == b':',
        "time is RFC 3339 in UTC with seconds: {time}"
    );
}

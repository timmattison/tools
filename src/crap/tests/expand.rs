//! End-to-end tests for `crap --here --expand`, driving the real binary.
//!
//! `--expand` writes an expanded import: a copy of a transcript under a new
//! id, with each compaction undone. The tests build realistic transcripts in a
//! throwaway `HOME`, run `crap`, and read what it printed and wrote.

mod common;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{json, Value};

/// The id of the original session, planted in every throwaway `HOME`.
const ORIG: &str = "11111111-2222-3333-4444-555555555555";
/// A well-formed UUID that a test gives as the id of the fork.
const NEW: &str = "99999999-8888-7777-6666-555555555555";

// These mirror the wire contract of `--here` (see `format_here_output` in
// `main.rs`). The test file re-states the contract it pins.
const HERE_SENTINEL: &str = "__CRAP_HERE__";

/// The uuids of a transcript with one partial compaction, oldest first.
const BEFORE: [&str; 4] = ["m1", "m2", "m3", "m4"];
const AFTER: [&str; 2] = ["m5", "m6"];

/// One throwaway world: a `HOME` that holds the original transcript, the
/// directory of the session, and the directory that `crap` runs in.
struct World {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    /// The directory that `crap` runs in, as the child process sees it.
    work: PathBuf,
    /// The path of the original transcript.
    original: PathBuf,
}

impl World {
    /// Makes a world whose original transcript holds the lines that `make`
    /// builds. `make` gets the `cwd` that the lines record: a real directory
    /// of the world.
    fn new(make: impl FnOnce(&str) -> Vec<Value>) -> Self {
        let tmp = tempfile::Builder::new()
            .prefix("crap-expand-")
            .tempdir()
            .unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        let work = root.join("work");
        fs::create_dir_all(&work).unwrap();
        let session_dir = root.join("session-dir");
        fs::create_dir_all(&session_dir).unwrap();
        let lines = make(&session_dir.to_string_lossy());
        let folder = home.join(".claude/projects/orig-project");
        fs::create_dir_all(&folder).unwrap();
        let original = folder.join(format!("{ORIG}.jsonl"));
        fs::write(&original, transcript_bytes(&lines)).unwrap();
        Self {
            _tmp: tmp,
            home,
            work,
            original,
        }
    }

    /// Runs `crap <args>` from the work directory.
    fn run(&self, args: &[&str]) -> Output {
        common::crap_command(&self.home)
            .current_dir(&self.work)
            .args(args)
            .output()
            .expect("crap binary should run")
    }

    /// The projects tree of the throwaway `HOME`.
    fn projects(&self) -> PathBuf {
        self.home.join(".claude/projects")
    }

    /// The project folder that `crap --here` imports into.
    fn work_folder(&self) -> PathBuf {
        self.projects().join(encode_project_dir(&self.work))
    }

    /// Where the binary puts the fork log (see `dirs::data_dir`).
    fn fork_log(&self) -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            self.home
                .join("Library/Application Support/crap/forks.jsonl")
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.home.join(".local/share/crap/forks.jsonl")
        }
    }
}

/// Joins `lines` into the bytes of a transcript, as Claude writes them.
fn transcript_bytes(lines: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for line in lines {
        bytes.extend_from_slice(serde_json::to_string(line).unwrap().as_bytes());
        bytes.push(b'\n');
    }
    bytes
}

/// The folder name that Claude gives to a working directory: each character
/// that is not an ASCII letter or digit becomes `-`.
fn encode_project_dir(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// A user or assistant message line.
fn message(uuid: &str, parent: Option<&str>, kind: &str, cwd: &str) -> Value {
    json!({
        "parentUuid": parent,
        "isSidechain": false,
        "type": kind,
        "message": { "role": kind, "content": format!("text of {uuid}") },
        "uuid": uuid,
        "timestamp": "2026-11-01T10:00:00.000Z",
        "cwd": cwd,
        "sessionId": ORIG,
        "version": "2.1.318",
    })
}

/// A compact boundary line.
fn boundary(uuid: &str, logical_parent: &str, pre_tokens: u64, cwd: &str) -> Value {
    json!({
        "parentUuid": null,
        "logicalParentUuid": logical_parent,
        "isSidechain": false,
        "type": "system",
        "subtype": "compact_boundary",
        "content": "Conversation compacted",
        "level": "info",
        "compactMetadata": {
            "trigger": "manual",
            "preTokens": pre_tokens,
            "postTokens": 21332,
        },
        "uuid": uuid,
        "timestamp": "2026-11-01T11:00:00.000Z",
        "cwd": cwd,
        "sessionId": ORIG,
        "version": "2.1.318",
    })
}

/// A compact summary line.
fn summary(uuid: &str, parent: &str, cwd: &str) -> Value {
    json!({
        "parentUuid": parent,
        "isSidechain": false,
        "type": "user",
        "message": {
            "role": "user",
            "content": "This session is being continued from a previous conversation",
        },
        "isVisibleInTranscriptOnly": true,
        "isCompactSummary": true,
        "uuid": uuid,
        "timestamp": "2026-11-01T11:00:01.000Z",
        "session_id": ORIG,
        "cwd": cwd,
        "sessionId": ORIG,
        "version": "2.1.318",
    })
}

/// A chain of messages that alternate between user and assistant. The first
/// message gets `first_parent`.
fn chain(uuids: &[&str], first_parent: Option<&str>, cwd: &str) -> Vec<Value> {
    let mut lines = Vec::new();
    let mut parent = first_parent.map(str::to_string);
    for (index, uuid) in uuids.iter().enumerate() {
        let kind = if index % 2 == 0 { "user" } else { "assistant" };
        lines.push(message(uuid, parent.as_deref(), kind, cwd));
        parent = Some((*uuid).to_string());
    }
    lines
}

/// A transcript with one partial compaction: `m1..m4`, boundary `b1`, summary
/// `s1`, then `m5` and `m6`.
fn one_compaction(cwd: &str) -> Vec<Value> {
    let mut lines = chain(&BEFORE, None, cwd);
    lines.push(boundary("b1", "m4", 1000, cwd));
    lines.push(summary("s1", "b1", cwd));
    lines.extend(chain(&AFTER, Some("s1"), cwd));
    lines
}

/// Walks a transcript the way Claude loads it. It starts at the last line that
/// has a uuid, a `type` of `user` or `assistant`, and no `isSidechain`. It
/// follows `parentUuid` to the root. The last line with a uuid wins. The result
/// lists the uuids from the last message to the first. It panics on a cycle and
/// on an unknown uuid.
fn walk(transcript: &[u8]) -> Vec<String> {
    let values: Vec<Value> = std::str::from_utf8(transcript)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let mut parents: HashMap<String, Option<String>> = HashMap::new();
    for value in &values {
        if let Some(uuid) = value["uuid"].as_str() {
            parents.insert(
                uuid.to_string(),
                value["parentUuid"].as_str().map(str::to_string),
            );
        }
    }
    let last = values
        .iter()
        .rev()
        .find(|value| {
            value["uuid"].is_string()
                && matches!(value["type"].as_str(), Some("user" | "assistant"))
                && value["isSidechain"] != json!(true)
        })
        .expect("the transcript has a message");
    let mut current = last["uuid"].as_str().map(str::to_string);
    let mut seen = Vec::new();
    while let Some(uuid) = current {
        assert!(!seen.contains(&uuid), "cycle at {uuid}");
        let parent = parents
            .get(&uuid)
            .unwrap_or_else(|| panic!("unknown uuid {uuid}"))
            .clone();
        seen.push(uuid);
        current = parent;
    }
    seen
}

/// Splits the stdout of a successful `--here` run into its four fields.
fn here_fields(out: &Output) -> Vec<String> {
    assert!(
        out.status.success(),
        "exit status was {:?}, stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout.clone()).unwrap();
    assert!(stdout.ends_with('\n'), "stdout: {stdout:?}");
    let fields: Vec<String> = stdout.lines().map(str::to_string).collect();
    assert_eq!(fields.len(), 4, "stdout: {stdout:?}");
    assert_eq!(fields[0], HERE_SENTINEL);
    fields
}

/// A world with the transcript of [`one_compaction`].
fn compacted_world() -> World {
    World::new(one_compaction)
}

#[test]
fn here_expand_prints_the_wire_shape_and_writes_the_import() {
    let world = compacted_world();
    let before = fs::read(&world.original).unwrap();

    let out = world.run(&["--here", "--expand", ORIG]);
    let fields = here_fields(&out);

    let import_id = &fields[1];
    let fork_id = &fields[2];
    assert!(common::is_generated_fork_id(import_id), "{import_id:?}");
    assert!(common::is_generated_fork_id(fork_id), "{fork_id:?}");
    assert_ne!(import_id, ORIG);
    assert_ne!(import_id, fork_id);

    let link = world.work_folder().join(format!("{import_id}.jsonl"));
    assert_eq!(fields[3], link.display().to_string());
    let meta = fs::symlink_metadata(&link).expect("the import exists");
    assert!(meta.file_type().is_file(), "the import is a regular file");

    assert_eq!(
        fs::read(&world.original).unwrap(),
        before,
        "the original transcript is only read"
    );
}

/// Runs `crap --here --expand ORIG` in `world` and returns the bytes of the
/// import that it wrote.
fn expanded_import(world: &World) -> Vec<u8> {
    let out = world.run(&["--here", "--expand", ORIG]);
    let fields = here_fields(&out);
    fs::read(world.work_folder().join(format!("{}.jsonl", fields[1]))).unwrap()
}

#[test]
fn the_import_holds_the_full_history_without_the_compaction() {
    let world = compacted_world();
    let import = expanded_import(&world);

    // The last message first, the first message last: each message once.
    let expected: Vec<String> = AFTER
        .iter()
        .rev()
        .chain(BEFORE.iter().rev())
        .map(|uuid| (*uuid).to_string())
        .collect();
    assert_eq!(walk(&import), expected);

    for line in String::from_utf8(import).unwrap().lines() {
        let value: Value = serde_json::from_str(line).unwrap();
        assert_ne!(value["subtype"], json!("compact_boundary"), "{line}");
        assert_ne!(value["isCompactSummary"], json!(true), "{line}");
    }
}

//! End-to-end tests of the op-cache audit trail. Each test builds a fixture
//! with a fake `op` on PATH and runs the real `op-cache` binary against it.
//! The process environment of the test is never changed.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;

const REFERENCE: &str = "op://Private/Item/field";

/// One throwaway world: a git repo, a fake `op`, and private state directories.
struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create fixture dir");
        for dir in ["bin", "home", "state", "claude", "repo"] {
            fs::create_dir_all(root.path().join(dir)).expect("create fixture subdir");
        }
        let mut git = Command::new("git");
        gitscratch::shed_inherited_git_environment(&mut git);
        let status = git
            .args(["init", "-q"])
            .current_dir(root.path().join("repo"))
            .status()
            .expect("run git init");
        assert!(status.success(), "git init failed");
        Self { root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    /// Writes an executable `#!/bin/sh` script named `name` into `<fixture>/bin`.
    fn install_script(&self, name: &str, body: &str) {
        let script = self.path("bin").join(name);
        fs::write(&script, format!("#!/bin/sh\n{body}\n")).expect("write script");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod script");
    }

    fn install_fake_op(&self, body: &str) {
        self.install_script("op", body);
    }

    /// The audit file: `<fixture>/state/op-cache/audit.jsonl`.
    fn audit_file(&self) -> PathBuf {
        self.path("state").join("op-cache").join("audit.jsonl")
    }

    /// A command for the real binary with a hermetic environment.
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_op-cache"));
        gitscratch::shed_inherited_git_environment(&mut command);
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let mut path = self.path("bin").into_os_string();
        path.push(":");
        path.push(inherited);
        command
            .env("PATH", path)
            .env("HOME", self.path("home"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("CLAUDE_CONFIG_DIR", self.path("claude"))
            .env("OP_CACHE_NOTIFY", "0")
            .env_remove("ZELLIJ_SESSION_NAME")
            .env_remove("ZELLIJ_PANE_ID")
            .env_remove("TERM_PROGRAM")
            .current_dir(self.path("repo"));
        command
    }

    fn read(&self, reference: &str) -> Output {
        self.command()
            .args(["read", reference])
            .output()
            .expect("run op-cache")
    }
}

/// A token no other test shares, so a leak of the value is easy to see.
fn unique_token(label: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    format!("SENTINEL-{label}-{}-{nanos}", std::process::id())
}

/// The raw text of the audit file.
fn audit_text(fixture: &Fixture) -> String {
    let path = fixture.audit_file();
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("no audit file at {}: {error}", path.display()))
}

/// Each line of the audit file, parsed as JSON.
fn records(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("bad JSON {line:?}: {e}")))
        .collect()
}

/// True when `text` looks like `2026-11-06T15:38:24.333Z`.
fn is_millisecond_utc_time(text: &str) -> bool {
    const SHAPE: &str = "dddd-dd-ddTdd:dd:dd.dddZ";
    text.chars().count() == SHAPE.len()
        && text
            .chars()
            .zip(SHAPE.chars())
            .all(|(got, want)| match want {
                'd' => got.is_ascii_digit(),
                other => got == other,
            })
}

#[test]
fn read_prints_the_value_byte_for_byte() {
    let fixture = Fixture::new();
    let sentinel = unique_token("stdout");
    fixture.install_fake_op(&format!("echo '{sentinel}'"));

    let output = fixture.read(REFERENCE);

    assert!(
        output.status.success(),
        "op-cache failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, sentinel.as_bytes());
}

#[test]
fn read_writes_one_op_start_and_one_ok_op_end_and_never_the_value() {
    let fixture = Fixture::new();
    let sentinel = unique_token("record");
    fixture.install_fake_op(&format!("echo '{sentinel}'"));

    let output = fixture.read(REFERENCE);
    assert!(output.status.success());

    let text = audit_text(&fixture);
    assert!(
        !text.contains(&sentinel),
        "the secret value reached the trail"
    );
    let records = records(&text);
    assert_eq!(records.len(), 2, "audit file: {text}");

    let (start, end) = (&records[0], &records[1]);
    assert_eq!(start["event"], "op_start");
    assert_eq!(start["attempt"], 1);
    assert_eq!(start["operation"], "read");
    assert_eq!(start["reference"], REFERENCE);
    assert!(
        start["cache_path"]
            .as_str()
            .is_some_and(|path| path.ends_with(".op-cache.json")),
        "cache_path: {}",
        start["cache_path"]
    );
    assert_eq!(end["event"], "op_end");
    assert_eq!(end["outcome"], "ok");
    assert_eq!(end["exit_code"], 0);
    assert!(
        end.get("op_stderr").is_none(),
        "an ok record has no op_stderr"
    );
    assert!(start["request_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
    assert_eq!(start["request_id"], end["request_id"]);
    for record in [start, end] {
        let time = record["time"].as_str().unwrap_or_default();
        assert!(is_millisecond_utc_time(time), "bad time {time:?}");
    }
}

#[test]
fn failed_op_writes_three_failed_pairs_with_its_stderr() {
    let fixture = Fixture::new();
    let message = "[ERROR] authorization prompt dismissed";
    fixture.install_fake_op(&format!("echo '{message}' >&2\nexit 1"));

    let output = fixture.read(REFERENCE);

    assert!(!output.status.success(), "op-cache should fail");
    assert!(output.stdout.is_empty(), "nothing may reach stdout");
    let records = records(&audit_text(&fixture));
    assert_eq!(records.len(), 6, "three pairs expected: {records:?}");
    let mut request_ids = Vec::new();
    for (index, pair) in records.chunks(2).enumerate() {
        let (start, end) = (&pair[0], &pair[1]);
        let attempt = u64::try_from(index + 1).expect("small index");
        assert_eq!(start["event"], "op_start");
        assert_eq!(start["attempt"], attempt);
        assert_eq!(end["event"], "op_end");
        assert_eq!(end["attempt"], attempt);
        assert_eq!(start["request_id"], end["request_id"]);
        assert_eq!(end["outcome"], "failed");
        assert_eq!(end["exit_code"], 1);
        assert_eq!(end["op_stderr"], message);
        request_ids.push(start["request_id"].as_str().expect("id").to_owned());
    }
    request_ids.sort();
    request_ids.dedup();
    assert_eq!(request_ids.len(), 3, "each attempt has its own request_id");
}

#[test]
fn op_stderr_is_truncated_to_512_bytes_on_a_char_boundary() {
    let fixture = Fixture::new();
    let full = "日".repeat(300);
    fixture.install_fake_op(&format!("printf '%s' '{full}' >&2\nexit 1"));

    let output = fixture.read(REFERENCE);

    assert!(!output.status.success(), "op-cache should fail");
    let records = records(&audit_text(&fixture));
    let ends: Vec<_> = records
        .iter()
        .filter(|record| record["event"] == "op_end")
        .collect();
    assert_eq!(ends.len(), 3);
    for end in ends {
        let stderr = end["op_stderr"].as_str().expect("op_stderr is text");
        assert!(!stderr.is_empty(), "op_stderr is empty");
        assert!(stderr.len() <= 512, "op_stderr has {} bytes", stderr.len());
        assert!(full.starts_with(stderr), "op_stderr is not a prefix");
        assert_eq!(stderr.len(), 510);
        assert_eq!(stderr.chars().count(), 170);
    }
}

#[test]
fn unstartable_op_writes_spawn_error_pairs() {
    let fixture = Fixture::new();
    let interpreter = unique_token("interpreter");
    let script = fixture.path("bin").join("op");
    fs::write(&script, format!("#!/nonexistent/{interpreter}\n")).expect("write script");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod script");

    // A failed execve of one PATH entry makes execvp go on to the next entry,
    // so a real `op` further along PATH would run. PATH holds the fixture and the
    // system directories (for `git`), which have no `op`.
    let mut path = fixture.path("bin").into_os_string();
    path.push(":/usr/bin:/bin");
    let output = fixture
        .command()
        .env("PATH", path)
        .args(["read", REFERENCE])
        .output()
        .expect("run op-cache");

    assert!(!output.status.success(), "op-cache should fail");
    let records = records(&audit_text(&fixture));
    assert_eq!(records.len(), 6, "three pairs expected: {records:?}");
    for pair in records.chunks(2) {
        let (start, end) = (&pair[0], &pair[1]);
        assert_eq!(start["event"], "op_start");
        assert_eq!(end["event"], "op_end");
        assert_eq!(start["request_id"], end["request_id"]);
        assert_eq!(end["outcome"], "spawn_error");
        assert!(
            end["error"].as_str().is_some_and(|text| !text.is_empty()),
            "error: {}",
            end["error"]
        );
        assert!(
            end["exit_code"].is_null(),
            "exit_code: {}",
            end["exit_code"]
        );
        assert!(
            end.get("op_stderr").is_none(),
            "no op_stderr on a spawn error"
        );
    }
}

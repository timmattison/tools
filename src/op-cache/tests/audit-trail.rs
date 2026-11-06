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
        // A failed execve of one PATH entry makes execvp go on to the next entry,
        // so a real `op` further along PATH would run. /usr/bin holds `git`;
        // neither it nor /bin holds an `op`. The PATH of the test process is
        // never read.
        let mut path = self.path("bin").into_os_string();
        path.push(":/usr/bin:/bin");
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

    let output = fixture.read(REFERENCE);

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

#[test]
fn read_binary_writes_a_pair_with_operation_read_binary() {
    let fixture = Fixture::new();
    let bytes = unique_token("binary");
    // Called as: op read --out-file <path> <reference>
    fixture.install_fake_op(&format!("printf '%s' '{bytes}' > \"$3\""));

    let output = fixture
        .command()
        .args(["read-binary", "op://Private/Item/file.png", "out/file.png"])
        .output()
        .expect("run op-cache");

    assert!(
        output.status.success(),
        "op-cache failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let written = fs::read(fixture.path("repo").join("out").join("file.png")).expect("out file");
    assert_eq!(written, bytes.as_bytes());
    let text = audit_text(&fixture);
    assert!(!text.contains(&bytes), "the file bytes reached the trail");
    let records = records(&text);
    assert_eq!(records.len(), 2, "audit file: {text}");
    assert_eq!(records[0]["event"], "op_start");
    assert_eq!(records[0]["operation"], "read-binary");
    assert_eq!(records[0]["reference"], "op://Private/Item/file.png");
    assert_eq!(records[1]["event"], "op_end");
    assert_eq!(records[1]["outcome"], "ok");
}

const OP_MUST_NOT_RUN: &str = "echo 'op must not run' >&2\nexit 1";

#[test]
fn cache_hit_writes_no_record() {
    let fixture = Fixture::new();
    fixture.install_fake_op(OP_MUST_NOT_RUN);
    fs::write(
        fixture.path("repo").join(".op-cache.json"),
        format!(
            r#"{{"{REFERENCE}":{{"value":"cached-value","fetchedAt":"2026-01-01T00:00:00Z"}}}}"#
        ),
    )
    .expect("write cache");

    let output = fixture.read(REFERENCE);

    assert!(
        output.status.success(),
        "op-cache failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"cached-value");
    assert!(!fixture.audit_file().exists(), "a cache hit wrote a record");
}

#[test]
fn env_var_override_writes_no_record() {
    let fixture = Fixture::new();
    fixture.install_fake_op(OP_MUST_NOT_RUN);
    let variable = unique_token("ENVVAR").replace('-', "_");

    let output = fixture
        .command()
        .args(["read", REFERENCE, "--env-var", &variable])
        .env(&variable, "from-env")
        .output()
        .expect("run op-cache");

    assert!(
        output.status.success(),
        "op-cache failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"from-env");
    assert!(!fixture.audit_file().exists(), "an override wrote a record");
}

#[test]
fn blocked_audit_directory_gives_one_warning_and_the_read_succeeds() {
    let fixture = Fixture::new();
    let sentinel = unique_token("blocked");
    fixture.install_fake_op(&format!("echo '{sentinel}'"));
    fs::write(fixture.path("state").join("op-cache"), "not a directory").expect("block dir");

    let output = fixture.read(REFERENCE);

    assert!(
        output.status.success(),
        "op-cache failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, sentinel.as_bytes());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.lines().count(), 1, "stderr: {stderr}");
    assert!(stderr.contains("warning:"), "stderr: {stderr}");
}

const SESSION_NAME: &str = "name with \" and \\ in it";

#[test]
fn op_start_names_the_ancestors_the_claude_session_and_the_terminal() {
    let fixture = Fixture::new();
    fixture.install_fake_op("echo value");
    let session_id = unique_token("session").replace("SENTINEL", "sess");
    let zellij_session = unique_token("zellij").replace("SENTINEL", "zs");
    let test_pid = std::process::id();
    let sessions = fixture.path("claude").join("sessions");
    fs::create_dir_all(&sessions).expect("create sessions dir");
    let session = serde_json::json!({
        "pid": test_pid,
        "sessionId": session_id,
        "name": SESSION_NAME,
    });
    fs::write(
        sessions.join(format!("{test_pid}.json")),
        session.to_string(),
    )
    .expect("write session file");

    let output = fixture
        .command()
        .args(["read", REFERENCE])
        .env("ZELLIJ_SESSION_NAME", &zellij_session)
        .env("ZELLIJ_PANE_ID", "7")
        .env("TERM_PROGRAM", "WezTerm")
        .output()
        .expect("run op-cache");

    assert!(
        output.status.success(),
        "op-cache failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = records(&audit_text(&fixture));
    let start = &records[0];
    assert_eq!(start["event"], "op_start");
    let ancestors = start["ancestors"].as_array().expect("ancestors array");
    assert!(!ancestors.is_empty(), "no ancestors");
    assert!(ancestors.len() <= 32, "{} ancestors", ancestors.len());
    assert_eq!(ancestors[0]["pid"], test_pid);
    assert!(ancestors[0]["name"]
        .as_str()
        .is_some_and(|name| !name.is_empty()));
    assert_eq!(
        start["claude"],
        serde_json::json!({
            "pid": test_pid,
            "session_id": session_id,
            "name": SESSION_NAME,
        })
    );
    assert_eq!(start["zellij_session"], zellij_session.as_str());
    assert_eq!(start["zellij_pane"], "7");
    assert_eq!(start["term_program"], "WezTerm");
}

#[test]
fn op_start_has_no_claude_without_a_session_file() {
    let fixture = Fixture::new();
    fixture.install_fake_op("echo value");

    let output = fixture.read(REFERENCE);

    assert!(output.status.success());
    let records = records(&audit_text(&fixture));
    assert!(
        records[0]["claude"].is_null(),
        "claude: {}",
        records[0]["claude"]
    );
}

/// The tests of the macOS notification. Each one puts a fake `osascript` in
/// `<fixture>/bin` BEFORE it turns notifications on, because the real
/// `/usr/bin/osascript` is on the PATH of the fixture and would show a real
/// notification on the screen of whoever runs the tests.
#[cfg(target_os = "macos")]
mod notification {
    use super::*;

    /// Where the fake `osascript` leaves one file for each call.
    fn record_dir(fixture: &Fixture) -> PathBuf {
        fixture.path("osascript")
    }

    /// Installs the fake `osascript`: it writes its arguments, each ended by
    /// a NUL byte, to a file in the record directory (atomically, through a
    /// rename), and then runs `after`.
    fn install_fake_osascript(fixture: &Fixture, after: &str) {
        let dir = record_dir(fixture);
        fs::create_dir_all(&dir).expect("create record dir");
        let tmp = fixture.path("osascript-tmp");
        fixture.install_script(
            "osascript",
            &format!(
                "printf '%s\\0' \"$@\" > '{tmp}'-$$\nmv '{tmp}'-$$ '{dir}'/$$\n{after}",
                tmp = tmp.display(),
                dir = dir.display(),
            ),
        );
    }

    /// Shell text that waits, for 20 seconds at most, until the record
    /// directory holds at least `count_expr` files.
    fn wait_for_records(fixture: &Fixture, count_expr: &str) -> String {
        let dir = record_dir(fixture);
        format!(
            "i=0\nwhile [ \"$(ls '{dir}' | wc -l)\" -lt {count_expr} ] && [ \"$i\" -lt 200 ]; do sleep 0.1; i=$((i+1)); done",
            dir = dir.display(),
        )
    }

    /// A command with notifications turned on. It refuses to build when the
    /// fake `osascript` is missing.
    fn notifying_command(fixture: &Fixture) -> Command {
        let fake = fixture.path("bin").join("osascript");
        assert!(fake.exists(), "no fake osascript at {}", fake.display());
        let mut command = fixture.command();
        command.env_remove("OP_CACHE_NOTIFY");
        command
    }

    /// Each call of the fake `osascript`: (file name, arguments), by file name.
    fn osascript_calls(fixture: &Fixture) -> Vec<(String, Vec<String>)> {
        let mut calls: Vec<(String, Vec<String>)> = fs::read_dir(record_dir(fixture))
            .expect("read record dir")
            .map(|entry| {
                let entry = entry.expect("record entry");
                let bytes = fs::read(entry.path()).expect("read record");
                let text = String::from_utf8_lossy(&bytes).into_owned();
                let args = text
                    .strip_suffix('\0')
                    .map(|rest| rest.split('\0').map(str::to_owned).collect())
                    .unwrap_or_default();
                (entry.file_name().to_string_lossy().into_owned(), args)
            })
            .collect();
        calls.sort();
        calls
    }

    const SCRIPT_ARGS: [&str; 6] = [
        "-e",
        "on run argv",
        "-e",
        "display notification (item 3 of argv) with title (item 1 of argv) subtitle (item 2 of argv)",
        "-e",
        "end run",
    ];

    /// The three texts after the script arguments.
    fn expected_args(title: &str, subtitle: &str) -> Vec<String> {
        SCRIPT_ARGS
            .iter()
            .copied()
            .chain([title, subtitle, REFERENCE])
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn notification_names_the_claude_session_and_the_zellij_session() {
        let fixture = Fixture::new();
        let sentinel = unique_token("notify");
        install_fake_osascript(&fixture, "exit 0");
        fixture.install_fake_op(&format!(
            "{}\necho '{sentinel}'",
            wait_for_records(&fixture, "1")
        ));
        let zellij_session = unique_token("zellij").replace("SENTINEL", "zs");
        let test_pid = std::process::id();
        let sessions = fixture.path("claude").join("sessions");
        fs::create_dir_all(&sessions).expect("create sessions dir");
        fs::write(
            sessions.join(format!("{test_pid}.json")),
            serde_json::json!({ "pid": test_pid, "sessionId": "s", "name": SESSION_NAME })
                .to_string(),
        )
        .expect("write session file");

        let output = notifying_command(&fixture)
            .args(["read", REFERENCE])
            .env("ZELLIJ_SESSION_NAME", &zellij_session)
            .output()
            .expect("run op-cache");

        let calls = osascript_calls(&fixture);
        assert_eq!(calls.len(), 1, "calls: {calls:?}");
        assert_eq!(
            calls[0].1,
            expected_args(
                "op-cache → 1Password",
                &format!("{SESSION_NAME} · {zellij_session}")
            )
        );
        assert_eq!(output.stdout, sentinel.as_bytes());
    }

    #[test]
    fn notification_falls_back_to_the_repo_directory_name() {
        let fixture = Fixture::new();
        let sentinel = unique_token("notify");
        install_fake_osascript(&fixture, "exit 0");
        fixture.install_fake_op(&format!(
            "{}\necho '{sentinel}'",
            wait_for_records(&fixture, "1")
        ));

        let output = notifying_command(&fixture)
            .args(["read", REFERENCE])
            .output()
            .expect("run op-cache");

        let calls = osascript_calls(&fixture);
        assert_eq!(calls.len(), 1, "calls: {calls:?}");
        assert_eq!(calls[0].1, expected_args("op-cache → 1Password", "repo"));
        assert_eq!(output.stdout, sentinel.as_bytes());
    }

    #[test]
    fn op_cache_notify_0_shows_no_notification() {
        let fixture = Fixture::new();
        let sentinel = unique_token("quiet");
        install_fake_osascript(&fixture, "exit 0");
        // Waits 2 seconds at most for a record that must never come.
        let dir = record_dir(&fixture);
        fixture.install_fake_op(&format!(
            "i=0\nwhile [ \"$(ls '{dir}' | wc -l)\" -lt 1 ] && [ \"$i\" -lt 20 ]; do sleep 0.1; i=$((i+1)); done\necho '{sentinel}'",
            dir = dir.display()
        ));
        assert!(fixture.path("bin").join("osascript").exists());

        let output = fixture
            .command()
            .args(["read", REFERENCE])
            .env("OP_CACHE_NOTIFY", "0")
            .output()
            .expect("run op-cache");

        assert_eq!(output.stdout, sentinel.as_bytes());
        assert_eq!(osascript_calls(&fixture), vec![]);
    }
}

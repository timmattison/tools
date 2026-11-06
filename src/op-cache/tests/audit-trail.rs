//! End-to-end tests of the op-cache audit trail. Each test builds a fixture
//! with a fake `op` on PATH and runs the real `op-cache` binary against it.
//! The process environment of the test is never changed.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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

#[allow(dead_code)]
fn lines_of(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .expect("read audit file")
        .lines()
        .map(str::to_owned)
        .collect()
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

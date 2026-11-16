//! Tests of the launch of `crap` through an interactive shell.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use chf::app::Version;
use chf::launch::{exec, paste_command, script, shell_of, Shell};
use claude_transcript::SessionId;
use wait_timeout::ChildExt;

const ID: &str = "4733ee2a-1ad6-4619-a01a-11840b8e1901";
const BOUND: Duration = Duration::from_secs(60);
const FAKE_CRAP: &str = "crap() { printf '%s\\n' \"$@\" > \"$HOME/crap-args\"; printf '%s\\n' \"${CRAP_LAUNCHER-unset}\" > \"$HOME/crap-launcher\"; return 7; }\n";

fn id() -> SessionId {
    SessionId::parse(ID).expect("valid id")
}

fn shell(value: &str) -> Option<Shell> {
    shell_of(Some(OsStr::new(value)))
}

/// Run `<shell> -ic <script>` with a clean environment. Give the status and stderr.
fn run_shell(
    shell_name: &str,
    home: &Path,
    script_text: &str,
    extra_env: &[(&str, &str)],
) -> (Option<i32>, String) {
    let mut command = childenv::fixture_command(shell_name);
    command
        .arg("-ic")
        .arg(script_text)
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("TERM", "dumb");
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn shell");
    let mut out = child.stdout.take().expect("stdout");
    let mut err = child.stderr.take().expect("stderr");
    let out_thread = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = out.read_to_string(&mut text);
        text
    });
    let err_thread = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = err.read_to_string(&mut text);
        text
    });
    let status = child.wait_timeout(BOUND).expect("wait");
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("the shell did not end in {BOUND:?}");
    };
    let _ = out_thread.join();
    let stderr = err_thread.join().expect("stderr thread");
    (status.code(), stderr)
}

fn run_bash(home: &Path, script_text: &str, extra_env: &[(&str, &str)]) -> (Option<i32>, String) {
    run_shell("bash", home, script_text, extra_env)
}

fn home_with(rc_name: &str, rc: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    fs::write(home.path().join(rc_name), rc).expect("write rc");
    home
}

#[test]
fn shell_of_accepts_bash_and_zsh() {
    assert_eq!(shell("/bin/bash"), Some(Shell::Bash));
    assert_eq!(shell("/opt/homebrew/bin/bash"), Some(Shell::Bash));
    assert_eq!(shell("bash"), Some(Shell::Bash));
    assert_eq!(shell("/bin/zsh"), Some(Shell::Zsh));
    assert_eq!(shell("zsh"), Some(Shell::Zsh));
}

#[test]
fn shell_of_refuses_everything_else() {
    for value in [
        "/bin/sh",
        "/usr/bin/fish",
        "/bin/bashful",
        "/bin/zsh-5.9",
        "",
    ] {
        assert_eq!(shell(value), None, "value {value:?}");
    }
    assert_eq!(shell_of(None), None);
}

#[test]
fn paste_command_names_the_flags() {
    assert_eq!(
        paste_command(&id(), Version::Compacted),
        format!("crap --here '{ID}'")
    );
    assert_eq!(
        paste_command(&id(), Version::Expanded),
        format!("crap --here --expand '{ID}'")
    );
}

#[test]
fn exec_gives_back_the_error_of_a_missing_shell() {
    let error = exec(Path::new("/nonexistent/chf-test-shell"), "exit 0");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn compacted_calls_crap_with_here_and_passes_the_status() {
    let home = home_with(".bashrc", FAKE_CRAP);
    let (code, _) = run_bash(home.path(), &script(&id(), Version::Compacted), &[]);
    assert_eq!(code, Some(7));
    let args = fs::read_to_string(home.path().join("crap-args")).expect("crap-args");
    assert_eq!(args, format!("--here\n{ID}\n"));
}

#[test]
fn expanded_calls_crap_with_expand() {
    let home = home_with(".bashrc", FAKE_CRAP);
    let (code, _) = run_bash(home.path(), &script(&id(), Version::Expanded), &[]);
    assert_eq!(code, Some(7));
    let args = fs::read_to_string(home.path().join("crap-args")).expect("crap-args");
    assert_eq!(args, format!("--here\n--expand\n{ID}\n"));
}

#[test]
fn crap_launcher_reaches_the_function() {
    let home = home_with(".bashrc", FAKE_CRAP);
    run_bash(
        home.path(),
        &script(&id(), Version::Compacted),
        &[("CRAP_LAUNCHER", "c")],
    );
    let launcher = fs::read_to_string(home.path().join("crap-launcher")).expect("launcher");
    assert_eq!(launcher, "c\n");
}

#[test]
fn crap_launcher_is_unset_when_the_environment_has_none() {
    let home = home_with(".bashrc", FAKE_CRAP);
    run_bash(home.path(), &script(&id(), Version::Compacted), &[]);
    let launcher = fs::read_to_string(home.path().join("crap-launcher")).expect("launcher");
    assert_eq!(launcher, "unset\n");
}

#[test]
fn a_missing_function_gives_status_one_and_the_fix() {
    let home = home_with(".bashrc", "");
    let (code, stderr) = run_bash(home.path(), &script(&id(), Version::Compacted), &[]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("the crap shell function is not defined"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("crap --shell-setup"), "stderr: {stderr}");
    assert!(!home.path().join("crap-args").exists());
}

#[test]
fn zsh_calls_crap_with_here() {
    if !Path::new("/bin/zsh").exists() && !Path::new("/usr/bin/zsh").exists() {
        eprintln!("note: zsh is not on /usr/bin:/bin, so this test does nothing");
        return;
    }
    let home = home_with(".zshrc", FAKE_CRAP);
    let (code, _) = run_shell("zsh", home.path(), &script(&id(), Version::Compacted), &[]);
    assert_eq!(code, Some(7));
    let args = fs::read_to_string(home.path().join("crap-args")).expect("crap-args");
    assert_eq!(args, format!("--here\n{ID}\n"));
}

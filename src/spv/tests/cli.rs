//! Black-box tests for the `spv` binary, driving the real CLI end to end.
//!
//! Every test starts its own child process and inspects that child by its PID,
//! so a run never depends on what else runs on the machine. The name of the
//! child carries the process id of the test and a nanosecond timestamp, so two
//! concurrent runs of the same test stay apart.

use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Invoke the freshly-built `spv` binary.
fn spv() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spv"))
}

/// Builds a token that no concurrent run of this test can also build.
fn unique_token(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock stands after the epoch")
        .as_nanos();
    format!("{prefix}{}_{nanos}", std::process::id())
}

/// Splits the output of a command into its success flag, its stdout, and its stderr.
fn run(output: Output) -> (bool, String, String) {
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// A child process that the test starts, inspects, and kills.
///
/// The shell holds the marker in its own command line, because `sleep` is not
/// the last command of the script and the shell thus keeps its own process.
struct Sleeper {
    child: Child,
}

impl Sleeper {
    /// Starts a child whose command line holds `marker`.
    ///
    /// # Arguments
    ///
    /// * `marker` - Text to put in the command line of the child
    /// * `env` - Extra environment variables for the child
    fn spawn(marker: &str, env: &[(&str, &str)]) -> Self {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(format!("sleep 30; : {marker}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (name, value) in env {
            command.env(name, value);
        }
        let child = command.spawn().expect("/bin/sh starts");
        let sleeper = Self { child };
        sleeper.wait_until_visible();
        sleeper
    }

    /// The process id of the child.
    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Waits until `spv` can see the child, so a test never races the kernel.
    fn wait_until_visible(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let found = spv()
                .arg(self.pid().to_string())
                .output()
                .expect("spv runs")
                .status
                .success();
            if found {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("spv never saw the child process {}", self.pid());
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs `spv` so that it inspects its own process.
///
/// The shell expands `$$` to its own process id and then replaces itself with
/// `spv`, which keeps that id. Every platform gives a process its own
/// environment, and macOS gives the environment of another process to root
/// only, so this is the one way a test without root reads a real environment.
///
/// # Arguments
///
/// * `flags` - The flags to pass to `spv`, before the process id
/// * `env` - Extra environment variables for `spv`
fn spv_on_itself(flags: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(format!("exec \"$0\" {} $$", flags.join(" ")))
        .arg(env!("CARGO_BIN_EXE_spv"));
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("spv runs")
}

#[test]
fn the_environment_section_shows_a_chosen_variable() {
    let name = unique_token("SPV_MARK_");
    let (ok, stdout, stderr) = run(spv_on_itself(&["--env"], &[(name.as_str(), "hello")]));

    assert!(ok, "spv should succeed; stderr: {stderr}");
    assert!(
        stdout.contains(&name),
        "the environment section names the variable; stdout: {stdout}"
    );
    assert!(
        stdout.contains("hello"),
        "the environment section shows the value; stdout: {stdout}"
    );
}

#[test]
fn the_environment_section_hides_a_credential() {
    let name = unique_token("SPV_TOKEN_");
    let (ok, stdout, stderr) = run(spv_on_itself(&["--env"], &[(name.as_str(), "s3cret")]));

    assert!(ok, "spv should succeed; stderr: {stderr}");
    assert!(
        stdout.contains(&name),
        "the environment section names the variable; stdout: {stdout}"
    );
    assert!(
        stdout.contains("<redacted>"),
        "the value is hidden; stdout: {stdout}"
    );
    assert!(
        !stdout.contains("s3cret"),
        "the value never reaches the terminal; stdout: {stdout}"
    );
}

#[test]
fn show_secrets_prints_the_credential_in_full() {
    let name = unique_token("SPV_TOKEN_");
    let (ok, stdout, stderr) = run(spv_on_itself(
        &["--env", "--show-secrets"],
        &[(name.as_str(), "s3cret")],
    ));

    assert!(ok, "spv should succeed; stderr: {stderr}");
    assert!(
        stdout.contains("s3cret"),
        "--show-secrets prints the value; stdout: {stdout}"
    );
}

/// macOS hands the environment of another process to root only. A run without
/// root must say so rather than print an empty section, because an empty
/// section teaches the reader that the process carries no environment.
#[cfg(target_os = "macos")]
#[test]
fn the_environment_of_another_process_is_refused_with_a_reason() {
    // SAFETY: geteuid is a POSIX call that reads an integer out of the process
    // credentials. It takes no pointer, it cannot fail, and it changes nothing.
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        // As root the kernel hands the environment over, so there is nothing to refuse.
        return;
    }
    let child = Sleeper::spawn("plain", &[("SPV_PLAIN", "hello")]);

    let (ok, stdout, stderr) = run(
        spv()
            .args(["--env", &child.pid().to_string()])
            .output()
            .expect("spv runs"),
    );

    assert!(ok, "spv should succeed; stderr: {stderr}");
    assert!(
        stdout.contains("Environment for"),
        "the section names itself; stdout: {stdout}"
    );
    assert!(
        stdout.contains("unavailable:") && stdout.contains("sudo"),
        "the section says why it is empty and names the remedy; stdout: {stdout}"
    );
    assert!(
        !stdout.contains("none found"),
        "a refusal never reads as an empty environment; stdout: {stdout}"
    );
}

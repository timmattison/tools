//! Black-box tests for the `spv` binary, driving the real CLI end to end.
//!
//! Every test starts its own child process and inspects that child by its PID,
//! so a run never depends on what else runs on the machine. The name of the
//! child carries the process id of the test and a nanosecond timestamp, so two
//! concurrent runs of the same test stay apart.

use std::net::TcpListener;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// Keeps a socket of this test process out of the children this file starts.
///
/// macOS has no `SOCK_CLOEXEC`, so the standard library creates a socket and
/// then marks it close-on-exec in a second call. A `spawn` on another thread
/// between the two calls hands the socket to the child, and `lsof` then reports
/// a listening port for a process that never opened one. That is what a run of
/// this file did: the sleeping shell held file descriptor 46, the loopback
/// listener of the test below it.
///
/// Every start of a child and every bind of a socket takes this lock, so the
/// two never overlap.
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

/// Takes the lock, and takes it again after a test that panicked poisoned it.
fn spawn_lock() -> MutexGuard<'static, ()> {
    SPAWN_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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

/// How long a child waits before it stops on its own.
///
/// `Drop` kills the child when the test ends, so this bound only matters when a
/// run is killed outright. It must still outlast the slowest run of this file:
/// a bound of 30 seconds lost its children under load, and three tests then
/// reported that the process they had just started did not exist.
const SLEEPER_SECONDS: u32 = 300;

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
            .arg(format!("sleep {SLEEPER_SECONDS}; : {marker}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // The shell forks `sleep`, so killing the shell alone leaves the
            // grandchild behind, reparented to process 1. A group of its own
            // lets `Drop` kill both with one call.
            .process_group(0);
        for (name, value) in env {
            command.env(name, value);
        }
        // `spawn` returns after the child has replaced itself with /bin/sh: a
        // failed exec comes back as an error here. So the command line of the
        // child already stands, and no test needs to wait for it.
        let child = {
            let _guard = spawn_lock();
            command.spawn().expect("/bin/sh starts")
        };
        Self { child }
    }

    /// The process id of the child.
    fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        if let Ok(group) = i32::try_from(self.child.id()) {
            // SAFETY: kill is a POSIX call that takes no pointer. A negative
            // argument names the process group of that number, which
            // `process_group(0)` made equal to the process id of the shell. So
            // this reaches the shell and the `sleep` it forked.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts a child, waits for it to finish, and returns the id it no longer uses.
fn reaped_pid() -> u32 {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("exit 0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("/bin/sh starts");
    let pid = child.id();
    let _ = child.wait();
    pid
}

#[test]
fn a_name_that_matches_nothing_is_reported_and_the_run_fails() {
    let name = unique_token("spv_no_such_process_");
    let (ok, _stdout, stderr) = run(spv().arg(&name).output().expect("spv runs"));

    assert!(!ok, "a search that matches nothing fails");
    assert!(
        stderr.contains(&name),
        "the message names the pattern; stderr: {stderr}"
    );
}

#[test]
fn no_argument_prints_the_usage_text_and_fails() {
    let (ok, _stdout, stderr) = run(spv().output().expect("spv runs"));

    assert!(!ok, "a run without a pattern fails");
    assert!(
        stderr.contains("Usage"),
        "the run prints the usage text; stderr: {stderr}"
    );
}

#[test]
fn a_process_id_that_is_gone_gives_a_message_and_not_a_panic() {
    let pid = reaped_pid();
    let (ok, _stdout, stderr) = run(spv()
        .args(["--all", &pid.to_string()])
        .output()
        .expect("spv runs"));

    assert!(!ok, "a process that is gone is not found");
    assert!(
        stderr.contains(&pid.to_string()),
        "the message names the process id; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "the run never panics; stderr: {stderr}"
    );
}

#[test]
fn the_two_searches_disagree_on_a_name_whose_case_differs() {
    let mixed = unique_token("SpvCase_");
    let lowered = mixed.to_lowercase();
    let _child = Sleeper::spawn(&mixed, &[]);

    let found_ignoring_case = spv()
        .args(["--full", &lowered])
        .output()
        .expect("spv runs")
        .status
        .success();
    assert!(
        found_ignoring_case,
        "the default search ignores case, so {lowered} finds {mixed}"
    );

    let found_respecting_case = spv()
        .args(["--full", "--case-sensitive", &lowered])
        .output()
        .expect("spv runs")
        .status
        .success();
    assert!(
        !found_respecting_case,
        "--case-sensitive respects case, so {lowered} misses {mixed}"
    );

    let found_exactly = spv()
        .args(["--full", "--case-sensitive", &mixed])
        .output()
        .expect("spv runs")
        .status
        .success();
    assert!(
        found_exactly,
        "--case-sensitive finds the exact name {mixed}"
    );
}

#[test]
fn a_command_line_of_multi_byte_characters_is_found_and_shown() {
    let marker = format!("日本語🎉{}", unique_token(""));
    let _child = Sleeper::spawn(&marker, &[]);

    let (ok, stdout, stderr) = run(spv().args(["--full", &marker]).output().expect("spv runs"));

    assert!(ok, "spv finds the child; stderr: {stderr}");
    assert!(
        stdout.contains("日本語"),
        "the command column shows the characters; stdout: {stdout}"
    );
}

/// Runs `spv` so that it inspects its own process.
///
/// The shell expands `$$` to its own process id and then replaces itself with
/// `spv`, which keeps that id. Every platform gives a process its own
/// environment, and macOS gives the environment of another process to root
/// only, so this is the one way a test without root reads a real environment.
///
/// The environment starts empty. `--show-secrets` prints every value in full,
/// and the environment of the test runner holds the real credentials of whoever
/// runs the suite. An inherited environment would put them in the output of the
/// tool, and a failure message would then carry them into the log.
///
/// # Arguments
///
/// * `flags` - The flags to pass to `spv`, before the process id
/// * `env` - The whole environment for `spv`
fn spv_on_itself(flags: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(format!("exec \"$0\" {} $$", flags.join(" ")))
        .arg(env!("CARGO_BIN_EXE_spv"))
        .env_clear();
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
    // The failure message names no output. This run prints every value in full,
    // so the output is the one place in this file that must not reach a log.
    assert!(stdout.contains("s3cret"), "--show-secrets prints the value");
}

#[test]
fn the_network_section_shows_a_socket_this_test_opened() {
    // Port 0 asks the operating system for a free port, so two concurrent runs
    // of this test never claim the same one. The lock keeps this socket out of
    // any child that another test starts while it is open.
    let _guard = spawn_lock();
    let listener = TcpListener::bind("127.0.0.1:0").expect("the loopback interface accepts a bind");
    let port = listener
        .local_addr()
        .expect("a bound listener carries an address")
        .port();

    let (ok, stdout, stderr) = run(spv()
        .args(["--net", &std::process::id().to_string()])
        .output()
        .expect("spv runs"));

    assert!(ok, "spv should succeed; stderr: {stderr}");
    assert!(
        stdout.contains("Network connections for"),
        "the section names itself; stdout: {stdout}"
    );
    assert!(
        stdout.contains(&port.to_string()),
        "the section shows the port this test opened ({port}); stdout: {stdout}"
    );
}

#[test]
fn the_network_section_says_it_found_nothing() {
    let child = Sleeper::spawn("quiet", &[]);

    let (ok, stdout, stderr) = run(spv()
        .args(["--net", &child.pid().to_string()])
        .output()
        .expect("spv runs"));

    assert!(ok, "spv should succeed; stderr: {stderr}");
    assert!(
        stdout.contains("Network connections for"),
        "the section names itself even with nothing to show; stdout: {stdout}"
    );
    assert!(
        stdout.contains("none found"),
        "the section says it found nothing; stdout: {stdout}"
    );
}

#[test]
fn a_process_owned_by_another_user_raises_a_warning() {
    // SAFETY: geteuid is a POSIX call that reads an integer out of the process
    // credentials. It takes no pointer, it cannot fail, and it changes nothing.
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        // As root every process is readable, so there is nothing to warn about.
        return;
    }

    // PID 1 belongs to root on every Unix.
    let (ok, _stdout, stderr) = run(spv().args(["--net", "1"]).output().expect("spv runs"));

    assert!(ok, "spv should succeed; stderr: {stderr}");
    assert!(
        stderr.contains("Warning"),
        "the run warns about the owner; stderr: {stderr}"
    );
    assert!(
        stderr.contains("root") && stderr.contains("sudo"),
        "the warning names the owner and the remedy; stderr: {stderr}"
    );
}

#[test]
fn a_run_without_a_section_raises_no_warning() {
    let (ok, _stdout, stderr) = run(spv().arg("1").output().expect("spv runs"));

    assert!(ok, "spv should succeed; stderr: {stderr}");
    assert!(
        !stderr.contains("Warning"),
        "a plain listing needs no permission, so it warns about none; stderr: {stderr}"
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

    let (ok, stdout, stderr) = run(spv()
        .args(["--env", &child.pid().to_string()])
        .output()
        .expect("spv runs"));

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

/// A PATH that holds no `lsof`.
const PATH_WITHOUT_LSOF: &str = "/nonexistent";

/// Runs `spv` on one process with a PATH that holds no `lsof`.
///
/// `spv()` names the binary by its absolute path, so an empty PATH leaves `spv`
/// itself alone and stops only the sections that read `lsof`. `spv` looks for
/// `lsof` once and keeps that answer for the life of the process, so each run
/// needs a child of its own.
///
/// # Arguments
///
/// * `flag` - The section flag to pass to `spv`
/// * `pid` - The process to inspect
///
/// # Returns
///
/// The output of the run.
fn spv_without_lsof(flag: &str, pid: u32) -> Output {
    spv()
        .args([flag, &pid.to_string()])
        .env("PATH", PATH_WITHOUT_LSOF)
        .output()
        .expect("spv runs")
}

/// Finds the line of a section that says the section is unavailable.
///
/// # Arguments
///
/// * `stdout` - The whole standard output of a run
///
/// # Returns
///
/// The line, or `None` when the run printed no such line.
fn unavailable_line(stdout: &str) -> Option<&str> {
    stdout.lines().find(|line| line.contains("unavailable:"))
}

#[test]
fn the_open_files_section_names_the_missing_lsof() {
    // The marker never holds the name of the tool, because the command column
    // of the table prints the marker back into the same output.
    let child = Sleeper::spawn(&unique_token("spv_no_tool_files_"), &[]);

    let (ok, stdout, stderr) = run(spv_without_lsof("--lsof", child.pid()));

    assert!(ok, "spv should succeed; stderr: {stderr}");
    assert!(
        stdout.contains("Open files for"),
        "the section names itself; stdout: {stdout}"
    );
    let note = unavailable_line(&stdout)
        .unwrap_or_else(|| panic!("the section says it is unavailable; stdout: {stdout}"));
    assert!(
        note.contains("lsof"),
        "the section names the tool it cannot find: {note}"
    );
}

#[test]
fn the_network_section_names_the_missing_lsof() {
    // The marker never holds the name of the tool, because the command column
    // of the table prints the marker back into the same output.
    let child = Sleeper::spawn(&unique_token("spv_no_tool_net_"), &[]);

    let (ok, stdout, stderr) = run(spv_without_lsof("--net", child.pid()));

    assert!(ok, "spv should succeed; stderr: {stderr}");
    assert!(
        stdout.contains("Network connections for"),
        "the section names itself; stdout: {stdout}"
    );
    let note = unavailable_line(&stdout)
        .unwrap_or_else(|| panic!("the section says it is unavailable; stdout: {stdout}"));
    assert!(
        note.contains("lsof"),
        "the section names the tool it cannot find: {note}"
    );
}

/// `lsof` gives the same answer when it finds nothing and when the kernel
/// refuses it. So an empty section of another user's process must say on
/// standard output why it can be empty. A reader who keeps only standard
/// output otherwise keeps the false half and drops the true half.
#[test]
fn a_section_the_kernel_can_refuse_says_so_on_standard_output() {
    // SAFETY: geteuid is a POSIX call that reads an integer out of the process
    // credentials. It takes no pointer, it cannot fail, and it changes nothing.
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        // As root the kernel refuses nothing, so there is nothing to say.
        return;
    }

    // PID 1 belongs to root on every Unix.
    for (flag, heading) in [
        ("--lsof", "Open files for"),
        ("--net", "Network connections for"),
    ] {
        let (ok, stdout, stderr) = run(spv().args([flag, "1"]).output().expect("spv runs"));

        assert!(ok, "{flag} should succeed; stderr: {stderr}");
        assert!(
            stdout.contains(heading),
            "the section names itself; stdout: {stdout}"
        );
        assert!(
            stdout.contains("sudo"),
            "{flag} names the remedy on standard output; stdout: {stdout}"
        );
        assert!(
            !stdout.contains("none found"),
            "{flag} never reads as a process that holds nothing; stdout: {stdout}"
        );
    }
}

/// The characters that the `UTF8_FULL` preset of `comfy-table` draws with.
///
/// `--raw` promises output without table formatting, so a raw run prints none
/// of these anywhere.
const TABLE_BORDER_CHARACTERS: &[char] = &[
    '┌', '┬', '┐', '├', '┼', '┤', '└', '┴', '┘', '─', '│', '┆', '╞', '╪', '╡', '═',
];

/// Finds the first table border character in the output of a run.
///
/// # Arguments
///
/// * `stdout` - The whole standard output of a run
///
/// # Returns
///
/// The character, or `None` when the run drew no table.
fn table_border_character(stdout: &str) -> Option<char> {
    stdout
        .chars()
        .find(|character| TABLE_BORDER_CHARACTERS.contains(character))
}

#[test]
fn raw_prints_the_open_files_section_without_a_table() {
    // The marker holds no path, because the command column prints the marker
    // back into the same output.
    let child = Sleeper::spawn(&unique_token("spv_raw_files_"), &[]);

    let (ok, stdout, stderr) = run(spv()
        .args(["--raw", "--lsof", &child.pid().to_string()])
        .output()
        .expect("spv runs"));

    assert!(ok, "spv should succeed; stderr: {stderr}");
    assert!(
        stdout.contains("Open files for"),
        "the section names itself; stdout: {stdout}"
    );
    assert!(
        stdout.contains("/dev/null"),
        "the section shows the null device the child holds; stdout: {stdout}"
    );
    assert!(
        table_border_character(&stdout).is_none(),
        "--raw draws no table; stdout: {stdout}"
    );
}

#[test]
fn raw_prints_the_environment_section_without_a_table() {
    let name = unique_token("SPV_RAW_ENV_");
    let value = unique_token("value_");
    let (ok, stdout, stderr) = run(spv_on_itself(
        &["--raw", "--env"],
        &[(name.as_str(), value.as_str())],
    ));

    assert!(ok, "spv should succeed; stderr: {stderr}");
    assert!(
        stdout.contains(&name),
        "the section names the variable; stdout: {stdout}"
    );
    assert!(
        stdout.contains(&value),
        "the section shows the value; stdout: {stdout}"
    );
    assert!(
        table_border_character(&stdout).is_none(),
        "--raw draws no table; stdout: {stdout}"
    );
}

#[test]
fn raw_prints_the_network_section_without_a_table() {
    // Port 0 asks the operating system for a free port, so two concurrent runs
    // of this test never claim the same one. The lock keeps this socket out of
    // any child that another test starts while it is open.
    let _guard = spawn_lock();
    let listener = TcpListener::bind("127.0.0.1:0").expect("the loopback interface accepts a bind");
    let port = listener
        .local_addr()
        .expect("a bound listener carries an address")
        .port();

    let (ok, stdout, stderr) = run(spv()
        .args(["--raw", "--net", &std::process::id().to_string()])
        .output()
        .expect("spv runs"));

    assert!(ok, "spv should succeed; stderr: {stderr}");
    assert!(
        stdout.contains("Network connections for"),
        "the section names itself; stdout: {stdout}"
    );
    assert!(
        stdout.contains(&port.to_string()),
        "the section shows the port this test opened ({port}); stdout: {stdout}"
    );
    assert!(
        table_border_character(&stdout).is_none(),
        "--raw draws no table; stdout: {stdout}"
    );
}

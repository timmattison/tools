//! What one run of `glance` does with the status item of its target: start it
//! here, start it in the background, or stop it.
//!
//! Each target has one status item (see [`crate::instance`]). A run first
//! looks for the status item of its target. When one runs, the run stops it,
//! starts nothing, and ends. Thus a second run of the same command closes the
//! status item that the first run opened. A run for a different target
//! leaves that status item alone.
//!
//! A background start makes a copy of `glance` with the hidden flag
//! `--spawned-copy`. It does not `fork` without `exec`: macOS refuses
//! AppKit in a forked child. The copy starts a new session, so the terminal
//! of the start cannot stop it, and it takes the lock itself. Thus two starts
//! at the same moment can never give two status items for one target.
//!
//! Each status item runs from the app bundle of glance (see
//! [`crate::bundle`]), so that macOS gives it a bundle identifier. A
//! background start makes sure that the bundle holds a copy of this program.
//! It starts its copy from the executable of the bundle. A foreground run that
//! does not run from the bundle also makes sure of the bundle. It replaces
//! itself with the executable of the bundle through `exec`, with the same
//! arguments. The PID and the terminal stay the same. The `exec` comes before
//! the run takes the lock, because Rust opens the lock file with
//! close-on-exec. A run from the bundle does not `exec` again.
//!
//! The stdout and the stderr of the copy are pipes to the start. A thread of
//! the start reads the stderr to its end, so a long text on it cannot block
//! the copy. The copy writes one line into the stdout when its status item is
//! up, as a foreground run writes that line to the terminal. Then the copy
//! points its stdout and its stderr at `/dev/null`, because nobody reads the
//! pipes after the start ends. The start copies the line to its own stdout
//! and ends. A copy that ends before its status item is up closes the stdout
//! with no line. The start then writes what the copy wrote to its stderr,
//! and after that the exit status of the copy. Thus the start needs no
//! clock: the read ends with the line or with the end of the copy.

use std::env;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::num::NonZeroU32;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::thread;

use crate::bundle::Bundle;
use crate::connection::{HostNaming, Remote};
use crate::flags::{
    flag, flag_with_value, EXIT_AFTER, HOST, INTERVAL, REMOTE_BINARY, RESOLVE, SPAWNED_COPY,
};
use crate::instance::{self, ClaimError, LockFile, Pid, Target};
use crate::interval::Interval;
use crate::shell;

/// The exit code of a run that failed.
const EXIT_FAILURE: u8 = 1;

/// The working directory of the background copy.
///
/// The copy runs long after its start ended, and a volume cannot unmount
/// while a process holds a directory on it. The root directory is on no
/// volume that a user unmounts.
const CHILD_DIRECTORY: &str = "/";

/// How a run starts the status item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The status item runs in this process, which holds the terminal. A run
    /// with `--foreground` takes this mode.
    Foreground,
    /// This process starts a copy that runs the status item, and returns. A
    /// run with no `--foreground` takes this mode.
    Background,
    /// This process is the copy that a background start made.
    SpawnedCopy,
}

/// The settings of the status item.
#[derive(Debug, Clone)]
pub struct Settings {
    /// The time between two samples.
    pub interval: Interval,
    /// The second Mac that the status item shows, or `None` for this Mac.
    pub remote: Option<Remote>,
    /// The life of the status item after it is up, or `None` for no bound.
    pub exit_after: Option<NonZeroU32>,
}

impl Settings {
    /// Gives the target of these settings. The naming of the second Mac does
    /// not change the target.
    #[must_use]
    pub fn target(&self) -> Target {
        self.remote.as_ref().map_or(Target::Local, |remote| {
            Target::Host(remote.destination.clone())
        })
    }
}

/// Gives the arguments that a background start gives to its copy.
///
/// The copy parses them with the same command line as a person's run, so
/// the copy gets the same settings.
#[must_use]
pub fn child_arguments(settings: &Settings) -> Vec<String> {
    let mut arguments = vec![
        flag(SPAWNED_COPY),
        flag_with_value(INTERVAL, &settings.interval.to_string()),
    ];
    if let Some(remote) = &settings.remote {
        arguments.push(flag_with_value(HOST, remote.destination.as_str()));
        arguments.push(flag_with_value(
            REMOTE_BINARY,
            remote.remote_binary.as_str(),
        ));
        if remote.naming == HostNaming::Reported {
            arguments.push(flag(RESOLVE));
        }
    }
    if let Some(seconds) = settings.exit_after {
        arguments.push(flag_with_value(EXIT_AFTER, &seconds.to_string()));
    }
    arguments
}

/// Starts or stops the status item of the target of `settings`, as `mode`
/// tells, and gives the exit code of the run.
///
/// A foreground run returns only when the status item cannot start. A
/// background run returns when the status item of its copy is up.
#[must_use]
pub fn run(mode: Mode, settings: Settings) -> ExitCode {
    let target = settings.target();
    let lock = match LockFile::for_user(&target) {
        Ok(lock) => lock,
        Err(error) => return fail(&format!("cannot find the lock file: {error}")),
    };
    if mode != Mode::SpawnedCopy {
        match instance::holder(&lock) {
            Err(error) => return fail(&lock_problem(&lock, &error)),
            Ok(Some(pid)) => return stop(&lock, &target, pid),
            Ok(None) => {}
        }
    }
    match mode {
        // The `exec` must come before the claim: Rust opens the lock file with
        // close-on-exec, so an `exec` after the claim would drop the lock.
        Mode::Foreground => match run_from_bundle() {
            Ok(()) => run_here(&lock, settings, false),
            Err(message) => fail(&message),
        },
        Mode::Background => start_in_background(&lock, &settings),
        Mode::SpawnedCopy => match instance::start_a_session() {
            Ok(()) => run_here(&lock, settings, true),
            Err(error) => fail(&format!("cannot start a new session: {error}")),
        },
    }
}

/// Stops the status item of `target`, which runs as `pid`.
fn stop(lock: &LockFile, target: &Target, pid: Pid) -> ExitCode {
    match instance::stop(lock, pid) {
        Ok(()) => {
            say(&format!("{}\n", stopped_line(target, pid)));
            ExitCode::SUCCESS
        }
        Err(error) => fail(&error.to_string()),
    }
}

/// Takes the lock and runs the status item in this process. When the status
/// item is up, writes the line that tells so to stdout: to the terminal of a
/// foreground run, or to the pipe of a background start. When `is_copy` is
/// true, this process is the background copy. It then points its stdout and
/// its stderr at `/dev/null` after the line, because its start ends.
fn run_here(lock: &LockFile, settings: Settings, is_copy: bool) -> ExitCode {
    // The claim must live until the process ends. The run loop of the shell
    // never returns while the status item runs.
    let _claim = match instance::claim(lock) {
        Ok(claim) => claim,
        Err(ClaimError::Held(pid)) => return fail(&another_started(pid)),
        Err(ClaimError::Io(error)) => return fail(&lock_problem(lock, &error)),
    };
    if let Err(error) = instance::end_with_success_on_terminate() {
        return fail(&format!("cannot catch SIGTERM: {error}"));
    }
    let target = settings.target();
    let line = format!("{}\n", ready_line(&target, Pid::of_this_process()));
    let exit_after = settings.exit_after;
    let autosave_name = target.autosave_name();
    shell::run(
        settings.interval,
        settings.remote,
        &autosave_name,
        move || {
            say(&line);
            if is_copy {
                // The start has its line and nobody reads either pipe, so an
                // error here has no channel to report on.
                let _ = instance::send_output_nowhere();
            }
            if let Some(seconds) = exit_after {
                instance::end_after(seconds);
            }
        },
    )
}

/// Starts a copy that runs the status item from the executable of the app
/// bundle, and waits until its status item is up. When the copy ends first,
/// writes what the copy wrote to its stderr and its exit status, and gives the
/// exit code of a failure.
fn start_in_background(lock: &LockFile, settings: &Settings) -> ExitCode {
    if let Err(error) = lock.prepare() {
        return fail(&lock_problem(lock, &error));
    }
    let bundle = match prepare_bundle() {
        Ok(bundle) => bundle,
        Err(message) => return fail(&message),
    };
    let spawned =
        copy_pipes(childenv::command(bundle.executable()).args(child_arguments(settings)))
            .current_dir(CHILD_DIRECTORY)
            .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return fail(&format!("cannot start the background copy: {error}")),
    };
    match await_ready(&mut child) {
        Ok(line) => {
            say(&line);
            ExitCode::SUCCESS
        }
        Err(failure) => {
            eprint!("{}", failure.report());
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

/// Makes sure that the app bundle of the user holds a copy of this program,
/// and gives the bundle.
fn prepare_bundle() -> Result<Bundle, String> {
    let (current, bundle) = this_program_and_bundle()?;
    ensure_bundle(&bundle, &current)?;
    Ok(bundle)
}

/// Gives the path of this program and the app bundle of the user.
fn this_program_and_bundle() -> Result<(PathBuf, Bundle), String> {
    let current =
        env::current_exe().map_err(|error| format!("cannot find the path of glance: {error}"))?;
    let bundle =
        Bundle::for_user().map_err(|error| format!("cannot find the bundle of glance: {error}"))?;
    Ok((current, bundle))
}

/// Makes sure that `bundle` holds a copy of the program `current`.
fn ensure_bundle(bundle: &Bundle, current: &Path) -> Result<(), String> {
    bundle.ensure(current).map_err(|error| {
        format!(
            "cannot make the bundle {}: {error}",
            bundle.path().display()
        )
    })
}

/// Makes sure that this process runs from the executable of the app bundle.
///
/// When this process does not, the function makes sure of the bundle and
/// replaces this process with the executable of the bundle through `exec`,
/// with the same arguments. The PID and the terminal stay the same. The
/// function returns `Ok` only when this process runs from the bundle.
fn run_from_bundle() -> Result<(), String> {
    let (current, bundle) = this_program_and_bundle()?;
    if bundle.runs(&current) {
        return Ok(());
    }
    ensure_bundle(&bundle, &current)?;
    let error = childenv::command(bundle.executable())
        .args(env::args_os().skip(1))
        .exec();
    Err(format!(
        "cannot start glance from the bundle {}: {error}",
        bundle.path().display()
    ))
}

/// Sets the pipes of the background copy: no stdin, and a pipe to the start
/// for each of stdout and stderr.
fn copy_pipes(command: &mut Command) -> &mut Command {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
}

/// How a background copy failed before its status item was up.
#[derive(Debug)]
struct CopyFailure {
    /// What the copy wrote to its stderr.
    said: String,
    /// The result of the wait for the copy.
    status: io::Result<ExitStatus>,
}

impl CopyFailure {
    /// The whole text that the start writes to its stderr.
    fn report(&self) -> String {
        let mut text = self.said.clone();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        format!("{text}glance: {}\n", copy_stopped(&self.status))
    }
}

/// Waits for the ready line of `child`. Gives the line, which ends in a
/// newline, or the failure of a child that closed its stdout before it
/// wrote a whole line.
///
/// A thread reads the stderr of the child to its end. A child that writes
/// more than a pipe holds to its stderr then never waits for the start. The
/// thread stays unjoined when the line arrives, and it ends with the stderr
/// of the child.
fn await_ready(child: &mut Child) -> Result<String, CopyFailure> {
    let drain = child.stderr.take().map(|mut stderr| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        })
    });
    let line = child.stdout.take().and_then(|stdout| {
        let mut line = String::new();
        match BufReader::new(stdout).read_line(&mut line) {
            Ok(_) if line.ends_with('\n') => Some(line),
            _ => None,
        }
    });
    line.ok_or_else(|| {
        let status = child.wait();
        let bytes = drain
            .and_then(|thread| thread.join().ok())
            .unwrap_or_default();
        CopyFailure {
            said: String::from_utf8_lossy(&bytes).into_owned(),
            status,
        }
    })
}

/// Writes `text` to stdout. A closed stdout is no reason to fail, because
/// the status item runs or stopped already.
fn say(text: &str) {
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

/// Writes `message` to stderr and gives the exit code of a failure.
fn fail(message: &str) -> ExitCode {
    eprintln!("glance: {message}");
    ExitCode::from(EXIT_FAILURE)
}

/// The line that tells that the status item of `target` is up.
fn ready_line(target: &Target, pid: Pid) -> String {
    match target {
        Target::Local => {
            format!("glance: the status item runs as PID {pid}. Run glance again to stop it.")
        }
        Target::Host(destination) => format!(
            "glance: the status item of the host {destination} runs as PID {pid}. \
             Run glance again with the same --host to stop it."
        ),
    }
}

/// The line that tells that a run stopped the status item of `target`.
fn stopped_line(target: &Target, pid: Pid) -> String {
    match target {
        Target::Local => format!("glance: stopped the status item that ran as PID {pid}."),
        Target::Host(destination) => format!(
            "glance: stopped the status item of the host {destination} that ran as PID {pid}."
        ),
    }
}

/// The reason of a run that found the lock taken when it claimed the lock.
/// The background copy writes it, and its start relays it.
fn another_started(pid: Pid) -> String {
    format!("another glance started the same status item at the same time, as PID {pid}")
}

/// The reason of a lock file that cannot be used.
fn lock_problem(lock: &LockFile, error: &io::Error) -> String {
    format!("cannot use the lock file {lock}: {error}")
}

/// The reason of a background copy that ended before its status item was up.
fn copy_stopped(status: &io::Result<ExitStatus>) -> String {
    format!(
        "the background copy stopped before its status item was up ({})",
        Shown(status)
    )
}

/// Shows the result of a wait for a child.
struct Shown<'a>(&'a io::Result<ExitStatus>);

impl fmt::Display for Shown<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Ok(status) => write!(formatter, "{status}"),
            Err(error) => write!(formatter, "the wait failed: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::{HostNaming, RemoteBinary};
    use std::time::Duration;

    /// The PID of the lines in these tests.
    fn pid() -> Pid {
        Pid::new(4242).expect("a PID")
    }

    /// The settings of the host `destination` with the remote binary `binary`.
    fn host_settings(destination: &str, binary: &str, naming: HostNaming) -> Settings {
        Settings {
            interval: Interval::DEFAULT,
            remote: Some(Remote {
                destination: destination.parse().expect("a destination"),
                remote_binary: binary.parse::<RemoteBinary>().expect("a remote binary"),
                naming,
            }),
            exit_after: NonZeroU32::new(30),
        }
    }

    #[test]
    fn the_local_lines_name_the_pid_and_the_command_that_stops() {
        assert_eq!(
            ready_line(&Target::Local, pid()),
            "glance: the status item runs as PID 4242. Run glance again to stop it."
        );
        assert_eq!(
            stopped_line(&Target::Local, pid()),
            "glance: stopped the status item that ran as PID 4242."
        );
    }

    #[test]
    fn the_host_lines_name_the_destination() {
        let target = host_settings("tim@m1-ts", "glance", HostNaming::Destination).target();
        assert_eq!(
            ready_line(&target, pid()),
            "glance: the status item of the host tim@m1-ts runs as PID 4242. \
             Run glance again with the same --host to stop it."
        );
        assert_eq!(
            stopped_line(&target, pid()),
            "glance: stopped the status item of the host tim@m1-ts that ran as PID 4242."
        );
    }

    #[test]
    fn settings_with_no_remote_target_this_mac() {
        let settings = Settings {
            interval: Interval::DEFAULT,
            remote: None,
            exit_after: None,
        };
        assert_eq!(settings.target(), Target::Local);
        assert_eq!(
            child_arguments(&settings),
            ["--spawned-copy", "--interval=1"]
        );
    }

    #[test]
    fn the_copy_of_a_resolve_start_gets_the_resolve_flag() {
        assert_eq!(
            child_arguments(&host_settings("m1-ts", "-odd/glance", HostNaming::Reported)),
            [
                "--spawned-copy",
                "--interval=1",
                "--host=m1-ts",
                "--remote-binary=-odd/glance",
                "--resolve",
                "--exit-after=30",
            ]
        );
    }

    #[test]
    fn the_copy_of_a_host_gets_the_destination_the_binary_and_the_bound() {
        assert_eq!(
            child_arguments(&host_settings(
                "m1-ts",
                "-odd/glance",
                HostNaming::Destination
            )),
            [
                "--spawned-copy",
                "--interval=1",
                "--host=m1-ts",
                "--remote-binary=-odd/glance",
                "--exit-after=30",
            ]
        );
    }

    #[test]
    fn a_copy_that_stopped_names_its_status() {
        let failed: io::Result<ExitStatus> = Err(io::Error::other("no child"));
        assert_eq!(
            copy_stopped(&failed),
            "the background copy stopped before its status item was up (the wait failed: no child)"
        );
    }

    /// Starts `sh -c script` through the pipes of the background copy.
    fn spawn_script(script: &str) -> Child {
        let mut command = childenv::fixture_command("sh");
        copy_pipes(command.args(["-c", script]))
            .spawn()
            .expect("sh starts")
    }

    #[test]
    fn a_copy_that_writes_its_line_gives_the_line() {
        let mut child = spawn_script("printf 'up\\n'");
        assert_eq!(await_ready(&mut child).expect("a line"), "up\n");
    }

    #[test]
    fn a_copy_that_stops_gives_what_it_wrote_to_stderr_and_its_status() {
        let mut child = spawn_script("echo 'glance: boom' >&2; exit 3");
        let failure = await_ready(&mut child).expect_err("no line");
        assert_eq!(
            failure.report(),
            "glance: boom\nglance: the background copy stopped before its status item was up \
             (exit status: 3)\n"
        );
    }

    #[test]
    fn a_stderr_text_with_no_newline_gets_one() {
        let mut child = spawn_script("printf half >&2; exit 1");
        let failure = await_ready(&mut child).expect_err("no line");
        assert!(
            failure.report().starts_with("half\nglance: "),
            "{failure:?}"
        );
    }

    #[test]
    fn a_copy_that_says_nothing_gives_only_its_status() {
        let mut child = spawn_script("exit 4");
        let failure = await_ready(&mut child).expect_err("no line");
        assert_eq!(
            failure.report(),
            "glance: the background copy stopped before its status item was up \
             (exit status: 4)\n"
        );
    }

    #[test]
    fn a_copy_with_a_long_stderr_does_not_block_the_start() {
        let mut child = spawn_script("head -c 200000 /dev/zero | tr '\\0' x >&2; printf 'up\\n'");
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(await_ready(&mut child).ok());
        });
        let line = receiver
            .recv_timeout(Duration::from_secs(60))
            .expect("the start blocked on a full stderr pipe");
        assert_eq!(line.as_deref(), Some("up\n"));
    }
}

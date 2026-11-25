//! What one run of a tool does with the instance of its target: start it
//! here, start it in the background, or stop it.
//!
//! Each target has one instance (see [`crate::instance`]). A run first
//! looks for the instance of its target. When one runs, the run stops it,
//! starts nothing, and ends. Thus a second run of the same command closes the
//! instance that the first run opened. A run for a different target
//! leaves that instance alone.
//!
//! A tool describes itself with the [`Program`] trait, and [`run`] does the
//! rest.
//!
//! A background start makes a copy of the tool with a hidden flag. It does not
//! `fork` without `exec`: macOS refuses AppKit in a forked child. The copy
//! starts a new session, so the terminal of the start cannot stop it, and it
//! takes the lock itself. Thus two starts at the same moment can never give two
//! instances for one target.
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
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::thread;

use crate::instance::{self, ClaimError, LockFile, Pid};

/// The exit code of a run that failed.
const EXIT_FAILURE: u8 = 1;

/// The working directory of the background copy.
///
/// The copy runs long after its start ended, and a volume cannot unmount
/// while a process holds a directory on it. The root directory is on no
/// volume that a user unmounts.
const CHILD_DIRECTORY: &str = "/";

/// How a run starts the instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The instance runs in this process, which holds the terminal.
    Foreground,
    /// This process starts a copy that runs the instance, and returns.
    Background,
    /// This process is the copy that a background start made.
    SpawnedCopy,
}

/// What a tool tells [`run`] about itself.
pub trait Program {
    /// The name at the start of each message, for example `glance`.
    fn name(&self) -> &str;

    /// The lock file of this run.
    ///
    /// # Errors
    ///
    /// Returns the error of [`LockFile::for_user`].
    fn lock(&self) -> io::Result<LockFile>;

    /// The arguments that a background start gives to its copy. They include
    /// the hidden flag of the copy.
    fn child_arguments(&self) -> Vec<String>;

    /// The line that tells that the item is up, with no newline.
    fn ready_line(&self, pid: Pid) -> String;

    /// The line that tells that a run stopped the item, with no newline.
    fn stopped_line(&self, pid: Pid) -> String;

    /// The life of the item after it is up, or `None` for no bound.
    fn exit_after(&self) -> Option<NonZeroU32>;

    /// Runs the item in this process. Calls `on_ready` once when the item is
    /// up. Returns only when the item cannot start or stops.
    fn run_here(self, on_ready: Box<dyn FnOnce()>) -> ExitCode;
}

/// Starts or stops the instance of the target of `program`, as `mode` tells,
/// and gives the exit code of the run.
///
/// A foreground run returns only when the item cannot start. A background run
/// returns when the item of its copy is up.
#[must_use]
pub fn run<P: Program>(mode: Mode, program: P) -> ExitCode {
    let lock = match program.lock() {
        Ok(lock) => lock,
        Err(error) => {
            return fail(
                program.name(),
                &format!("cannot find the lock file: {error}"),
            )
        }
    };
    if mode != Mode::SpawnedCopy {
        match instance::holder(&lock) {
            Err(error) => return fail(program.name(), &lock_problem(&lock, &error)),
            Ok(Some(pid)) => return stop(&lock, &program, pid),
            Ok(None) => {}
        }
    }
    match mode {
        Mode::Foreground => run_here(&lock, program, false),
        Mode::Background => start_in_background(&lock, &program),
        Mode::SpawnedCopy => match instance::start_a_session() {
            Ok(()) => run_here(&lock, program, true),
            Err(error) => fail(
                program.name(),
                &format!("cannot start a new session: {error}"),
            ),
        },
    }
}

/// Stops the item of `program`, which runs as `pid`.
fn stop<P: Program>(lock: &LockFile, program: &P, pid: Pid) -> ExitCode {
    match instance::stop(lock, pid) {
        Ok(()) => {
            say(&format!("{}\n", program.stopped_line(pid)));
            ExitCode::SUCCESS
        }
        Err(error) => fail(program.name(), &error.to_string()),
    }
}

/// Takes the lock and runs the item in this process. When the item is up,
/// writes the line that tells so to stdout: to the terminal of a foreground
/// run, or to the pipe of a background start. When `is_copy` is true, this
/// process is the background copy. It then points its stdout and its stderr
/// at `/dev/null` after the line, because its start ends.
fn run_here<P: Program>(lock: &LockFile, program: P, is_copy: bool) -> ExitCode {
    // The claim must live until the process ends. The run of the item never
    // returns while the item runs.
    let _claim = match instance::claim(lock) {
        Ok(claim) => claim,
        Err(ClaimError::Held(pid)) => {
            return fail(program.name(), &another_started(program.name(), pid))
        }
        Err(ClaimError::Io(error)) => {
            return fail(program.name(), &lock_problem(lock, &error));
        }
    };
    if let Err(error) = instance::end_with_success_on_terminate() {
        return fail(program.name(), &format!("cannot catch SIGTERM: {error}"));
    }
    let line = format!("{}\n", program.ready_line(Pid::of_this_process()));
    let exit_after = program.exit_after();
    program.run_here(Box::new(move || {
        say(&line);
        if is_copy {
            // The start has its line and nobody reads either pipe, so an
            // error here has no channel to report on.
            let _ = instance::send_output_nowhere();
        }
        if let Some(seconds) = exit_after {
            instance::end_after(seconds);
        }
    }))
}

/// Starts a copy that runs the item, and waits until its item is up. When the
/// copy ends first, writes what the copy wrote to its stderr and its exit
/// status, and gives the exit code of a failure.
fn start_in_background<P: Program>(lock: &LockFile, program: &P) -> ExitCode {
    let name = program.name();
    if let Err(error) = lock.prepare() {
        return fail(name, &lock_problem(lock, &error));
    }
    let executable = match env::current_exe() {
        Ok(executable) => executable,
        Err(error) => return fail(name, &format!("cannot find the path of {name}: {error}")),
    };
    let spawned = copy_pipes(childenv::command(executable).args(program.child_arguments()))
        .current_dir(CHILD_DIRECTORY)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return fail(name, &format!("cannot start the background copy: {error}")),
    };
    match await_ready(&mut child) {
        Ok(line) => {
            say(&line);
            ExitCode::SUCCESS
        }
        Err(failure) => {
            eprint!("{}", failure.report(name));
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

/// Sets the pipes of the background copy: no stdin, and a pipe to the start
/// for each of stdout and stderr.
fn copy_pipes(command: &mut Command) -> &mut Command {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
}

/// How a background copy failed before its item was up.
#[derive(Debug)]
struct CopyFailure {
    /// What the copy wrote to its stderr.
    said: String,
    /// The result of the wait for the copy.
    status: io::Result<ExitStatus>,
}

impl CopyFailure {
    /// The whole text that the start writes to its stderr, with `name` at the
    /// start of the last line.
    fn report(&self, name: &str) -> String {
        let mut text = self.said.clone();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        format!("{text}{name}: {}\n", copy_stopped(&self.status))
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
/// the item runs or stopped already.
fn say(text: &str) {
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

/// Writes `message` to stderr, after `name`, and gives the exit code of a
/// failure.
fn fail(name: &str, message: &str) -> ExitCode {
    eprintln!("{name}: {message}");
    ExitCode::from(EXIT_FAILURE)
}

/// The reason of a run that found the lock taken when it claimed the lock.
/// The background copy writes it, and its start relays it.
fn another_started(name: &str, pid: Pid) -> String {
    format!("another {name} started the same status item at the same time, as PID {pid}")
}

/// The reason of a lock file that cannot be used.
fn lock_problem(lock: &LockFile, error: &io::Error) -> String {
    format!("cannot use the lock file {lock}: {error}")
}

/// The reason of a background copy that ended before its item was up.
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
    use std::time::Duration;

    #[test]
    fn a_copy_that_stopped_names_its_status() {
        let failed: io::Result<ExitStatus> = Err(io::Error::other("no child"));
        assert_eq!(
            copy_stopped(&failed),
            "the background copy stopped before its status item was up (the wait failed: no child)"
        );
    }

    #[test]
    fn the_second_start_message_names_the_tool_and_the_pid() {
        assert_eq!(
            another_started("glance", Pid::new(7).expect("a PID")),
            "another glance started the same status item at the same time, as PID 7"
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
            failure.report("glance"),
            "glance: boom\nglance: the background copy stopped before its status item was up \
             (exit status: 3)\n"
        );
    }

    #[test]
    fn a_stderr_text_with_no_newline_gets_one() {
        let mut child = spawn_script("printf half >&2; exit 1");
        let failure = await_ready(&mut child).expect_err("no line");
        assert!(
            failure.report("glance").starts_with("half\nglance: "),
            "{failure:?}"
        );
    }

    #[test]
    fn a_copy_that_says_nothing_gives_only_its_status() {
        let mut child = spawn_script("exit 4");
        let failure = await_ready(&mut child).expect_err("no line");
        assert_eq!(
            failure.report("glance"),
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

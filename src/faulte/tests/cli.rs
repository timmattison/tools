//! Tests that run the built `faulte` binary.

use std::process::{Command, Output};

/// The path of the binary that cargo built for these tests.
const BIN: &str = env!("CARGO_BIN_EXE_faulte");

/// The terminal width that each run states, so that a layout of the help text
/// does not change with the window of the person who runs the tests.
const COLUMNS: &str = "100";

/// The variable that makes `clap` paint its help and its errors on any output.
///
/// The pre-commit hook of this repository sets it for `cargo test`. Each run
/// states it too, so that every run of these tests reads the painted output
/// that the hook reads, whatever the environment of the person who runs them.
const CLICOLOR_FORCE: &str = "CLICOLOR_FORCE";

/// The value of [`CLICOLOR_FORCE`] that turns the paint on.
const PAINT: &str = "1";

/// The exit code that clap gives for a usage error.
const USAGE_ERROR: i32 = 2;

/// Runs the binary with `args` and returns what it wrote and how it ended.
fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("COLUMNS", COLUMNS)
        .env(CLICOLOR_FORCE, PAINT)
        .output()
        .expect("the faulte binary starts")
}

/// Gives the glyphs of what the binary wrote, with the escape codes taken out.
///
/// `clap` paints the names of flags, the values, and the headings whenever it
/// decides that the run wants color, and it reads `CLICOLOR_FORCE` for that
/// decision, not only the terminal. The pre-commit hook of this repository
/// sets that variable, and so does [`run`]. Thus a flag arrives as
/// `\x1b[1m--interval\x1b[0m`, and a test that looks for `--interval` wants
/// the glyphs. Each read of stdout or stderr in this file goes through here.
/// See "Colored Output in Tests" in CLAUDE.md.
fn glyphs(bytes: &[u8]) -> String {
    testcolor::strip_ansi(&String::from_utf8_lossy(bytes))
}

/// Gives the glyphs of `text` with each run of white space made one space, so
/// that a line break in the help text does not split a phrase that a test
/// looks for.
fn one_line(text: &[u8]) -> String {
    glyphs(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `--limit` belongs to the ranking, and `faulte kill` refuses it.
///
/// The issue gives `--limit` to the ranking and `--max` to `faulte kill`. A
/// flag that a command takes and then ignores is worse than a flag that it
/// refuses: a person who writes `faulte kill --limit 3` reads the plan of
/// every candidate and believes that it holds three.
#[test]
fn kill_refuses_the_limit_of_the_ranking() {
    let output = run(&["kill", "--limit", "3", "--older-than", "9999d"]);
    let stderr = one_line(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(USAGE_ERROR),
        "faulte kill --limit is a usage error, it wrote {stderr:?}"
    );
    assert!(
        stderr.contains("--limit"),
        "the message names the flag that faulte kill does not take: {stderr:?}"
    );

    // The ranking takes it, before the name of a command as well.
    assert!(
        run(&[
            "--limit",
            "3",
            "kill",
            "--interval",
            "1s",
            "--older-than",
            "9999d"
        ])
        .status
        .success(),
        "the ranking flag stands before the name of the command"
    );
}

/// `--version` gives the name, the package version, the git hash, and the
/// state of the tree, in the format that every tool of this repository uses.
#[test]
fn version_names_the_tool_the_release_and_the_build() {
    let output = run(&["--version"]);
    let stdout = glyphs(&output.stdout);
    let stderr = glyphs(&output.stderr);

    assert!(
        output.status.success(),
        "faulte --version must exit 0, it ended with {:?} and wrote {stderr:?}",
        output.status
    );
    assert!(
        stdout.starts_with("faulte 0.1.0 ("),
        "the version line must name the tool and the release: {stdout:?}"
    );
    assert!(
        stdout.ends_with(")\n"),
        "the version line must end with the build facts in parentheses: {stdout:?}"
    );
}

/// Each flag that takes a duration reads it with the parser of the library.
/// A bad duration is a usage error that names the flag and the bad text, and
/// no command starts.
#[test]
fn a_bad_duration_in_any_flag_is_a_usage_error_that_names_the_text() {
    let cases: [(&[&str], &str, &str); 6] = [
        (&["--interval", "0s"], "--interval", "0s"),
        (&["--interval", "5x"], "--interval", "5x"),
        (&["kill", "--interval", "0"], "--interval", "0"),
        (&["kill", "--older-than=-7d"], "--older-than", "-7d"),
        (&["kill", "--older-than", "7日"], "--older-than", "7日"),
        (&["kill", "--idle-for", "1.5h"], "--idle-for", "1.5h"),
    ];
    for (args, flag, text) in cases {
        let output = run(args);
        let stderr = glyphs(&output.stderr);

        assert_eq!(
            output.status.code(),
            Some(USAGE_ERROR),
            "faulte {args:?} is a usage error: {stderr:?}"
        );
        assert!(
            stderr.contains(&format!("invalid value '{text}' for '{flag} <DURATION>'")),
            "clap names the flag {flag} and the text {text:?}: {stderr:?}"
        );
        assert!(
            stderr.contains(&format!("{text:?}")),
            "the message of the parser names the text {text:?}: {stderr:?}"
        );
    }
}

/// The one test that reads this Mac.
///
/// It runs the built binary with the shortest interval that the parser
/// accepts, and it holds the shape of the output alone. It states no number,
/// because every number is what this Mac did while the test ran. A number that
/// a test states here would fail on a Mac that is quiet, and on a Mac that is
/// busy.
#[test]
fn the_ranking_states_a_header_and_a_table_of_this_mac() {
    let output = run(&["--interval", "1s", "--limit", "5"]);
    let stdout = glyphs(&output.stdout);
    let stderr = glyphs(&output.stderr);

    assert!(
        output.status.success(),
        "faulte ranks the processes of this Mac and exits 0, it ended with {:?} and wrote {stderr:?}",
        output.status
    );
    let one_line = one_line(&output.stdout);
    for phrase in [
        "window (interval 1s)",
        "faults,",
        "swap:",
        "compressor",
        "Claude:",
    ] {
        assert!(
            one_line.contains(phrase),
            "the header states {phrase:?}: {stdout}"
        );
    }
    for column in ["PID", "OWNER", "FAULTS/S", "SHARE", "RSS", "AGE", "COMMAND"] {
        assert!(
            stdout.contains(column),
            "the table states the column {column:?}: {stdout}"
        );
    }
}

/// An age that no process of any Mac reaches. The oldest process of a Mac
/// started when the Mac started, and no Mac runs for 27 years.
const NO_PROCESS_IS_THIS_OLD: &str = "9999d";

/// The second test that reads this Mac.
///
/// `--older-than 9999d` selects nothing on any Mac, so the plan names no
/// session and `faulte` asks nothing. The test states no number, because every
/// count under the plan is what this Mac runs while the test runs. It signals
/// nothing: a run with no candidate reaches no signal at all.
#[test]
fn a_kill_that_selects_nothing_prints_the_plan_and_exits_zero() {
    let output = run(&[
        "kill",
        "--interval",
        "1s",
        "--older-than",
        NO_PROCESS_IS_THIS_OLD,
    ]);
    let stdout = glyphs(&output.stdout);
    let stderr = glyphs(&output.stderr);

    assert!(
        output.status.success(),
        "a plan with no candidate exits 0, it ended with {:?} and wrote {stderr:?}",
        output.status
    );
    let one_line = one_line(&output.stdout);
    let older_than = format!("older than {NO_PROCESS_IS_THIS_OLD}");
    for phrase in [
        older_than.as_str(),
        "idle for more than 10m",
        "no live descendant",
        "faulte stops nothing.",
    ] {
        assert!(
            one_line.contains(phrase),
            "the plan states {phrase:?}: {stdout}"
        );
    }
    assert!(
        !stdout.contains("[y/N]"),
        "a plan with no candidate asks nothing: {stdout}"
    );
}

/// `--help` gives each flag and its default, so the defaults of the issue are
/// held here: a 5 s sample, 25 rows, sessions older than 7 days, and idle for
/// more than 10 minutes.
#[test]
fn help_gives_each_flag_and_its_default() {
    let top = run(&["--help"]);
    let kill = run(&["kill", "--help"]);
    assert!(top.status.success(), "faulte --help exits 0");
    assert!(kill.status.success(), "faulte kill --help exits 0");

    let top = one_line(&top.stdout);
    for phrase in [
        "--interval <DURATION>",
        "[default: 5s]",
        "--limit <N>",
        "[default: 25]",
        "kill",
    ] {
        assert!(
            top.contains(phrase),
            "faulte --help gives {phrase:?}: {top}"
        );
    }

    let kill = one_line(&kill.stdout);
    for phrase in [
        "--older-than <DURATION>",
        "[default: 7d]",
        "--idle-for <DURATION>",
        "[default: 10m]",
        "--max <N>",
        "--interval <DURATION>",
    ] {
        assert!(
            kill.contains(phrase),
            "faulte kill --help gives {phrase:?}: {kill}"
        );
    }
}

/// The tests of the real signals of this Mac.
///
/// These are the one place where `faulte` signals a process for real, and each
/// one of them signals a child that the test itself started. No test here runs
/// `faulte kill` against this Mac: that command stops a Claude Code session,
/// and the sessions of this Mac belong to a person.
#[cfg(target_os = "macos")]
mod signals {
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::thread;
    use std::time::{Duration, SystemTime};

    use faulte::machine::macos::Mac;
    use faulte::machine::{Machine, MachineError, Signal};
    use faulte::pid::Pid;

    /// The program that each child of these tests runs.
    const SLEEP: &str = "/bin/sleep";

    /// How long that program sleeps for, in seconds.
    ///
    /// It is far longer than any test here takes, so a child that a test finds
    /// alive is a child that the signal did not stop. It is short enough that
    /// a child which escapes a test is gone one minute later.
    const SLEEP_SECONDS: &str = "60";

    /// The shell that makes a child which ignores `SIGTERM`.
    const SHELL: &str = "/bin/sh";

    /// The process that every account of this Mac can name and no account
    /// other than root can signal.
    const LAUNCHD: Pid = Pid::new(1);

    /// The longest time that a test waits for a child to stop.
    ///
    /// A child of these tests stops in milliseconds. This bound is what makes
    /// the wait end on a Mac that is short of memory, which is the Mac that
    /// `faulte` is for.
    const DEADLINE: Duration = Duration::from_secs(10);

    /// The time between two asks about a child that is stopping.
    const STEP: Duration = Duration::from_millis(20);

    /// The greatest age that a child of a test can have, in seconds.
    ///
    /// The process table states the start time to the second. A child that a
    /// test started moments ago is younger than this, on a Mac of any speed.
    const YOUNGEST_MINUTES: u64 = 10;

    /// Starts a child that sleeps and stops on `SIGTERM`.
    ///
    /// The command is the sleeping program itself, and no shell stands between
    /// the test and it. Thus the number that [`Child::id`] gives is the number
    /// of the process that the test signals.
    fn sleeping_child() -> Child {
        Command::new(SLEEP)
            .arg(SLEEP_SECONDS)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the sleeping program starts")
    }

    /// Starts a child that sleeps and ignores `SIGTERM`.
    ///
    /// The shell sets `SIGTERM` to be ignored, and then it replaces itself
    /// with the sleeping program. A signal that a process ignores stays
    /// ignored over that replacement, which POSIX states. Thus this child is
    /// one process, the same as [`sleeping_child`], and it answers no
    /// `SIGTERM`.
    fn deaf_child() -> Child {
        Command::new(SHELL)
            .arg("-c")
            .arg(format!("trap '' TERM; exec {SLEEP} {SLEEP_SECONDS}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the shell starts")
    }

    /// Waits until the child `pid` replaced itself with the sleeping program.
    ///
    /// A shell that did not reach the `exec` yet is a shell that did not set
    /// the trap yet, and a `SIGTERM` in that moment stops it in the usual way.
    /// The process table tells the two states apart: before the `exec` the row
    /// gives the whole command line of the shell, and after it the row gives
    /// the sleeping program and its one argument.
    ///
    /// The wait is bounded by [`DEADLINE`], the same as [`exit_of`].
    fn wait_until_deaf(mac: &Mac, pid: Pid) -> bool {
        let deaf = format!("{SLEEP} {SLEEP_SECONDS}");
        for _ in 0..asks() {
            let replaced = mac.process_table().is_ok_and(|table| {
                table
                    .iter()
                    .any(|row| row.pid == pid && !row.zombie && row.command == deaf)
            });
            if replaced {
                return true;
            }
            thread::sleep(STEP);
        }
        false
    }

    /// Gives the number of asks that [`DEADLINE`] holds, one every [`STEP`].
    fn asks() -> u128 {
        DEADLINE.as_millis() / STEP.as_millis()
    }

    /// Waits for `child` to stop, and gives how it stopped.
    ///
    /// The wait is bounded by [`DEADLINE`]. A test that waits without a bound
    /// holds its whole run when the behavior under test is broken, which is
    /// the one time that a test must fail fast.
    ///
    /// A child that stopped is collected here, so it leaves no zombie behind.
    fn exit_of(child: &mut Child) -> Option<ExitStatus> {
        for _ in 0..asks() {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) => thread::sleep(STEP),
                Err(_) => return None,
            }
        }
        None
    }

    /// Stops `child` and collects it, whatever the test found.
    ///
    /// A child that stopped already is collected already, and this function
    /// then does nothing. A child that survived the test gets `SIGKILL`, so a
    /// test that failed leaves no process of its own behind.
    fn clean_up(child: &mut Child) {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Gives the time now in seconds since the Unix epoch.
    fn epoch_seconds() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock of this Mac is after 1970")
            .as_secs()
    }

    /// `SIGTERM` reaches a child of this test, and `SIGTERM` is what stops it.
    ///
    /// `faulte kill` sends this signal first, so that Claude Code closes its
    /// transcript. The exit status names the signal, so the test proves that
    /// the child stopped for this reason and not for another one.
    #[test]
    fn sigterm_stops_a_child_of_this_test() {
        let mac = Mac::new();
        let mut child = sleeping_child();

        let sent = mac.signal(Pid::new(child.id()), Signal::Terminate);
        let stopped = exit_of(&mut child);
        clean_up(&mut child);

        assert!(
            sent.is_ok(),
            "the signal reaches a child of this test: {sent:?}"
        );
        let status = stopped.expect("the child stops inside the deadline of the test");
        assert_eq!(
            status.signal(),
            Some(libc::SIGTERM),
            "SIGTERM is what stopped the child: {status:?}"
        );
    }

    /// `SIGKILL` stops a child that ignores `SIGTERM`.
    ///
    /// `faulte kill` sends this signal after the grace period, to each target
    /// that is still the same process. A session that handles no signal is the
    /// reason why that step exists, and this child is such a process.
    #[test]
    fn sigkill_stops_a_child_that_ignores_sigterm() {
        let mac = Mac::new();
        let mut child = deaf_child();
        let pid = Pid::new(child.id());

        let deaf = wait_until_deaf(&mac, pid);
        let terminate = mac.signal(pid, Signal::Terminate);
        // The child ignores the first signal, so it is alive here. A child
        // that stopped would make the second signal fail, and the test would
        // then report the wrong fault.
        let ignored = matches!(child.try_wait(), Ok(None));
        let kill = mac.signal(pid, Signal::Kill);
        let stopped = exit_of(&mut child);
        clean_up(&mut child);

        assert!(deaf, "the child sets the trap and replaces itself in time");
        assert!(
            terminate.is_ok(),
            "SIGTERM reaches the child: {terminate:?}"
        );
        assert!(ignored, "the child ignores SIGTERM and keeps running");
        assert!(kill.is_ok(), "SIGKILL reaches the child: {kill:?}");
        let status = stopped.expect("the child stops inside the deadline of the test");
        assert_eq!(
            status.signal(),
            Some(libc::SIGKILL),
            "SIGKILL is what stopped the child: {status:?}"
        );
    }

    /// The process table of this Mac holds a child of this test, with the time
    /// that it started.
    ///
    /// Every rule of `faulte kill` reads that time. The plan selects a session
    /// by its age, and the check before each signal compares the time again,
    /// because a PID alone is not an identity: the number comes back for
    /// another process.
    #[test]
    fn the_process_table_holds_a_child_of_this_test_with_its_start_time() {
        let mac = Mac::new();
        let mut child = sleeping_child();
        let pid = Pid::new(child.id());

        let table = mac.process_table();
        let now = epoch_seconds();
        clean_up(&mut child);

        let table = table.expect("ps gives the process table of this Mac");
        let row = table
            .iter()
            .find(|row| row.pid == pid)
            .unwrap_or_else(|| panic!("the table holds the child {pid} of this test"));
        assert!(!row.zombie, "the child of the test is alive: {row:?}");
        assert!(
            row.command.contains(SLEEP),
            "the row names the command of the child: {row:?}"
        );
        assert!(
            row.started_at_epoch_secs <= now,
            "the child started before the table was read: {row:?}, and now is {now}"
        );
        assert!(
            row.started_at_epoch_secs + YOUNGEST_MINUTES * 60 >= now,
            "the child started moments ago: {row:?}, and now is {now}"
        );
    }

    /// A signal to a process of another account gives an error, and never a
    /// panic.
    ///
    /// Two accounts share the Mac that `faulte` is for, and the report of a
    /// stop names each session that `faulte` could not signal. A run that
    /// panicked instead would leave the person with no report at all.
    ///
    /// The test skips itself under root. Root signals every process of this
    /// Mac, so there is no process for the test to be refused by.
    #[test]
    fn a_signal_to_a_process_of_another_account_is_an_error() {
        let mac = Mac::new();
        if mac.viewer().is_root {
            return;
        }

        let refused = mac.signal(LAUNCHD, Signal::Terminate);

        let error = refused.expect_err("a plain account cannot signal the first process");
        assert!(
            matches!(error, MachineError::SignalFailed { .. }),
            "the error is a signal that failed: {error:?}"
        );
        assert!(
            error
                .to_string()
                .contains(&format!("kill({LAUNCHD}, SIGTERM)")),
            "the message names the call that failed: {error}"
        );
    }
}

/// `faulte check` reads this Mac, states one verdict on its first line, and
/// gives the exit code of that verdict.
///
/// The verdict depends on the Mac at the time of the test, thus the test holds
/// the rule that links the text to the exit code, and not one verdict.
#[cfg(target_os = "macos")]
#[test]
fn check_states_a_verdict_and_the_exit_code_of_that_verdict() {
    let output = run(&["check", "--interval", "1s"]);
    let text = glyphs(&output.stdout);
    let first = text.lines().next().unwrap_or_default();

    assert!(first.starts_with("faulte check: "), "the verdict: {text}");
    let expected = i32::from(first != "faulte check: OK");
    assert_eq!(
        output.status.code(),
        Some(expected),
        "the exit code: {text}"
    );
    assert!(text.contains("compressions/s"), "the rates: {text}");
    assert!(text.contains("% system"), "the CPU split: {text}");
    assert!(
        text.contains("new processes/s"),
        "the new processes: {text}"
    );
}

/// One line of a watch reaches a reader that stops after that line, and the
/// run then ends with no panic.
#[cfg(target_os = "macos")]
#[test]
fn check_watch_prints_lines_and_ends_when_the_reader_goes_away() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let mut child = Command::new(BIN)
        .args(["check", "--watch", "--interval", "1s"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the faulte binary starts");
    let stdout = child.stdout.take().expect("the pipe of the output");
    let mut first = String::new();
    BufReader::new(stdout)
        .read_line(&mut first)
        .expect("one line of the watch");
    // The read end of the pipe closes at the end of the statement above. The
    // next line of the watch then finds no reader.
    let status = child.wait().expect("the watch ends");

    assert!(first.contains("% "), "a line of the watch: {first:?}");
    assert!(first.contains("/s"), "the rates of the watch: {first:?}");
    assert_eq!(
        status.code(),
        Some(0),
        "a watch with no reader ends cleanly"
    );
}

/// A reader that leaves before the report starts does not change how the run
/// ends. The run keeps the exit code of its verdict and does not panic.
///
/// The verdict depends on the Mac at the time of the test, thus the test holds
/// the two exit codes that a verdict can give.
#[cfg(target_os = "macos")]
#[test]
fn check_with_no_reader_keeps_the_exit_code_of_its_verdict() {
    use std::process::Stdio;

    let mut child = Command::new(BIN)
        .args(["check", "--interval", "1s"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the faulte binary starts");
    // The read end of the pipe closes here, one second before the report.
    drop(child.stdout.take());
    let output = child.wait_with_output().expect("the check ends");

    let stderr = glyphs(&output.stderr);
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "the exit code of a verdict, not {:?}: {stderr}",
        output.status.code(),
    );
    assert!(!stderr.contains("panicked"), "no panic: {stderr}");
}

/// The longest time that a run with no room for its output may take.
///
/// A `--watch` run whose write does not fail never ends, thus the helper stops
/// it at this bound.
#[cfg(target_os = "macos")]
const NO_ROOM_BOUND: std::time::Duration = std::time::Duration::from_secs(300);

/// Runs `faulte` with `args` and a file size limit of 0 on its output file.
///
/// The first write to the file fails with `EFBIG`. The shell ignores
/// `SIGXFSZ`, thus the write gives an error and the signal does not stop the
/// run. `exec` makes the child process the `faulte` binary itself. `name`
/// keeps two parallel tests apart. Gives the exit code and the glyphs of the
/// error output.
#[cfg(target_os = "macos")]
fn run_with_no_room_for_output(name: &str, args: &[&str]) -> (Option<i32>, String) {
    use std::process::Stdio;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_nanos();
    let file = std::env::temp_dir().join(format!(
        "faulte-no-room-{name}-{}-{nanos}.txt",
        std::process::id()
    ));
    let mut child = Command::new("/bin/sh")
        .args([
            "-c",
            r#"trap "" XFSZ; ulimit -f 0; output=$1; shift; exec "$@" > "$output""#,
            "sh",
        ])
        .arg(&file)
        .arg(BIN)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the shell starts");
    let start = Instant::now();
    loop {
        if child.try_wait().expect("the child can be polled").is_some() {
            break;
        }
        if start.elapsed() >= NO_ROOM_BOUND {
            child.kill().expect("the child can be killed");
            child.wait().expect("the child ends after the kill");
            let _ = std::fs::remove_file(&file);
            panic!("the run did not end within {NO_ROOM_BOUND:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().expect("the output is read");
    std::fs::remove_file(&file).expect("the shell made the file");
    (output.status.code(), glyphs(&output.stderr))
}

/// A report that cannot be written gives the exit code 2 and a message.
#[cfg(target_os = "macos")]
#[test]
fn check_with_no_room_for_its_report_is_an_error() {
    let (code, stderr) = run_with_no_room_for_output("report", &["check", "--interval", "1s"]);

    assert_eq!(code, Some(2), "stderr: {stderr}");
    assert!(
        stderr.contains("the check did not write its output"),
        "the message: {stderr}"
    );
}

/// A watch line that cannot be written gives the exit code 2 and a message.
#[cfg(target_os = "macos")]
#[test]
fn check_watch_with_no_room_for_its_line_is_an_error() {
    let (code, stderr) =
        run_with_no_room_for_output("watch", &["check", "--watch", "--interval", "1s"]);

    assert_eq!(code, Some(2), "stderr: {stderr}");
    assert!(
        stderr.contains("the check did not write its output"),
        "the message: {stderr}"
    );
}

//! Black-box tests for the `prgz` binary.
//!
//! Each test drives the binary that cargo built. Each test that touches the
//! file system makes its own temporary directory, thus two copies of this test
//! binary can run at the same moment.
//!
//! Each test also sets the locale of the child process itself. It gives the
//! child the value of `LANG` that the test needs, and it removes `LC_ALL` and
//! `LC_NUMERIC` from the child. POSIX gives those two variables precedence
//! over `LANG`, thus a shell that exports either one would else override the
//! locale that a test sets. No test reads the environment of the test
//! process, and no test writes that environment, thus the result of a test
//! does not follow the shell that started it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "every unwrap and expect in this file is an assertion, not an unhandled error: on the temporary directories and fixture files that the test just made, on the spawn of the binary that cargo just built, and on the read back of a file that the test itself wrote. The error paths of the binary are never unwrapped. A test reads them through the exit status and the standard error stream"
)]

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::read::GzDecoder;
use tempfile::TempDir;

/// The value of `LANG` that every test gives the child, except the test that
/// states what the locale does. American English groups the digits with a
/// comma, thus a report of that locale reads the same on every machine.
const AMERICAN_ENGLISH: &str = "en_US.UTF-8";

/// The value of `LANG` of the second run of the test of the locale. German
/// groups the digits with a point, thus it is the opposite of
/// [`AMERICAN_ENGLISH`] in both separators.
const GERMAN: &str = "de_DE.UTF-8";

/// The name of the environment variable that carries the locale.
const LANG_VARIABLE: &str = "LANG";

/// The name of the environment variable that carries the locale of the
/// user's whole session. POSIX gives this variable precedence over both
/// [`LC_NUMERIC_VARIABLE`] and [`LANG_VARIABLE`].
const LC_ALL_VARIABLE: &str = "LC_ALL";

/// The name of the environment variable that carries the locale of numbers
/// alone. POSIX gives this variable precedence over [`LANG_VARIABLE`], but
/// [`LC_ALL_VARIABLE`] still wins over it.
const LC_NUMERIC_VARIABLE: &str = "LC_NUMERIC";

/// The status that the binary answers when a run fails. Every failure path of
/// the Go tool that this binary replaces exits with this status.
const FAILURE_STATUS: i32 = 1;

/// The text of the fixture that compresses well. The same short line many
/// times over gives gzip a great deal to remove.
const COMPRESSIBLE_LINE: &[u8] = b"the quick brown fox\n";

/// The count of times that a fixture repeats [`COMPRESSIBLE_LINE`]. The
/// product is 8000 bytes, thus the report of the run carries a number with a
/// group separator in it.
const COMPRESSIBLE_REPEATS: usize = 400;

/// The size of the compressible fixture, as an American English reader of the
/// report reads it.
const SIZE_IN_AMERICAN_ENGLISH: &str = "8,000 bytes";

/// The same size, as a German reader of the report reads it.
const SIZE_IN_GERMAN: &str = "8.000 bytes";

/// The count of bytes of the fixture that gzip cannot make smaller.
const INCOMPRESSIBLE_BYTES: usize = 4_096;

/// The multiplier of the generator that makes bytes that gzip cannot compress.
const GENERATOR_MULTIPLIER: u64 = 6_364_136_223_846_793_005;

/// The increment of that generator.
const GENERATOR_INCREMENT: u64 = 1_442_695_040_888_963_407;

/// The first state of that generator.
const GENERATOR_SEED: u64 = 0x2545_f491_4f6c_dd1d;

/// The count of bits that the generator drops to reach its top byte.
const TOP_BYTE_SHIFT: u32 = 56;

/// What the run of a test answered: whether it was a success, the status code,
/// the standard output stream, and the standard error stream.
struct Run {
    /// Whether the process exited with a status of success.
    ok: bool,
    /// The status code of the process. A process that a signal stopped has
    /// none.
    code: Option<i32>,
    /// The text of the standard output stream.
    stdout: String,
    /// The text of the standard error stream.
    stderr: String,
}

/// Start the binary that cargo built, with the locale of the test.
///
/// The command gets the value of [`AMERICAN_ENGLISH`] in `LANG`, and it loses
/// `LC_ALL` and `LC_NUMERIC` if the shell that started the test carried
/// either one. POSIX gives those two variables precedence over `LANG`, thus a
/// leaked value would silently override the locale that a test sets.
fn prgz() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_prgz"));
    command.env(LANG_VARIABLE, AMERICAN_ENGLISH);
    command.env_remove(LC_ALL_VARIABLE);
    command.env_remove(LC_NUMERIC_VARIABLE);
    command
}

/// Run a command and read the three answers of the process.
fn run(command: &mut Command) -> Run {
    let output = command.output().unwrap();
    Run {
        ok: output.status.success(),
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Make a temporary directory that holds one input file, and answer both the
/// directory and the path of that file.
fn fixture(name: &str, bytes: &[u8]) -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join(name);
    std::fs::write(&input, bytes).unwrap();
    (directory, input)
}

/// The bytes of a fixture that compresses well.
fn compressible() -> Vec<u8> {
    COMPRESSIBLE_LINE.repeat(COMPRESSIBLE_REPEATS)
}

/// Make bytes that gzip cannot make smaller.
///
/// The generator is a linear congruential one, thus every run of the test gets
/// the same bytes and a failure of the test repeats.
fn incompressible(count: usize) -> Vec<u8> {
    let mut state = GENERATOR_SEED;
    (0..count)
        .map(|_| {
            state = state
                .wrapping_mul(GENERATOR_MULTIPLIER)
                .wrapping_add(GENERATOR_INCREMENT);
            u8::try_from(state >> TOP_BYTE_SHIFT).unwrap_or_default()
        })
        .collect()
}

/// Read a gzip file and answer the bytes that it holds.
fn gunzip(path: &Path) -> Vec<u8> {
    let file = File::open(path).unwrap();
    let mut decoder = GzDecoder::new(file);
    let mut bytes = Vec::new();
    decoder.read_to_end(&mut bytes).unwrap();
    bytes
}

/// Answer the line of a report that starts with a label.
fn line_with<'a>(report: &'a str, label: &str) -> &'a str {
    report
        .lines()
        .find(|line| line.contains(label))
        .unwrap_or_default()
}

#[test]
fn a_run_writes_a_gzip_file_that_holds_the_bytes_of_the_input() {
    let bytes = compressible();
    let (directory, input) = fixture("data.txt", &bytes);
    let output = directory.path().join("data.txt.gz");

    let answer = run(prgz().arg("--input").arg(&input));

    assert!(answer.ok, "the run failed with {}", answer.stderr);
    assert!(output.exists(), "the run wrote no file at {output:?}");
    assert_eq!(gunzip(&output), bytes);
}

#[test]
fn a_run_writes_the_file_that_the_output_flag_names() {
    let bytes = compressible();
    let (directory, input) = fixture("data.txt", &bytes);
    let output = directory.path().join("named.gz");

    let answer = run(prgz()
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output));

    assert!(answer.ok, "the run failed with {}", answer.stderr);
    assert!(output.exists(), "the run wrote no file at {output:?}");
    assert!(
        !directory.path().join("data.txt.gz").exists(),
        "the run also wrote the default output file"
    );
    assert_eq!(gunzip(&output), bytes);
}

#[test]
fn a_run_with_no_output_flag_adds_the_suffix_to_the_whole_input_name() {
    let bytes = compressible();
    let (directory, input) = fixture("notes.tar", &bytes);
    let output = directory.path().join("notes.tar.gz");

    let answer = run(prgz().arg("--input").arg(&input));

    assert!(answer.ok, "the run failed with {}", answer.stderr);
    assert!(output.exists(), "the run wrote no file at {output:?}");
    assert_eq!(gunzip(&output), bytes);
}

#[test]
fn a_run_with_no_input_flag_prints_the_help_and_fails() {
    let answer = run(&mut prgz());

    assert_eq!(
        answer.code,
        Some(FAILURE_STATUS),
        "the run answered a status of {:?} with {}",
        answer.code,
        answer.stderr
    );
    assert!(
        answer.stderr.contains("--input"),
        "the help text does not name the input flag: {}",
        answer.stderr
    );
}

#[test]
fn a_run_over_a_file_that_is_not_there_names_that_file_and_fails() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("not-there.txt");

    let answer = run(prgz().arg("--input").arg(&input));

    assert!(!answer.ok, "the run answered a success");
    assert!(
        answer.stderr.contains(&input.display().to_string()),
        "the message does not name the input file: {}",
        answer.stderr
    );
}

#[test]
fn a_run_that_writes_into_a_directory_that_is_not_there_names_that_file_and_fails() {
    let (directory, input) = fixture("data.txt", &compressible());
    let output = directory.path().join("no-such-directory").join("data.gz");

    let answer = run(prgz()
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output));

    assert!(!answer.ok, "the run answered a success");
    assert!(
        answer.stderr.contains(&output.display().to_string()),
        "the message does not name the output file: {}",
        answer.stderr
    );
}

#[test]
fn a_run_over_a_name_of_many_bytes_per_character_holds_the_bytes_of_the_input() {
    for name in [
        "\u{65e5}\u{672c}\u{8a9e}.txt",
        "caf\u{e9}.txt",
        "\u{1f389}.txt",
    ] {
        let bytes = compressible();
        let (directory, input) = fixture(name, &bytes);
        let output = directory.path().join(format!("{name}.gz"));

        let answer = run(prgz().arg("--input").arg(&input));

        assert!(
            answer.ok,
            "the run over {name} failed with {}",
            answer.stderr
        );
        assert!(output.exists(), "the run over {name} wrote no file");
        assert_eq!(gunzip(&output), bytes, "the run over {name} lost bytes");
    }
}

#[test]
fn the_version_flag_names_the_tool_and_the_build() {
    let answer = run(prgz().arg("--version"));

    assert!(answer.ok, "the run failed with {}", answer.stderr);
    let line = answer.stdout.lines().next().unwrap_or_default();
    assert!(line.starts_with("prgz "), "the line is {line:?}");
    assert!(line.contains('('), "the line is {line:?}");
    assert!(line.contains(')'), "the line is {line:?}");
}

#[test]
fn a_run_over_bytes_that_gzip_cannot_compress_warns_and_shows_a_negative_change() {
    let (_directory, input) = fixture("noise.bin", &incompressible(INCOMPRESSIBLE_BYTES));

    let answer = run(prgz().arg("--input").arg(&input));

    assert!(answer.ok, "the run failed with {}", answer.stderr);
    assert!(
        answer.stdout.contains("Warning"),
        "the report holds no warning: {}",
        answer.stdout
    );
    let change = line_with(&answer.stdout, "Size change:");
    assert!(change.contains('-'), "the line is {change:?}");
    assert!(change.contains('%'), "the line is {change:?}");
}

#[test]
fn a_run_where_the_output_names_the_input_leaves_the_file_untouched_and_fails() {
    let bytes = compressible();
    let (_directory, input) = fixture("notes.txt", &bytes);

    let answer = run(prgz()
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&input));

    assert!(!answer.ok, "the run answered a success");
    assert!(
        answer.stderr.contains(&input.display().to_string()),
        "the message does not name the file: {}",
        answer.stderr
    );
    assert_eq!(
        std::fs::read(&input).unwrap(),
        bytes,
        "the run changed the bytes of the input file"
    );
}

#[test]
fn a_run_where_a_different_spelling_of_the_output_path_still_names_the_input_leaves_it_untouched() {
    let bytes = compressible();
    let (directory, input) = fixture("notes.txt", &bytes);
    let output = directory.path().join(".").join("notes.txt");

    let answer = run(prgz()
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output));

    assert!(!answer.ok, "the run answered a success");
    assert_eq!(
        std::fs::read(&input).unwrap(),
        bytes,
        "the run changed the bytes of the input file"
    );
}

#[cfg(unix)]
#[test]
fn a_run_where_the_output_is_a_hard_link_to_the_input_leaves_it_untouched() {
    let bytes = compressible();
    let (directory, input) = fixture("a.txt", &bytes);
    let output = directory.path().join("b.txt");
    std::fs::hard_link(&input, &output).unwrap();

    let answer = run(prgz()
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output));

    assert!(!answer.ok, "the run answered a success");
    assert_eq!(
        std::fs::read(&input).unwrap(),
        bytes,
        "the run changed the bytes of the input file"
    );
}

#[test]
fn a_report_follows_the_locale_of_the_environment() {
    let (_directory, input) = fixture("data.txt", &compressible());

    let american = run(prgz()
        .env(LANG_VARIABLE, AMERICAN_ENGLISH)
        .arg("--input")
        .arg(&input));
    let german = run(prgz().env(LANG_VARIABLE, GERMAN).arg("--input").arg(&input));

    assert!(
        american.ok,
        "the American run failed with {}",
        american.stderr
    );
    assert!(german.ok, "the German run failed with {}", german.stderr);
    assert!(
        american.stdout.contains(SIZE_IN_AMERICAN_ENGLISH),
        "the American report is {}",
        american.stdout
    );
    assert!(
        german.stdout.contains(SIZE_IN_GERMAN),
        "the German report is {}",
        german.stdout
    );
    assert_ne!(american.stdout, german.stdout);
}

#[test]
fn a_report_follows_lc_all_over_lang() {
    let (_directory, input) = fixture("data.txt", &compressible());

    let answer = run(prgz()
        .env(LC_ALL_VARIABLE, GERMAN)
        .env(LANG_VARIABLE, AMERICAN_ENGLISH)
        .arg("--input")
        .arg(&input));

    assert!(answer.ok, "the run failed with {}", answer.stderr);
    assert!(
        answer.stdout.contains(SIZE_IN_GERMAN),
        "the report is {}",
        answer.stdout
    );
}

#[test]
fn a_report_follows_lc_numeric_over_lang() {
    let (_directory, input) = fixture("data.txt", &compressible());

    let answer = run(prgz()
        .env(LC_NUMERIC_VARIABLE, GERMAN)
        .env(LANG_VARIABLE, AMERICAN_ENGLISH)
        .arg("--input")
        .arg(&input));

    assert!(answer.ok, "the run failed with {}", answer.stderr);
    assert!(
        answer.stdout.contains(SIZE_IN_GERMAN),
        "the report is {}",
        answer.stdout
    );
}

#[test]
fn a_report_follows_lc_all_over_lc_numeric() {
    let (_directory, input) = fixture("data.txt", &compressible());

    let answer = run(prgz()
        .env(LC_ALL_VARIABLE, AMERICAN_ENGLISH)
        .env(LC_NUMERIC_VARIABLE, GERMAN)
        .arg("--input")
        .arg(&input));

    assert!(answer.ok, "the run failed with {}", answer.stderr);
    assert!(
        answer.stdout.contains(SIZE_IN_AMERICAN_ENGLISH),
        "the report is {}",
        answer.stdout
    );
}

#[test]
fn a_report_follows_lang_when_lc_all_is_empty() {
    let (_directory, input) = fixture("data.txt", &compressible());

    let answer = run(prgz()
        .env(LC_ALL_VARIABLE, "")
        .env(LANG_VARIABLE, GERMAN)
        .arg("--input")
        .arg(&input));

    assert!(answer.ok, "the run failed with {}", answer.stderr);
    assert!(
        answer.stdout.contains(SIZE_IN_GERMAN),
        "the report is {}",
        answer.stdout
    );
}

/// The exit status that a shell reports for a process that SIGINT ends
/// outright. POSIX gives SIGINT the number 2, and the shell convention adds
/// 128 to a signal number to report a process that the signal ended.
const SECOND_SIGINT_EXIT_STATUS: i32 = 130;

/// The name of the input of the section of the file below. The input is a
/// named pipe, thus a read of it blocks until a writer gives it bytes.
const STALLED_INPUT_NAME: &str = "stalled.fifo";

/// The name of the output of the section of the file below. The run never
/// reads a byte off the stalled input, thus this file gains no bytes that a
/// test must check, only its own presence on the disk.
const STALLED_OUTPUT_NAME: &str = "stalled.fifo.gz";

/// The time between polls of the exit state of a child process. A short
/// interval keeps a passing test quick.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// The time that the test gives a run to stay alive after one SIGINT that a
/// blocked read keeps the run from answering a second time. This machine
/// runs many cargo jobs at once, thus the window is generous.
const STAYS_ALIVE_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);

/// The time that the test gives a run to end after the SIGINT that follows
/// the first one. The deadline is generous for the same reason as
/// [`STAYS_ALIVE_WINDOW`].
const ENDS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// The time that the test gives the write end of the FIFO to open. The open
/// call blocks until the run opens the read end of the same path, thus a run
/// that never reaches that open call would else block the open call, and the
/// test, forever.
const WRITER_OPEN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// Ends a child process, and reaps it, when the guard drops.
///
/// A test of this section spawns a run that a read keeps alive on purpose. A
/// panic that unwinds through such a test must still leave no process behind,
/// thus every path through the test owns one of these before it sends a
/// single signal.
#[cfg(unix)]
struct KillOnDrop(std::process::Child);

#[cfg(unix)]
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Make a named pipe at `path` with the `mkfifo` command of the platform.
#[cfg(unix)]
fn mkfifo(path: &Path) {
    let status = Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("the platform has no mkfifo command");
    assert!(status.success(), "mkfifo failed for {path:?}");
}

/// Open the write end of the FIFO at `path` and answer the handle.
///
/// The open call blocks until a reader opens the same path, thus the
/// function runs it on its own thread and gives the caller a bounded wait
/// through [`WRITER_OPEN_DEADLINE`]. A run that never opens the read end
/// would else block the open call, and the caller, forever.
#[cfg(unix)]
fn open_fifo_writer(path: &Path) -> File {
    let path = path.to_path_buf();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let file = std::fs::OpenOptions::new().write(true).open(&path);
        let _ = sender.send(file);
    });
    receiver
        .recv_timeout(WRITER_OPEN_DEADLINE)
        .expect("the write end of the FIFO did not open within the deadline")
        .expect("the write end of the FIFO did not open")
}

/// Poll `child` for [`STAYS_ALIVE_WINDOW`] and answer whether it was still
/// running at the end of the window.
#[cfg(unix)]
fn stays_alive(child: &mut std::process::Child) -> bool {
    let deadline = std::time::Instant::now() + STAYS_ALIVE_WINDOW;
    while std::time::Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return false;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    true
}

/// Poll `child` for [`ENDS_DEADLINE`] and answer its exit status once it
/// ends, or `None` when the deadline passes first.
#[cfg(unix)]
fn wait_for_exit(child: &mut std::process::Child) -> Option<std::process::ExitStatus> {
    let deadline = std::time::Instant::now() + ENDS_DEADLINE;
    while std::time::Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    None
}

/// Send SIGINT to the process named by `pid` with the `kill` command of the
/// platform.
#[cfg(unix)]
fn send_sigint(pid: u32) {
    let status = Command::new("kill")
        .arg("-INT")
        .arg(pid.to_string())
        .status()
        .expect("the platform has no kill command");
    assert!(status.success(), "kill -INT {pid} failed");
}

#[cfg(unix)]
#[test]
fn a_first_stop_signal_that_cannot_land_leaves_the_process_alive_and_a_second_one_ends_it() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join(STALLED_INPUT_NAME);
    let output = directory.path().join(STALLED_OUTPUT_NAME);
    mkfifo(&input);

    let child = prgz()
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .spawn()
        .expect("the run did not start");
    let mut guard = KillOnDrop(child);
    let pid = guard.0.id();

    // The open call of the write end blocks until the run opens the read
    // end of the same path, thus its return also tells the test that the
    // run passed its own open and moved on toward the read that never
    // returns.
    let writer = open_fifo_writer(&input);

    send_sigint(pid);
    assert!(
        stays_alive(&mut guard.0),
        "the run ended after one SIGINT, though its read of the stalled \
         input never returns and never gives the run a second look at the \
         stop flag; a wrong order of registration answers the first signal \
         instead of the second"
    );

    send_sigint(pid);
    let status = wait_for_exit(&mut guard.0)
        .expect("the run did not end within the deadline after the second SIGINT");
    drop(writer);

    assert_eq!(
        status.code(),
        Some(SECOND_SIGINT_EXIT_STATUS),
        "the run ended with a status of {status:?}, not the status that a \
         process ended by SIGINT reports"
    );
    assert!(
        output.exists(),
        "the second SIGINT should leave the part-written output file on \
         the disk, because no cleanup runs on that path"
    );
}

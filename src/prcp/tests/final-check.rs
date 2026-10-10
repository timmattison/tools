//! End-to-end tests of the final check. After the last copy, `prcp` walks the
//! source and the destination again, and reports each change that it did not
//! make. A copy reports and fails. A move also keeps the originals.
//!
//! A test must change the tree in the middle of a real run. The question that
//! `prcp` asks before it overwrites a file holds the run between two files, so
//! each test starts a run without `-y` into a destination that already holds
//! one planned file. While `prcp` waits for the answer, the test changes the
//! tree, as a person or a program beside `prcp` does. Then it answers yes.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![warn(clippy::panic)]
#![deny(clippy::unimplemented)]
#![warn(clippy::cast_possible_truncation)]
#![warn(clippy::cast_sign_loss)]
#![warn(clippy::cast_precision_loss)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "each unwrap and expect here acts on the temporary directory the test just made, or on the spawn of the freshly built binary. A failure of either is a broken harness, not the behavior under test"
)]

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// Terminal width that every run states, so the layout does not depend on the real terminal.
const TEST_COLUMNS: &str = "80";

/// The end of the question that `prcp` asks before it overwrites a file.
const OVERWRITE_QUESTION: &str = "Overwrite? (y/N): ";

/// The longest wait for the question. Only a hung run gets near it.
const QUESTION_TIMEOUT: Duration = Duration::from_secs(120);

/// The first line of the note that ends a report with a change from outside `prcp`.
const OUTSIDE_NOTE: &str =
    "Something outside prcp changed the source or the destination while prcp ran.";

/// The files of the source tree, in the order that the run copies them, with their content.
const SOURCE_FILES: [(&str, &str); 3] = [("a.txt", "a"), ("b.txt", "b"), ("c.txt", "c")];

/// The source file whose destination exists before the run, so `prcp` asks about it.
const ASKED_FILE: &str = "b.txt";

/// The output of one run.
struct Run {
    status: ExitStatus,
    stderr: String,
}

/// A child process that is killed when the test lets it go, also when the test panics.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The source, the destination, and the root of the copy in the destination.
struct Fixture {
    _temp: TempDir,
    src: PathBuf,
    dest: PathBuf,
    root: PathBuf,
}

impl Fixture {
    /// Make the source tree, and a destination root that already holds [`ASKED_FILE`].
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        let dest = temp.path().join("dest");
        for (name, content) in SOURCE_FILES {
            write_file(&src.join(name), content);
        }
        let root = dest.join("src");
        write_file(&root.join(ASKED_FILE), "old content of b");
        Self {
            _temp: temp,
            src,
            dest,
            root,
        }
    }

    /// Return the arguments of a recursive run from the source into the destination.
    fn args(&self, flags: &[&str]) -> Vec<OsString> {
        let mut args: Vec<OsString> = flags.iter().map(OsString::from).collect();
        args.push(OsString::from("-R"));
        args.push(OsString::from("-q"));
        args.push(self.src.clone().into_os_string());
        args.push(self.dest.clone().into_os_string());
        args
    }

    /// Assert that every source file is still there with its content.
    fn assert_sources_stay(&self, run: &Run) {
        for (name, content) in SOURCE_FILES {
            assert_eq!(
                fs::read_to_string(self.src.join(name)).ok().as_deref(),
                Some(content),
                "the source {name} must stay. stderr: {}",
                run.stderr
            );
        }
    }
}

/// Write a file and make its parent directories.
fn write_file(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// Run `prcp` until it asks to overwrite a file, run `meddle`, then answer yes.
///
/// The run gets no `-y`, and standard input is a pipe, so `prcp` asks no
/// question before the first copy. It asks only at the file that exists.
fn run_and_meddle_at_the_question(args: Vec<OsString>, meddle: impl FnOnce()) -> Run {
    let mut child = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_prcp"))
            .env("COLUMNS", TEST_COLUMNS)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .args(args)
            .spawn()
            .expect("the prcp binary must start"),
    );
    let stdout = child.0.stdout.take().unwrap();
    let stdout_reader = thread::spawn(move || drain(stdout));
    let mut stderr = child.0.stderr.take().unwrap();
    let (sender, chunks) = mpsc::channel::<Vec<u8>>();
    let stderr_reader = thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        while let Ok(count @ 1..) = stderr.read(&mut buffer) {
            if sender.send(buffer[..count].to_vec()).is_err() {
                break;
            }
        }
    });

    let mut seen = Vec::new();
    let deadline = Instant::now() + QUESTION_TIMEOUT;
    while !String::from_utf8_lossy(&seen).contains(OVERWRITE_QUESTION) {
        let left = deadline.saturating_duration_since(Instant::now());
        match chunks.recv_timeout(left) {
            Ok(chunk) => seen.extend(chunk),
            Err(_) => break,
        }
    }
    assert!(
        String::from_utf8_lossy(&seen).contains(OVERWRITE_QUESTION),
        "prcp did not ask to overwrite. stderr: {}",
        String::from_utf8_lossy(&seen)
    );

    meddle();
    let mut stdin = child.0.stdin.take().unwrap();
    stdin.write_all(b"y\n").unwrap();
    drop(stdin);

    let status = child.0.wait().unwrap();
    stderr_reader.join().unwrap();
    seen.extend(chunks.try_iter().flatten());
    let _ = stdout_reader.join();
    Run {
        status,
        stderr: testcolor::strip_ansi(&String::from_utf8_lossy(&seen)),
    }
}

/// Run `prcp` to its end with standard input on null, as a script runs it.
fn run_without_questions(args: Vec<OsString>) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_prcp"))
        .env("COLUMNS", TEST_COLUMNS)
        .stdin(Stdio::null())
        .args(args)
        .output()
        .expect("the prcp binary must start");
    Run {
        status: output.status,
        stderr: testcolor::strip_ansi(&String::from_utf8_lossy(&output.stderr)),
    }
}

/// Read a pipe to its end, so the child never blocks on a full pipe.
fn drain(mut pipe: impl Read) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = pipe.read_to_end(&mut bytes);
    bytes
}

#[test]
fn a_copy_that_nobody_changes_passes_the_final_check() {
    let fixture = Fixture::new();

    let run = run_and_meddle_at_the_question(fixture.args(&[]), || {});

    assert!(run.status.success(), "stderr: {}", run.stderr);
    for (name, content) in SOURCE_FILES {
        assert_eq!(
            fs::read_to_string(fixture.root.join(name)).unwrap(),
            content
        );
    }
}

#[test]
fn a_copy_reports_a_file_that_appears_at_the_destination_during_the_run() {
    let fixture = Fixture::new();
    let intruder = fixture.root.join("intruder.txt");

    let run = run_and_meddle_at_the_question(fixture.args(&[]), || {
        write_file(&intruder, "not from prcp");
    });

    assert!(!run.status.success(), "stderr: {}", run.stderr);
    let line = format!(
        "'{}' appeared at the destination during the run, and prcp did not make it",
        intruder.display()
    );
    assert!(run.stderr.contains(&line), "stderr: {}", run.stderr);
    assert!(
        run.stderr.contains(&format!(
            "prcp found problems with the copy of '{}':",
            fixture.src.display()
        )),
        "stderr: {}",
        run.stderr
    );
    assert!(run.stderr.contains(OUTSIDE_NOTE), "stderr: {}", run.stderr);
    assert!(
        run.stderr
            .contains("The copy did not pass its final check: 1 source(s) have problems."),
        "stderr: {}",
        run.stderr
    );
    assert_eq!(fs::read_to_string(&intruder).unwrap(), "not from prcp");
    fixture.assert_sources_stay(&run);
}

#[test]
fn a_copy_without_a_hash_check_reports_a_destination_file_that_changes() {
    let fixture = Fixture::new();
    let copy = fixture.root.join("a.txt");

    let run = run_and_meddle_at_the_question(fixture.args(&["--no-verify"]), || {
        fs::write(&copy, "a longer text than the copy").unwrap();
    });

    assert!(!run.status.success(), "stderr: {}", run.stderr);
    let line = format!(
        "'{}' changed at the destination during the run",
        copy.display()
    );
    assert!(run.stderr.contains(&line), "stderr: {}", run.stderr);
    assert!(run.stderr.contains(OUTSIDE_NOTE), "stderr: {}", run.stderr);
}

#[cfg(unix)]
#[test]
fn a_move_reports_a_destination_file_that_is_renamed_during_the_run() {
    let fixture = Fixture::new();
    let copy = fixture.root.join("a.txt");
    let renamed = fixture.root.join("a-renamed.txt");

    let run = run_and_meddle_at_the_question(fixture.args(&["--rm"]), || {
        fs::rename(&copy, &renamed).unwrap();
    });

    assert!(!run.status.success(), "stderr: {}", run.stderr);
    let line = format!(
        "'{}' was renamed or moved to '{}' during the run",
        copy.display(),
        renamed.display()
    );
    assert!(run.stderr.contains(&line), "stderr: {}", run.stderr);
    assert!(
        run.stderr.contains(&format!(
            "Kept the originals of '{}'. prcp removed nothing from it:",
            fixture.src.display()
        )),
        "stderr: {}",
        run.stderr
    );
    assert!(run.stderr.contains(OUTSIDE_NOTE), "stderr: {}", run.stderr);
    fixture.assert_sources_stay(&run);
}

#[test]
fn a_file_that_appears_at_a_planned_path_during_the_run_is_not_overwritten() {
    let fixture = Fixture::new();
    let source = fixture.src.join("c.txt");
    let intruder = fixture.root.join("c.txt");

    let run = run_and_meddle_at_the_question(fixture.args(&[]), || {
        write_file(&intruder, "not from prcp");
    });

    assert!(!run.status.success(), "stderr: {}", run.stderr);
    let line = format!(
        "prcp did not copy '{}': '{}' appeared at the destination during the run, and prcp did \
         not make it.",
        source.display(),
        intruder.display()
    );
    assert!(run.stderr.contains(&line), "stderr: {}", run.stderr);
    assert!(run.stderr.contains(OUTSIDE_NOTE), "stderr: {}", run.stderr);
    assert_eq!(
        run.stderr.matches(OVERWRITE_QUESTION).count(),
        1,
        "prcp must not ask about a path that somebody else made. stderr: {}",
        run.stderr
    );
    assert_eq!(fs::read_to_string(&intruder).unwrap(), "not from prcp");
    fixture.assert_sources_stay(&run);
}

#[test]
fn a_copy_that_skips_an_existing_file_passes_the_final_check() {
    let fixture = Fixture::new();

    let run = run_without_questions(fixture.args(&["--skip-existing"]));

    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert_eq!(
        fs::read_to_string(fixture.root.join(ASKED_FILE)).unwrap(),
        "old content of b"
    );
    assert_eq!(fs::read_to_string(fixture.root.join("a.txt")).unwrap(), "a");
    assert_eq!(fs::read_to_string(fixture.root.join("c.txt")).unwrap(), "c");
}

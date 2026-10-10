//! End-to-end tests of the landing preview. Before the first copy of a
//! recursive run, `prcp` shows where each source lands. When a person can
//! answer, it then asks before it starts.
//!
//! `prcp` asks only when standard input is a terminal. So the tests that answer
//! the question run the real binary with standard input on a pseudo-terminal,
//! and type the answer into it. The other tests run it with standard input on
//! null, the same as a script.

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
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

/// Terminal width that every run states, so the layout does not depend on the real terminal.
const TEST_COLUMNS: u16 = 80;

/// The question that `prcp` asks before the first copy.
const QUESTION: &str = "Continue? (y/N): ";

/// The relative paths of the files in the sample tree, with their content.
const SAMPLE_FILES: [(&str, &str); 3] = [
    ("one.txt", "depth one"),
    ("sub/two.txt", "depth two"),
    ("sub/deeper/three.txt", "depth three"),
];

/// Make a `prcp` command with the given arguments. The caller sets the standard streams.
fn prcp(args: &[OsString]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_prcp"));
    command.env("COLUMNS", TEST_COLUMNS.to_string()).args(args);
    command
}

/// Run `prcp` with standard input on null, the same as a script does.
fn run_without_a_terminal(args: &[OsString]) -> Output {
    prcp(args)
        .stdin(Stdio::null())
        .output()
        .expect("the prcp binary must start")
}

/// Run `prcp` with standard input on a terminal, and type `typed` into it.
#[cfg(unix)]
fn run_and_type(args: &[OsString], typed: &str) -> Output {
    gitscratch::testing::pty::Pty::open(TEST_COLUMNS)
        .run_with_stdin_on_terminal(prcp(args), typed.as_bytes())
}

/// Return standard error as text without ANSI codes. The pre-commit hook forces color on.
fn visible_stderr(output: &Output) -> String {
    testcolor::strip_ansi(&String::from_utf8_lossy(&output.stderr))
}

/// Return standard output as text without ANSI codes.
fn visible_stdout(output: &Output) -> String {
    testcolor::strip_ansi(&String::from_utf8_lossy(&output.stdout))
}

/// Make the sample tree under `root`.
fn make_sample_tree(root: &Path) {
    for (relative, content) in SAMPLE_FILES {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
}

/// Return true when `root` holds every file of the sample tree with its content.
fn holds_sample_tree(root: &Path) -> bool {
    SAMPLE_FILES.iter().all(|(relative, content)| {
        fs::read_to_string(root.join(relative)).ok().as_deref() == Some(*content)
    })
}

/// Return the temporary directory with every symlink resolved, as the preview shows it.
fn real(temp: &TempDir) -> PathBuf {
    fs::canonicalize(temp.path()).unwrap()
}

/// Turn the arguments of a run into the form that `Command` takes.
fn args(parts: &[&dyn AsRef<std::ffi::OsStr>]) -> Vec<OsString> {
    parts
        .iter()
        .map(|part| part.as_ref().to_os_string())
        .collect()
}

#[test]
fn a_recursive_copy_shows_where_the_tree_lands_and_copies_without_a_terminal() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    make_sample_tree(&src);
    fs::create_dir(&dest).unwrap();

    let output = run_without_a_terminal(&args(&[&"-R", &"-q", &src, &dest]));

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    let line = format!(
        "  {} -> {}/  (new directory, 3 files)\n",
        src.display(),
        real(&temp).join("dest").join("src").display()
    );
    assert!(
        stderr.starts_with("prcp will copy:\n"),
        "the preview must come before anything else. stderr: {stderr}"
    );
    assert!(stderr.contains(&line), "stderr: {stderr}");
    assert!(
        !stderr.contains(QUESTION),
        "without a terminal, prcp must not ask. stderr: {stderr}"
    );
    assert!(holds_sample_tree(&dest.join("src")));
}

#[test]
fn a_slash_at_the_end_of_the_source_gets_the_note_and_the_tree_lands_under_its_name() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    make_sample_tree(&src);
    fs::create_dir(&dest).unwrap();
    let slashed = src.join("");

    let output = run_without_a_terminal(&args(&[&"-R", &"-y", &"-q", &slashed, &dest]));

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains("Note: a '/' or a '/.' at the end of a source changes nothing."),
        "stderr: {stderr}"
    );
    let landing = format!("{}/  (", real(&temp).join("dest").join("src").display());
    assert!(stderr.contains(&landing), "stderr: {stderr}");
    assert!(
        holds_sample_tree(&dest.join("src")),
        "the tree must land where the preview said"
    );
}

#[test]
fn a_recursive_move_says_that_it_moves() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    make_sample_tree(&src);

    let output = run_without_a_terminal(&args(&[&"--rm", &"-R", &"-y", &"-q", &src, &dest]));

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(stderr.starts_with("prcp will move:\n"), "stderr: {stderr}");
    assert!(holds_sample_tree(&dest));
    assert!(!src.exists());
}

#[test]
fn a_copy_of_files_alone_shows_no_preview() {
    let temp = TempDir::new().unwrap();
    let first = temp.path().join("a.txt");
    let second = temp.path().join("b.txt");
    let dest = temp.path().join("dest");
    fs::write(&first, "a").unwrap();
    fs::write(&second, "b").unwrap();

    let output = run_without_a_terminal(&args(&[&"-R", &"-y", &"-q", &first, &second, &dest]));

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(!stderr.contains("prcp will"), "stderr: {stderr}");
    assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "a");
}

#[cfg(unix)]
#[test]
fn no_at_the_question_moves_nothing() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    make_sample_tree(&src);

    let output = run_and_type(&args(&[&"--rm", &"-R", &"-q", &src, &dest]), "n\n");

    let stderr = visible_stderr(&output);
    let stdout = visible_stdout(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(stderr.starts_with("prcp will move:\n"), "stderr: {stderr}");
    assert!(stderr.ends_with(QUESTION), "stderr: {stderr}");
    assert!(
        stdout.contains("Operation cancelled"),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(!dest.exists(), "a declined run must make nothing");
    assert!(
        holds_sample_tree(&src),
        "a declined move must keep every original"
    );
}

#[cfg(unix)]
#[test]
fn yes_at_the_question_copies_the_tree() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    make_sample_tree(&src);

    let output = run_and_type(&args(&[&"-R", &"-q", &src, &dest]), "y\n");

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains(QUESTION), "stderr: {stderr}");
    assert!(holds_sample_tree(&dest));
    assert!(holds_sample_tree(&src));
}

#[cfg(unix)]
#[test]
fn the_yes_flag_skips_the_question_on_a_terminal() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    make_sample_tree(&src);

    let output = run_and_type(&args(&[&"-R", &"-y", &"-q", &src, &dest]), "n\n");

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(stderr.starts_with("prcp will copy:\n"), "stderr: {stderr}");
    assert!(!stderr.contains(QUESTION), "stderr: {stderr}");
    assert!(holds_sample_tree(&dest));
}

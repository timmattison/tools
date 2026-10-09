//! End-to-end tests of recursive copies and moves in `prcp`. Each test runs the
//! real binary. Every run gives `-y` and copies at least two files, because a
//! run with one file puts the terminal of the developer in raw mode.

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

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

/// Terminal width that every run states, so the layout does not depend on the real terminal.
const TEST_COLUMNS: &str = "80";

/// Run the real `prcp` binary with the given arguments and collect its output.
fn run_prcp<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_prcp"))
        .env("COLUMNS", TEST_COLUMNS)
        .stdin(Stdio::null())
        .args(args)
        .output()
        .expect("the prcp binary must start")
}

/// Return standard error as text without ANSI codes. The pre-commit hook forces color on.
fn visible_stderr(output: &Output) -> String {
    testcolor::strip_ansi(&String::from_utf8_lossy(&output.stderr))
}

/// Return standard output as text without ANSI codes.
#[allow(dead_code, reason = "later tests in this file read standard output")]
fn visible_stdout(output: &Output) -> String {
    testcolor::strip_ansi(&String::from_utf8_lossy(&output.stdout))
}

/// Write a file and make its parent directories.
fn write_file(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn non_recursive_glob_skips_directory_matches_and_warns() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    write_file(&src.join("a.txt"), "a");
    write_file(&src.join("b.txt"), "b");
    write_file(&src.join("sub").join("c.txt"), "c");
    fs::create_dir(&dest).unwrap();

    let mut glob_arg = OsString::from(src.as_os_str());
    glob_arg.push("/*");
    let output = run_prcp([
        OsString::from("-y"),
        OsString::from("-q"),
        glob_arg,
        dest.clone().into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(dest.join("a.txt").exists());
    assert!(dest.join("b.txt").exists());
    assert!(!dest.join("sub").exists());
    assert!(stderr.contains("Skipping directory"), "stderr: {stderr}");
    assert!(
        stderr.contains(&src.join("sub").display().to_string()),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("--recursive"), "stderr: {stderr}");
}

#[test]
fn two_files_into_a_missing_destination_make_it_a_directory() {
    let temp = TempDir::new().unwrap();
    let a = temp.path().join("a.txt");
    let b = temp.path().join("b.txt");
    let newdir = temp.path().join("newdir");
    write_file(&a, "content of a");
    write_file(&b, "content of b");

    let output = run_prcp([
        OsString::from("-y"),
        OsString::from("-q"),
        a.into_os_string(),
        b.into_os_string(),
        newdir.clone().into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(
        fs::read_to_string(newdir.join("a.txt")).unwrap(),
        "content of a"
    );
    assert_eq!(
        fs::read_to_string(newdir.join("b.txt")).unwrap(),
        "content of b"
    );
}

/// The relative paths of the files in the sample tree, with their content.
const SAMPLE_FILES: [(&str, &str); 3] = [
    ("one.txt", "depth one"),
    ("sub/two.txt", "depth two"),
    ("sub/deeper/three.txt", "depth three"),
];

/// The relative path of the empty directory in the sample tree.
const SAMPLE_EMPTY_DIR: &str = "empty";

/// Make the sample tree under `root`: files at depth 1, 2, and 3, and one empty directory.
fn make_sample_tree(root: &Path) {
    for (relative, content) in SAMPLE_FILES {
        write_file(&root.join(relative), content);
    }
    fs::create_dir_all(root.join(SAMPLE_EMPTY_DIR)).unwrap();
}

/// Return the Blake3 hash of a file.
fn file_hash(path: &Path) -> blake3::Hash {
    blake3::hash(&fs::read(path).unwrap())
}

/// Assert that `copy` holds the sample tree and that `original` is unchanged.
fn assert_sample_copied(original: &Path, copy: &Path) {
    for (relative, content) in SAMPLE_FILES {
        assert_eq!(
            file_hash(&copy.join(relative)),
            file_hash(&original.join(relative)),
            "hash of {relative}"
        );
        assert_eq!(
            fs::read_to_string(original.join(relative)).unwrap(),
            content,
            "source {relative} changed"
        );
    }
    assert!(
        copy.join(SAMPLE_EMPTY_DIR).is_dir(),
        "the empty directory must exist at the destination"
    );
    assert!(original.join(SAMPLE_EMPTY_DIR).is_dir());
}

#[test]
fn recursive_copy_to_a_missing_destination_makes_the_tree() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    make_sample_tree(&src);

    let output = run_prcp([
        OsString::from("-R"),
        OsString::from("-y"),
        OsString::from("-q"),
        src.clone().into_os_string(),
        dest.clone().into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert_sample_copied(&src, &dest);
}

#[test]
fn recursive_copy_into_an_existing_directory_lands_under_the_source_name() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    make_sample_tree(&src);
    fs::create_dir(&dest).unwrap();

    let output = run_prcp([
        OsString::from("-R"),
        OsString::from("-y"),
        OsString::from("-q"),
        src.clone().into_os_string(),
        dest.clone().into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert_sample_copied(&src, &dest.join("src"));
}

#[cfg(unix)]
#[test]
fn recursive_copy_recreates_symlinks_without_following_them() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    write_file(&src.join("a.txt"), "a");
    write_file(&src.join("sub").join("b.txt"), "b");
    symlink("a.txt", src.join("link")).unwrap();
    symlink("..", src.join("sub").join("loop")).unwrap();

    let output = run_prcp([
        OsString::from("-R"),
        OsString::from("-y"),
        OsString::from("-q"),
        src.clone().into_os_string(),
        dest.clone().into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    for (relative, target) in [("link", "a.txt"), ("sub/loop", "..")] {
        let copy = dest.join(relative);
        assert!(
            fs::symlink_metadata(&copy)
                .unwrap_or_else(|e| panic!("{relative} is missing: {e}"))
                .file_type()
                .is_symlink(),
            "{relative} must be a symlink"
        );
        assert_eq!(fs::read_link(&copy).unwrap(), Path::new(target));
    }
}

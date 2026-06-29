//! Black-box tests for the `bm` binary, driving the real CLI end to end.
//!
//! Each test builds its own temporary tree, so concurrent test runs stay
//! isolated (see the parallel-safety note in the project guidelines).

use std::path::Path;
use std::process::{Command, Output};

/// Invoke the freshly-built `bm` binary.
fn bm() -> Command {
    Command::new(env!("CARGO_BIN_EXE_bm"))
}

fn write_file(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn run(output: Output) -> (bool, String, String) {
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn moves_matching_files_into_destination() {
    let root = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    write_file(&root.path().join("keep.mkv"), b"1");
    write_file(&root.path().join("sub/also.mkv"), b"2");
    write_file(&root.path().join("notes.txt"), b"3");

    let (ok, _stdout, stderr) = run(bm()
        .args(["--suffix", ".mkv", "--destination"])
        .arg(dest.path())
        .arg(root.path())
        .output()
        .unwrap());

    assert!(ok, "bm should succeed; stderr: {stderr}");
    assert!(dest.path().join("keep.mkv").exists());
    assert!(dest.path().join("also.mkv").exists());
    assert!(root.path().join("notes.txt").exists());
}

#[test]
fn errors_when_no_pattern_is_given() {
    let dest = tempfile::tempdir().unwrap();
    let (ok, _out, _err) = run(bm().arg("--destination").arg(dest.path()).output().unwrap());
    assert!(!ok, "missing pattern must be an error");
}

#[test]
fn errors_when_multiple_patterns_are_given() {
    let root = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let (ok, _out, _err) = run(bm()
        .args(["--suffix", ".mkv", "--prefix", "IMG_", "--destination"])
        .arg(dest.path())
        .arg(root.path())
        .output()
        .unwrap());
    assert!(!ok, "more than one pattern must be an error");
}

#[test]
fn errors_when_destination_does_not_exist() {
    let root = tempfile::tempdir().unwrap();
    write_file(&root.path().join("a.mkv"), b"1");
    let missing = root.path().join("no-such-dir");

    let (ok, _out, _err) = run(bm()
        .args(["--suffix", ".mkv", "--destination"])
        .arg(&missing)
        .arg(root.path())
        .output()
        .unwrap());
    assert!(!ok, "a missing destination must be an error");
    assert!(root.path().join("a.mkv").exists(), "nothing should move");
}

#[test]
fn aborts_on_collision_and_moves_nothing() {
    let root = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    write_file(&root.path().join("dup.mkv"), b"new");
    write_file(&dest.path().join("dup.mkv"), b"old");

    let (ok, _out, _err) = run(bm()
        .args(["--suffix", ".mkv", "--destination"])
        .arg(dest.path())
        .arg(root.path())
        .output()
        .unwrap());

    assert!(!ok, "default abort policy must fail on a collision");
    assert!(root.path().join("dup.mkv").exists(), "source must remain");
    assert_eq!(
        std::fs::read(dest.path().join("dup.mkv")).unwrap(),
        b"old",
        "the existing destination file must be untouched"
    );
}

#[test]
fn dry_run_reports_but_moves_nothing() {
    let root = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    write_file(&root.path().join("a.mkv"), b"1");

    let (ok, _stdout, _stderr) = run(bm()
        .args(["--suffix", ".mkv", "--dry-run", "--destination"])
        .arg(dest.path())
        .arg(root.path())
        .output()
        .unwrap());

    assert!(ok, "dry run should succeed");
    assert!(
        root.path().join("a.mkv").exists(),
        "dry run must not move anything"
    );
    assert!(!dest.path().join("a.mkv").exists());
}

#[test]
fn skip_policy_moves_noncolliding_and_leaves_collision() {
    let root = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    write_file(&root.path().join("fresh.mkv"), b"1");
    write_file(&root.path().join("dup.mkv"), b"new");
    write_file(&dest.path().join("dup.mkv"), b"old");

    let (ok, _out, stderr) = run(bm()
        .args([
            "--suffix",
            ".mkv",
            "--on-collision",
            "skip",
            "--destination",
        ])
        .arg(dest.path())
        .arg(root.path())
        .output()
        .unwrap());

    assert!(ok, "skip policy should succeed; stderr: {stderr}");
    assert!(
        dest.path().join("fresh.mkv").exists(),
        "non-colliding moves"
    );
    assert!(root.path().join("dup.mkv").exists(), "colliding file stays");
    assert_eq!(std::fs::read(dest.path().join("dup.mkv")).unwrap(), b"old");
}

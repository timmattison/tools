//! End-to-end tests of the same-file refusal. A run whose destination is its
//! own source must stop before the first copy and leave the source as it was.
//! Without the refusal, `File::create` truncates the shared file to 0 bytes.

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

use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

/// Terminal width that every run states, so the layout does not depend on the real terminal.
const COLUMNS: &str = "80";

/// Run `prcp` with `args` in `dir`. Standard input is null.
fn run_prcp(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_prcp"))
        .args(args)
        .current_dir(dir)
        .env("COLUMNS", COLUMNS)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn a_file_copied_into_its_own_directory_is_refused_and_keeps_its_content() {
    let temp = TempDir::new().unwrap();
    let content = "thirteen byte";
    fs::write(temp.path().join("a.txt"), content).unwrap();

    let output = run_prcp(temp.path(), &["-y", "a.txt", "."]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains("are the same file"), "stderr: {stderr}");
    assert_eq!(
        fs::read_to_string(temp.path().join("a.txt")).unwrap(),
        content
    );
}

#[cfg(unix)]
#[test]
fn a_merge_onto_a_symlink_to_its_own_source_is_refused_and_keeps_its_content() {
    let temp = TempDir::new().unwrap();
    let content = "tree content";
    let source_file = temp.path().join("src").join("f.txt");
    fs::create_dir_all(source_file.parent().unwrap()).unwrap();
    fs::write(&source_file, content).unwrap();
    let dest_dir = temp.path().join("dest").join("src");
    fs::create_dir_all(&dest_dir).unwrap();
    std::os::unix::fs::symlink(&source_file, dest_dir.join("f.txt")).unwrap();

    let output = run_prcp(temp.path(), &["-R", "-y", "src", "dest"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains("are the same file"), "stderr: {stderr}");
    assert_eq!(fs::read_to_string(&source_file).unwrap(), content);
}

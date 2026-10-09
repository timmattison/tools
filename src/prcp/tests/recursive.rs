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

#[cfg(unix)]
#[test]
fn symlink_replaces_an_existing_file_at_the_destination() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    write_file(&src.join("a.txt"), "a");
    symlink("a.txt", src.join("link")).unwrap();
    write_file(&dest.join("src").join("link"), "old file");
    write_file(&dest.join("src").join("a.txt"), "old a");

    let output = run_prcp([
        OsString::from("-R"),
        OsString::from("-y"),
        OsString::from("-q"),
        src.clone().into_os_string(),
        dest.clone().into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    let copy = dest.join("src").join("link");
    assert!(fs::symlink_metadata(&copy)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&copy).unwrap(), Path::new("a.txt"));
}

/// Set the mode of every path to `0o755` when dropped, so `TempDir` can delete the tree.
#[cfg(unix)]
struct ModeRestore(Vec<std::path::PathBuf>);

#[cfg(unix)]
impl Drop for ModeRestore {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        for path in &self.0 {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
        }
    }
}

/// Return the permission bits of a path.
#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[cfg(unix)]
#[test]
fn recursive_copy_keeps_the_modes_of_files_and_directories() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    write_file(&src.join("private.txt"), "p");
    write_file(&src.join("other.txt"), "o");
    write_file(&src.join("group").join("g.txt"), "g");
    write_file(&src.join("locked").join("l.txt"), "l");
    let _restore = ModeRestore(vec![src.join("locked"), dest.join("locked")]);
    fs::set_permissions(src.join("private.txt"), fs::Permissions::from_mode(0o640)).unwrap();
    fs::set_permissions(src.join("group"), fs::Permissions::from_mode(0o750)).unwrap();
    fs::set_permissions(src.join("locked"), fs::Permissions::from_mode(0o555)).unwrap();

    let output = run_prcp([
        OsString::from("-R"),
        OsString::from("-y"),
        OsString::from("-q"),
        src.clone().into_os_string(),
        dest.clone().into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(mode_of(&dest.join("private.txt")), 0o640);
    assert_eq!(mode_of(&dest.join("group")), 0o750);
    assert_eq!(mode_of(&dest.join("locked")), 0o555);
    assert_eq!(
        fs::read_to_string(dest.join("locked").join("l.txt")).unwrap(),
        "l"
    );
}

#[test]
fn recursive_copy_handles_multibyte_names() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    write_file(&src.join("日本語").join("café.txt"), "café");
    write_file(&src.join("🎉.txt"), "party");

    let output = run_prcp([
        OsString::from("-R"),
        OsString::from("-y"),
        OsString::from("-q"),
        src.clone().into_os_string(),
        dest.clone().into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    for relative in ["日本語/café.txt", "🎉.txt"] {
        assert_eq!(
            file_hash(&dest.join(relative)),
            file_hash(&src.join(relative)),
            "hash of {relative}"
        );
    }
}

#[test]
fn directory_source_without_recursive_fails_and_copies_nothing() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    make_sample_tree(&src);

    let output = run_prcp([
        OsString::from("-y"),
        OsString::from("-q"),
        src.into_os_string(),
        dest.clone().into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains("--recursive"), "stderr: {stderr}");
    assert!(!dest.exists());
}

#[test]
fn directory_onto_an_existing_file_fails() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest.txt");
    make_sample_tree(&src);
    write_file(&dest, "keep me");

    let output = run_prcp([
        OsString::from("-R"),
        OsString::from("-y"),
        OsString::from("-q"),
        src.into_os_string(),
        dest.clone().into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains("non-directory"), "stderr: {stderr}");
    assert_eq!(fs::read_to_string(&dest).unwrap(), "keep me");
}

#[test]
fn directory_into_its_own_subdirectory_fails() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    make_sample_tree(&src);
    let dest = src.join("sub");

    let output = run_prcp([
        OsString::from("-R"),
        OsString::from("-y"),
        OsString::from("-q"),
        src.clone().into_os_string(),
        dest.into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains("into itself"), "stderr: {stderr}");
    assert!(!src.join("sub").join("src").exists());
}

#[test]
fn recursive_copy_merges_into_an_existing_tree_and_overwrites_files() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    write_file(&src.join("a.txt"), "new a");
    write_file(&src.join("b.txt"), "new b");
    write_file(&dest.join("src").join("a.txt"), "old a");
    write_file(&dest.join("src").join("extra.txt"), "extra");

    let output = run_prcp([
        OsString::from("-R"),
        OsString::from("-y"),
        OsString::from("-q"),
        src.into_os_string(),
        dest.clone().into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    let merged = dest.join("src");
    assert_eq!(fs::read_to_string(merged.join("a.txt")).unwrap(), "new a");
    assert_eq!(fs::read_to_string(merged.join("b.txt")).unwrap(), "new b");
    assert_eq!(
        fs::read_to_string(merged.join("extra.txt")).unwrap(),
        "extra"
    );
}

#[cfg(unix)]
#[test]
fn recursive_copy_skips_a_fifo_and_warns() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    write_file(&src.join("a.txt"), "a");
    write_file(&src.join("b.txt"), "b");
    let fifo = src.join("pipe");
    let status = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(status.success(), "mkfifo must work");

    let output = run_prcp([
        OsString::from("-R"),
        OsString::from("-y"),
        OsString::from("-q"),
        src.into_os_string(),
        dest.clone().into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains(&fifo.display().to_string()),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("fifo"), "stderr: {stderr}");
    assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "a");
    assert_eq!(fs::read_to_string(dest.join("b.txt")).unwrap(), "b");
    assert!(
        fs::symlink_metadata(dest.join("pipe")).is_err(),
        "the fifo must not exist at the destination"
    );
}

/// Make a source tree with two readable files and one directory that nobody can read.
/// Return `None` when the lock does not work, which is the case for root.
#[cfg(unix)]
fn make_tree_with_locked_directory(src: &Path) -> Option<ModeRestore> {
    use std::os::unix::fs::PermissionsExt;

    write_file(&src.join("a.txt"), "a");
    write_file(&src.join("b.txt"), "b");
    write_file(&src.join("locked").join("c.txt"), "c");
    let locked = src.join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let restore = ModeRestore(vec![locked.clone()]);
    if fs::read_dir(&locked).is_ok() {
        return None;
    }
    Some(restore)
}

#[cfg(unix)]
#[test]
fn unreadable_subdirectory_stops_the_copy_before_it_starts() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    let dest = temp.path().join("dest");
    let Some(_restore) = make_tree_with_locked_directory(&src) else {
        return;
    };

    let output = run_prcp([
        OsString::from("-R"),
        OsString::from("-y"),
        OsString::from("-q"),
        src.clone().into_os_string(),
        dest.clone().into_os_string(),
    ]);

    let stderr = visible_stderr(&output);
    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains(&src.join("locked").display().to_string()),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("--continue-on-error"), "stderr: {stderr}");
    assert!(!dest.exists(), "nothing may be copied");
}

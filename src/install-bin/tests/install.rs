//! Behavioral tests for `install_binary`, ported from the original TypeScript
//! `install-bin` test suite.
//!
//! The core regression under test: installing over an existing destination MUST
//! give the destination a new inode. A naive in-place copy (cp semantics) keeps
//! the inode, and on Apple Silicon macOS the kernel's per-vnode code-signature
//! cache then kills every exec of the new bytes with SIGKILL.
//!
//! Parallel-safety: every test gets its own `tempfile::tempdir()` sandbox
//! (unique per call), so concurrent runs never share a path.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use install_bin::install_binary;

/// Write `content` to `path` and mark it executable (mode `0o755`).
fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).expect("write file");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
}

#[test]
fn installing_over_an_existing_destination_allocates_a_new_inode() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("source-bin");
    let dest = dir.path().join("dest-bin");
    write_executable(&source, "#!/bin/sh\necho new-build\n");
    write_executable(&dest, "#!/bin/sh\necho old-build\n");
    let old_inode = fs::metadata(&dest).expect("stat dest").ino();

    let result = install_binary(&source, &dest).expect("install");

    assert_ne!(
        fs::metadata(&dest).expect("stat dest after").ino(),
        old_inode,
        "destination kept its inode — this is the exact macOS signature-cache SIGKILL bug",
    );
    assert!(result.replaced_existing);
}

#[test]
fn installed_file_matches_source_content_and_is_executable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("source-bin");
    // Destination lives under directories that do not exist yet.
    let dest = dir.path().join("sub").join("dir").join("dest-bin");
    let payload = "#!/bin/sh\necho payload\n";
    write_executable(&source, payload);

    let result = install_binary(&source, &dest).expect("install");

    assert_eq!(fs::read_to_string(&dest).expect("read dest"), payload);
    let mode = fs::metadata(&dest).expect("stat dest").permissions().mode();
    assert_ne!(
        mode & 0o111,
        0,
        "installed file must have an executable bit"
    );
    assert!(!result.replaced_existing, "nothing existed at dest yet");
}

#[test]
fn refuses_to_install_a_file_onto_itself() {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path().join("the-bin");
    let content = "#!/bin/sh\necho hi\n";
    write_executable(&p, content);

    match install_binary(&p, &p) {
        Ok(_) => panic!("must refuse installing a file onto itself"),
        Err(err) => assert!(
            err.to_string().to_lowercase().contains("same file"),
            "expected a 'same file' error, got: {err}",
        ),
    }
    assert_eq!(
        fs::read_to_string(&p).expect("read source"),
        content,
        "refusal must not destroy the file",
    );
}

#[test]
fn refuses_a_source_that_is_not_a_regular_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A directory is not a regular file.
    let source = dir.path().join("a-directory");
    fs::create_dir(&source).expect("mkdir source");
    let dest = dir.path().join("dest");

    match install_binary(&source, &dest) {
        Ok(_) => panic!("must refuse a non-regular-file source"),
        Err(err) => assert!(
            err.to_string()
                .to_lowercase()
                .contains("not a regular file"),
            "expected a 'not a regular file' error, got: {err}",
        ),
    }
}

#[test]
fn a_failed_copy_leaves_the_existing_destination_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("source-bin");
    let dest = dir.path().join("dest-bin");
    write_executable(&source, "#!/bin/sh\necho new-build\n");
    write_executable(&dest, "#!/bin/sh\necho OLD-BUILD\n");
    // Make the source unreadable so the copy step fails.
    fs::set_permissions(&source, fs::Permissions::from_mode(0o000)).expect("chmod source");

    let result = install_binary(&source, &dest);

    assert!(
        result.is_err(),
        "an unreadable source must fail the install"
    );
    // The pre-existing binary must survive a failed install — atomic replace,
    // not unlink-then-copy. This is the whole point of the fix.
    assert_eq!(
        fs::read_to_string(&dest).expect("dest must still exist after a failed install"),
        "#!/bin/sh\necho OLD-BUILD\n",
        "a failed copy must not destroy the previously-installed binary",
    );
}

#[test]
fn refuses_a_missing_source_with_a_clear_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("nope");
    let dest = dir.path().join("dest");

    match install_binary(&source, &dest) {
        Ok(_) => panic!("must refuse a missing source"),
        Err(err) => assert!(
            err.to_string().to_lowercase().contains("does not exist"),
            "expected a 'does not exist' error, got: {err}",
        ),
    }
}

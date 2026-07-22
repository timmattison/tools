//! Behavioral tests for `install_binary`, ported from `install-bin.test.ts`.
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

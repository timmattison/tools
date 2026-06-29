//! End-to-end tests for `bm`'s public API against a real (temporary) filesystem.
//!
//! Every test uses its own `tempfile::tempdir()` so concurrent runs (including a
//! background `bacon` loop sharing `./target`) never clobber each other.

use bm::{collect_sources, execute_plan, plan_moves, CollisionPolicy, FilterType};
use std::fs;
use std::path::Path;

/// Create `path` (and any parent directories) with the given contents.
fn write_file(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

/// The set of file basenames in `paths`, for order-independent assertions.
fn basenames(paths: &[std::path::PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn collect_sources_finds_matching_files_recursively() {
    let root = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    write_file(&root.path().join("a.mkv"), b"a");
    write_file(&root.path().join("b.txt"), b"b");
    write_file(&root.path().join("sub/c.mkv"), b"c");

    let sources = collect_sources(
        &[root.path().to_path_buf()],
        FilterType::Suffix(".mkv".into()),
        dest.path(),
    )
    .unwrap();

    let names = basenames(&sources);
    assert_eq!(sources.len(), 2, "found: {sources:?}");
    assert!(names.contains(&"a.mkv".to_string()));
    assert!(names.contains(&"c.mkv".to_string()));
    assert!(!names.contains(&"b.txt".to_string()));
}

#[test]
fn collect_sources_excludes_files_already_in_destination() {
    let root = tempfile::tempdir().unwrap();
    write_file(&root.path().join("a.mkv"), b"a");
    let dest = root.path().join("dest");
    write_file(&dest.join("already.mkv"), b"x");

    let sources = collect_sources(
        &[root.path().to_path_buf()],
        FilterType::Suffix(".mkv".into()),
        &dest,
    )
    .unwrap();

    let names = basenames(&sources);
    assert!(names.contains(&"a.mkv".to_string()));
    assert!(
        !names.contains(&"already.mkv".to_string()),
        "a file already in the destination must not be collected: {sources:?}"
    );
}

#[test]
fn execute_plan_moves_planned_files_and_counts_renames() {
    let root = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    write_file(&root.path().join("a.mkv"), b"a");
    write_file(&root.path().join("b.mkv"), b"b");

    let sources = collect_sources(
        &[root.path().to_path_buf()],
        FilterType::Suffix(".mkv".into()),
        dest.path(),
    )
    .unwrap();
    let plan = plan_moves(&sources, dest.path(), CollisionPolicy::Abort, |p| {
        p.exists()
    })
    .unwrap();

    let summary = execute_plan(&plan, |_, _| panic!("same-volume move must not copy")).unwrap();

    assert_eq!(summary.renamed, 2);
    assert_eq!(summary.copied, 0);
    assert_eq!(summary.moved(), 2);
    assert!(dest.path().join("a.mkv").exists());
    assert!(dest.path().join("b.mkv").exists());
    assert!(!root.path().join("a.mkv").exists());
    assert!(!root.path().join("b.mkv").exists());
}

#[test]
fn execute_plan_reports_skipped_count_and_leaves_files_untouched() {
    let root = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    write_file(&root.path().join("dup.mkv"), b"new");
    write_file(&dest.path().join("dup.mkv"), b"old");

    let sources = collect_sources(
        &[root.path().to_path_buf()],
        FilterType::Suffix(".mkv".into()),
        dest.path(),
    )
    .unwrap();
    let plan = plan_moves(&sources, dest.path(), CollisionPolicy::Skip, |p| p.exists()).unwrap();

    let summary = execute_plan(&plan, |_, _| panic!("nothing should be copied")).unwrap();

    assert_eq!(summary.skipped, 1);
    assert_eq!(summary.moved(), 0);
    assert_eq!(fs::read(dest.path().join("dup.mkv")).unwrap(), b"old");
    assert!(root.path().join("dup.mkv").exists());
}

#[test]
fn end_to_end_moves_only_matching_files_into_destination() {
    let root = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    write_file(&root.path().join("keep.mkv"), b"1");
    write_file(&root.path().join("photos/IMG_2.mkv"), b"2");
    write_file(&root.path().join("notes.txt"), b"3");

    let sources = collect_sources(
        &[root.path().to_path_buf()],
        FilterType::Suffix(".mkv".into()),
        dest.path(),
    )
    .unwrap();
    let plan = plan_moves(&sources, dest.path(), CollisionPolicy::Abort, |p| {
        p.exists()
    })
    .unwrap();
    let summary = execute_plan(&plan, |_, _| panic!("same-volume move must not copy")).unwrap();

    assert_eq!(summary.moved(), 2);
    assert!(dest.path().join("keep.mkv").exists());
    assert!(dest.path().join("IMG_2.mkv").exists());
    assert!(
        root.path().join("notes.txt").exists(),
        "an unmatched file must stay where it is"
    );
}

#[test]
fn copy_file_copies_contents_and_reports_progress() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("a.bin");
    let data = vec![7_u8; 10_000];
    fs::write(&src, &data).unwrap();
    let dst = dir.path().join("b.bin");

    let mut last_reported = 0_u64;
    let n = bm::copy_file(&src, &dst, |bytes| last_reported = bytes).unwrap();

    assert_eq!(n, data.len() as u64);
    assert_eq!(last_reported, data.len() as u64);
    assert_eq!(fs::read(&dst).unwrap(), data);
}

#[test]
fn copy_file_preserves_modification_time() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("a.bin");
    fs::write(&src, b"data").unwrap();
    // A distinctive past mtime so "now" can't accidentally match without the fix.
    let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    std::fs::File::options()
        .write(true)
        .open(&src)
        .unwrap()
        .set_modified(past)
        .unwrap();
    let dst = dir.path().join("b.bin");

    bm::copy_file(&src, &dst, |_| {}).unwrap();

    let dst_mtime = fs::metadata(&dst).unwrap().modified().unwrap();
    assert_eq!(
        dst_mtime, past,
        "destination must keep the source's modification time"
    );
}

#[test]
fn execute_plan_reports_partial_summary_when_a_move_fails() {
    use bm::{MovePlan, PlannedMove};

    let dir = tempfile::tempdir().unwrap();
    let src_a = dir.path().join("a.txt");
    let src_b = dir.path().join("b.txt");
    fs::write(&src_a, b"a").unwrap();
    fs::write(&src_b, b"b").unwrap();

    let good_dest = dir.path().join("a-moved.txt");
    // Parent dir doesn't exist, so renaming here fails (ENOENT, not cross-device).
    let bad_dest = dir.path().join("missing").join("b.txt");

    let plan = MovePlan {
        moves: vec![
            PlannedMove {
                source: src_a.clone(),
                destination: good_dest.clone(),
            },
            PlannedMove {
                source: src_b.clone(),
                destination: bad_dest,
            },
        ],
        skipped: Vec::new(),
    };

    let err = execute_plan(&plan, |_, _| panic!("no cross-volume copy expected")).unwrap_err();

    assert_eq!(
        err.summary.moved(),
        1,
        "the first successful move must be counted in the partial summary"
    );
    assert!(good_dest.exists(), "the first file was moved before the failure");
    assert!(src_b.exists(), "the failed move leaves its source in place");
}

#[cfg(unix)]
#[test]
fn copy_file_preserves_unix_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("a.sh");
    fs::write(&src, b"#!/bin/sh\n").unwrap();
    fs::set_permissions(&src, fs::Permissions::from_mode(0o755)).unwrap();
    let dst = dir.path().join("b.sh");

    bm::copy_file(&src, &dst, |_| {}).unwrap();

    let mode = fs::metadata(&dst).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
}

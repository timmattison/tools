//! End-to-end tests for `bm`'s public API against a real (temporary) filesystem.
//!
//! Every test uses its own `tempfile::tempdir()` so concurrent runs (including a
//! background `bacon` loop sharing `./target`) never clobber each other.

use bm::{collect_sources, FilterType};
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

//! Behavioral tests for the recursive, concurrent scan.
//!
//! Every test builds its own [`TempDir`] fixture, so the suite is safe to run
//! concurrently with another copy of itself.

use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tempfile::TempDir;
use zth::{find_all_zero_files, Jobs, NoProgress, ScanProgress};

/// Records every progress callback so tests can assert on the counts a caller's
/// progress bar would have been driven with.
#[derive(Default)]
struct Recorder {
    discovered: Mutex<Vec<u64>>,
    scanned: Mutex<Vec<u64>>,
}

impl Recorder {
    /// Returns the recorded totals, sorted. Worker threads report concurrently,
    /// so only the multiset of values is deterministic, not their order.
    fn sorted(values: &Mutex<Vec<u64>>) -> Vec<u64> {
        let mut values = values
            .lock()
            .expect("recorder mutex should not be poisoned")
            .clone();
        values.sort_unstable();
        values
    }
}

impl ScanProgress for Recorder {
    fn files_discovered(&self, total: u64) {
        self.discovered
            .lock()
            .expect("recorder mutex should not be poisoned")
            .push(total);
    }

    fn files_scanned(&self, total: u64) {
        self.scanned
            .lock()
            .expect("recorder mutex should not be poisoned")
            .push(total);
    }
}

/// Writes `contents` to `dir/name`, creating any parent directories.
fn write(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("creating fixture directories should succeed");
    }
    fs::write(&path, contents).expect("writing the fixture should succeed");
    path
}

/// A temp dir plus its canonical path.
///
/// Tests compare against canonical paths because `zth` resolves its root once
/// up front - on macOS a temp dir lives under `/var/...`, which is a symlink to
/// `/private/var/...`.
fn fixture() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("creating a temp dir should succeed");
    let canonical =
        fs::canonicalize(dir.path()).expect("canonicalizing the temp dir should succeed");
    (dir, canonical)
}

/// Scans with a fixed worker count and no progress reporting.
fn scan(root: &Path) -> Vec<PathBuf> {
    find_all_zero_files(root, Jobs::new(4), &NoProgress)
}

#[test]
fn finds_non_empty_all_zero_files_recursively() {
    let (_dir, root) = fixture();
    let top = write(&root, "top.bin", &[0_u8; 512]);
    let nested = write(&root, "a/b/nested.bin", &[0_u8; 1]);
    write(&root, "a/not-zeroes.bin", &[0_u8, 0, 7, 0]);
    write(&root, "a/b/empty.bin", &[]);

    let mut expected = vec![top, nested];
    expected.sort();

    assert_eq!(
        scan(&root),
        expected,
        "only non-empty all-zero files, at any depth, should be reported"
    );
}

#[test]
fn results_are_absolute_and_sorted() {
    let (_dir, root) = fixture();
    write(&root, "zzz.bin", &[0_u8; 8]);
    write(&root, "aaa.bin", &[0_u8; 8]);
    write(&root, "mmm/deep.bin", &[0_u8; 8]);

    let found = scan(&root);

    assert_eq!(found.len(), 3, "all three all-zero files should be found");
    assert!(
        found.iter().all(|path| path.is_absolute()),
        "every reported path must be absolute, got {found:?}"
    );

    let mut sorted = found.clone();
    sorted.sort();
    assert_eq!(found, sorted, "results should come back sorted");
}

#[test]
fn a_root_that_is_itself_a_file_is_scanned() {
    let (_dir, root) = fixture();
    let file = write(&root, "lonely.bin", &[0_u8; 32]);

    assert_eq!(
        scan(&file),
        vec![file.clone()],
        "pointing zth at a single file should scan just that file"
    );
}

#[test]
fn a_root_that_is_a_non_zero_file_reports_nothing() {
    let (_dir, root) = fixture();
    let file = write(&root, "lonely.bin", &[1_u8; 32]);

    assert!(
        scan(&file).is_empty(),
        "a file with content should not be reported"
    );
}

#[test]
fn a_missing_root_reports_nothing_instead_of_failing() {
    let (_dir, root) = fixture();

    assert!(
        scan(&root.join("no-such-directory")).is_empty(),
        "an unwalkable root is an error to skip, not a panic"
    );
}

#[test]
fn an_empty_tree_reports_nothing() {
    let (_dir, root) = fixture();
    fs::create_dir_all(root.join("a/b/c")).expect("creating fixture directories should succeed");

    assert!(
        scan(&root).is_empty(),
        "directories alone should never be reported"
    );
}

#[test]
fn multi_byte_paths_are_found() {
    let (_dir, root) = fixture();
    let path = write(&root, "日本語/🎉/café.bin", &[0_u8; 128]);

    assert_eq!(
        scan(&root),
        vec![path],
        "non-ASCII directory and file names are ordinary paths"
    );
}

#[test]
fn one_worker_finds_the_same_files_as_many() {
    let (_dir, root) = fixture();
    for index in 0_u8..32 {
        write(&root, &format!("zero-{index}.bin"), &[0_u8; 64]);
        write(&root, &format!("data-{index}.bin"), &[index + 1; 64]);
    }

    assert_eq!(
        find_all_zero_files(&root, Jobs::new(1), &NoProgress),
        find_all_zero_files(&root, Jobs::new(8), &NoProgress),
        "the worker count must not change the result set"
    );
    assert_eq!(
        find_all_zero_files(&root, Jobs::new(1), &NoProgress).len(),
        32,
        "all 32 all-zero files should be found"
    );
}

#[test]
fn progress_reports_every_file_exactly_once() {
    let (_dir, root) = fixture();
    write(&root, "zero.bin", &[0_u8; 64]);
    write(&root, "sub/data.bin", &[3_u8; 64]);
    write(&root, "sub/empty.bin", &[]);

    let recorder = Recorder::default();
    let _found = find_all_zero_files(&root, Jobs::new(4), &recorder);

    assert_eq!(
        Recorder::sorted(&recorder.discovered),
        vec![1, 2, 3],
        "discovery should report a running total, once per file found"
    );
    assert_eq!(
        Recorder::sorted(&recorder.scanned),
        vec![1, 2, 3],
        "scanning should report a running total, once per file read"
    );
}

/// Totals that arrive out of order are not a cosmetic problem: indicatif treats
/// any backwards `set_position` as a seek and throws its whole ETA estimate
/// away, so a scan with several workers would spend its life resetting the
/// estimate it is supposed to be refining.
#[test]
fn progress_totals_arrive_in_order() {
    let (_dir, root) = fixture();
    for index in 0..2_000 {
        write(&root, &format!("f-{index}.bin"), &[0_u8; 4]);
    }

    let recorder = Recorder::default();
    let _found = find_all_zero_files(&root, Jobs::new(16), &recorder);

    let scanned = recorder
        .scanned
        .lock()
        .expect("recorder mutex should not be poisoned")
        .clone();

    assert_eq!(scanned.len(), 2_000, "every file should be reported once");

    let backwards: Vec<_> = scanned
        .windows(2)
        .filter(|pair| pair[0] > pair[1])
        .map(|pair| (pair[0], pair[1]))
        .collect();

    assert!(
        backwards.is_empty(),
        "scanned totals must never go backwards, got {} reversals, e.g. {:?}",
        backwards.len(),
        &backwards[..backwards.len().min(5)]
    );
}

/// How many all-zero files the observer-panic fixtures hold.
///
/// Large enough that the work is spread across every worker, so exactly one of
/// them is lost when a single callback panics - and the shortfall is visible.
const PANIC_FIXTURE_FILES: usize = 50;

/// A [`ScanProgress`] that panics the first time the chosen callback fires.
///
/// Observers fail for entirely ordinary reasons - arithmetic that overflows, a
/// channel whose receiver has already hung up, an `unwrap` on a lock another
/// panic poisoned - and which of the scan's threads happens to make the call is
/// not something the caller chose or can see.
struct PanicsOnce {
    on_discovered: bool,
    on_scanned: bool,
    fired: AtomicBool,
}

impl PanicsOnce {
    /// Panics inside the walker thread, on the first file discovered.
    fn on_discovered() -> Self {
        Self {
            on_discovered: true,
            on_scanned: false,
            fired: AtomicBool::new(false),
        }
    }

    /// Panics inside a worker thread, on the first file scanned.
    fn on_scanned() -> Self {
        Self {
            on_discovered: false,
            on_scanned: true,
            fired: AtomicBool::new(false),
        }
    }

    /// Panics on the first call and returns quietly on every one after it.
    ///
    /// Panicking once rather than every time is what makes the failure a
    /// *truncation*: the rest of the scan carries on and produces a result that
    /// looks entirely plausible.
    fn fire(&self) {
        if !self.fired.swap(true, Ordering::SeqCst) {
            panic!("the observer is broken - this panic is the point of the test");
        }
    }
}

impl ScanProgress for PanicsOnce {
    fn files_discovered(&self, _total: u64) {
        if self.on_discovered {
            self.fire();
        }
    }

    fn files_scanned(&self, _total: u64) {
        if self.on_scanned {
            self.fire();
        }
    }
}

/// Builds a tree of [`PANIC_FIXTURE_FILES`] all-zero files and scans it with
/// `progress`, reporting how many matches came back if the scan wrongly ran to
/// completion, and `None` if the observer's panic reached the caller.
fn matches_survived(progress: &dyn ScanProgress) -> Option<usize> {
    let (_dir, root) = fixture();
    for index in 0..PANIC_FIXTURE_FILES {
        write(&root, &format!("zero-{index}.bin"), &[0_u8; 64]);
    }

    panic::catch_unwind(AssertUnwindSafe(|| {
        find_all_zero_files(&root, Jobs::new(4), progress)
    }))
    .ok()
    .map(|found| found.len())
}

/// The silent half of the bug: a worker thread's panic used to be swallowed by
/// the `join()` that collected its matches, so the scan handed back everything
/// the *other* workers found and said nothing about the list it dropped on the
/// floor. `find_all_zero_files` returns a bare `Vec`, so a caller has no way to
/// tell that result apart from a complete one.
#[test]
fn a_panicking_scanned_observer_aborts_the_scan() {
    assert_eq!(
        matches_survived(&PanicsOnce::on_scanned()),
        None,
        "a panicking files_scanned observer must abort the scan, \
         not return a silently truncated result"
    );
}

/// The other half: whichever thread the observer happens to panic in, the
/// outcome a caller sees has to be the same one.
#[test]
fn a_panicking_discovered_observer_aborts_the_scan() {
    assert_eq!(
        matches_survived(&PanicsOnce::on_discovered()),
        None,
        "a panicking files_discovered observer must abort the scan, \
         not return a silently truncated result"
    );
}

/// A sparse file is all zeroes without a single zero byte existing on disk, and
/// it is exactly the shape a scan should be able to settle without reading -
/// the ones worth finding are routinely enormous.
#[cfg(unix)]
#[test]
fn sparse_files_are_found_alongside_written_ones() {
    let (_dir, root) = fixture();
    let written = write(&root, "written.bin", &[0_u8; 512]);

    let sparse = root.join("sparse.bin");
    fs::File::create(&sparse)
        .expect("creating the fixture should succeed")
        .set_len(64 << 20)
        .expect("extending the fixture should succeed");

    let empty_sparse = root.join("empty-sparse.bin");
    fs::File::create(&empty_sparse)
        .expect("creating the fixture should succeed")
        .set_len(0)
        .expect("truncating the fixture should succeed");

    let mut expected = vec![written, sparse];
    expected.sort();

    assert_eq!(
        scan(&root),
        expected,
        "a hole is zeroes, but a zero-length hole is still an empty file"
    );
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_followed() {
    use std::os::unix::fs::symlink;

    let (_outside_dir, outside) = fixture();
    let target = write(&outside, "target.bin", &[0_u8; 64]);

    let (_dir, root) = fixture();
    symlink(&target, root.join("link.bin")).expect("creating the symlink should succeed");
    symlink(&outside, root.join("link-dir")).expect("creating the symlink should succeed");

    assert!(
        scan(&root).is_empty(),
        "following symlinks risks escaping the tree (and reading /dev/zero forever)"
    );
}

#[cfg(unix)]
#[test]
fn unreadable_files_are_skipped_silently() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, root) = fixture();
    let readable = write(&root, "readable.bin", &[0_u8; 64]);
    let locked = write(&root, "locked.bin", &[0_u8; 64]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
        .expect("chmod on a temp file should succeed");

    // Running as root defeats the permission bits entirely; skip rather than
    // assert something the environment cannot honor.
    if fs::read(&locked).is_ok() {
        return;
    }

    let recorder = Recorder::default();
    let found = find_all_zero_files(&root, Jobs::new(2), &recorder);

    assert_eq!(
        found,
        vec![readable],
        "an unreadable file is skipped, and the readable one is still reported"
    );
    assert_eq!(
        Recorder::sorted(&recorder.scanned),
        vec![1, 2],
        "a file that fails to read still counts as scanned, so the bar completes"
    );
}

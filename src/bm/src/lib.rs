//! Core logic for `bm` (Bulk Move): recursively find files matching a pattern
//! and move them to a destination directory.
//!
//! This crate is split into a small, narrow public surface (a [`run`] entry
//! point plus the pieces it composes) hiding the real work: pattern selection,
//! collision-safe move planning, and a cross-volume move fallback that ordinary
//! `rename(2)` cannot perform.

use std::path::{Path, PathBuf};

pub use filewalker::FilterType;

/// The filename portion of a path, as an owned [`PathBuf`] of just the basename.
///
/// Returns `None` for paths that have no final component (e.g. `/`).
fn basename(path: &Path) -> Option<&std::ffi::OsStr> {
    path.file_name()
}

/// What to do when two files would land on the same destination path.
///
/// A collision happens either because a file with that basename already exists
/// in the destination, or because two matched source files share a basename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum CollisionPolicy {
    /// Refuse to move anything; report every collision. The safe default.
    #[default]
    Abort,
    /// Move the non-colliding files and skip the colliding ones.
    Skip,
    /// Move everything, disambiguating colliding names (`foo.mkv` -> `foo-1.mkv`).
    Rename,
    /// Move everything, letting later files clobber earlier ones (lossy).
    Overwrite,
}

/// Why a particular file would not be moved as-is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollisionKind {
    /// A file with this basename already exists in the destination directory.
    DestinationExists,
    /// Another matched source file in this batch claims the same basename.
    DuplicateBasename,
}

/// A single move the plan will perform: `source` -> `destination`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedMove {
    /// The file to move.
    pub source: PathBuf,
    /// The full path it will be moved to (destination dir + final filename).
    pub destination: PathBuf,
}

/// A file the plan will leave in place, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedMove {
    /// The file that will not be moved.
    pub source: PathBuf,
    /// The destination path it collided with.
    pub destination: PathBuf,
    /// Why it was skipped.
    pub reason: CollisionKind,
}

/// The result of planning a batch of moves under a [`CollisionPolicy`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MovePlan {
    /// Moves that will be executed.
    pub moves: Vec<PlannedMove>,
    /// Files that will be skipped (only populated under [`CollisionPolicy::Skip`]).
    pub skipped: Vec<SkippedMove>,
}

/// One destination path that more than one file wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    /// The contested destination path.
    pub destination: PathBuf,
    /// The source file(s) that would land there. For [`CollisionKind::DestinationExists`]
    /// this is the single matched file; for [`CollisionKind::DuplicateBasename`] it is
    /// every batch file sharing the basename.
    pub sources: Vec<PathBuf>,
    /// The nature of the collision.
    pub kind: CollisionKind,
}

/// Returned by [`plan_moves`] under [`CollisionPolicy::Abort`] when any collision exists.
#[derive(Debug, thiserror::Error)]
#[error("{} destination collision(s) detected", .collisions.len())]
pub struct CollisionError {
    /// Every collision found, in deterministic order.
    pub collisions: Vec<Collision>,
}

/// Plan which of `sources` to move into `destination`, honoring `policy`.
///
/// `exists` reports whether a candidate destination path is already occupied on
/// disk; it is injected so the planning logic stays pure and testable. Sources
/// are processed in sorted order so the plan is deterministic.
///
/// # Errors
///
/// Under [`CollisionPolicy::Abort`], returns [`CollisionError`] listing every
/// collision and plans no moves. Other policies never error.
pub fn plan_moves(
    sources: &[PathBuf],
    destination: &Path,
    policy: CollisionPolicy,
    exists: impl Fn(&Path) -> bool,
) -> Result<MovePlan, CollisionError> {
    // Deterministic order so plans (and collision reports) are stable across runs.
    let mut ordered: Vec<&PathBuf> = sources.iter().collect();
    ordered.sort();

    // Skip anything without a final path component (can't form a destination).
    let entries: Vec<(&PathBuf, PathBuf)> = ordered
        .iter()
        .filter_map(|source| basename(source).map(|name| (*source, destination.join(name))))
        .collect();

    match policy {
        CollisionPolicy::Abort => plan_abort(&entries, &exists),
        CollisionPolicy::Skip => Ok(plan_skip(&entries, &exists)),
        CollisionPolicy::Rename => Ok(plan_rename(&entries, &exists)),
        CollisionPolicy::Overwrite => Ok(plan_overwrite(&entries)),
    }
}

/// Overwrite planning: move every file to its basename in the destination,
/// letting later moves clobber earlier ones. Lossy by design.
fn plan_overwrite(entries: &[(&PathBuf, PathBuf)]) -> MovePlan {
    let moves = entries
        .iter()
        .map(|(source, candidate)| PlannedMove {
            source: (*source).clone(),
            destination: candidate.clone(),
        })
        .collect();
    MovePlan {
        moves,
        skipped: Vec::new(),
    }
}

/// Append `-N` before the extension: `foo.mkv`,1 -> `foo-1.mkv`; `README`,2 -> `README-2`.
///
/// `n == 0` yields the basename unchanged. Splitting on the file stem keeps the
/// extension recognizable (`archive.tar.gz` -> `archive.tar-1.gz`).
fn disambiguated_name(basename: &std::ffi::OsStr, n: usize) -> std::ffi::OsString {
    use std::ffi::OsString;

    if n == 0 {
        return basename.to_os_string();
    }

    let as_path = Path::new(basename);
    let stem = as_path.file_stem().unwrap_or(basename);
    let mut name = OsString::new();
    name.push(stem);
    name.push(format!("-{n}"));
    if let Some(ext) = as_path.extension() {
        name.push(".");
        name.push(ext);
    }
    name
}

/// Rename planning: move every file, disambiguating any name that would collide
/// with an existing file or with another file already placed in this batch.
fn plan_rename(entries: &[(&PathBuf, PathBuf)], exists: &impl Fn(&Path) -> bool) -> MovePlan {
    use std::collections::HashSet;

    let mut taken: HashSet<PathBuf> = HashSet::new();
    let mut moves = Vec::new();

    for (source, candidate) in entries {
        // `candidate` is `destination_dir/basename`; reuse its pieces to rename.
        let dir = candidate.parent().unwrap_or_else(|| Path::new(""));
        let base = candidate.file_name().unwrap_or_default();

        let mut n = 0;
        let final_destination = loop {
            let attempt = dir.join(disambiguated_name(base, n));
            if !exists(&attempt) && !taken.contains(&attempt) {
                break attempt;
            }
            n += 1;
        };

        taken.insert(final_destination.clone());
        moves.push(PlannedMove {
            source: (*source).clone(),
            destination: final_destination,
        });
    }

    MovePlan {
        moves,
        skipped: Vec::new(),
    }
}

/// Skip planning: move what can be moved, skip anything that would collide.
///
/// Processing in sorted order means the first source claiming a basename wins
/// and later duplicates are skipped.
fn plan_skip(entries: &[(&PathBuf, PathBuf)], exists: &impl Fn(&Path) -> bool) -> MovePlan {
    use std::collections::HashSet;

    let mut claimed: HashSet<PathBuf> = HashSet::new();
    let mut moves = Vec::new();
    let mut skipped = Vec::new();

    for (source, candidate) in entries {
        let reason = if exists(candidate) {
            Some(CollisionKind::DestinationExists)
        } else if claimed.contains(candidate) {
            Some(CollisionKind::DuplicateBasename)
        } else {
            None
        };

        match reason {
            Some(reason) => skipped.push(SkippedMove {
                source: (*source).clone(),
                destination: candidate.clone(),
                reason,
            }),
            None => {
                claimed.insert(candidate.clone());
                moves.push(PlannedMove {
                    source: (*source).clone(),
                    destination: candidate.clone(),
                });
            }
        }
    }

    MovePlan { moves, skipped }
}

/// Abort planning: surface every collision, plan nothing if any exists.
fn plan_abort(
    entries: &[(&PathBuf, PathBuf)],
    exists: &impl Fn(&Path) -> bool,
) -> Result<MovePlan, CollisionError> {
    use std::collections::BTreeMap;

    // Group sources by their candidate destination to find intra-batch duplicates.
    let mut by_destination: BTreeMap<&Path, Vec<PathBuf>> = BTreeMap::new();
    for (source, candidate) in entries {
        by_destination
            .entry(candidate.as_path())
            .or_default()
            .push((*source).clone());
    }

    let mut collisions = Vec::new();
    for (candidate, batch_sources) in by_destination {
        if batch_sources.len() > 1 {
            collisions.push(Collision {
                destination: candidate.to_path_buf(),
                sources: batch_sources,
                kind: CollisionKind::DuplicateBasename,
            });
        } else if exists(candidate) {
            collisions.push(Collision {
                destination: candidate.to_path_buf(),
                sources: batch_sources,
                kind: CollisionKind::DestinationExists,
            });
        }
    }

    if !collisions.is_empty() {
        return Err(CollisionError { collisions });
    }

    let moves = entries
        .iter()
        .map(|(source, candidate)| PlannedMove {
            source: (*source).clone(),
            destination: candidate.clone(),
        })
        .collect();
    Ok(MovePlan {
        moves,
        skipped: Vec::new(),
    })
}

/// Whether `err` is the "cross-device link" error (`EXDEV`).
///
/// `rename(2)` raises `EXDEV` when the source and destination live on different
/// filesystems — exactly the case bm must handle by copying then deleting, since
/// the original Go tool (and a naive `fs::rename`) simply fails here.
#[cfg(unix)]
pub fn is_cross_device_error(err: &std::io::Error) -> bool {
    err.raw_os_error() == Some(libc::EXDEV)
}

/// On Windows, `std::fs::rename` uses `MoveFileEx`, which already moves files
/// across volumes, so there is no cross-device error to fall back from.
#[cfg(not(unix))]
pub fn is_cross_device_error(_err: &std::io::Error) -> bool {
    false
}

/// How a file actually reached its destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveOutcome {
    /// Moved instantly via `rename(2)` (same filesystem).
    Renamed,
    /// Copied then deleted because source and destination are on different volumes.
    Copied,
}

/// Move a single file from `source` to `destination`.
///
/// Tries a fast `rename(2)` first. If that fails with a cross-device error
/// ([`is_cross_device_error`]), it falls back to `copy_across_volumes` and then
/// deletes the source — the original Go `bm` simply failed in this case.
///
/// # Errors
///
/// Propagates any rename error that is not cross-device, any error from
/// `copy_across_volumes`, and any failure to remove the source after copying.
pub fn move_file(
    source: &Path,
    destination: &Path,
    copy_across_volumes: impl FnOnce(&Path, &Path) -> std::io::Result<u64>,
) -> std::io::Result<MoveOutcome> {
    move_file_with(
        source,
        destination,
        |s, d| std::fs::rename(s, d),
        copy_across_volumes,
    )
}

/// [`move_file`] with the rename step injected, so the cross-device fallback can
/// be exercised deterministically in tests without a second real filesystem.
fn move_file_with(
    source: &Path,
    destination: &Path,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    copy_across_volumes: impl FnOnce(&Path, &Path) -> std::io::Result<u64>,
) -> std::io::Result<MoveOutcome> {
    let _ = (source, destination, rename, copy_across_volumes);
    todo!("driven by tests")
}

/// Error returned when the user did not specify exactly one search pattern.
#[derive(Debug, thiserror::Error)]
pub enum FilterSelectionError {
    /// Zero, or more than one, of `--suffix`/`--prefix`/`--substring` was given.
    #[error("exactly one of --suffix, --prefix, or --substring must be specified")]
    NotExactlyOne,
}

/// Select the single [`FilterType`] the user asked for.
///
/// Exactly one of `suffix`, `prefix`, or `substring` must be `Some`; anything
/// else is a usage error.
///
/// # Errors
///
/// Returns [`FilterSelectionError::NotExactlyOne`] if the number of supplied
/// patterns is not exactly one.
pub fn select_filter(
    suffix: Option<String>,
    prefix: Option<String>,
    substring: Option<String>,
) -> Result<FilterType, FilterSelectionError> {
    let mut selected = None;
    let mut count = 0;

    if let Some(suffix) = suffix {
        selected = Some(FilterType::Suffix(suffix));
        count += 1;
    }
    if let Some(prefix) = prefix {
        selected = Some(FilterType::Prefix(prefix));
        count += 1;
    }
    if let Some(substring) = substring {
        selected = Some(FilterType::Substring(substring));
        count += 1;
    }

    match (count, selected) {
        (1, Some(filter)) => Ok(filter),
        _ => Err(FilterSelectionError::NotExactlyOne),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_filter_returns_suffix_when_only_suffix_given() {
        let filter = select_filter(Some(".mkv".to_string()), None, None).unwrap();
        assert!(matches!(filter, FilterType::Suffix(s) if s == ".mkv"));
    }

    #[test]
    fn select_filter_returns_prefix_when_only_prefix_given() {
        let filter = select_filter(None, Some("IMG_".to_string()), None).unwrap();
        assert!(matches!(filter, FilterType::Prefix(s) if s == "IMG_"));
    }

    #[test]
    fn select_filter_returns_substring_when_only_substring_given() {
        let filter = select_filter(None, None, Some("2024".to_string())).unwrap();
        assert!(matches!(filter, FilterType::Substring(s) if s == "2024"));
    }

    #[test]
    fn select_filter_rejects_when_none_given() {
        assert!(select_filter(None, None, None).is_err());
    }

    #[test]
    fn select_filter_rejects_when_multiple_given() {
        assert!(select_filter(Some(".mkv".to_string()), Some("IMG_".to_string()), None).is_err());
    }

    // --- plan_moves: abort policy ---

    #[test]
    fn plan_abort_moves_all_when_no_collisions() {
        let sources = vec![PathBuf::from("/a/one.mkv"), PathBuf::from("/b/two.mkv")];
        let plan = plan_moves(&sources, Path::new("/dest"), CollisionPolicy::Abort, |_| {
            false
        })
        .unwrap();
        assert_eq!(plan.moves.len(), 2);
        assert!(plan.skipped.is_empty());
        assert!(plan
            .moves
            .iter()
            .any(|m| m.destination == Path::new("/dest/one.mkv")));
        assert!(plan
            .moves
            .iter()
            .any(|m| m.destination == Path::new("/dest/two.mkv")));
    }

    #[test]
    fn plan_abort_errors_when_destination_file_exists() {
        let sources = vec![PathBuf::from("/a/one.mkv")];
        let err = plan_moves(&sources, Path::new("/dest"), CollisionPolicy::Abort, |p| {
            p == Path::new("/dest/one.mkv")
        })
        .unwrap_err();
        assert_eq!(err.collisions.len(), 1);
        assert_eq!(
            err.collisions[0].destination,
            PathBuf::from("/dest/one.mkv")
        );
        assert_eq!(err.collisions[0].kind, CollisionKind::DestinationExists);
    }

    #[test]
    fn plan_abort_errors_on_intra_batch_duplicate_basenames() {
        let sources = vec![PathBuf::from("/a/dup.mkv"), PathBuf::from("/b/dup.mkv")];
        let err = plan_moves(&sources, Path::new("/dest"), CollisionPolicy::Abort, |_| {
            false
        })
        .unwrap_err();
        assert_eq!(err.collisions.len(), 1);
        assert_eq!(err.collisions[0].kind, CollisionKind::DuplicateBasename);
        assert_eq!(err.collisions[0].sources.len(), 2);
    }

    // --- plan_moves: skip policy ---

    #[test]
    fn plan_skip_moves_noncolliding_and_skips_existing() {
        let sources = vec![PathBuf::from("/a/keep.mkv"), PathBuf::from("/a/dupe.mkv")];
        let plan = plan_moves(&sources, Path::new("/dest"), CollisionPolicy::Skip, |p| {
            p == Path::new("/dest/dupe.mkv")
        })
        .unwrap();
        assert_eq!(plan.moves.len(), 1);
        assert_eq!(plan.moves[0].destination, PathBuf::from("/dest/keep.mkv"));
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].source, PathBuf::from("/a/dupe.mkv"));
        assert_eq!(plan.skipped[0].reason, CollisionKind::DestinationExists);
    }

    #[test]
    fn plan_skip_keeps_first_of_duplicate_basenames() {
        let sources = vec![PathBuf::from("/b/dup.mkv"), PathBuf::from("/a/dup.mkv")];
        let plan = plan_moves(&sources, Path::new("/dest"), CollisionPolicy::Skip, |_| {
            false
        })
        .unwrap();
        assert_eq!(plan.moves.len(), 1);
        // Sorted order means /a/dup.mkv wins; /b/dup.mkv is skipped.
        assert_eq!(plan.moves[0].source, PathBuf::from("/a/dup.mkv"));
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].source, PathBuf::from("/b/dup.mkv"));
        assert_eq!(plan.skipped[0].reason, CollisionKind::DuplicateBasename);
    }

    // --- plan_moves: rename policy ---

    #[test]
    fn plan_rename_disambiguates_duplicate_basenames() {
        let sources = vec![PathBuf::from("/a/dup.mkv"), PathBuf::from("/b/dup.mkv")];
        let plan = plan_moves(
            &sources,
            Path::new("/dest"),
            CollisionPolicy::Rename,
            |_| false,
        )
        .unwrap();
        assert_eq!(plan.moves.len(), 2);
        assert!(plan.skipped.is_empty());
        let dests: Vec<_> = plan.moves.iter().map(|m| m.destination.clone()).collect();
        assert!(dests.contains(&PathBuf::from("/dest/dup.mkv")));
        assert!(dests.contains(&PathBuf::from("/dest/dup-1.mkv")));
    }

    #[test]
    fn plan_rename_avoids_existing_destination_files() {
        let sources = vec![PathBuf::from("/a/foo.mkv")];
        // foo.mkv and foo-1.mkv already exist, so the file must become foo-2.mkv.
        let plan = plan_moves(&sources, Path::new("/dest"), CollisionPolicy::Rename, |p| {
            p == Path::new("/dest/foo.mkv") || p == Path::new("/dest/foo-1.mkv")
        })
        .unwrap();
        assert_eq!(plan.moves.len(), 1);
        assert_eq!(plan.moves[0].destination, PathBuf::from("/dest/foo-2.mkv"));
    }

    #[test]
    fn plan_rename_handles_files_without_extension() {
        let sources = vec![PathBuf::from("/a/README"), PathBuf::from("/b/README")];
        let plan = plan_moves(
            &sources,
            Path::new("/dest"),
            CollisionPolicy::Rename,
            |_| false,
        )
        .unwrap();
        let dests: Vec<_> = plan.moves.iter().map(|m| m.destination.clone()).collect();
        assert!(dests.contains(&PathBuf::from("/dest/README")));
        assert!(dests.contains(&PathBuf::from("/dest/README-1")));
    }

    // --- plan_moves: overwrite policy ---

    #[test]
    fn plan_overwrite_moves_everything_ignoring_existing() {
        let sources = vec![PathBuf::from("/a/one.mkv"), PathBuf::from("/b/two.mkv")];
        let plan = plan_moves(
            &sources,
            Path::new("/dest"),
            CollisionPolicy::Overwrite,
            |_| true,
        )
        .unwrap();
        assert_eq!(plan.moves.len(), 2);
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn plan_overwrite_keeps_all_duplicate_basenames() {
        let sources = vec![PathBuf::from("/a/dup.mkv"), PathBuf::from("/b/dup.mkv")];
        let plan = plan_moves(
            &sources,
            Path::new("/dest"),
            CollisionPolicy::Overwrite,
            |_| false,
        )
        .unwrap();
        assert_eq!(plan.moves.len(), 2);
        assert!(plan
            .moves
            .iter()
            .all(|m| m.destination == Path::new("/dest/dup.mkv")));
    }

    // --- cross-device detection ---

    #[cfg(unix)]
    #[test]
    fn is_cross_device_error_true_only_for_exdev() {
        use std::io::Error;
        assert!(is_cross_device_error(&Error::from_raw_os_error(
            libc::EXDEV
        )));
        assert!(!is_cross_device_error(&Error::from_raw_os_error(
            libc::ENOENT
        )));
        assert!(!is_cross_device_error(&Error::from_raw_os_error(
            libc::EACCES
        )));
    }

    // --- move_file: rename + cross-volume fallback ---

    #[test]
    fn move_file_renames_within_same_volume() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        std::fs::write(&src, b"hi").unwrap();
        let dst = dir.path().join("b.txt");

        let outcome =
            move_file(&src, &dst, |_, _| panic!("same-volume move must not copy")).unwrap();

        assert_eq!(outcome, MoveOutcome::Renamed);
        assert!(!src.exists());
        assert_eq!(std::fs::read(&dst).unwrap(), b"hi");
    }

    #[cfg(unix)]
    #[test]
    fn move_file_falls_back_to_copy_when_rename_is_cross_device() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        std::fs::write(&src, b"payload").unwrap();
        let dst = dir.path().join("b.txt");

        let outcome = move_file_with(
            &src,
            &dst,
            |_, _| Err(std::io::Error::from_raw_os_error(libc::EXDEV)),
            |s, d| std::fs::copy(s, d),
        )
        .unwrap();

        assert_eq!(outcome, MoveOutcome::Copied);
        assert!(
            !src.exists(),
            "source must be removed after a successful cross-volume copy"
        );
        assert_eq!(std::fs::read(&dst).unwrap(), b"payload");
    }

    #[test]
    fn move_file_propagates_non_cross_device_errors() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        std::fs::write(&src, b"x").unwrap();
        let dst = dir.path().join("b.txt");

        let err = move_file_with(
            &src,
            &dst,
            |_, _| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "denied",
                ))
            },
            |_, _| panic!("must not copy on a non-cross-device error"),
        )
        .unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(
            src.exists(),
            "nothing should be moved when rename fails fatally"
        );
    }
}

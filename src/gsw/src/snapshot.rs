//! Combine parsed git data + filesystem ages into a [`Snapshot`].

use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::Duration;

use crate::git::{FileEntry, NumStat};
use crate::render::{RenderEntry, Snapshot};

/// Assemble a [`Snapshot`] from parsed git outputs.
///
/// `staged_numstat` and `unstaged_numstat` are keyed on the post-rename path.
/// `ages` maps file path → mtime age; missing entries become `None`.
#[allow(
    clippy::too_many_arguments,
    reason = "build_snapshot fans in five distinct inputs; grouping them obscures rather than clarifies"
)]
pub fn build_snapshot(
    branch: String,
    base: String,
    commits_ahead: u32,
    commits_behind: u32,
    last_commit_age: Option<Duration>,
    status_entries: Vec<FileEntry>,
    staged_numstat: &HashMap<String, NumStat>,
    unstaged_numstat: &HashMap<String, NumStat>,
    ages: &HashMap<String, Duration>,
) -> Snapshot {
    let mut files: Vec<RenderEntry> = status_entries
        .into_iter()
        .map(|e| {
            let side = if e.staged {
                staged_numstat
            } else {
                unstaged_numstat
            };
            let (adds, dels, binary) = side
                .get(&e.path)
                .map_or((0, 0, false), |n| (n.adds, n.dels, n.binary));
            let age = ages.get(&e.path).copied();
            RenderEntry {
                path: e.path,
                orig_path: e.orig_path,
                status: e.status,
                staged: e.staged,
                adds,
                dels,
                binary,
                age,
            }
        })
        .collect();
    // Newest mtime on top; rows with no mtime (deleted files, untracked
    // dirs) sink to the bottom. Stable so same-mtime rows keep git's order
    // (e.g., staged + unstaged of the same path).
    files.sort_by(|a, b| match (a.age, b.age) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    Snapshot {
        branch,
        base,
        commits_ahead,
        commits_behind,
        last_commit_age,
        files,
        log: Vec::new(),
        upstream: None,
        operation: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::FileStatus;

    fn ns(adds: u32, dels: u32) -> NumStat {
        NumStat {
            adds,
            dels,
            binary: false,
        }
    }

    fn binstat() -> NumStat {
        NumStat {
            adds: 0,
            dels: 0,
            binary: true,
        }
    }

    #[test]
    fn header_fields_pass_through() {
        let snap = build_snapshot(
            "gsv".into(),
            "main".into(),
            5,
            7,
            Some(Duration::from_secs(120)),
            vec![],
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(snap.branch, "gsv");
        assert_eq!(snap.base, "main");
        assert_eq!(snap.commits_ahead, 5);
        assert_eq!(snap.commits_behind, 7);
        assert_eq!(snap.last_commit_age, Some(Duration::from_secs(120)));
    }

    #[test]
    fn staged_entry_picks_staged_numstat() {
        let entries = vec![FileEntry {
            path: "src/foo.rs".into(),
            orig_path: None,
            status: FileStatus::Modified,
            staged: true,
        }];
        let mut staged = HashMap::new();
        staged.insert("src/foo.rs".to_string(), ns(10, 2));
        let mut unstaged = HashMap::new();
        unstaged.insert("src/foo.rs".to_string(), ns(99, 99));

        let snap = build_snapshot(
            "x".into(),
            "main".into(),
            0,
            0,
            Some(Duration::ZERO),
            entries,
            &staged,
            &unstaged,
            &HashMap::new(),
        );
        assert_eq!(snap.files.len(), 1);
        assert_eq!(snap.files[0].adds, 10);
        assert_eq!(snap.files[0].dels, 2);
        assert!(snap.files[0].staged);
    }

    #[test]
    fn unstaged_entry_picks_unstaged_numstat() {
        let entries = vec![FileEntry {
            path: "src/foo.rs".into(),
            orig_path: None,
            status: FileStatus::Modified,
            staged: false,
        }];
        let mut staged = HashMap::new();
        staged.insert("src/foo.rs".to_string(), ns(99, 99));
        let mut unstaged = HashMap::new();
        unstaged.insert("src/foo.rs".to_string(), ns(7, 1));

        let snap = build_snapshot(
            "x".into(),
            "main".into(),
            0,
            0,
            Some(Duration::ZERO),
            entries,
            &staged,
            &unstaged,
            &HashMap::new(),
        );
        assert_eq!(snap.files.len(), 1);
        assert_eq!(snap.files[0].adds, 7);
        assert_eq!(snap.files[0].dels, 1);
        assert!(!snap.files[0].staged);
    }

    #[test]
    fn binary_flag_propagates() {
        let entries = vec![FileEntry {
            path: "img.png".into(),
            orig_path: None,
            status: FileStatus::Modified,
            staged: true,
        }];
        let mut staged = HashMap::new();
        staged.insert("img.png".to_string(), binstat());

        let snap = build_snapshot(
            "x".into(),
            "main".into(),
            0,
            0,
            Some(Duration::ZERO),
            entries,
            &staged,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(snap.files.len(), 1);
        assert!(snap.files[0].binary);
        assert_eq!(snap.files[0].adds, 0);
        assert_eq!(snap.files[0].dels, 0);
    }

    #[test]
    fn age_lookup_by_path() {
        let entries = vec![FileEntry {
            path: "src/foo.rs".into(),
            orig_path: None,
            status: FileStatus::Modified,
            staged: false,
        }];
        let mut ages = HashMap::new();
        ages.insert("src/foo.rs".to_string(), Duration::from_secs(45));

        let snap = build_snapshot(
            "x".into(),
            "main".into(),
            0,
            0,
            Some(Duration::ZERO),
            entries,
            &HashMap::new(),
            &HashMap::new(),
            &ages,
        );
        assert_eq!(snap.files[0].age, Some(Duration::from_secs(45)));
    }

    #[test]
    fn missing_age_is_none() {
        let entries = vec![FileEntry {
            path: "src/foo.rs".into(),
            orig_path: None,
            status: FileStatus::Modified,
            staged: false,
        }];

        let snap = build_snapshot(
            "x".into(),
            "main".into(),
            0,
            0,
            Some(Duration::ZERO),
            entries,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(snap.files[0].age, None);
    }

    #[test]
    fn rename_info_preserved() {
        let entries = vec![FileEntry {
            path: "src/new.rs".into(),
            orig_path: Some("src/old.rs".into()),
            status: FileStatus::Renamed,
            staged: true,
        }];
        let mut staged = HashMap::new();
        staged.insert("src/new.rs".to_string(), ns(5, 2));

        let snap = build_snapshot(
            "x".into(),
            "main".into(),
            0,
            0,
            Some(Duration::ZERO),
            entries,
            &staged,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(snap.files[0].path, "src/new.rs");
        assert_eq!(snap.files[0].orig_path.as_deref(), Some("src/old.rs"));
    }

    #[test]
    fn both_sides_become_two_render_entries() {
        // A file modified in both index and worktree shows up as two FileEntry rows.
        let entries = vec![
            FileEntry {
                path: "src/foo.rs".into(),
                orig_path: None,
                status: FileStatus::Modified,
                staged: true,
            },
            FileEntry {
                path: "src/foo.rs".into(),
                orig_path: None,
                status: FileStatus::Modified,
                staged: false,
            },
        ];
        let mut staged = HashMap::new();
        staged.insert("src/foo.rs".to_string(), ns(10, 0));
        let mut unstaged = HashMap::new();
        unstaged.insert("src/foo.rs".to_string(), ns(0, 3));

        let snap = build_snapshot(
            "x".into(),
            "main".into(),
            0,
            0,
            Some(Duration::ZERO),
            entries,
            &staged,
            &unstaged,
            &HashMap::new(),
        );
        assert_eq!(snap.files.len(), 2);
        let staged_row = snap.files.iter().find(|e| e.staged).unwrap();
        let unstaged_row = snap.files.iter().find(|e| !e.staged).unwrap();
        assert_eq!(staged_row.adds, 10);
        assert_eq!(staged_row.dels, 0);
        assert_eq!(unstaged_row.adds, 0);
        assert_eq!(unstaged_row.dels, 3);
    }

    #[test]
    fn files_sorted_newest_mtime_first_then_none() {
        // Three files with mtimes (newest, mid, oldest) and one with no mtime.
        // Git status emits them in an arbitrary order; the snapshot should
        // re-order so newest mtime is on top and the no-mtime row sinks to
        // the bottom.
        let entries = vec![
            FileEntry {
                path: "mid.rs".into(),
                orig_path: None,
                status: FileStatus::Modified,
                staged: false,
            },
            FileEntry {
                path: "no_age.rs".into(),
                orig_path: None,
                status: FileStatus::Deleted,
                staged: false,
            },
            FileEntry {
                path: "newest.rs".into(),
                orig_path: None,
                status: FileStatus::Modified,
                staged: false,
            },
            FileEntry {
                path: "oldest.rs".into(),
                orig_path: None,
                status: FileStatus::Modified,
                staged: false,
            },
        ];
        let mut ages = HashMap::new();
        ages.insert("newest.rs".to_string(), Duration::from_secs(10));
        ages.insert("mid.rs".to_string(), Duration::from_secs(60));
        ages.insert("oldest.rs".to_string(), Duration::from_secs(3600));

        let snap = build_snapshot(
            "x".into(),
            "main".into(),
            0,
            0,
            Some(Duration::ZERO),
            entries,
            &HashMap::new(),
            &HashMap::new(),
            &ages,
        );
        let paths: Vec<&str> = snap.files.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["newest.rs", "mid.rs", "oldest.rs", "no_age.rs"]);
    }

    #[test]
    fn sort_is_stable_for_equal_ages() {
        // Same-path staged + unstaged rows share an mtime; their input order
        // must be preserved after sorting.
        let entries = vec![
            FileEntry {
                path: "src/foo.rs".into(),
                orig_path: None,
                status: FileStatus::Modified,
                staged: true,
            },
            FileEntry {
                path: "src/foo.rs".into(),
                orig_path: None,
                status: FileStatus::Modified,
                staged: false,
            },
        ];
        let mut ages = HashMap::new();
        ages.insert("src/foo.rs".to_string(), Duration::from_secs(30));

        let snap = build_snapshot(
            "x".into(),
            "main".into(),
            0,
            0,
            Some(Duration::ZERO),
            entries,
            &HashMap::new(),
            &HashMap::new(),
            &ages,
        );
        assert_eq!(snap.files.len(), 2);
        assert!(snap.files[0].staged);
        assert!(!snap.files[1].staged);
    }

    #[test]
    fn untracked_entries_keep_status_and_skip_numstat() {
        let entries = vec![FileEntry {
            path: "scratch.txt".into(),
            orig_path: None,
            status: FileStatus::Untracked,
            staged: false,
        }];

        let snap = build_snapshot(
            "x".into(),
            "main".into(),
            0,
            0,
            Some(Duration::ZERO),
            entries,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(snap.files.len(), 1);
        assert!(matches!(snap.files[0].status, FileStatus::Untracked));
        assert_eq!(snap.files[0].adds, 0);
        assert_eq!(snap.files[0].dels, 0);
    }
}

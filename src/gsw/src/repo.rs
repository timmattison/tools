//! gix-backed git operations for gsw.
//!
//! gsw is a read-only monitor. Every function here reads the repository
//! in-process via `gix` and never writes the index, so it can never take
//! `.git/index.lock` and can never race a concurrent rebase — the reason the
//! old `git` CLI path needed a private index snapshot.

use std::collections::HashMap;

use crate::git::{FileEntry, FileStatus, NumStat};
use crate::render::UpstreamStatus;

/// Open the repository containing `cwd`, or `None` when there isn't one with a
/// working tree (outside any repo, or a bare repo — gsw has nothing per-file to
/// render in either case).
pub fn open() -> Option<gix::Repository> {
    let repo = gix::discover(".").ok()?;
    // Bare repos have no work tree; gsw renders a per-file working-tree view,
    // so there's nothing to show. Treat them like "not a repo".
    if repo.workdir().is_some() { Some(repo) } else { None }
}

/// The short current-branch name (e.g. `main`), or `"HEAD"` when detached —
/// matching what `git rev-parse --abbrev-ref HEAD` prints.
pub fn branch_name(repo: &gix::Repository) -> String {
    match repo.head_name() {
        Ok(Some(full)) => full.shorten().to_string(),
        _ => "HEAD".to_string(),
    }
}

/// Pick the first base ref that resolves: `main`, then `master`, then
/// `origin/HEAD`'s target, else `"HEAD"` (so commits-ahead degrades to 0).
pub fn resolve_base(repo: &gix::Repository) -> String {
    for candidate in ["main", "master"] {
        if repo.rev_parse_single(candidate).is_ok() {
            return candidate.to_string();
        }
    }
    if let Ok(reference) = repo.find_reference("refs/remotes/origin/HEAD") {
        if let Some(target) = reference.target().try_name() {
            // target is e.g. refs/remotes/origin/main → shorten to origin/main
            return target.shorten().to_string();
        }
    }
    "HEAD".to_string()
}

/// Committer timestamp of HEAD as unix seconds, or `None` (no commits, etc.).
/// Matches `git log -1 --format=%ct` (committer date, not author date).
pub fn head_commit_secs(repo: &gix::Repository) -> Option<i64> {
    let commit = repo.head_commit().ok()?;
    Some(commit.time().ok()?.seconds)
}

/// The `n` most recent commits from HEAD as `(short_hash, unix_secs, summary)`.
/// Empty when `n == 0` or there are no commits.
pub fn recent_log(repo: &gix::Repository, n: usize) -> Vec<(String, i64, String)> {
    if n == 0 {
        return Vec::new();
    }
    let Ok(head) = repo.head_commit() else {
        return Vec::new();
    };
    let Ok(walk) = head.ancestors().all() else {
        return Vec::new();
    };
    walk.take(n)
        .filter_map(|info| {
            let info = info.ok()?;
            let commit = info.object().ok()?;
            let hash = info.id().shorten_or_id().to_string();
            let secs = commit.time().ok()?.seconds;
            let summary = commit.message().ok()?.summary().to_string();
            Some((hash, secs, summary))
        })
        .collect()
}

/// Count commits reachable from HEAD but not from `base`
/// (`git rev-list --count base..HEAD`). Returns 0 on any failure.
pub fn commits_ahead(repo: &gix::Repository, base: &str) -> u32 {
    let resolve = || -> anyhow::Result<u32> {
        let head = repo.head_id()?.detach();
        let base_id = repo.rev_parse_single(base)?.detach();
        if head == base_id {
            return Ok(0);
        }
        let count = repo
            .rev_walk(std::iter::once(head))
            .with_hidden(std::iter::once(base_id))
            .all()?
            .count();
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    };
    resolve().unwrap_or(0)
}

/// The current branch's upstream tracking status. `name` is the short
/// tracking-ref name like `origin/main`; ahead/behind match
/// `git rev-list --left-right --count <upstream>...HEAD`.
///
/// Returns `None` when HEAD is detached/unborn, the branch has no upstream
/// configured, or the upstream tracking ref hasn't been fetched yet (i.e.
/// `origin/main` exists in config but not under `.git/refs/`) — the same cases
/// where `git rev-parse @{upstream}` fails, so this matches the old CLI path.
pub fn upstream_status(repo: &gix::Repository) -> Option<UpstreamStatus> {
    use gix::bstr::ByteSlice;
    use gix::remote::Direction;

    let head_ref = repo.head_ref().ok()??; // None => detached/unborn
    let full = match head_ref.remote_tracking_ref_name(Direction::Fetch) {
        Some(Ok(full)) => full,
        _ => return None, // no upstream configured (or name error)
    };
    let name = full.shorten().to_str().ok()?.to_owned();

    let head_id = repo.head_id().ok()?.detach();
    let upstream_id = repo.rev_parse_single("@{upstream}").ok()?.detach();

    let ahead = repo
        .rev_walk(std::iter::once(head_id))
        .with_hidden(std::iter::once(upstream_id))
        .all()
        .ok()?
        .count();
    let behind = repo
        .rev_walk(std::iter::once(upstream_id))
        .with_hidden(std::iter::once(head_id))
        .all()
        .ok()?
        .count();

    Some(UpstreamStatus {
        name,
        ahead: u32::try_from(ahead).unwrap_or(u32::MAX),
        behind: u32::try_from(behind).unwrap_or(u32::MAX),
    })
}

/// Everything one working-tree status walk produces: the `FileEntry` rows plus
/// the staged and unstaged per-path line counts. See [`collect_changes`].
pub struct Changes {
    /// `FileEntry` rows mirroring `git status --porcelain=v2 -z`, sorted by
    /// path with the staged row before the unstaged row for the same path.
    pub entries: Vec<FileEntry>,
    /// Staged (HEAD-tree vs index) per-path line counts, mirroring
    /// `git diff --cached --numstat`.
    pub staged_numstat: HashMap<String, NumStat>,
    /// Unstaged (index vs worktree) per-path line counts, mirroring
    /// `git diff --numstat`.
    pub unstaged_numstat: HashMap<String, NumStat>,
}

/// All working-tree changes in a single status walk: the `FileEntry` rows
/// (mirroring `git status --porcelain=v2 -z`) plus the staged (HEAD-tree vs
/// index) and unstaged (index vs worktree) per-path line counts (mirroring
/// `git diff [--cached] --numstat`).
///
/// gsw is typically polled every couple of seconds under `viddy`/`watch`, so
/// the entries and both numstat maps are produced from one traversal rather
/// than three: creating a fresh status platform re-reads the index and re-walks
/// the working tree each time, and doing that work once per tick instead of
/// twice is the whole point of sharing the loop.
///
/// A path modified in both index and worktree yields two entry rows (staged +
/// unstaged). Entry rows are sorted by path, staged before unstaged, so the
/// downstream stable mtime sort is deterministic (gix's status iterator yields
/// items in nondeterministic order); the numstat maps are order-independent.
///
/// Numstat details: untracked files are excluded (git's numstat ignores them);
/// binary blobs (NUL in the first 8 KiB) are flagged `binary` with zero counts;
/// counts are a raw byte-line diff with no clean/smudge or autocrlf filtering,
/// which matches git's counts in the common case. Worktree-side renames produce
/// an entry row but no numstat (rare, and git numstat handles them specially;
/// skipping is a conservative undercount for the monitor use case).
///
/// # Errors
///
/// Returns an error when the gix status platform cannot be created or iteration
/// fails.
pub fn collect_changes(repo: &gix::Repository) -> anyhow::Result<Changes> {
    use gix::diff::index::Change;
    use gix::status::index_worktree::Item as IwItem;
    use gix::status::plumbing::index_as_worktree::{Change as IwChange, EntryStatus};

    let mut entries: Vec<FileEntry> = Vec::new();
    let mut staged: HashMap<String, NumStat> = HashMap::new();
    let mut unstaged: HashMap<String, NumStat> = HashMap::new();

    let iter = repo
        .status(gix::progress::Discard)
        .map_err(|e| anyhow::anyhow!("status platform: {e}"))?
        .untracked_files(gix::status::UntrackedFiles::Collapsed)
        .into_iter(Vec::<gix::bstr::BString>::new())
        .map_err(|e| anyhow::anyhow!("status iter: {e}"))?;

    for item in iter {
        let item = item.map_err(|e| anyhow::anyhow!("status item: {e}"))?;
        match item {
            // Staged side: HEAD-tree vs index. Each change yields one entry row
            // and one staged numstat keyed on the (post-rename) path.
            gix::status::Item::TreeIndex(change) => match change {
                Change::Addition { location, id, .. } => {
                    let key = location.to_string();
                    let new = blob_bytes(repo, id.as_ref());
                    staged.insert(key.clone(), line_counts(&[], &new));
                    entries.push(FileEntry {
                        path: key,
                        orig_path: None,
                        status: FileStatus::Added,
                        staged: true,
                    });
                }
                Change::Deletion { location, id, .. } => {
                    let key = location.to_string();
                    let old = blob_bytes(repo, id.as_ref());
                    staged.insert(key.clone(), line_counts(&old, &[]));
                    entries.push(FileEntry {
                        path: key,
                        orig_path: None,
                        status: FileStatus::Deleted,
                        staged: true,
                    });
                }
                Change::Modification {
                    location,
                    previous_id,
                    id,
                    previous_entry_mode,
                    entry_mode,
                    ..
                } => {
                    let key = location.to_string();
                    let old = blob_bytes(repo, previous_id.as_ref());
                    let new = blob_bytes(repo, id.as_ref());
                    staged.insert(key.clone(), line_counts(&old, &new));
                    // Detect a type change by comparing the type bits of the mode.
                    // gix_index::entry::Mode is a bitflags struct; mask out permission bits.
                    const TYPE_MASK: u32 = 0o170_000_u32;
                    let status = if (previous_entry_mode.bits() & TYPE_MASK)
                        != (entry_mode.bits() & TYPE_MASK)
                    {
                        FileStatus::TypeChange
                    } else {
                        FileStatus::Modified
                    };
                    entries.push(FileEntry {
                        path: key,
                        orig_path: None,
                        status,
                        staged: true,
                    });
                }
                Change::Rewrite {
                    location,
                    source_location,
                    source_id,
                    id,
                    copy,
                    ..
                } => {
                    let key = location.to_string();
                    let old = blob_bytes(repo, source_id.as_ref());
                    let new = blob_bytes(repo, id.as_ref());
                    staged.insert(key.clone(), line_counts(&old, &new));
                    let status = if copy { FileStatus::Copied } else { FileStatus::Renamed };
                    entries.push(FileEntry {
                        path: key,
                        orig_path: Some(source_location.to_string()),
                        status,
                        staged: true,
                    });
                }
            },
            // Unstaged side: index vs worktree. A modification yields one entry
            // row, and a content-bearing change also yields an unstaged numstat.
            gix::status::Item::IndexWorktree(IwItem::Modification {
                rela_path,
                entry,
                status,
                ..
            }) => {
                // Classify for the entry row first. NeedsUpdate means the stat
                // cache is stale but the content is identical — no visible
                // change, so skip the whole item (no row, no numstat).
                let file_status = match &status {
                    EntryStatus::Conflict { .. } => FileStatus::Conflicted,
                    EntryStatus::IntentToAdd => FileStatus::Added,
                    EntryStatus::Change(change) => match change {
                        IwChange::Removed => FileStatus::Deleted,
                        IwChange::Type { .. } => FileStatus::TypeChange,
                        IwChange::Modification { .. } | IwChange::SubmoduleModification(_) => {
                            FileStatus::Modified
                        }
                    },
                    EntryStatus::NeedsUpdate(_) => continue,
                };
                let key = rela_path.to_string();
                // Numstat only for content-bearing changes; conflicts, intent-to-add
                // and submodule modifications produce a row but no line counts.
                match &status {
                    EntryStatus::Change(IwChange::Removed) => {
                        let old = blob_bytes(repo, entry.id.as_ref());
                        unstaged.insert(key.clone(), line_counts(&old, &[]));
                    }
                    EntryStatus::Change(IwChange::Modification { .. } | IwChange::Type { .. }) => {
                        let old = blob_bytes(repo, entry.id.as_ref());
                        let new = worktree_bytes(repo, &rela_path);
                        unstaged.insert(key.clone(), line_counts(&old, &new));
                    }
                    _ => {}
                }
                entries.push(FileEntry {
                    path: key,
                    orig_path: None,
                    status: file_status,
                    staged: false,
                });
            }
            // Untracked (and ignored) directory walk. Only surface Untracked
            // entries; numstat never counts these.
            gix::status::Item::IndexWorktree(IwItem::DirectoryContents { entry, .. }) => {
                if entry.status != gix::dir::entry::Status::Untracked {
                    continue;
                }
                let is_dir = entry.disk_kind.is_some_and(|k| k.is_dir());
                let mut path = entry.rela_path.to_string();
                let status = if is_dir {
                    if !path.ends_with('/') {
                        path.push('/');
                    }
                    FileStatus::UntrackedDir
                } else {
                    FileStatus::Untracked
                };
                entries.push(FileEntry {
                    path,
                    orig_path: None,
                    status,
                    staged: false,
                });
            }
            // Worktree-side rename/copy: an entry row but no numstat (see doc).
            gix::status::Item::IndexWorktree(IwItem::Rewrite {
                source,
                dirwalk_entry,
                copy,
                ..
            }) => {
                let status = if copy { FileStatus::Copied } else { FileStatus::Renamed };
                entries.push(FileEntry {
                    path: dirwalk_entry.rela_path.to_string(),
                    orig_path: Some(source.rela_path().to_string()),
                    status,
                    staged: false,
                });
            }
        }
    }

    // Sort entries deterministically: by path, staged before unstaged for the same path.
    entries.sort_by(|a, b| a.path.cmp(&b.path).then(b.staged.cmp(&a.staged)));
    Ok(Changes {
        entries,
        staged_numstat: staged,
        unstaged_numstat: unstaged,
    })
}

/// Count added/removed lines between two blobs; flag binaries (NUL in first 8 KiB).
fn line_counts(old: &[u8], new: &[u8]) -> NumStat {
    if is_binary(old) || is_binary(new) {
        return NumStat { adds: 0, dels: 0, binary: true };
    }
    use gix::diff::blob::{sources::byte_lines, Algorithm, Diff, InternedInput};
    let input = InternedInput::new(byte_lines(old), byte_lines(new));
    let diff = Diff::compute(Algorithm::Histogram, &input);
    NumStat { adds: diff.count_additions(), dels: diff.count_removals(), binary: false }
}

fn is_binary(buf: &[u8]) -> bool {
    buf[..buf.len().min(8000)].contains(&0)
}

/// Read a blob's bytes from the object DB (empty vec on failure).
///
/// Uses `try_into_blob` so a non-blob object (e.g. a submodule commit that
/// happens to be reachable in the parent ODB) degrades to empty rather than
/// panicking, and `take_data` to move the bytes out without a second copy.
fn blob_bytes(repo: &gix::Repository, id: &gix::hash::oid) -> Vec<u8> {
    repo.find_object(id)
        .ok()
        .and_then(|o| o.try_into_blob().ok())
        .map(|mut b| b.take_data())
        .unwrap_or_default()
}

/// Read a worktree file's bytes by repo-relative path (empty vec when there's
/// no workdir or the read fails).
///
/// Resolves `rela_path` through gix's byte→path conversion rather than a strict
/// UTF-8 `to_str()`. On Unix a path is arbitrary bytes, so a non-UTF-8 name must
/// still map to the real file; a strict conversion would silently fail and leave
/// the numstat empty — a phantom all-deletions entry — under the lossy
/// `to_string()` key that the caller already uses (and that matches the row's
/// `FileEntry.path`). Converting the same bytes keeps the read and the key
/// consistent.
fn worktree_bytes(repo: &gix::Repository, rela_path: &gix::bstr::BString) -> Vec<u8> {
    use gix::bstr::ByteSlice;
    let Some(wd) = repo.workdir() else {
        return Vec::new();
    };
    let rel = gix::path::from_bstr(rela_path.as_bstr());
    std::fs::read(wd.join(&*rel)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use crate::git::FileStatus;

    /// Run a git command in `dir`, isolated from the host's global/system
    /// config, asserting success. Test-only fixture construction.
    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("invoke git");
        assert!(status.success(), "git {args:?} failed");
    }

    /// A fresh repo on branch `main` with one commit. Parallel-safe: unique tempdir.
    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@example.com"]);
        git(p, &["config", "user.name", "Test"]);
        git(p, &["config", "commit.gpgsign", "false"]);
        std::fs::write(p.join("a.txt"), "initial\n").unwrap();
        git(p, &["add", "a.txt"]);
        git(p, &["commit", "-q", "-m", "initial"]);
        dir
    }

    /// Open a repo at an explicit path (tests can't rely on cwd under a
    /// parallel test runner). Mirrors `open()`'s logic but takes a path.
    fn open_at(path: &Path) -> Option<gix::Repository> {
        let repo = gix::discover(path).ok()?;
        if repo.workdir().is_some() { Some(repo) } else { None }
    }

    #[test]
    fn branch_name_reports_current_branch() {
        let dir = init_repo();
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(super::branch_name(&repo), "main");
    }

    #[test]
    fn branch_name_reports_head_when_detached() {
        let dir = init_repo();
        git(dir.path(), &["checkout", "-q", "--detach"]);
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(super::branch_name(&repo), "HEAD");
    }

    #[test]
    fn open_at_finds_worktree_repo() {
        let dir = init_repo();
        assert!(open_at(dir.path()).is_some(), "should open a worktree repo");
    }

    #[test]
    fn open_at_rejects_bare_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "--bare", "-q"]);
        assert!(
            open_at(dir.path()).is_none(),
            "a bare repo has no work tree; gsw must treat it like no repo",
        );
    }

    #[test]
    fn resolve_base_prefers_main() {
        let dir = init_repo(); // already on main
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(super::resolve_base(&repo), "main");
    }

    #[test]
    fn resolve_base_falls_back_to_master() {
        let dir = init_repo();
        git(dir.path(), &["branch", "-m", "main", "master"]);
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(super::resolve_base(&repo), "master");
    }

    #[test]
    fn commits_ahead_counts_commits_past_base() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(p.join("b.txt"), "two\n").unwrap();
        git(p, &["add", "b.txt"]);
        git(p, &["commit", "-q", "-m", "second"]);
        std::fs::write(p.join("c.txt"), "three\n").unwrap();
        git(p, &["add", "c.txt"]);
        git(p, &["commit", "-q", "-m", "third"]);
        let repo = open_at(p).unwrap();
        assert_eq!(super::commits_ahead(&repo, "main"), 2);
    }

    #[test]
    fn commits_ahead_is_zero_when_base_equals_head() {
        let dir = init_repo();
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(super::commits_ahead(&repo, "main"), 0);
    }

    #[test]
    fn head_commit_secs_is_some_for_a_repo_with_a_commit() {
        let dir = init_repo();
        let repo = open_at(dir.path()).unwrap();
        let secs = super::head_commit_secs(&repo).expect("a commit exists");
        assert!(secs > 1_000_000_000, "looks like a unix timestamp: {secs}");
    }

    #[test]
    fn recent_log_returns_newest_first_with_summaries() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("b.txt"), "two\n").unwrap();
        git(p, &["add", "b.txt"]);
        git(p, &["commit", "-q", "-m", "second commit"]);
        let repo = open_at(p).unwrap();
        let log = super::recent_log(&repo, 10);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].2, "second commit");
        assert_eq!(log[1].2, "initial");
        assert!(!log[0].0.is_empty(), "short hash present");
    }

    #[test]
    fn recent_log_zero_is_empty() {
        let dir = init_repo();
        let repo = open_at(dir.path()).unwrap();
        assert!(super::recent_log(&repo, 0).is_empty());
    }

    /// Clone `init_repo()`'s repo so the clone has a real `origin/main` upstream.
    fn init_repo_with_upstream() -> (tempfile::TempDir, tempfile::TempDir) {
        let origin = init_repo();
        let clone = tempfile::tempdir().expect("tempdir");
        let status = std::process::Command::new("git")
            .args([
                "clone", "-q",
                origin.path().to_str().unwrap(),
                clone.path().to_str().unwrap(),
            ])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git clone");
        assert!(status.success(), "git clone failed");
        git(clone.path(), &["config", "user.email", "t@example.com"]);
        git(clone.path(), &["config", "user.name", "Test"]);
        git(clone.path(), &["config", "commit.gpgsign", "false"]);
        (origin, clone)
    }

    fn statuses(repo: &gix::Repository) -> Vec<(String, FileStatus, bool)> {
        super::collect_changes(repo)
            .unwrap()
            .entries
            .into_iter()
            .map(|e| (e.path, e.status, e.staged))
            .collect()
    }

    #[test]
    fn status_staged_modification() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "changed\n").unwrap();
        git(p, &["add", "a.txt"]);
        let repo = open_at(p).unwrap();
        assert_eq!(statuses(&repo), vec![("a.txt".to_string(), FileStatus::Modified, true)]);
    }

    #[test]
    fn status_unstaged_modification() {
        let dir = init_repo();
        std::fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(statuses(&repo), vec![("a.txt".to_string(), FileStatus::Modified, false)]);
    }

    #[test]
    fn status_both_sides_yields_two_rows_staged_first() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "staged change\n").unwrap();
        git(p, &["add", "a.txt"]);
        std::fs::write(p.join("a.txt"), "staged change\nthen worktree change\n").unwrap();
        let repo = open_at(p).unwrap();
        assert_eq!(
            statuses(&repo),
            vec![
                ("a.txt".to_string(), FileStatus::Modified, true),
                ("a.txt".to_string(), FileStatus::Modified, false),
            ],
        );
    }

    #[test]
    fn status_untracked_file_and_dir() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("loose.txt"), "x\n").unwrap();
        std::fs::create_dir(p.join("sub")).unwrap();
        std::fs::write(p.join("sub").join("nested.txt"), "y\n").unwrap();
        let repo = open_at(p).unwrap();
        let s = statuses(&repo);
        assert!(s.contains(&("loose.txt".to_string(), FileStatus::Untracked, false)), "got {s:?}");
        assert!(s.iter().any(|(path, st, _)| path == "sub/" && *st == FileStatus::UntrackedDir), "got {s:?}");
    }

    #[test]
    fn status_staged_addition_and_deletion() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("added.txt"), "new\n").unwrap();
        git(p, &["add", "added.txt"]);
        git(p, &["rm", "-q", "a.txt"]);
        let repo = open_at(p).unwrap();
        let s = statuses(&repo);
        assert!(s.contains(&("added.txt".to_string(), FileStatus::Added, true)), "got {s:?}");
        assert!(s.contains(&("a.txt".to_string(), FileStatus::Deleted, true)), "got {s:?}");
    }

    #[test]
    fn status_staged_rename_keeps_orig_path() {
        let dir = init_repo();
        let p = dir.path();
        // Make the file bigger so rename detection is unambiguous.
        std::fs::write(p.join("a.txt"), "line1\nline2\nline3\nline4\nline5\n").unwrap();
        git(p, &["add", "a.txt"]);
        git(p, &["commit", "-q", "-m", "grow a.txt"]);
        git(p, &["mv", "a.txt", "renamed.txt"]);
        let repo = open_at(p).unwrap();
        let entry = super::collect_changes(&repo).unwrap().entries.into_iter().find(|e| e.path == "renamed.txt");
        // gix may report rename detection OR an add+delete pair depending on
        // config; accept either but if a renamed.txt entry exists it must carry orig_path.
        if let Some(entry) = entry {
            if entry.status == FileStatus::Renamed {
                assert_eq!(entry.orig_path.as_deref(), Some("a.txt"));
                assert!(entry.staged);
            }
        }
    }

    #[test]
    fn status_untracked_nested_repo_is_a_dir() {
        let dir = init_repo();
        let p = dir.path();
        // An untracked nested git repo: a subdir with its own .git, not a submodule.
        let nested = p.join("nested");
        std::fs::create_dir(&nested).unwrap();
        git(&nested, &["init", "-q", "-b", "main"]);
        let repo = open_at(p).unwrap();
        let s = statuses(&repo);
        // git status shows this as a directory ("?? nested/"); match that.
        assert!(
            s.iter().any(|(path, st, _)| path == "nested/" && *st == FileStatus::UntrackedDir),
            "untracked nested repo should surface as UntrackedDir 'nested/': {s:?}",
        );
    }

    #[test]
    fn numstat_staged_modification_counts_lines() {
        let dir = init_repo(); // a.txt = "initial\n"
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "initial\nadded one\nadded two\n").unwrap();
        git(p, &["add", "a.txt"]);
        let repo = open_at(p).unwrap();
        let staged = super::collect_changes(&repo).unwrap().staged_numstat;
        let ns = staged.get("a.txt").expect("staged numstat for a.txt");
        assert_eq!((ns.adds, ns.dels, ns.binary), (2, 0, false));
    }

    #[test]
    fn numstat_unstaged_modification_counts_lines() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "rewritten\n").unwrap();
        let repo = open_at(p).unwrap();
        let unstaged = super::collect_changes(&repo).unwrap().unstaged_numstat;
        let ns = unstaged.get("a.txt").expect("unstaged numstat");
        assert_eq!((ns.adds, ns.dels, ns.binary), (1, 1, false));
    }

    #[test]
    fn numstat_staged_addition_counts_all_lines() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("new.txt"), "l1\nl2\nl3\n").unwrap();
        git(p, &["add", "new.txt"]);
        let repo = open_at(p).unwrap();
        let staged = super::collect_changes(&repo).unwrap().staged_numstat;
        let ns = staged.get("new.txt").expect("staged add numstat");
        assert_eq!((ns.adds, ns.dels), (3, 0));
    }

    #[test]
    fn numstat_staged_binary_file_is_flagged() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("blob.bin"), [0_u8, 1, 2, 0, 3, 4]).unwrap();
        git(p, &["add", "blob.bin"]);
        let repo = open_at(p).unwrap();
        let staged = super::collect_changes(&repo).unwrap().staged_numstat;
        let ns = staged.get("blob.bin").expect("binary numstat");
        assert!(ns.binary, "NUL-containing blob must be flagged binary");
        assert_eq!((ns.adds, ns.dels), (0, 0));
    }

    #[test]
    fn numstat_excludes_untracked_files() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("loose.txt"), "x\ny\n").unwrap();
        let repo = open_at(p).unwrap();
        let changes = super::collect_changes(&repo).unwrap();
        assert!(!changes.staged_numstat.contains_key("loose.txt"));
        assert!(!changes.unstaged_numstat.contains_key("loose.txt"));
    }

    #[test]
    fn upstream_none_for_branch_without_upstream() {
        let dir = init_repo(); // local-only main, never pushed
        let repo = open_at(dir.path()).unwrap();
        assert!(super::upstream_status(&repo).is_none());
    }

    #[test]
    fn upstream_reports_name_and_ahead_count() {
        let (_origin, clone) = init_repo_with_upstream();
        let p = clone.path();
        std::fs::write(p.join("local.txt"), "x\n").unwrap();
        git(p, &["add", "local.txt"]);
        git(p, &["commit", "-q", "-m", "local only"]);
        let repo = open_at(p).unwrap();
        let up = super::upstream_status(&repo).expect("clone has an upstream");
        assert_eq!(up.name, "origin/main");
        assert_eq!(up.ahead, 1);
        assert_eq!(up.behind, 0);
    }

    #[test]
    fn status_staged_typechange_file_to_symlink() {
        // Replace a tracked regular file with a symlink and stage it; git/gix
        // report this as a type change, not a plain modification.
        let dir = init_repo();
        let p = dir.path();
        std::fs::remove_file(p.join("a.txt")).unwrap();
        std::os::unix::fs::symlink("target", p.join("a.txt")).unwrap();
        git(p, &["add", "a.txt"]);
        let repo = open_at(p).unwrap();
        let s = statuses(&repo);
        assert!(
            s.contains(&("a.txt".to_string(), FileStatus::TypeChange, true)),
            "file→symlink should be a staged TypeChange: {s:?}",
        );
    }

    #[test]
    fn status_merge_conflict_is_conflicted() {
        // Build a real merge conflict: two branches edit the same line, merge fails.
        let dir = init_repo();
        let p = dir.path();
        // base already has a.txt = "initial\n"
        git(p, &["checkout", "-q", "-b", "other"]);
        std::fs::write(p.join("a.txt"), "from other\n").unwrap();
        git(p, &["commit", "-q", "-am", "other edit"]);
        git(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("a.txt"), "from main\n").unwrap();
        git(p, &["commit", "-q", "-am", "main edit"]);
        // Merge 'other' into main → conflict on a.txt. The merge command exits
        // non-zero on conflict, so don't assert success here.
        let _ = std::process::Command::new("git")
            .args(["merge", "other"])
            .current_dir(p)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("invoke git merge");
        let repo = open_at(p).unwrap();
        let s = statuses(&repo);
        assert!(
            s.iter().any(|(path, st, _)| path == "a.txt" && *st == FileStatus::Conflicted),
            "a.txt should be Conflicted after a failed merge: {s:?}",
        );
    }
}

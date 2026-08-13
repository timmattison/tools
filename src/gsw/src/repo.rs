//! gix-backed git operations for gsw.
//!
//! gsw is a read-only monitor. Every function here reads the repository
//! in-process via `gix` and never writes the index, so it can never take
//! `.git/index.lock` and can never race a concurrent rebase — the reason the
//! old `git` CLI path needed a private index snapshot.

use std::collections::HashMap;

use crate::git::{FileEntry, FileStatus, NumStat};
use crate::render::{Operation, StepProgress, UpstreamStatus};

/// A repository handle that can be re-opened between reads.
///
/// A [`gix::Repository`] snapshots `.git/config` when it is opened and never
/// reloads it. That is fine for the one-shot path — a fresh process opens a
/// fresh handle — but watch mode holds one handle for the lifetime of the
/// process, so every config key it caches goes stale the moment the user
/// changes it in another pane. The headline symptom is `git push -u origin
/// <branch>`: it writes `branch.<name>.remote` and `branch.<name>.merge`, which
/// is exactly where [`upstream_status`] resolves the tracking ref from, so the
/// `↑0 ↓0 origin/<branch>` header segment never appears until gsw restarts. The
/// mirror (`git branch --unset-upstream` leaving stale arrows on screen), a
/// renamed remote, and a changed `core.excludesFile` are all the same bug.
///
/// Rather than special-case `branch.*`, this handle owns the re-open: callers
/// that want the config as it is *right now* ask for [`reopened`] and get a
/// handle built from a freshly-read config, which fixes the whole class at once.
///
/// [`reopened`]: RepoHandle::reopened
pub struct RepoHandle {
    /// The repository as of the last successful open.
    repo: gix::Repository,
    /// The work-tree root, captured when the repository was first discovered so
    /// a re-open never has to search for it again. See
    /// [`reopened`](RepoHandle::reopened) for why the re-open deliberately does
    /// not re-discover.
    workdir: std::path::PathBuf,
}

impl RepoHandle {
    /// Open the repository containing the current directory, or `None` when
    /// there isn't one with a working tree (outside any repo, or a bare repo —
    /// gsw has nothing per-file to render in either case).
    pub fn open() -> Option<Self> {
        Self::discover(std::path::Path::new("."))
    }

    /// Open the repository containing `path`, walking up from it the way `git`
    /// itself does. Same `None` cases as [`open`](Self::open); this is the
    /// cwd-free form, which is also what the tests use since a parallel test
    /// runner shares one process-wide current directory.
    pub fn discover(path: &std::path::Path) -> Option<Self> {
        Self::from_repo(gix::discover(path).ok()?)
    }

    /// Wrap an opened repository, rejecting one gsw can't render.
    ///
    /// Bare repos have no work tree; gsw renders a per-file working-tree view,
    /// so there's nothing to show. Treat them like "not a repo".
    ///
    /// This is the discovery path only; [`reopened`](Self::reopened) admits a
    /// repository by the same rule — it must have a work tree — but applies it
    /// inline. See there for why the check is stated twice rather than shared.
    fn from_repo(repo: gix::Repository) -> Option<Self> {
        let workdir = repo.workdir()?.to_path_buf();
        Some(Self { repo, workdir })
    }

    /// The repository as it was last opened. Does not re-read `.git/config`, so
    /// two calls with no intervening [`reopened`](Self::reopened) always see the
    /// same configuration.
    pub fn repo(&self) -> &gix::Repository {
        &self.repo
    }

    /// Re-open the repository so configuration written since the last call
    /// takes effect, and return the resulting handle.
    ///
    /// Callers that refresh on a timer (watch mode) use this instead of
    /// [`repo`](Self::repo) so an upstream configured — or unset — in another
    /// pane shows up on the next tick rather than on the next process start.
    ///
    /// The re-open goes through `gix::open` against the captured work-tree
    /// root, deliberately **not** `gix::discover`: `open` resolves `path/.git`
    /// (including the `.git` *file* a linked worktree uses) but never walks up
    /// the directory tree, so if the git dir momentarily vanishes — mid-`git
    /// gc`, mid-checkout, a worktree being pruned — it simply errors instead of
    /// silently latching onto a *parent* repository and rendering someone
    /// else's status.
    ///
    /// A failed re-open, or one that comes back without a work tree, keeps the
    /// handle already in hand: a monitor that blanks out for one tick because
    /// it caught git mid-write is worse than a monitor that repaints one
    /// tick-old configuration and recovers on the next call. The work-tree
    /// check is written out here rather than borrowed from
    /// [`from_repo`](Self::from_repo): only the repository is being replaced,
    /// and building a second work-tree root just to drop it would suggest the
    /// captured one gets refreshed, which is exactly what this must not do.
    /// That leaves one rule stated in two places, which could drift — an
    /// accepted cost, because each side is a single `workdir()` call, small
    /// enough to compare at a glance, and changing one without the other would
    /// leave a handle whose `workdir` field no longer describes what a re-open
    /// will accept.
    ///
    /// That "never a blank screen" property is **not** delivered here alone —
    /// this fallback only guarantees a usable *handle*. Reading a repository
    /// that is mid-`gc`, mid-checkout, or renamed away can still fail on the
    /// status walk performed against the handle it hands back, and watch mode's
    /// `event_loop` is what absorbs *that* failure by re-rendering the last good
    /// snapshot at its true age. Removing either half re-breaks the guarantee:
    /// drop this fallback and a momentary re-open failure loses the
    /// configuration; drop the loop's absorption and the same momentary failure
    /// ends watch mode outright, which is precisely the bug this pairing was
    /// written to close.
    pub fn reopened(&mut self) -> &gix::Repository {
        if let Some(fresh) = gix::open(&self.workdir)
            .ok()
            .filter(|repo| repo.workdir().is_some())
        {
            self.repo = fresh;
        }
        &self.repo
    }
}

/// What [`branch_name`] reports when HEAD is detached, matching what
/// `git rev-parse --abbrev-ref HEAD` prints.
///
/// Usable as a sentinel — "this is not a branch" — because git rejects `HEAD`
/// as a branch name (`git branch HEAD` fails), so no real branch can ever carry
/// it. Named rather than spelled inline at each site so the two readers
/// ([`branch_name`] and the push planner, which must refuse to push a detached
/// HEAD) are tied to one definition instead of two matching string literals.
pub const DETACHED_HEAD: &str = "HEAD";

/// The short current-branch name (e.g. `main`), or [`DETACHED_HEAD`] when
/// detached — matching what `git rev-parse --abbrev-ref HEAD` prints.
pub fn branch_name(repo: &gix::Repository) -> String {
    match repo.head_name() {
        Ok(Some(full)) => full.shorten().to_string(),
        _ => DETACHED_HEAD.to_string(),
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

/// How HEAD relates to its base ref, as a pair of commit counts. See
/// [`base_status`].
pub struct BaseStatus {
    /// Commits reachable from HEAD but not from the base
    /// (`git rev-list --count base..HEAD`).
    pub ahead: u32,
    /// Commits reachable from the base but not from HEAD
    /// (`git rev-list --count HEAD..base`) — i.e. how far behind the base HEAD
    /// is, the needs-rebase signal.
    pub behind: u32,
}

/// Count how far HEAD is ahead of and behind its `base` ref.
///
/// `ahead` is the number of commits reachable from HEAD but not from `base`
/// (`git rev-list --count base..HEAD`); `behind` is the mirror — commits
/// reachable from `base` but not from HEAD (`git rev-list --count HEAD..base`),
/// which is nonzero when the base has moved on past the fork point and HEAD
/// needs a rebase.
///
/// Any resolution or walk failure degrades to `BaseStatus { ahead: 0, behind:
/// 0 }`, so a missing or unresolvable base produces no behind segment. When
/// HEAD already points at the base commit the walks are short-circuited to
/// `(0, 0)`. Each count is clamped to `u32::MAX`.
pub fn base_status(repo: &gix::Repository, base: &str) -> BaseStatus {
    let resolve = || -> Option<(u32, u32)> {
        let head = repo.head_id().ok()?.detach();
        let base_id = repo.rev_parse_single(base).ok()?.detach();
        ahead_behind(repo, head, base_id)
    };
    let (ahead, behind) = resolve().unwrap_or((0, 0));
    BaseStatus { ahead, behind }
}

/// Count how far `ours` is ahead of and behind `theirs` as `(ahead, behind)`.
///
/// `ahead` is the number of commits reachable from `ours` but not from `theirs`
/// (`git rev-list --count theirs..ours`); `behind` is the mirror — commits
/// reachable from `theirs` but not from `ours` (`git rev-list --count
/// ours..theirs`). Each count is clamped to `u32::MAX`.
///
/// Returns `None` if either rev walk fails. When `ours == theirs` the walks are
/// short-circuited to `Some((0, 0))` (the walks would return `(0, 0)` anyway).
/// Both `base_status` and `upstream_status` delegate here so the mirrored
/// hidden-walk pair lives in exactly one place.
fn ahead_behind(
    repo: &gix::Repository,
    ours: gix::ObjectId,
    theirs: gix::ObjectId,
) -> Option<(u32, u32)> {
    if ours == theirs {
        return Some((0, 0));
    }
    // ahead: theirs..ours — commits on `ours` not on `theirs`.
    let ahead = repo
        .rev_walk(std::iter::once(ours))
        .with_hidden(std::iter::once(theirs))
        .all()
        .ok()?
        .count();
    // behind: ours..theirs — the mirror walk, `theirs` with `ours` hidden.
    let behind = repo
        .rev_walk(std::iter::once(theirs))
        .with_hidden(std::iter::once(ours))
        .all()
        .ok()?
        .count();
    Some((
        u32::try_from(ahead).unwrap_or(u32::MAX),
        u32::try_from(behind).unwrap_or(u32::MAX),
    ))
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

    let (ahead, behind) = ahead_behind(repo, head_id, upstream_id)?;

    Some(UpstreamStatus {
        name,
        ahead,
        behind,
    })
}

/// The remote to publish a branch that has no upstream to, or `None` when the
/// repository gives no unambiguous answer.
///
/// Only consulted for a branch with no upstream. A branch that already tracks
/// something is pushed with a bare `git push`, which reads the remote out of
/// the branch config — so this never has to second-guess a tracking branch.
///
/// Read fresh on every walk, like every other configuration gsw renders: a
/// remote added in another pane takes effect on the next refresh rather than at
/// the next restart.
pub fn push_remote(repo: &gix::Repository) -> Option<String> {
    use gix::bstr::ByteSlice;

    let names: Vec<String> = repo
        .remote_names()
        .iter()
        .filter_map(|name| name.to_str().ok().map(str::to_owned))
        .collect();

    // The config snapshot borrows the repository, so the picked name is cloned
    // out before it is dropped at the end of this scope.
    let config = repo.config_snapshot();
    let push_default = config
        .string("remote.pushDefault")
        .and_then(|value| value.to_str().ok().map(str::to_owned));

    pick_push_remote(&names, push_default.as_deref())
}

/// Pure core of [`push_remote`]: pick the remote from the names the repository
/// has and whatever `remote.pushDefault` says.
///
/// The rules, in order:
///
/// 1. **`remote.pushDefault` wins outright**, and is taken verbatim — even when
///    it names no configured remote. That setting exists precisely so the user
///    can override the default, and git itself is the authority on whether the
///    name resolves. Second-guessing it here would mean pushing somewhere the
///    user did not configure. A bogus value fails in `git push`, where the error
///    says what is actually wrong.
/// 2. **Exactly one remote** is the answer whatever it is called, so a
///    repository whose only remote is `fork` is not told there is no remote.
/// 3. **`origin` among several**, matching what every other tool assumes.
/// 4. Otherwise `None`. Several remotes, none named `origin`, and no
///    `pushDefault` is a genuine ambiguity, and guessing would publish a branch
///    to a remote the user never named.
fn pick_push_remote(names: &[String], push_default: Option<&str>) -> Option<String> {
    if let Some(configured) = push_default {
        return Some(configured.to_string());
    }
    match names {
        [only] => Some(only.clone()),
        _ => names.iter().find(|name| *name == ORIGIN).cloned(),
    }
}

/// The remote name every git tool assumes when a repository has several and the
/// user has expressed no preference.
const ORIGIN: &str = "origin";

/// The in-progress git operation gsw should surface in the header, or `None`
/// for a clean tree or an out-of-scope operation.
///
/// Classification uses gix's native [`gix::Repository::state`], which is modeled
/// on git's own `wt-status.c` / `git-prompt.sh` logic: it inspects `MERGE_HEAD`,
/// `rebase-merge/`, and `rebase-apply/` under the git dir, so it is
/// worktree-aware and takes no locks — consistent with gsw's read-only,
/// gix-only philosophy. `conflicts` is the unmerged-path count the caller
/// already has from the status walk, so this does no extra git work.
///
/// Every rebase flavor collapses to `Operation::Rebase`: `ApplyMailboxRebase`
/// is gix's name for a bare `rebase-apply/` directory carrying neither the
/// `applying` nor the `rebasing` marker, which cannot be told apart from an
/// apply-backend rebase, so it is treated as one. Cherry-pick, revert, bisect,
/// and plain `git am` are intentionally out of scope and yield `None`.
pub fn operation_state(repo: &gix::Repository, conflicts: u32) -> Option<Operation> {
    use gix::state::InProgress;

    match repo.state()? {
        InProgress::Merge => Some(Operation::Merge { conflicts }),
        InProgress::Rebase | InProgress::RebaseInteractive | InProgress::ApplyMailboxRebase => {
            Some(Operation::Rebase {
                step: rebase_step(repo.path()),
                conflicts,
            })
        }
        InProgress::ApplyMailbox
        | InProgress::CherryPick
        | InProgress::CherryPickSequence
        | InProgress::Revert
        | InProgress::RevertSequence
        | InProgress::Bisect => None,
    }
}

/// Both rebase step-counter pairs, in the order [`gix::Repository::state`]
/// resolves the directories they live in: `rebase-apply/` before
/// `rebase-merge/`. See [`rebase_step`], which reads them, for why the order
/// is load-bearing.
///
/// This is the single source of truth for both that order and the file names
/// themselves. The design spec
/// (`specs/2026-07-01-gsw-rebase-merge-indicators-design.md`) restates them in
/// prose. That restatement is not checked automatically, so update it by hand
/// whenever this table changes.
const REBASE_COUNTERS: [(&str, &str); 2] = [
    ("rebase-apply/next", "rebase-apply/last"),
    ("rebase-merge/msgnum", "rebase-merge/end"),
];

/// How far through a rebase git is, or `None` when the counters cannot be read.
///
/// gix classifies the operation but does not expose its progress, so the two
/// counter pairs git itself writes are read straight out of `git_dir` — the
/// same base [`gix::Repository::state`] inspects, which keeps this worktree-
/// aware — exactly as git's own prompt does:
///
/// - `rebase-apply/next` + `rebase-apply/last` — the apply backend
///   (`git rebase --apply`).
/// - `rebase-merge/msgnum` + `rebase-merge/end` — the merge backend, used by
///   both plain and interactive rebases.
///
/// They are listed — and tried — in the order [`gix::Repository::state`] itself
/// resolves them, `rebase-apply/` before `rebase-merge/`, so the step counts
/// can only ever come from the directory the classification came from. git
/// never leaves both directories present at once, but pinning the order means
/// the two can't disagree even if it did.
///
/// A missing, unreadable, or unparseable counter degrades to `None` rather than
/// failing the whole indicator: the operation is still worth surfacing without
/// its `current/total` clause.
fn rebase_step(git_dir: &std::path::Path) -> Option<StepProgress> {
    let read = |name: &str| -> Option<u32> {
        std::fs::read_to_string(git_dir.join(name))
            .ok()?
            .trim()
            .parse()
            .ok()
    };

    REBASE_COUNTERS.iter().find_map(|&(current, total)| {
        Some(StepProgress {
            current: read(current)?,
            total: read(total)?,
        })
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
                    let status = if copy {
                        FileStatus::Copied
                    } else {
                        FileStatus::Renamed
                    };
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
                let status = if copy {
                    FileStatus::Copied
                } else {
                    FileStatus::Renamed
                };
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
        return NumStat {
            adds: 0,
            dels: 0,
            binary: true,
        };
    }
    use gix::diff::blob::{sources::byte_lines, Algorithm, Diff, InternedInput};
    let input = InternedInput::new(byte_lines(old), byte_lines(new));
    let diff = Diff::compute(Algorithm::Histogram, &input);
    NumStat {
        adds: diff.count_additions(),
        dels: diff.count_removals(),
        binary: false,
    }
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

    use tempfile::TempDir;

    use super::RepoHandle;
    use crate::git::FileStatus;
    use crate::render::{Operation, StepProgress};
    use crate::testrepo::{
        git, git_allowing_failure, init_repo, init_repo_with_upstream, init_repo_with_worktree,
    };

    /// Open a repo at an explicit path (tests can't rely on cwd under a
    /// parallel test runner).
    ///
    /// Delegates to [`RepoHandle::discover`] — the same discovery and
    /// bare-repo rejection production uses — and unwraps to the bare
    /// repository, which is what the assertions below want. Tests that need to
    /// re-open mid-test hold a [`RepoHandle`] instead.
    fn open_at(path: &Path) -> Option<gix::Repository> {
        RepoHandle::discover(path).map(|handle| handle.repo)
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
    fn resolve_base_follows_a_base_branch_created_after_the_handle_was_opened() {
        // `resolve_base` picks the base ref by *resolving* candidates rather
        // than by reading a config key, so — unlike `upstream_status`, whose
        // `branch.<name>.remote` lookup is exactly what the per-refresh re-open
        // exists to un-stale — it was expected to already be immune to a
        // long-lived handle. Expected is not verified: if gix ever snapshotted
        // the ref store the way it snapshots `.git/config`, a `main` branch
        // created mid-watch would leave the header comparing against `master`
        // forever, silently reporting the wrong ahead/behind counts against a
        // base the user abandoned. Assert the property on a handle that is
        // deliberately never re-opened, so the test fails if that immunity
        // is ever lost rather than being propped up by the re-open.
        let dir = init_repo();
        let p = dir.path();
        git(p, &["branch", "-m", "main", "master"]);

        // Opened while `master` is the only candidate, and held across the
        // creation of `main` — no `reopened()` anywhere in this test.
        let held = RepoHandle::discover(p).expect("fixture is a worktree repo");
        assert_eq!(
            super::resolve_base(held.repo()),
            "master",
            "with no `main`, the base falls back to `master`",
        );

        // What `git branch main` (or a fetch that lands one) does mid-watch.
        git(p, &["branch", "main"]);

        assert_eq!(
            super::resolve_base(held.repo()),
            "main",
            "`main` outranks `master`, and a ref-driven resolve must see one \
             created after the handle was opened — without re-opening it",
        );
    }

    #[test]
    fn base_status_reports_behind_when_base_advances_past_fork_point() {
        // Fork `feature` off main, advance both: feature gets one commit, then
        // main gets one commit. From feature's view the base (main) has moved
        // on, so feature is 1 ahead and 1 behind.
        let dir = init_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(p.join("b.txt"), "two\n").unwrap();
        git(p, &["add", "b.txt"]);
        git(p, &["commit", "-q", "-m", "feature work"]);
        git(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("d.txt"), "main moved\n").unwrap();
        git(p, &["add", "d.txt"]);
        git(p, &["commit", "-q", "-m", "main moved on"]);
        git(p, &["checkout", "-q", "feature"]);
        let repo = open_at(p).unwrap();
        let status = super::base_status(&repo, "main");
        assert_eq!(
            status.ahead, 1,
            "feature has one commit past the fork point"
        );
        assert_eq!(
            status.behind, 1,
            "main moved one commit past the fork point"
        );
    }

    #[test]
    fn base_status_counts_commits_past_base() {
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
        let status = super::base_status(&repo, "main");
        assert_eq!(status.ahead, 2);
        assert_eq!(status.behind, 0, "base has not moved");
    }

    #[test]
    fn base_status_is_zero_when_base_equals_head() {
        let dir = init_repo();
        let repo = open_at(dir.path()).unwrap();
        let status = super::base_status(&repo, "main");
        assert_eq!((status.ahead, status.behind), (0, 0));
    }

    #[test]
    fn base_status_is_zero_when_base_unresolvable() {
        let dir = init_repo();
        let repo = open_at(dir.path()).unwrap();
        let status = super::base_status(&repo, "no-such-branch");
        assert_eq!((status.ahead, status.behind), (0, 0));
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
        assert_eq!(
            statuses(&repo),
            vec![("a.txt".to_string(), FileStatus::Modified, true)]
        );
    }

    #[test]
    fn status_unstaged_modification() {
        let dir = init_repo();
        std::fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(
            statuses(&repo),
            vec![("a.txt".to_string(), FileStatus::Modified, false)]
        );
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
        assert!(
            s.contains(&("loose.txt".to_string(), FileStatus::Untracked, false)),
            "got {s:?}"
        );
        assert!(
            s.iter()
                .any(|(path, st, _)| path == "sub/" && *st == FileStatus::UntrackedDir),
            "got {s:?}"
        );
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
        assert!(
            s.contains(&("added.txt".to_string(), FileStatus::Added, true)),
            "got {s:?}"
        );
        assert!(
            s.contains(&("a.txt".to_string(), FileStatus::Deleted, true)),
            "got {s:?}"
        );
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
        let entry = super::collect_changes(&repo)
            .unwrap()
            .entries
            .into_iter()
            .find(|e| e.path == "renamed.txt");
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
            s.iter()
                .any(|(path, st, _)| path == "nested/" && *st == FileStatus::UntrackedDir),
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
    fn upstream_reports_behind_count_when_remote_advances() {
        // Advance the origin by one commit, fetch it into the clone without
        // moving the clone's HEAD: the clone is now 1 behind origin/main and 0
        // ahead. Locks in the behind direction (previously only covered by the
        // integration test) before the ahead_behind helper extraction.
        let (origin, clone) = init_repo_with_upstream();
        let op = origin.path();
        std::fs::write(op.join("remote.txt"), "x\n").unwrap();
        git(op, &["add", "remote.txt"]);
        git(op, &["commit", "-q", "-m", "remote moved on"]);
        let p = clone.path();
        git(p, &["fetch", "-q"]);
        let repo = open_at(p).unwrap();
        let up = super::upstream_status(&repo).expect("clone has an upstream");
        assert_eq!(up.name, "origin/main");
        assert_eq!(up.ahead, 0, "clone has no local commits past origin");
        assert_eq!(up.behind, 1, "origin advanced one commit past the clone");
    }

    #[test]
    fn reopened_handle_sees_upstream_configured_after_open() {
        // Watch mode's failure shape: gsw opens the repository once, and only
        // later does the user run `git push -u origin <branch>` in another
        // pane. That writes `branch.feature.remote`/`.merge` into
        // `.git/config`, which the already-open gix handle has cached and will
        // never re-read — so the header's upstream segment stays missing until
        // the process restarts. A handle that re-opens must see it.
        let (_origin, clone) = init_repo_with_upstream();
        let p = clone.path();
        git(p, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(p.join("feature.txt"), "x\n").unwrap();
        git(p, &["add", "feature.txt"]);
        git(p, &["commit", "-q", "-m", "feature work"]);

        // Opened BEFORE the push, and held across it, exactly like watch mode.
        let mut held = RepoHandle::discover(p).expect("clone is a worktree repo");
        assert!(
            super::upstream_status(held.repo()).is_none(),
            "a local-only branch has no upstream yet",
        );

        git(p, &["push", "-q", "-u", "origin", "feature"]);

        let up = super::upstream_status(held.reopened())
            .expect("a re-opened handle must see the upstream configured after it was opened");
        assert_eq!(up.name, "origin/feature");
        assert_eq!(
            (up.ahead, up.behind),
            (0, 0),
            "the push left the branch level with its brand-new upstream",
        );
    }

    /// Read the `gsw.probe` key out of the repository's config as it stands
    /// *right now*, owned so the snapshot's borrow ends with the call.
    ///
    /// The key is one git itself never consults, so a non-`None` answer can only
    /// have come from a fresh read of the config file the handle resolves to —
    /// exactly the observation the re-open is supposed to make possible.
    fn probe(repo: &gix::Repository) -> Option<String> {
        repo.config_snapshot()
            .string("gsw.probe")
            .map(|value| value.to_string())
    }

    #[test]
    fn reopened_handle_in_a_linked_worktree_sees_config_written_after_open() {
        // Nearly all work on this repository happens in a linked worktree
        // (`nwt`), so that is where gsw's watch mode actually runs, and it is
        // the one layout whose work-tree root holds a `.git` *file* pointing at
        // `<repo>/.git/worktrees/<name>` rather than a `.git` directory.
        // `reopened()` re-opens with `gix::open` — chosen over `gix::discover`
        // precisely because it resolves that pointer without walking up — so if
        // resolution ever regressed, `open` would simply fail, the handle would
        // fall back to the stale repository it already holds, and every config
        // change made mid-watch (`git push -u origin <branch>` above all) would
        // go unseen until restart, with nothing on screen to say so. The rest
        // of the re-open tests run against a plain repo or a clone, where a
        // broken pointer costs nothing.
        //
        // This guard is not red-first: the property already holds. It exists so
        // a gix upgrade or a later refactor cannot turn the #334 fix into a
        // no-op in the environment it is used in most while the suite stays
        // green.
        let (_repo, linked) = init_repo_with_worktree();

        // The fixture only proves anything if it really is a linked worktree —
        // a plain clone would exercise the `.git`-directory path and satisfy
        // every assertion below while covering none of the pointer resolution.
        assert!(
            linked.join(".git").is_file(),
            "a linked worktree's `.git` is a gitdir pointer file, not a directory",
        );

        // Opened BEFORE the config write and held across it, like watch mode.
        let mut held = RepoHandle::discover(&linked).expect("linked worktree is a worktree repo");
        assert_ne!(
            held.repo().git_dir(),
            held.repo().common_dir(),
            "a linked worktree's per-worktree git dir lives under the main repo's common dir",
        );
        assert_eq!(probe(held.repo()), None, "nothing has written the key yet");

        // What any `git config` run in another pane does mid-watch. From a
        // linked worktree this lands in the *common* `.git/config`, which is
        // reachable only by following the gitdir pointer.
        git(&linked, &["config", "gsw.probe", "written-after-open"]);

        assert_eq!(
            probe(held.repo()),
            None,
            "the handle opened before the write still has the old config cached",
        );
        assert_eq!(
            probe(held.reopened()).as_deref(),
            Some("written-after-open"),
            "re-opening from a work-tree root whose `.git` is a pointer file must \
             still re-read the configuration it points at",
        );
    }

    #[test]
    fn reopened_keeps_the_previous_handle_when_the_repo_is_momentarily_gone() {
        // Watch mode calls `reopened()` on a timer, so it will eventually land
        // in the middle of a `git gc`, a checkout, or a worktree prune and find
        // the git dir missing. Falling back to the handle already in hand keeps
        // the monitor rendering; going blank (or panicking) for a tick would be
        // worse than repainting one tick-old configuration.
        //
        // This guard cannot be written red-first: before `reopened()` re-opened
        // anything it returned the cached handle unconditionally and passed
        // trivially. It ships with the branch it protects.
        let dir = init_repo();
        let p = dir.path();
        let mut held = RepoHandle::discover(p).expect("worktree repo");
        let git_dir = held.repo().git_dir().to_path_buf();

        std::fs::rename(p.join(".git"), p.join(".git-moved")).unwrap();
        assert_eq!(
            held.reopened().git_dir(),
            git_dir,
            "a failed re-open must keep the previous handle, not drop the repo",
        );

        std::fs::rename(p.join(".git-moved"), p.join(".git")).unwrap();
        assert_eq!(
            super::branch_name(held.reopened()),
            "main",
            "and the next re-open must recover once the git dir is back",
        );
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
        git_allowing_failure(p, &["merge", "other"]);
        let repo = open_at(p).unwrap();
        let s = statuses(&repo);
        assert!(
            s.iter()
                .any(|(path, st, _)| path == "a.txt" && *st == FileStatus::Conflicted),
            "a.txt should be Conflicted after a failed merge: {s:?}",
        );
    }

    #[test]
    fn operation_state_reports_merge_with_conflict_count() {
        // A real in-progress merge (MERGE_HEAD present) must classify as
        // Operation::Merge, carrying the caller-supplied conflict count.
        let dir = init_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "other"]);
        std::fs::write(p.join("a.txt"), "from other\n").unwrap();
        git(p, &["commit", "-q", "-am", "other edit"]);
        git(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("a.txt"), "from main\n").unwrap();
        git(p, &["commit", "-q", "-am", "main edit"]);
        // The merge exits non-zero on conflict; don't assert success.
        git_allowing_failure(p, &["merge", "other"]);
        let repo = open_at(p).unwrap();
        assert_eq!(
            super::operation_state(&repo, 1),
            Some(Operation::Merge { conflicts: 1 }),
        );
    }

    #[test]
    fn operation_state_is_none_for_clean_tree() {
        let dir = init_repo();
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(super::operation_state(&repo, 0), None);
    }

    /// A repo whose `feature` branch holds two commits, the first of which
    /// conflicts with `main`'s edit to the same line of `a.txt`. HEAD is left
    /// on `feature`, so rebasing onto `main` stops on step 1 of 2 with `a.txt`
    /// unmerged — the shape every in-progress-rebase assertion below needs.
    fn diverged_repo() -> TempDir {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(p.join("a.txt"), "from feature\n").expect("write a.txt");
        git(p, &["commit", "-q", "-am", "feature edit"]);
        std::fs::write(p.join("b.txt"), "second\n").expect("write b.txt");
        git(p, &["add", "b.txt"]);
        git(p, &["commit", "-q", "-m", "feature second"]);
        git(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("a.txt"), "from main\n").expect("write a.txt");
        git(p, &["commit", "-q", "-am", "main edit"]);
        git(p, &["checkout", "-q", "feature"]);
        dir
    }

    /// [`diverged_repo`] with `feature`'s conflicting commit turned into a
    /// mailbox patch and applied on `main`, where it fails — leaving a `git am`
    /// in progress (a `rebase-apply/` directory carrying the `applying`
    /// marker). HEAD is left on `main`.
    fn failed_am_repo() -> TempDir {
        let dir = diverged_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "main"]);
        git(p, &["format-patch", "-1", "-o", "patches", "feature~1"]);
        let patch = std::fs::read_dir(p.join("patches"))
            .expect("read patches dir")
            .next()
            .expect("format-patch wrote a patch")
            .expect("patch dir entry")
            .path();
        // `git am` exits non-zero when the patch does not apply — that failure
        // *is* the fixture.
        git_allowing_failure(p, &["am", patch.to_str().expect("utf-8 patch path")]);
        dir
    }

    #[test]
    fn operation_state_reports_rebase_with_step_counts_and_conflict_count() {
        let dir = diverged_repo();
        let p = dir.path();
        // The rebase stops on the first of the two commits; it exits non-zero.
        git_allowing_failure(p, &["rebase", "main"]);
        let repo = open_at(p).unwrap();
        assert_eq!(
            super::operation_state(&repo, 1),
            Some(Operation::Rebase {
                step: Some(StepProgress {
                    current: 1,
                    total: 2,
                }),
                conflicts: 1,
            }),
        );
    }

    #[test]
    fn operation_state_reports_rebase_inside_a_linked_worktree() {
        // A rebase started in a *linked* worktree writes its state to that
        // worktree's own git dir — `<repo>/.git/worktrees/<name>/rebase-merge/`
        // — not to the shared common dir every worktree of the repo points at.
        // `rebase_step` therefore has to read `repo.path()` (the per-worktree
        // git dir, the same base `gix::Repository::state` classifies from) and
        // never the common dir. Resolving the common dir instead would still
        // classify the rebase correctly while silently finding no counters, so
        // every worktree user would lose the `current/total` clause with the
        // suite staying green — and this repository mandates that all work
        // happen in linked worktrees, so that is gsw's main path, not an edge
        // case.
        //
        // This guard is not red-first: the property already holds. It exists so
        // a later refactor of the git-dir resolution cannot quietly drop it.
        let (dir, linked) = init_repo_with_worktree();
        let p = dir.path();

        // The fixture only proves anything if it really is a linked worktree —
        // in a plain repo the per-worktree and common git dirs are the same
        // path, so reading either would satisfy the assertion below.
        assert!(
            linked.join(".git").is_file(),
            "a linked worktree's `.git` is a gitdir pointer file, not a directory",
        );

        // `diverged_repo`'s shape, built across the two worktrees: the linked
        // worktree's branch gets two commits, the first of which conflicts with
        // `main`'s edit to the same line of `a.txt`, so rebasing `linked` onto
        // `main` stops on step 1 of 2 with `a.txt` unmerged.
        std::fs::write(linked.join("a.txt"), "from linked\n").expect("write a.txt");
        git(&linked, &["commit", "-q", "-am", "linked edit"]);
        std::fs::write(linked.join("b.txt"), "second\n").expect("write b.txt");
        git(&linked, &["add", "b.txt"]);
        git(&linked, &["commit", "-q", "-m", "linked second"]);
        std::fs::write(p.join("a.txt"), "from main\n").expect("write a.txt");
        git(p, &["commit", "-q", "-am", "main edit"]);

        // Run the rebase *inside* the linked worktree; it exits non-zero when
        // it stops on the conflict.
        git_allowing_failure(&linked, &["rebase", "main"]);

        let repo = open_at(&linked).expect("linked worktree is a worktree repo");
        assert_ne!(
            repo.git_dir(),
            repo.common_dir(),
            "the rebase state must live in a git dir distinct from the common one",
        );
        assert_eq!(
            super::operation_state(&repo, 1),
            Some(Operation::Rebase {
                step: Some(StepProgress {
                    current: 1,
                    total: 2,
                }),
                conflicts: 1,
            }),
        );
    }

    #[test]
    fn operation_state_reports_rebase_for_the_apply_backend() {
        // `git rebase --apply` uses the apply backend, which records its state
        // in `rebase-apply/` (with the `rebasing` marker) and counts steps in
        // `next`/`last` rather than `msgnum`/`end`.
        let dir = diverged_repo();
        let p = dir.path();
        git_allowing_failure(p, &["rebase", "--apply", "main"]);
        let repo = open_at(p).unwrap();
        assert_eq!(
            super::operation_state(&repo, 1),
            Some(Operation::Rebase {
                step: Some(StepProgress {
                    current: 1,
                    total: 2,
                }),
                conflicts: 1,
            }),
        );
    }

    #[test]
    fn rebase_step_prefers_the_directory_gix_classified_from() {
        // Pins the precedence, not just the happy path: `gix::Repository::state`
        // resolves `rebase-apply/` *before* `rebase-merge/`, so when both
        // directories exist the step counts must come from the same directory
        // the classification did. The `rebase-merge/` decoy below is deliberate
        // — git never leaves both present at once, so only a hand-built one can
        // catch the counters being read in the opposite order from the state.
        let dir = diverged_repo();
        let p = dir.path();
        git_allowing_failure(p, &["rebase", "--apply", "main"]);
        assert!(
            p.join(".git/rebase-apply/rebasing").exists(),
            "the apply backend must have left its `rebasing` marker, which is \
             what makes gix classify from `rebase-apply/`",
        );
        let decoy = p.join(".git/rebase-merge");
        std::fs::create_dir_all(&decoy).expect("create rebase-merge decoy");
        std::fs::write(decoy.join("msgnum"), "7\n").expect("write decoy msgnum");
        std::fs::write(decoy.join("end"), "9\n").expect("write decoy end");
        let repo = open_at(p).unwrap();
        assert_eq!(
            super::operation_state(&repo, 1),
            Some(Operation::Rebase {
                step: Some(StepProgress {
                    current: 1,
                    total: 2,
                }),
                conflicts: 1,
            }),
        );
    }

    #[test]
    fn operation_state_reports_rebase_without_steps_when_counter_files_are_absent() {
        // Graceful degradation: the rebase is still surfaced when its step
        // counters cannot be read, just without the `current/total` clause.
        let dir = diverged_repo();
        let p = dir.path();
        git_allowing_failure(p, &["rebase", "main"]);
        std::fs::remove_file(p.join(".git/rebase-merge/msgnum")).expect("remove msgnum");
        std::fs::remove_file(p.join(".git/rebase-merge/end")).expect("remove end");
        let repo = open_at(p).unwrap();
        assert_eq!(
            super::operation_state(&repo, 1),
            Some(Operation::Rebase {
                step: None,
                conflicts: 1,
            }),
        );
    }

    #[test]
    fn operation_state_reports_rebase_without_steps_when_counters_are_unparseable() {
        let dir = diverged_repo();
        let p = dir.path();
        git_allowing_failure(p, &["rebase", "main"]);
        std::fs::write(p.join(".git/rebase-merge/msgnum"), "not-a-number\n").expect("write msgnum");
        let repo = open_at(p).unwrap();
        assert_eq!(
            super::operation_state(&repo, 1),
            Some(Operation::Rebase {
                step: None,
                conflicts: 1,
            }),
        );
    }

    #[test]
    fn operation_state_treats_an_ambiguous_rebase_apply_dir_as_a_rebase() {
        // A `rebase-apply/` directory carrying neither the `applying` nor the
        // `rebasing` marker is gix's `ApplyMailboxRebase`: it cannot be told
        // apart from an apply-backend rebase, so it is surfaced as one.
        let dir = failed_am_repo();
        let p = dir.path();
        std::fs::remove_file(p.join(".git/rebase-apply/applying")).expect("remove applying marker");
        let repo = open_at(p).unwrap();
        assert_eq!(
            super::operation_state(&repo, 0),
            Some(Operation::Rebase {
                step: Some(StepProgress {
                    current: 1,
                    total: 1,
                }),
                conflicts: 0,
            }),
        );
    }

    #[test]
    fn operation_state_is_none_for_plain_git_am() {
        // Applying a mailbox is not a rebase and is out of scope, so it gets no
        // indicator — even though it shares the `rebase-apply/` directory that
        // the apply-backend rebase uses.
        let dir = failed_am_repo();
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(super::operation_state(&repo, 0), None);
    }

    #[test]
    fn operation_state_is_none_for_cherry_pick() {
        // Out of scope: a cherry-pick conflict must not be reported as a merge
        // or a rebase. A clean repo cannot catch that mis-mapping — it has no
        // in-progress state at all — so this fixture is the real guard.
        let dir = diverged_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "main"]);
        git_allowing_failure(p, &["cherry-pick", "feature~1"]);
        let repo = open_at(p).unwrap();
        assert_eq!(super::operation_state(&repo, 1), None);
    }
}

#[cfg(test)]
mod push_remote_tests {
    use super::{pick_push_remote, push_remote, RepoHandle};
    use crate::testrepo::{git, init_repo, init_repo_with_upstream};

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn the_only_remote_is_the_answer_whatever_it_is_called() {
        // A repo whose single remote is `fork` must not be told it has no
        // remote just because the name is not `origin`.
        assert_eq!(
            pick_push_remote(&names(&["fork"]), None),
            Some("fork".to_string()),
        );
    }

    #[test]
    fn origin_wins_among_several_remotes() {
        assert_eq!(
            pick_push_remote(&names(&["upstream", "origin", "fork"]), None),
            Some("origin".to_string()),
        );
    }

    #[test]
    fn several_remotes_without_origin_are_ambiguous() {
        // Guessing here would publish a branch to a remote the user never
        // named. Refusing sends them to `git push` with their own choice.
        assert_eq!(pick_push_remote(&names(&["upstream", "fork"]), None), None);
    }

    #[test]
    fn no_remote_at_all_has_no_answer() {
        assert_eq!(pick_push_remote(&[], None), None);
    }

    #[test]
    fn push_default_overrides_origin() {
        // The setting exists to override the default. Ignoring it would push to
        // `origin` while the user's own config says otherwise.
        assert_eq!(
            pick_push_remote(&names(&["origin", "upstream"]), Some("upstream")),
            Some("upstream".to_string()),
        );
    }

    #[test]
    fn push_default_resolves_an_otherwise_ambiguous_repository() {
        assert_eq!(
            pick_push_remote(&names(&["alpha", "beta"]), Some("beta")),
            Some("beta".to_string()),
        );
    }

    #[test]
    fn push_default_is_taken_verbatim_even_when_it_names_nothing() {
        // git is the authority on whether the name resolves. Falling back to
        // `origin` here would push somewhere the user did not configure, and
        // the fallback would be invisible. `git push` reports the real error.
        assert_eq!(
            pick_push_remote(&names(&["origin"]), Some("typo")),
            Some("typo".to_string()),
        );
    }

    #[test]
    fn a_clone_reports_its_origin() {
        // The end-to-end read, against a repository that really has a remote.
        let (_origin, clone) = init_repo_with_upstream();
        let handle = RepoHandle::discover(clone.path()).expect("clone is a worktree repo");
        assert_eq!(push_remote(handle.repo()), Some("origin".to_string()));
    }

    #[test]
    fn a_repository_with_no_remote_reports_none() {
        let dir = init_repo();
        let handle = RepoHandle::discover(dir.path()).expect("fixture is a worktree repo");
        assert_eq!(push_remote(handle.repo()), None);
    }

    #[test]
    fn a_repository_reports_its_only_remote_by_name() {
        let dir = init_repo();
        git(
            dir.path(),
            &["remote", "add", "fork", "https://example.invalid/x.git"],
        );
        let handle = RepoHandle::discover(dir.path()).expect("fixture is a worktree repo");
        assert_eq!(push_remote(handle.repo()), Some("fork".to_string()));
    }

    #[test]
    fn a_repository_reads_its_own_push_default() {
        // The config read, end to end: `remote.pushDefault` must reach the
        // answer, not just the pure picker.
        let dir = init_repo();
        git(
            dir.path(),
            &["remote", "add", "origin", "https://example.invalid/a.git"],
        );
        git(
            dir.path(),
            &["remote", "add", "fork", "https://example.invalid/b.git"],
        );
        git(dir.path(), &["config", "remote.pushDefault", "fork"]);
        let handle = RepoHandle::discover(dir.path()).expect("fixture is a worktree repo");
        assert_eq!(push_remote(handle.repo()), Some("fork".to_string()));
    }

    #[test]
    fn a_walk_puts_the_remote_on_the_snapshot() {
        // The field has to arrive where the push prompt reads it, not merely
        // exist on the repository.
        let (_origin, clone) = init_repo_with_upstream();
        let handle = RepoHandle::discover(clone.path()).expect("clone is a worktree repo");
        let cfg = crate::RenderConfig {
            base: None,
            max_files: None,
            bar_width: 20,
            log_lines: 0,
            truecolor: false,
            width_offset: 0,
            refresh_interval: None,
        };
        let snapshot = crate::collect_snapshot(handle.repo(), &cfg).expect("walk the clone");
        assert_eq!(snapshot.push_remote, Some("origin".to_string()));
    }
}

//! Domain types describing working-tree changes, produced by the gix-backed
//! `repo` module and consumed by `snapshot` and `render`.

/// What kind of change a [`FileEntry`] represents.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChange,
    Untracked,
    UntrackedDir,
    Conflicted,
}

/// One row in the rendered output.
///
/// A file with both staged and unstaged changes produces two entries —
/// that mirrors how `git status` displays it and what the user wants to see.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct FileEntry {
    pub path: String,
    /// Set for renames/copies; this is the pre-rename path.
    pub orig_path: Option<String>,
    pub status: FileStatus,
    /// True when the change is in the index (staged), false when only in the worktree.
    pub staged: bool,
}

/// Adds/deletes for a single file, from `git diff --numstat`.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
pub struct NumStat {
    pub adds: u32,
    pub dels: u32,
    /// Binary files report `-`/`-` in numstat; we surface that here so render
    /// can show `bin` instead of an empty bar.
    pub binary: bool,
}

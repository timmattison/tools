//! The git-level view of a worktree: what `git worktree list --porcelain` says,
//! and nothing about how `cwt` presents or selects one.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Number of characters of a commit hash to show (git uses 7 by default).
const SHORT_COMMIT_HASH_LENGTH: usize = 7;

/// Represents a single git worktree.
#[derive(Debug, Clone)]
pub struct Worktree {
    /// The filesystem path to this worktree.
    pub path: PathBuf,
    /// The HEAD commit hash.
    pub head: String,
    /// The branch name (without refs/heads/ prefix), or None for detached HEAD.
    pub branch: Option<String>,
}

impl Worktree {
    /// Get the final directory name (e.g., "absurd-rock" from full path).
    pub fn dir_name(&self) -> Option<&str> {
        self.path.file_name()?.to_str()
    }

    /// Get the branch name for display, or short commit hash for detached HEAD.
    ///
    /// A detached HEAD shows the first `SHORT_COMMIT_HASH_LENGTH` characters of
    /// the hash, or the whole hash when it is shorter than that. Counting
    /// characters rather than bytes keeps a head that carries multi-byte UTF-8
    /// from panicking or from losing a character to a truncated byte sequence.
    pub fn display_branch(&self) -> String {
        if let Some(branch) = &self.branch {
            branch.clone()
        } else {
            // Show short commit hash for detached HEAD
            let short_hash: String = self.head.chars().take(SHORT_COMMIT_HASH_LENGTH).collect();
            format!("HEAD@{short_hash}")
        }
    }
}

/// Parses the output of `git worktree list --porcelain`.
///
/// The porcelain format looks like:
/// ```text
/// worktree /path/to/repo
/// HEAD abc123...
/// branch refs/heads/main
///
/// worktree /path/to/worktree
/// HEAD def456...
/// branch refs/heads/feature
/// ```
///
/// For detached HEAD, the branch line is absent.
pub fn parse_worktree_list(output: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_head: Option<String> = None;
    let mut current_branch: Option<String> = None;

    for line in output.lines() {
        if line.is_empty() {
            // End of a worktree block, save if we have the required fields.
            // Note: .take() already leaves the Option as None, so no need to reassign.
            if let (Some(path), Some(head)) = (current_path.take(), current_head.take()) {
                worktrees.push(Worktree {
                    path,
                    head,
                    branch: current_branch.take(),
                });
            }
        } else if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path));
        } else if let Some(head) = line.strip_prefix("HEAD ") {
            current_head = Some(head.to_string());
        } else if let Some(branch) = line.strip_prefix("branch ") {
            // Strip the refs/heads/ prefix
            let branch_name = branch.strip_prefix("refs/heads/").unwrap_or(branch);
            current_branch = Some(branch_name.to_string());
        }
        // Ignore other lines (like "bare" or "detached")
    }

    // Handle last block if output doesn't end with blank line
    if let (Some(path), Some(head)) = (current_path, current_head) {
        worktrees.push(Worktree {
            path,
            head,
            branch: current_branch,
        });
    }

    // Sort by path for consistent ordering
    worktrees.sort_by(|a, b| a.path.cmp(&b.path));

    worktrees
}

/// Finds the main worktree in the output of `git worktree list --porcelain`.
///
/// Git lists the main worktree first and the linked worktrees after it, so the
/// first `worktree` line names the main one. `parse_worktree_list` sorts by
/// path and loses that order, which is why this reads the raw output.
pub fn parse_main_worktree(output: &str) -> Option<PathBuf> {
    output
        .lines()
        .find_map(|line| line.strip_prefix("worktree ").map(PathBuf::from))
}

/// Every worktree of one repository.
#[derive(Debug, Clone)]
pub struct RepoWorktrees {
    /// The main worktree — the checkout that owns the `.git` directory.
    pub main: PathBuf,
    /// All worktrees of the repository, sorted by path.
    pub all: Vec<Worktree>,
}

/// Gets all worktrees for the repository at the given root.
pub fn list_worktrees(repo_root: &Path) -> Result<RepoWorktrees, String> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("Failed to execute git: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree list failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let main = parse_main_worktree(&stdout).ok_or_else(|| {
        format!(
            "git worktree list named no worktree in {}",
            repo_root.display()
        )
    })?;

    Ok(RepoWorktrees {
        main,
        all: parse_worktree_list(&stdout),
    })
}

/// Compares two paths, handling case-insensitivity on macOS.
pub fn paths_equal(a: &Path, b: &Path) -> bool {
    // On macOS, the default filesystem is case-insensitive
    #[cfg(target_os = "macos")]
    {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    }

    #[cfg(not(target_os = "macos"))]
    {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_worktree_list_single() {
        let output = "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main\n";
        let worktrees = parse_worktree_list(output);
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].path, PathBuf::from("/path/to/repo"));
        assert_eq!(worktrees[0].head, "abc123");
        assert_eq!(worktrees[0].branch, Some("main".to_string()));
    }

    #[test]
    fn test_parse_worktree_list_multiple() {
        let output = "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main\n\nworktree /path/to/wt\nHEAD def456\nbranch refs/heads/feature\n";
        let worktrees = parse_worktree_list(output);
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].path, PathBuf::from("/path/to/repo"));
        assert_eq!(worktrees[1].path, PathBuf::from("/path/to/wt"));
    }

    #[test]
    fn test_parse_worktree_list_detached_head() {
        let output = "worktree /path/to/repo\nHEAD abc123\ndetached\n";
        let worktrees = parse_worktree_list(output);
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].branch, None);
    }

    #[test]
    fn test_parse_worktree_list_sorted() {
        let output = "worktree /z/repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /a/repo\nHEAD def\nbranch refs/heads/feature\n";
        let worktrees = parse_worktree_list(output);
        assert_eq!(worktrees.len(), 2);
        // Should be sorted by path
        assert_eq!(worktrees[0].path, PathBuf::from("/a/repo"));
        assert_eq!(worktrees[1].path, PathBuf::from("/z/repo"));
    }

    #[test]
    fn test_worktree_dir_name() {
        let wt = Worktree {
            path: PathBuf::from("/repo-worktrees/absurd-rock"),
            head: "abc".to_string(),
            branch: Some("feature".to_string()),
        };
        assert_eq!(wt.dir_name(), Some("absurd-rock"));
    }

    #[test]
    fn test_worktree_display_branch() {
        let with_branch = Worktree {
            path: PathBuf::from("/repo"),
            head: "abc".to_string(),
            branch: Some("main".to_string()),
        };
        assert_eq!(with_branch.display_branch(), "main");

        let detached = Worktree {
            path: PathBuf::from("/repo"),
            head: "abc1234567890".to_string(),
            branch: None,
        };
        assert_eq!(detached.display_branch(), "HEAD@abc1234");
    }

    /// A detached HEAD is normally hex ASCII, but `display_branch` must never
    /// panic on a head that carries multi-byte characters, and must count the
    /// short hash in characters rather than bytes.
    #[test]
    fn test_worktree_display_branch_counts_characters_not_bytes() {
        let japanese = Worktree {
            path: PathBuf::from("/repo"),
            head: "日本語テスト".to_string(),
            branch: None,
        };
        assert_eq!(
            japanese.display_branch(),
            "HEAD@日本語テスト",
            "6 characters is shorter than the 7-character short hash, so all of it shows"
        );

        let emoji = Worktree {
            path: PathBuf::from("/repo"),
            head: "🎉🎊🎁🎈🎂🎃🎄🎆".to_string(),
            branch: None,
        };
        assert_eq!(
            emoji.display_branch(),
            "HEAD@🎉🎊🎁🎈🎂🎃🎄",
            "8 characters truncates to the first 7 characters, not the first 7 bytes"
        );

        let mixed = Worktree {
            path: PathBuf::from("/repo"),
            head: "café1234".to_string(),
            branch: None,
        };
        assert_eq!(
            mixed.display_branch(),
            "HEAD@café123",
            "the accented character costs two bytes but only one character"
        );
    }

    #[test]
    fn test_parse_main_worktree_is_the_first_block() {
        // Git lists the main worktree first, whatever the paths sort like.
        let output = "worktree /z/repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /a/repo-wt\nHEAD def\nbranch refs/heads/feature\n";
        assert_eq!(
            parse_main_worktree(output),
            Some(PathBuf::from("/z/repo")),
            "the sorted list starts with /a/repo-wt, but the main worktree is /z/repo"
        );
    }

    #[test]
    fn test_parse_main_worktree_of_empty_output() {
        assert_eq!(parse_main_worktree(""), None);
    }

    #[test]
    fn test_parse_worktree_no_trailing_newline() {
        let output = "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main";
        let worktrees = parse_worktree_list(output);
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].branch, Some("main".to_string()));
    }
}

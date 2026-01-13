use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use ignore::WalkBuilder;
use walkdir::{DirEntry, WalkDir};

pub fn find_git_repo() -> Option<PathBuf> {
    let mut current_dir = env::current_dir().ok()?;
    
    loop {
        let git_dir = current_dir.join(".git");
        if git_dir.exists() {
            return Some(current_dir);
        }
        
        if !current_dir.pop() {
            break;
        }
    }
    
    None
}

pub fn is_git_worktree(dir: &Path) -> bool {
    let git_path = dir.join(".git");

    if git_path.is_file() {
        if let Ok(content) = fs::read_to_string(&git_path) {
            return content.trim().starts_with("gitdir:");
        }
    }

    false
}

/// Finds the root of the main git repository, even when called from a worktree.
///
/// Unlike `find_git_repo()` which returns the current worktree directory if inside one,
/// this function always returns the path to the main repository (where `.git` is a directory,
/// not a file).
///
/// Uses `git rev-parse --git-common-dir` to find the common git directory, which points
/// to the main repository's `.git` directory for both worktrees and the main repo.
pub fn find_main_repo() -> Option<PathBuf> {
    let git_dir = find_git_repo()?;
    let git_path = git_dir.join(".git");

    if git_path.is_dir() {
        // Already in the main repo
        return Some(git_dir);
    }

    // We're in a worktree - use git to find the common directory
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(&git_dir)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    // Output is like: /path/to/main-repo/.git (usually absolute, but can be relative)
    let common_dir = String::from_utf8(output.stdout).ok()?.trim().to_string();
    let common_path = PathBuf::from(&common_dir);

    // Handle both absolute and relative paths from git rev-parse.
    // In practice, --git-common-dir returns absolute paths from worktrees because the
    // worktree's .git file contains an absolute gitdir: reference. However, git doesn't
    // guarantee this, so we handle relative paths defensively by joining with git_dir.
    let common_path = if common_path.is_absolute() {
        common_path
    } else {
        git_dir.join(&common_path)
    };

    // The parent of the .git directory is the repo root
    common_path.parent().map(|p| p.to_path_buf())
}

pub struct RepoWalker {
    root: PathBuf,
    skip_node_modules: bool,
    skip_worktrees: bool,
    respect_gitignore: bool,
    include_hidden: bool,
}

impl RepoWalker {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            skip_node_modules: true,
            skip_worktrees: true,
            respect_gitignore: true,
            include_hidden: false,
        }
    }
    
    pub fn skip_node_modules(mut self, skip: bool) -> Self {
        self.skip_node_modules = skip;
        self
    }
    
    pub fn skip_worktrees(mut self, skip: bool) -> Self {
        self.skip_worktrees = skip;
        self
    }
    
    pub fn respect_gitignore(mut self, respect: bool) -> Self {
        self.respect_gitignore = respect;
        self
    }
    
    pub fn include_hidden(mut self, include: bool) -> Self {
        self.include_hidden = include;
        self
    }
    
    pub fn walk_with_walkdir(&self) -> impl Iterator<Item = DirEntry> {
        let root = self.root.clone();
        let skip_node_modules = self.skip_node_modules;
        let skip_worktrees = self.skip_worktrees;
        
        WalkDir::new(&self.root)
            .into_iter()
            .filter_entry(move |e| {
                if skip_node_modules && e.file_name() == "node_modules" {
                    return false;
                }
                
                if skip_worktrees && e.file_type().is_dir() && is_git_worktree(e.path()) {
                    if e.path() != root {
                        println!("Skipping git worktree directory: {}", e.path().display());
                        return false;
                    }
                }
                
                true
            })
            .filter_map(|e| e.ok())
    }
    
    pub fn walk_with_ignore(&self) -> impl Iterator<Item = ignore::DirEntry> + '_ {
        let mut builder = WalkBuilder::new(&self.root);

        builder
            .git_ignore(self.respect_gitignore)
            .git_global(self.respect_gitignore)
            .git_exclude(self.respect_gitignore)
            .hidden(!self.include_hidden);

        if self.skip_node_modules {
            builder.filter_entry(move |entry| {
                entry.file_name() != "node_modules"
            });
        }

        if self.skip_worktrees {
            let root = self.root.clone();
            builder.filter_entry(move |entry| {
                if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                    if is_git_worktree(entry.path()) && entry.path() != root {
                        println!("Skipping git worktree directory: {}", entry.path().display());
                        return false;
                    }
                }
                true
            });
        }

        builder.build().filter_map(|e| e.ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_git_repo() {
        let path = find_git_repo().expect("Tests should run within a git repository");
        assert!(path.join(".git").exists());
    }

    #[test]
    fn test_find_main_repo() {
        let path = find_main_repo().expect("Tests should run within a git repository");
        assert!(
            path.join(".git").is_dir(),
            "Main repo should have .git directory, not file"
        );
    }
}

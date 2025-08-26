use std::env;
use std::fs;
use std::path::{Path, PathBuf};
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
        
        builder.build().filter_map(|e| e.ok()).filter(move |entry| {
            if self.skip_worktrees && entry.file_type().is_some_and(|ft| ft.is_dir()) {
                if is_git_worktree(entry.path()) && entry.path() != self.root {
                    println!("Skipping git worktree directory: {}", entry.path().display());
                    return false;
                }
            }
            true
        })
    }
}

    #[test]
    fn test_find_git_repo() {
-        let repo = find_git_repo();
-        if let Some(path) = repo {
-            assert!(path.join(".git").exists());
        let path = find_git_repo().expect("Tests should run within a git repository");
        assert!(path.join(".git").exists());
    }
# repowalker

A shared Rust library for walking repository directories with intelligent filtering and gitignore support.

## Features

- **Git repository detection**: Find the root of a git repository from any subdirectory
- **Git worktree detection**: Identify and optionally skip git worktree directories
- **Gitignore support**: Respect `.gitignore`, `.git/info/exclude`, and global git ignore files
- **Configurable filtering**: Skip node_modules, hidden files, and other patterns
- **Dual API**: Use either `walkdir` or `ignore` crate backends depending on your needs

## Usage

```rust
use repowalker::{find_git_repo, RepoWalker};

// Find the git repository root
let repo_root = find_git_repo().expect("Not in a git repository");

// Create a walker with default settings
let walker = RepoWalker::new(repo_root)
    .respect_gitignore(true)
    .skip_node_modules(true)
    .skip_worktrees(true);

// Walk using the ignore crate (respects gitignore)
for entry in walker.walk_with_ignore() {
    println!("Found: {}", entry.path().display());
}

// Or walk using walkdir (doesn't respect gitignore but simpler)
for entry in walker.walk_with_walkdir() {
    println!("Found: {}", entry.path().display());
}
```

## Configuration Options

- `skip_node_modules(bool)`: Skip node_modules directories (default: true)
- `skip_worktrees(bool)`: Skip git worktree directories except the root (default: true)
- `respect_gitignore(bool)`: Respect gitignore files when using `walk_with_ignore()` (default: true)
- `include_hidden(bool)`: Include hidden files and directories (default: false)

## Used By

This library is used by several tools in this repository:
- `goup`: Update Go dependencies across a repository
- `polish`: Update Rust crate dependencies across a repository
- `nodeup`: Update Node.js dependencies across a repository

## Dependencies

- `walkdir`: For basic directory traversal
- `ignore`: For gitignore-aware directory traversal
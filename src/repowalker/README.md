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

## Child Repositories

A container repository keeps other repositories one level below it. Each of
these is a child repository. `child_repositories(dir)` finds them.

A child repository is a directory one level below `dir` that holds a `.git`
entry. The entry is a `.git` directory for a main worktree, or a `.git` file for
a linked worktree. `holds_git_entry(dir)` tells if a directory holds one. It
follows a symbolic link.

The search does not go below the first level. A child of a child is not a
child. The result is sorted by path. A directory that does not exist, or that
the function cannot read, has no children.

```rust
use repowalker::{child_repositories, holds_git_entry};
use std::path::Path;

let container = Path::new("/code/workspace");
if holds_git_entry(container) {
    for child in child_repositories(container) {
        println!("Child: {}", child.display());
    }
}
```

`cwt` and `nwt` both call these functions, so the two tools use one definition
of a child.

## Configuration Options

- `skip_node_modules(bool)`: Skip node_modules directories (default: true)
- `skip_worktrees(bool)`: Skip git worktree directories except the root (default: true)
- `respect_gitignore(bool)`: Respect gitignore files when using `walk_with_ignore()` (default: true)
- `include_hidden(bool)`: Include hidden files and directories (default: false)

## Used By

This library is used by several tools in this repository:
- `cwt`: Find the child repositories of a container, to list their worktrees
  with the worktrees of the container
- `nwt`: Find the child repositories of a container, to link them into each new
  worktree of the container
- `goup`: Update Go dependencies across a repository
- `polish`: Update Rust crate dependencies across a repository
- `nodeup`: Update Node.js dependencies across a repository

## Dependencies

- `walkdir`: For basic directory traversal
- `ignore`: For gitignore-aware directory traversal
//! The walk: which files the tool counts.
//!
//! The walk is the `ignore` crate, which is the walk of `ripgrep`, so
//! `.gitignore`, `.ignore`, and the exclude file of the repository all hold by
//! default. A counter that reported the vendored tree of a repository would
//! report a number nobody recognises.
//!
//! # Two things the walk states rather than assumes
//!
//! **A root that does not exist is an error.** The `ignore` crate reports such
//! a root through the iterator rather than up front, and a caller that reads
//! only the entries it yields therefore counts a mistyped path as an empty
//! tree — a run that prints a table of zeros and exits zero. Every root is
//! checked before the walk starts, so the message names the path.
//!
//! **A `.gitignore` outside a repository still holds.** The `ignore` crate
//! reads the git-specific ignore files only inside a git repository unless it
//! is told otherwise, so a tree that was copied out of a repository, or one
//! that is not a repository yet, would count files that its own `.gitignore`
//! names. [`walk`] calls `require_git(false)` for that reason: the tool counts
//! what the tree says to count, and the tree says it in `.gitignore` whether or
//! not `.git` is beside it.

use anyhow::{bail, Context, Result};
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

/// What the walk includes beyond the default.
#[derive(Clone, Copy, Debug, Default)]
pub struct WalkOptions {
    /// Include a hidden file or directory.
    pub hidden: bool,
    /// Ignore every ignore file, including `.gitignore`.
    pub no_ignore: bool,
}

/// Every file under `roots`, paired with the path relative to the root that
/// found it.
///
/// A root that is itself a file yields exactly that file, paired with its own
/// name, so a path rule reads the same name whether the file arrived on its own
/// or through the tree above it.
///
/// The result is sorted by path. A report whose row order changes between two
/// runs over the same tree is a report nobody can diff, and the order the
/// `ignore` crate yields entries in is the order its threads finish in.
///
/// # Errors
///
/// Returns an error when a root does not exist, and when a directory under one
/// cannot be read.
pub fn walk(roots: &[PathBuf], options: WalkOptions) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut found = Vec::new();

    for root in roots {
        let exists = root
            .try_exists()
            .with_context(|| format!("cannot read `{}`", root.display()))?;
        if !exists {
            bail!("`{}` does not exist", root.display());
        }

        let mut builder = WalkBuilder::new(root);
        builder
            .ignore(!options.no_ignore)
            .git_ignore(!options.no_ignore)
            .git_global(!options.no_ignore)
            .git_exclude(!options.no_ignore)
            .require_git(false)
            .hidden(!options.hidden);

        for entry in builder.build() {
            let entry = entry
                .with_context(|| format!("cannot read the tree under `{}`", root.display()))?;
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.into_path();
            let relative = relative_to(root, &path);
            found.push((path, relative));
        }
    }

    found.sort();
    Ok(found)
}

/// The path of a file as the rules see it: relative to the root that found it.
///
/// A root that is the file itself strips to nothing, and an empty path matches
/// no glob and prints as nothing, so such a file answers with its own name
/// instead.
fn relative_to(root: &Path, path: &Path) -> PathBuf {
    match path.strip_prefix(root) {
        Ok(rest) if rest.as_os_str().is_empty() => {
            PathBuf::from(path.file_name().unwrap_or(path.as_os_str()))
        }
        Ok(rest) => rest.to_path_buf(),
        Err(_) => path.to_path_buf(),
    }
}

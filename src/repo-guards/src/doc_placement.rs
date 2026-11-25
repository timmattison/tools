//! Guard: every spec and every plan lives at the repository root.
//!
//! A spec goes in `specs/` and a plan goes in `plans/`, both at the repository
//! root. The superpowers skills write them to `docs/superpowers/` by default,
//! and this repository took that default in #198, in #200, and in #191.
//! [`misplaced`] finds each file in a `specs` or `plans` directory that is not
//! at the root, and [`remediation`] tells the reader how to move it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The directory names that hold specs and plans. Each name is correct only as
/// the first component of a path.
const DOC_DIRS: [&str; 2] = ["specs", "plans"];

/// The directories that the walk does not enter, at any depth.
const SKIPPED_DIRS: [&str; 3] = [
    // The metadata of git. It holds no file of the working tree.
    ".git",
    // The output of cargo. The build writes it, and no person puts a document there.
    "target",
    // The packages that pnpm installs. Their layout belongs to their authors.
    "node_modules",
];

/// Every file below `root` that is in a `specs` or `plans` directory other than
/// the one at the root, as sorted paths relative to `root`.
///
/// Only the directories above a file count, so a file that is itself named
/// `specs` is not misplaced. An empty directory is not misplaced. The walk does
/// not follow a symlink, and it reads a symlink as a file. The walk does not
/// enter a directory below `root` that holds a `.git` entry, because that
/// directory is another checkout: a worktree, a clone, or a submodule.
///
/// # Errors
///
/// Returns the I/O error of the first directory that the walk cannot read, with
/// that directory in the message. A directory that the walk cannot read is
/// never a clean result.
pub fn misplaced(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![PathBuf::new()];

    while let Some(relative) = pending.pop() {
        let dir = root.join(&relative);
        let unreadable = |error: io::Error| {
            io::Error::new(
                error.kind(),
                format!("cannot read {}: {error}", dir.display()),
            )
        };

        for entry in fs::read_dir(&dir).map_err(unreadable)? {
            let entry = entry.map_err(unreadable)?;
            let name = entry.file_name();
            let path = relative.join(&name);
            // `DirEntry::file_type` does not follow a symlink.
            if entry.file_type().map_err(unreadable)?.is_dir() {
                if !SKIPPED_DIRS.iter().any(|skip| name == *skip)
                    && !is_checkout(&root.join(&path))?
                {
                    pending.push(path);
                }
            } else if in_nested_doc_dir(&path) {
                found.push(path);
            }
        }
    }

    found.sort();
    Ok(found)
}

/// The message that names each file in `files` and tells the reader how to
/// move it.
#[must_use]
pub fn remediation(files: &[PathBuf]) -> String {
    let mut message = String::from(
        "These files are in a `specs` or `plans` directory other than the one at the repository root:\n\n",
    );
    for file in files {
        message.push_str("    ");
        message.push_str(&file.to_string_lossy());
        message.push('\n');
    }
    message.push_str(
        "\nUse `git mv` to move each spec into `specs/` and each plan into `plans/`, both at the repository root.",
    );
    message
}

/// True when `dir` holds a `.git` entry: a file in a linked worktree or a
/// submodule, a directory in a clone. The entry is not followed.
fn is_checkout(dir: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(dir.join(".git")) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("cannot read {}: {error}", dir.join(".git").display()),
        )),
    }
}

/// True when a directory above the file at `relative`, other than the first
/// component of the path, is named `specs` or `plans`.
fn in_nested_doc_dir(relative: &Path) -> bool {
    relative
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .skip(1)
        .any(|component| DOC_DIRS.iter().any(|name| component.as_os_str() == *name))
}

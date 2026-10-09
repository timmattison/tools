//! The copy plan: what `prcp` makes at the destination, decided before any copy starts.
//!
//! # Destination rules
//!
//! The plan has a container rule. It is decided once, before any entry.
//! The destination is a container when it is an existing directory, or when
//! there is more than one source. A source then maps to `destination/<name>`.
//! With one source and a destination that is missing or a file, the source
//! maps to the destination itself.
//!
//! The name of a source is its last path component. A source such as `.` or
//! `dir/..` has none. The name then comes from the canonical path of the
//! source. The root `/` has no name, and that is an error.
//!
//! A directory source needs `recursive`. The walk makes one entry for each
//! directory, file, and symlink under it, and every directory comes before its
//! contents. A directory source is an error when the destination holds a
//! non-directory at its root, or when the destination root is inside the
//! source.
//!
//! # Symlink rules
//!
//! The walk never follows a symlink. A symlink inside a tree becomes a
//! `Symlink` entry that keeps its target text. A cyclic link cannot make the
//! walk loop. A top-level symlink to a directory also becomes one `Symlink`
//! entry, and the plan does not walk it. A top-level symlink to a file is a
//! `File` entry, and the copy reads through the link.
//!
//! A FIFO, socket, or device in a tree is not copied. The plan records it as a
//! skipped entry. A part of a tree that the walk cannot read is a walk error.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Index of one source operand (one resolved source path), in the order given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct OperandId(usize);

impl OperandId {
    /// Return the position of the operand in the list of sources.
    pub(crate) fn index(self) -> usize {
        self.0
    }
}

/// What a top-level source operand is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperandKind {
    /// A regular file, or a symlink to a file.
    File,
    /// A directory that the plan walks.
    Directory,
    /// A symlink to a directory that the plan does not walk.
    Symlink,
}

/// One source operand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Operand {
    /// The source path as the user gave it.
    pub(crate) source: PathBuf,
    /// The path that the source itself becomes. For a directory, this is the root of the copy.
    pub(crate) destination: PathBuf,
    /// What the source is.
    pub(crate) kind: OperandKind,
}

/// What one plan entry makes at the destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EntryKind {
    /// Make a directory.
    Directory,
    /// Copy a file.
    File,
    /// Make a symlink with the given target text.
    Symlink {
        /// The text of the link.
        target: PathBuf,
    },
}

/// One unit of work: make `destination` from `source`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanEntry {
    /// The operand that this entry belongs to.
    pub(crate) operand: OperandId,
    /// The source path.
    pub(crate) source: PathBuf,
    /// The destination path.
    pub(crate) destination: PathBuf,
    /// What to make.
    pub(crate) kind: EntryKind,
}

/// A node in a source tree that prcp does not copy (FIFO, socket, device).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkippedEntry {
    /// The operand that holds the node.
    pub(crate) operand: OperandId,
    /// The path of the node.
    pub(crate) path: PathBuf,
    /// Why prcp skips the node.
    pub(crate) reason: &'static str,
}

/// A part of a source tree that the walk could not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalkError {
    /// The operand that holds the part.
    pub(crate) operand: OperandId,
    /// The path that failed.
    pub(crate) path: PathBuf,
    /// The text of the error.
    pub(crate) message: String,
}

/// The kind of one node in a walk of a source tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NodeKind {
    /// A directory.
    Directory,
    /// A regular file.
    File,
    /// A symlink with its target text.
    Symlink {
        /// The text of the link.
        target: PathBuf,
    },
    /// A node that prcp does not copy. The text names its kind.
    Special(&'static str),
}

/// Every node under one source directory, keyed by its full source path, plus the read errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeSnapshot {
    nodes: BTreeMap<PathBuf, NodeKind>,
    errors: Vec<(PathBuf, String)>,
}

impl TreeSnapshot {
    /// Walk `root` now. The snapshot includes `root` itself as a Directory node.
    pub(crate) fn take(root: &Path) -> Self {
        let mut nodes = BTreeMap::new();
        let mut errors = Vec::new();
        let walk = WalkDir::new(root)
            .follow_links(false)
            .follow_root_links(false)
            .sort_by_file_name();
        for result in walk {
            let entry = match result {
                Ok(entry) => entry,
                Err(error) => {
                    let path = error.path().unwrap_or(root).to_path_buf();
                    errors.push((path, error.to_string()));
                    continue;
                }
            };
            match node_kind(entry.path(), entry.file_type()) {
                Ok(kind) => {
                    nodes.insert(entry.into_path(), kind);
                }
                Err(error) => errors.push((entry.into_path(), error.to_string())),
            }
        }
        Self { nodes, errors }
    }

    /// Return every node of the walk, keyed by its full source path.
    pub(crate) fn nodes(&self) -> &BTreeMap<PathBuf, NodeKind> {
        &self.nodes
    }

    /// Return the parts of the tree that the walk could not read.
    pub(crate) fn errors(&self) -> &[(PathBuf, String)] {
        &self.errors
    }
}

/// Return the kind of the node at `path`, from the file type that a walk read.
///
/// The call reads the target of a symlink, and it never follows the link. A
/// link whose target cannot be read is an error.
pub(crate) fn node_kind(path: &Path, file_type: fs::FileType) -> std::io::Result<NodeKind> {
    Ok(if file_type.is_dir() {
        NodeKind::Directory
    } else if file_type.is_symlink() {
        NodeKind::Symlink {
            target: fs::read_link(path)?,
        }
    } else if file_type.is_file() {
        NodeKind::File
    } else {
        NodeKind::Special(special_kind(file_type))
    })
}

/// Decide what a top-level source operand is. The call does not follow a
/// symlink to find out if the operand is one.
///
/// A directory needs `recursive`. A symlink to a directory is a `Symlink`
/// operand, and the plan does not walk it. A symlink to a file counts as a
/// file, because the copy reads through the link. Anything else is an error.
fn classify_operand(source: &Path, recursive: bool) -> Result<OperandKind> {
    let file_type = fs::symlink_metadata(source)
        .with_context(|| format!("Cannot read source '{}'", source.display()))?
        .file_type();
    let kind = if file_type.is_symlink() {
        if source.is_dir() {
            OperandKind::Symlink
        } else if source.is_file() {
            OperandKind::File
        } else {
            bail!("Source '{}' is not a file", source.display());
        }
    } else if file_type.is_dir() {
        OperandKind::Directory
    } else if file_type.is_file() {
        OperandKind::File
    } else {
        bail!("Source '{}' is not a file", source.display());
    };
    if !recursive && kind != OperandKind::File {
        bail!(
            "Source '{}' is a directory ({})",
            source.display(),
            super::RECURSIVE_HINT
        );
    }
    Ok(kind)
}

/// Return the name that `source` takes under a container destination.
///
/// The name is the last component of the path. A path such as `.` or `dir/..`
/// has none, so the name then comes from the canonical path. The root `/` has
/// no name at all, and that is an error.
fn operand_name(source: &Path) -> Result<OsString> {
    if let Some(name) = source.file_name() {
        return Ok(name.to_os_string());
    }
    fs::canonicalize(source)
        .ok()
        .and_then(|canonical| canonical.file_name().map(OsStr::to_os_string))
        .ok_or_else(|| {
            anyhow!(
                "Source '{}' has no name to copy under the destination",
                source.display()
            )
        })
}

/// Resolve `path` to an absolute path with no symlinks, even when its tail does not exist yet.
///
/// The function resolves the deepest ancestor that exists. It then appends
/// the components that do not exist.
pub(crate) fn canonicalize_lenient(path: &Path) -> Result<PathBuf> {
    let mut missing: Vec<&OsStr> = Vec::new();
    let mut current = path;
    loop {
        match fs::canonicalize(current) {
            Ok(mut resolved) => {
                resolved.extend(missing.iter().rev());
                return Ok(resolved);
            }
            Err(error) => {
                let (Some(name), Some(parent)) = (current.file_name(), current.parent()) else {
                    return Err(error)
                        .with_context(|| format!("Cannot resolve '{}'", path.display()));
                };
                missing.push(name);
                current = if parent.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    parent
                };
            }
        }
    }
}

/// Name the kind of a node that is not a directory, a file, or a symlink.
#[cfg(unix)]
fn special_kind(file_type: fs::FileType) -> &'static str {
    use std::os::unix::fs::FileTypeExt;
    if file_type.is_fifo() {
        "fifo"
    } else if file_type.is_socket() {
        "socket"
    } else if file_type.is_block_device() {
        "block device"
    } else if file_type.is_char_device() {
        "character device"
    } else {
        SPECIAL_FILE
    }
}

/// Name the kind of a node that is not a directory, a file, or a symlink.
#[cfg(not(unix))]
fn special_kind(_file_type: fs::FileType) -> &'static str {
    SPECIAL_FILE
}

/// The reason for a special node of an unknown kind.
const SPECIAL_FILE: &str = "special file";

/// The full list of work for one run, decided before any copy starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CopyPlan {
    operands: Vec<Operand>,
    entries: Vec<PlanEntry>,
    skipped: Vec<SkippedEntry>,
    walk_errors: Vec<WalkError>,
    trees: BTreeMap<OperandId, TreeSnapshot>,
}

impl CopyPlan {
    /// Build the plan for `sources` and `destination`. See the module docs for the rules.
    pub(crate) fn build(sources: &[PathBuf], destination: &Path, recursive: bool) -> Result<Self> {
        let container = destination.is_dir() || sources.len() > 1;
        if sources.len() > 1 && destination.exists() && !destination.is_dir() {
            bail!(
                "Destination '{}' is not a directory (required for multiple source files)",
                destination.display()
            );
        }
        let mut plan = Self {
            operands: Vec::new(),
            entries: Vec::new(),
            skipped: Vec::new(),
            walk_errors: Vec::new(),
            trees: BTreeMap::new(),
        };
        for (index, source) in sources.iter().enumerate() {
            let operand = OperandId(index);
            let name = operand_name(source)?;
            let root = if container {
                destination.join(name)
            } else {
                destination.to_path_buf()
            };
            match classify_operand(source, recursive)? {
                OperandKind::Directory => plan.add_tree(operand, source, &root)?,
                OperandKind::Symlink => {
                    let target = fs::read_link(source)?;
                    plan.add_leaf(
                        operand,
                        source,
                        root,
                        OperandKind::Symlink,
                        EntryKind::Symlink { target },
                    );
                }
                OperandKind::File => {
                    plan.add_leaf(operand, source, root, OperandKind::File, EntryKind::File);
                }
            }
        }
        Ok(plan)
    }

    /// Add an operand that has exactly one entry.
    fn add_leaf(
        &mut self,
        operand: OperandId,
        source: &Path,
        destination: PathBuf,
        operand_kind: OperandKind,
        kind: EntryKind,
    ) {
        self.operands.push(Operand {
            source: source.to_path_buf(),
            destination: destination.clone(),
            kind: operand_kind,
        });
        self.entries.push(PlanEntry {
            operand,
            source: source.to_path_buf(),
            destination,
            kind,
        });
    }

    /// Walk a Directory operand and add one entry for each node of the walk.
    ///
    /// `root` is the destination of the operand itself. The snapshot is the one
    /// source of the entries, so the plan and the snapshot cannot disagree.
    fn add_tree(&mut self, operand: OperandId, source: &Path, root: &Path) -> Result<()> {
        if fs::symlink_metadata(root).is_ok_and(|existing| !existing.is_dir()) {
            bail!(
                "Cannot overwrite non-directory '{}' with directory '{}'",
                root.display(),
                source.display()
            );
        }
        let canonical_source = fs::canonicalize(source)?;
        if canonicalize_lenient(root)?.starts_with(&canonical_source) {
            bail!(
                "Cannot copy directory '{}' into itself ('{}')",
                source.display(),
                root.display()
            );
        }
        self.operands.push(Operand {
            source: source.to_path_buf(),
            destination: root.to_path_buf(),
            kind: OperandKind::Directory,
        });
        let snapshot = TreeSnapshot::take(source);
        for (path, node) in snapshot.nodes() {
            let relative = path.strip_prefix(source)?;
            let destination = if relative.as_os_str().is_empty() {
                root.to_path_buf()
            } else {
                root.join(relative)
            };
            let kind = match node {
                NodeKind::Directory => EntryKind::Directory,
                NodeKind::File => EntryKind::File,
                NodeKind::Symlink { target } => EntryKind::Symlink {
                    target: target.clone(),
                },
                NodeKind::Special(reason) => {
                    self.skipped.push(SkippedEntry {
                        operand,
                        path: path.clone(),
                        reason,
                    });
                    continue;
                }
            };
            self.entries.push(PlanEntry {
                operand,
                source: path.clone(),
                destination,
                kind,
            });
        }
        for (path, message) in snapshot.errors() {
            self.walk_errors.push(WalkError {
                operand,
                path: path.clone(),
                message: message.clone(),
            });
        }
        self.trees.insert(operand, snapshot);
        Ok(())
    }

    /// Return the source operands, in the order given.
    pub(crate) fn operands(&self) -> &[Operand] {
        &self.operands
    }

    /// Return the entries, in the order that the copy must run them.
    pub(crate) fn entries(&self) -> &[PlanEntry] {
        &self.entries
    }

    /// Return the nodes that the plan skips.
    pub(crate) fn skipped(&self) -> &[SkippedEntry] {
        &self.skipped
    }

    /// Return the parts of the source trees that the walk could not read.
    pub(crate) fn walk_errors(&self) -> &[WalkError] {
        &self.walk_errors
    }

    /// Return the snapshot that `build` took of a Directory operand. None for other operands.
    pub(crate) fn tree(&self, operand: OperandId) -> Option<&TreeSnapshot> {
        self.trees.get(&operand)
    }

    /// Count the File entries.
    pub(crate) fn file_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .count()
    }

    /// Return true when the plan has exactly one entry and that entry is a File.
    pub(crate) fn is_single_file(&self) -> bool {
        matches!(self.entries.as_slice(), [only] if only.kind == EntryKind::File)
    }

    /// Give each destination directory the permissions of its source directory.
    ///
    /// Call this after the copy, because a read-only source directory would stop
    /// the copy of its contents. The function walks the Directory entries in
    /// reverse plan order, so children come before parents. It skips an entry
    /// whose destination is not a directory. It collects each error and goes on.
    pub(crate) fn apply_directory_permissions(&self) -> Vec<(PathBuf, std::io::Error)> {
        let mut errors = Vec::new();
        for entry in self.entries.iter().rev() {
            if entry.kind != EntryKind::Directory {
                continue;
            }
            let is_directory = fs::symlink_metadata(&entry.destination)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false);
            if !is_directory {
                continue;
            }
            let result = fs::metadata(&entry.source).and_then(|metadata| {
                fs::set_permissions(&entry.destination, metadata.permissions())
            });
            if let Err(error) = result {
                errors.push((entry.destination.clone(), error));
            }
        }
        errors
    }

    /// Return the sources of the File entries, in plan order.
    pub(crate) fn file_sources(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .map(|entry| entry.source.clone())
            .collect()
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap for brevity and clear failure messages"
)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Make a file with the given text. Make the parent directories first.
    fn write_file(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    /// Make a file entry with the operand set to the given index.
    fn file_entry(operand: usize, source: &Path, destination: &Path) -> PlanEntry {
        PlanEntry {
            operand: OperandId(operand),
            source: source.to_path_buf(),
            destination: destination.to_path_buf(),
            kind: EntryKind::File,
        }
    }

    /// Make a directory entry with the operand set to the given index.
    fn dir_entry(operand: usize, source: &Path, destination: &Path) -> PlanEntry {
        PlanEntry {
            operand: OperandId(operand),
            source: source.to_path_buf(),
            destination: destination.to_path_buf(),
            kind: EntryKind::Directory,
        }
    }

    #[test]
    fn one_file_into_existing_directory_lands_under_its_name() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("a.txt");
        write_file(&source, "a");
        let destination = temp.path().join("dest");
        fs::create_dir(&destination).unwrap();

        let plan = CopyPlan::build(std::slice::from_ref(&source), &destination, false).unwrap();

        assert_eq!(
            plan.entries(),
            [file_entry(0, &source, &destination.join("a.txt"))]
        );
    }

    #[test]
    fn one_file_to_missing_destination_becomes_the_destination() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("a.txt");
        write_file(&source, "a");
        let destination = temp.path().join("copy.txt");

        let plan = CopyPlan::build(std::slice::from_ref(&source), &destination, false).unwrap();

        assert_eq!(plan.entries(), [file_entry(0, &source, &destination)]);
    }

    #[test]
    fn two_files_to_missing_destination_make_it_a_container() {
        let temp = TempDir::new().unwrap();
        let first = temp.path().join("a");
        let second = temp.path().join("b");
        write_file(&first, "a");
        write_file(&second, "b");
        let destination = temp.path().join("newdir");

        let plan = CopyPlan::build(&[first.clone(), second.clone()], &destination, false).unwrap();

        assert_eq!(
            plan.entries(),
            [
                file_entry(0, &first, &destination.join("a")),
                file_entry(1, &second, &destination.join("b")),
            ]
        );
    }

    #[test]
    fn two_files_to_an_existing_file_is_an_error() {
        let temp = TempDir::new().unwrap();
        let first = temp.path().join("a");
        let second = temp.path().join("b");
        let destination = temp.path().join("target.txt");
        write_file(&first, "a");
        write_file(&second, "b");
        write_file(&destination, "old");

        let error = CopyPlan::build(&[first, second], &destination, false).unwrap_err();

        assert!(
            error.to_string().contains("is not a directory"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn directory_to_missing_destination_walks_in_sorted_order_parents_first() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("z.txt"), "z");
        write_file(&src.join("b").join("d").join("e.txt"), "e");
        write_file(&src.join("b").join("c.txt"), "c");
        write_file(&src.join("a.txt"), "a");
        let dest = temp.path().join("dest");

        let plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();

        assert_eq!(
            plan.entries(),
            [
                dir_entry(0, &src, &dest),
                file_entry(0, &src.join("a.txt"), &dest.join("a.txt")),
                dir_entry(0, &src.join("b"), &dest.join("b")),
                file_entry(0, &src.join("b/c.txt"), &dest.join("b/c.txt")),
                dir_entry(0, &src.join("b/d"), &dest.join("b/d")),
                file_entry(0, &src.join("b/d/e.txt"), &dest.join("b/d/e.txt")),
                file_entry(0, &src.join("z.txt"), &dest.join("z.txt")),
            ]
        );
    }

    #[test]
    fn directory_into_existing_directory_lands_under_its_name() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("a.txt"), "a");
        let dest = temp.path().join("dest");
        fs::create_dir(&dest).unwrap();

        let plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();

        assert_eq!(
            plan.entries(),
            [
                dir_entry(0, &src, &dest.join("src")),
                file_entry(0, &src.join("a.txt"), &dest.join("src/a.txt")),
            ]
        );
    }

    #[test]
    fn trailing_slash_on_the_source_gives_the_same_plan() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("a.txt"), "a");
        let dest = temp.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let slashed = src.join("");
        assert!(slashed.to_string_lossy().ends_with('/'));

        let plan = CopyPlan::build(std::slice::from_ref(&slashed), &dest, true).unwrap();

        let destinations: Vec<PathBuf> = plan
            .entries()
            .iter()
            .map(|entry| entry.destination.clone())
            .collect();
        assert_eq!(destinations, [dest.join("src"), dest.join("src/a.txt")]);
    }

    #[test]
    fn empty_subdirectory_is_a_directory_entry() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        fs::create_dir_all(src.join("empty")).unwrap();
        let dest = temp.path().join("dest");

        let plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();

        assert_eq!(
            plan.entries(),
            [
                dir_entry(0, &src, &dest),
                dir_entry(0, &src.join("empty"), &dest.join("empty")),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_in_the_tree_is_an_entry_that_keeps_its_target() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("a.txt"), "a");
        std::os::unix::fs::symlink("a.txt", src.join("link")).unwrap();
        let dest = temp.path().join("dest");

        let plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();

        let link = plan
            .entries()
            .iter()
            .find(|entry| entry.source == src.join("link"))
            .expect("the plan has an entry for the link");
        assert_eq!(
            link.kind,
            EntryKind::Symlink {
                target: PathBuf::from("a.txt")
            }
        );
        assert_eq!(link.destination, dest.join("link"));
    }

    #[cfg(unix)]
    #[test]
    fn cyclic_symlink_ends_the_walk_and_is_not_followed() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("sub").join("a.txt"), "a");
        std::os::unix::fs::symlink("..", src.join("sub").join("loop")).unwrap();
        let dest = temp.path().join("dest");

        let plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();

        let loops: Vec<&PlanEntry> = plan
            .entries()
            .iter()
            .filter(|entry| entry.source.ends_with("loop"))
            .collect();
        assert_eq!(loops.len(), 1);
        assert_eq!(
            loops[0].kind,
            EntryKind::Symlink {
                target: PathBuf::from("..")
            }
        );
        assert!(plan.entries().iter().all(|entry| !entry
            .source
            .ancestors()
            .skip(1)
            .any(|a| a.ends_with("loop"))));
    }

    #[cfg(unix)]
    #[test]
    fn top_level_symlink_to_a_directory_is_one_symlink_entry() {
        let temp = TempDir::new().unwrap();
        let real = temp.path().join("real");
        write_file(&real.join("a.txt"), "a");
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let dest = temp.path().join("dest");

        let plan = CopyPlan::build(std::slice::from_ref(&link), &dest, true).unwrap();

        assert_eq!(
            plan.entries(),
            [PlanEntry {
                operand: OperandId(0),
                source: link,
                destination: dest,
                kind: EntryKind::Symlink { target: real },
            }]
        );
        assert_eq!(plan.operands()[0].kind, OperandKind::Symlink);
        assert!(plan.tree(OperandId(0)).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn top_level_symlink_to_a_file_is_a_file_entry() {
        let temp = TempDir::new().unwrap();
        let real = temp.path().join("real.txt");
        write_file(&real, "a");
        let link = temp.path().join("link.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let dest = temp.path().join("dest");

        let plan = CopyPlan::build(std::slice::from_ref(&link), &dest, true).unwrap();

        assert_eq!(plan.entries(), [file_entry(0, &link, &dest)]);
        assert_eq!(plan.operands()[0].kind, OperandKind::File);
    }

    #[cfg(unix)]
    #[test]
    fn fifo_in_the_tree_is_skipped_and_kept_in_the_snapshot() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("a.txt"), "a");
        let fifo = src.join("pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        let dest = temp.path().join("dest");

        let plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();

        assert!(plan.entries().iter().all(|entry| entry.source != fifo));
        assert_eq!(
            plan.skipped(),
            [SkippedEntry {
                operand: OperandId(0),
                path: fifo.clone(),
                reason: "fifo",
            }]
        );
        let tree = plan.tree(OperandId(0)).unwrap();
        assert_eq!(tree.nodes().get(&fifo), Some(&NodeKind::Special("fifo")));
    }

    #[test]
    fn directory_into_its_own_subdirectory_is_an_error() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        fs::create_dir_all(src.join("sub")).unwrap();

        let error =
            CopyPlan::build(std::slice::from_ref(&src), &src.join("sub"), true).unwrap_err();

        assert!(
            error.to_string().contains("into itself"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn directory_into_its_own_parent_is_an_error() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("a.txt"), "a");

        let error = CopyPlan::build(std::slice::from_ref(&src), temp.path(), true).unwrap_err();

        assert!(
            error.to_string().contains("into itself"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn directory_onto_an_existing_file_is_an_error() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("a.txt"), "a");
        let dest = temp.path().join("dest.txt");
        write_file(&dest, "old");

        let error = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap_err();

        assert!(
            error.to_string().contains("non-directory"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn source_ending_in_dot_dot_takes_its_name_from_the_canonical_path() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        fs::create_dir_all(src.join("sub")).unwrap();
        write_file(&src.join("a.txt"), "a");
        let dotted = src.join("sub").join("..");
        let dest = temp.path().join("dest");
        fs::create_dir(&dest).unwrap();

        let plan = CopyPlan::build(std::slice::from_ref(&dotted), &dest, true).unwrap();

        assert_eq!(plan.entries()[0].destination, dest.join("src"));
        assert_eq!(plan.entries()[0].kind, EntryKind::Directory);
    }

    #[cfg(unix)]
    #[test]
    fn root_source_has_no_name_and_is_an_error() {
        let temp = TempDir::new().unwrap();

        let error = CopyPlan::build(&[PathBuf::from("/")], temp.path(), true).unwrap_err();

        assert!(
            error.to_string().contains("has no name"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn multibyte_names_map_to_the_matching_destination_paths() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("日本語");
        write_file(&src.join("café.txt"), "c");
        write_file(&src.join("🎉.txt"), "p");
        let dest = temp.path().join("dest");
        fs::create_dir(&dest).unwrap();

        let plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();

        assert_eq!(
            plan.entries(),
            [
                dir_entry(0, &src, &dest.join("日本語")),
                file_entry(0, &src.join("café.txt"), &dest.join("日本語/café.txt")),
                file_entry(0, &src.join("🎉.txt"), &dest.join("日本語/🎉.txt")),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_subdirectory_is_a_walk_error_and_the_rest_is_planned() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("a.txt"), "a");
        let locked = src.join("locked");
        write_file(&locked.join("hidden.txt"), "h");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::read_dir(&locked).is_ok();
        let dest = temp.path().join("dest");

        let result = CopyPlan::build(std::slice::from_ref(&src), &dest, true);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if readable {
            // The test runs as root, so the lock has no effect.
            return;
        }
        let plan = result.unwrap();

        assert_eq!(plan.walk_errors().len(), 1);
        assert_eq!(plan.walk_errors()[0].operand, OperandId(0));
        assert_eq!(plan.walk_errors()[0].path, locked);
        assert!(!plan.walk_errors()[0].message.is_empty());
        assert!(plan
            .entries()
            .iter()
            .any(|entry| entry.source == src.join("a.txt")));
    }

    #[test]
    fn mixed_operands_keep_the_order_given() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("f.txt");
        write_file(&file, "f");
        let src = temp.path().join("src");
        write_file(&src.join("a.txt"), "a");
        let dest = temp.path().join("dest");
        fs::create_dir(&dest).unwrap();

        let plan = CopyPlan::build(&[file.clone(), src.clone()], &dest, true).unwrap();

        assert_eq!(
            plan.entries(),
            [
                file_entry(0, &file, &dest.join("f.txt")),
                dir_entry(1, &src, &dest.join("src")),
                file_entry(1, &src.join("a.txt"), &dest.join("src/a.txt")),
            ]
        );
        let kinds: Vec<OperandKind> = plan.operands().iter().map(|op| op.kind).collect();
        assert_eq!(kinds, [OperandKind::File, OperandKind::Directory]);
        let landings: Vec<PathBuf> = plan
            .operands()
            .iter()
            .map(|op| op.destination.clone())
            .collect();
        assert_eq!(landings, [dest.join("f.txt"), dest.join("src")]);
        assert!(plan.tree(OperandId(0)).is_none());
        assert!(plan.tree(OperandId(1)).is_some());
    }

    #[test]
    fn directory_without_recursive_is_an_error_that_names_the_flag() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("a.txt"), "a");
        let dest = temp.path().join("dest");

        let error = CopyPlan::build(std::slice::from_ref(&src), &dest, false).unwrap_err();

        assert!(
            error.to_string().contains("--recursive"),
            "unexpected error: {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_source_is_not_a_file() {
        let temp = TempDir::new().unwrap();
        let link = temp.path().join("dangling");
        std::os::unix::fs::symlink(temp.path().join("missing"), &link).unwrap();
        let dest = temp.path().join("dest");

        let error = CopyPlan::build(std::slice::from_ref(&link), &dest, true).unwrap_err();

        assert!(
            error.to_string().contains("is not a file"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn queries_count_files_and_list_their_sources() {
        let temp = TempDir::new().unwrap();
        let single = temp.path().join("one.txt");
        write_file(&single, "1");
        let src = temp.path().join("src");
        write_file(&src.join("a.txt"), "a");
        write_file(&src.join("sub").join("b.txt"), "b");
        let dest = temp.path().join("dest");

        let file_plan = CopyPlan::build(std::slice::from_ref(&single), &dest, true).unwrap();
        assert!(file_plan.is_single_file());
        assert_eq!(file_plan.file_count(), 1);
        assert_eq!(file_plan.file_sources(), std::slice::from_ref(&single));

        let tree_plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();
        assert!(!tree_plan.is_single_file());
        assert_eq!(tree_plan.file_count(), 2);
        assert_eq!(
            tree_plan.file_sources(),
            [src.join("a.txt"), src.join("sub").join("b.txt")]
        );

        let one_file_dir = temp.path().join("holder");
        write_file(&one_file_dir.join("only.txt"), "o");
        let holder_plan =
            CopyPlan::build(std::slice::from_ref(&one_file_dir), &dest, true).unwrap();
        assert_eq!(holder_plan.entries().len(), 2);
        assert!(!holder_plan.is_single_file());
        assert_eq!(holder_plan.file_count(), 1);
    }

    #[test]
    fn new_snapshot_sees_a_file_that_the_plan_snapshot_does_not() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("a.txt"), "a");
        let dest = temp.path().join("dest");
        let plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();
        let added = src.join("late.txt");
        write_file(&added, "late");

        let fresh = TreeSnapshot::take(&src);

        assert_eq!(fresh.nodes().get(&added), Some(&NodeKind::File));
        assert_eq!(fresh.nodes().get(&src), Some(&NodeKind::Directory));
        assert!(fresh.errors().is_empty());
        let old = plan.tree(OperandId(0)).unwrap();
        assert!(!old.nodes().contains_key(&added));
        assert!(old.nodes().contains_key(&src.join("a.txt")));
    }

    /// Return the permission bits of a path.
    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    /// Set the permission bits of a path.
    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn apply_directory_permissions_gives_the_destination_the_mode_of_its_source() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("a.txt"), "a");
        set_mode(&src, 0o750);
        let dest = temp.path().join("dest");
        let plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();
        fs::create_dir_all(&dest).unwrap();
        set_mode(&dest, 0o755);

        let errors = plan.apply_directory_permissions();

        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(mode_of(&dest), 0o750);
    }

    #[cfg(unix)]
    #[test]
    fn apply_directory_permissions_sets_a_nested_directory_under_a_read_only_parent() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("sub").join("a.txt"), "a");
        set_mode(&src.join("sub"), 0o700);
        set_mode(&src, 0o555);
        let dest = temp.path().join("dest");
        let plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();
        fs::create_dir_all(dest.join("sub")).unwrap();

        let errors = plan.apply_directory_permissions();

        let parent_mode = mode_of(&dest);
        let child_mode = mode_of(&dest.join("sub"));
        // Make every directory writable again so that TempDir can delete the tree.
        for path in [&src, &dest, &dest.join("sub")] {
            set_mode(path, 0o755);
        }
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(parent_mode, 0o555);
        assert_eq!(child_mode, 0o700);
    }
}

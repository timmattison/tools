//! The move gate: the rule that decides when `--rm` may remove a source.
//!
//! # Rules
//!
//! With `--rm`, `prcp` removes no source while it copies. All removal happens
//! after the copy loop, through this gate. A run that stops early removes
//! nothing at all.
//!
//! The gate works for each operand. One operand is one source path from the
//! command line, after glob expansion. The gate first collects every problem
//! of an operand. Only an operand with no problem has its originals removed.
//! An operand with a problem keeps all of its originals, also the files that
//! copied and verified. Other operands still move.
//!
//! The gate finds these problems:
//!
//! 1. An entry of the operand has no positive record in the ledger. A failed
//!    copy, a failed Blake3 check, a skipped file, and an entry that the loop
//!    did not reach all leave no record.
//! 2. The plan skipped a special file (FIFO, socket, device) under the operand.
//!    The destination is not a full copy.
//! 3. The plan or the gate could not read a part of the tree.
//! 4. A Directory operand changed. The gate walks it again and compares the
//!    walk with the snapshot of the plan. A new path means that a file
//!    appeared after the copy started. A path that left, or a node that
//!    changed kind, also stops the move.
//! 5. A file changed on either side after its Blake3 check. The gate does not
//!    read the data a third time. The Blake3 check in the copy loop is the
//!    hash check. The gate compares the size and the modification time of both
//!    sides with the stamps taken before the copy and right after the check.
//! 6. A symlink no longer reads back with its target, on either side.
//! 7. A destination directory is no longer a directory.
//!
//! The ledger holds positive records only. An entry without a record is a
//! problem. That default is the safety rule: a new failure branch that forgets
//! to record still keeps the originals.
//!
//! # Removal
//!
//! For an operand with no problem, the gate removes each file and symlink
//! source in plan order. It then removes each directory source in reverse plan
//! order, children before parents, with `remove_dir`. That call refuses a
//! directory that is not empty, so a file that appears after the gate also
//! keeps its directory. The gate collects each removal error and goes on.

mod destination;

use crate::plan::{CopyPlan, EntryKind, OperandKind, TreeSnapshot};
use destination::{DestinationTree, TreeChange};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The device and the inode number of a node. Two paths with the same identity are one node.
///
/// A rename keeps the identity. A new node that replaces an old one gets
/// another identity, also when its size, time, and data are equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct NodeIdentity {
    device: u64,
    inode: u64,
}

impl NodeIdentity {
    /// Return the identity in `metadata`.
    ///
    /// Return `None` when the file system gives no inode number. Such a file
    /// system reports 0, and 0 names no node.
    #[cfg(unix)]
    pub(crate) fn of(metadata: &fs::Metadata) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        (metadata.ino() != 0).then(|| Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    /// Return `None`. This platform gives no stable inode number through the standard library.
    #[cfg(not(unix))]
    pub(crate) fn of(_metadata: &fs::Metadata) -> Option<Self> {
        None
    }

    /// Return the identity of the node at `path`. The call does not follow a symlink.
    fn of_node(path: &Path) -> io::Result<Option<Self>> {
        fs::symlink_metadata(path).map(|metadata| Self::of(&metadata))
    }
}

/// The size, the modification time, and the identity of a file at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    identity: Option<NodeIdentity>,
}

impl FileStamp {
    /// Take the stamp from metadata that the caller already holds.
    pub(crate) fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            identity: NodeIdentity::of(metadata),
        }
    }

    /// Take the stamp of `path` now. The call follows a symlink, as the copy does.
    pub(crate) fn of(path: &Path) -> io::Result<Self> {
        fs::metadata(path).map(|metadata| Self::from_metadata(&metadata))
    }
}

/// What the copy loop proved for one plan entry.
#[derive(Debug, Clone, Copy)]
enum Record {
    /// The destination directory exists.
    Directory {
        /// The identity of the destination directory right after the loop made it.
        identity: Option<NodeIdentity>,
    },
    /// The destination link exists and reads back with the target.
    Symlink {
        /// The identity of the destination link right after the loop made it.
        identity: Option<NodeIdentity>,
    },
    /// The destination passed its Blake3 check against the source.
    File {
        /// The stamp of the source before the copy.
        source: FileStamp,
        /// The stamp of the destination right after the Blake3 check.
        destination: FileStamp,
    },
}

/// The positive results of one run, one slot for each plan entry, and the destination before the run.
#[derive(Debug)]
pub(crate) struct MoveLedger<'a> {
    plan: &'a CopyPlan,
    records: Vec<Option<Record>>,
    /// The snapshot of each destination root of a Directory operand, taken before the first copy.
    destination_before: BTreeMap<PathBuf, DestinationTree>,
    /// Every destination path of the plan. prcp writes these, and the records judge them.
    written: BTreeSet<PathBuf>,
}

impl<'a> MoveLedger<'a> {
    /// Make an empty ledger for `plan`. Every entry starts without a record.
    ///
    /// The call also takes the snapshot of each destination tree. Make the
    /// ledger right before the first copy, so that the snapshot shows the
    /// destination as the copy found it.
    pub(crate) fn new(plan: &'a CopyPlan) -> Self {
        let destination_before = plan
            .operands()
            .iter()
            .filter(|operand| operand.kind == OperandKind::Directory)
            .map(|operand| {
                let root = operand.destination.clone();
                let tree = DestinationTree::take(&root);
                (root, tree)
            })
            .collect();
        let written = plan
            .entries()
            .iter()
            .map(|entry| entry.destination.clone())
            .collect();
        Self {
            plan,
            records: vec![None; plan.entries().len()],
            destination_before,
            written,
        }
    }

    /// Record that the directory of entry `entry` exists at the destination.
    ///
    /// The call reads the identity of the directory now. When that read fails,
    /// the entry stays without a record, so the gate keeps its originals.
    pub(crate) fn record_directory(&mut self, entry: usize) {
        if let Some(identity) = self.destination_identity(entry) {
            self.set(entry, Record::Directory { identity });
        }
    }

    /// Record that the symlink of entry `entry` was made and read back.
    ///
    /// The call reads the identity of the link now. When that read fails, the
    /// entry stays without a record, so the gate keeps its originals.
    pub(crate) fn record_symlink(&mut self, entry: usize) {
        if let Some(identity) = self.destination_identity(entry) {
            self.set(entry, Record::Symlink { identity });
        }
    }

    /// Read the identity of the destination of entry `entry` now. `None` when the read fails.
    fn destination_identity(&self, entry: usize) -> Option<Option<NodeIdentity>> {
        let plan_entry = self.plan.entries().get(entry)?;
        NodeIdentity::of_node(&plan_entry.destination).ok()
    }

    /// Record that the file of entry `entry` passed its Blake3 check.
    ///
    /// Call this right after the check. The call stamps the destination now.
    /// When that stamp fails, the entry stays without a record, so the gate
    /// keeps its originals.
    pub(crate) fn record_file(&mut self, entry: usize, source_before_copy: FileStamp) {
        let Some(plan_entry) = self.plan.entries().get(entry) else {
            return;
        };
        if let Ok(destination) = FileStamp::of(&plan_entry.destination) {
            self.set(
                entry,
                Record::File {
                    source: source_before_copy,
                    destination,
                },
            );
        }
    }

    /// Store a record. An index outside the plan is ignored.
    fn set(&mut self, entry: usize, record: Record) {
        if let Some(slot) = self.records.get_mut(entry) {
            *slot = Some(record);
        }
    }

    /// Run the gate for every operand, then remove the originals of each operand that passed.
    pub(crate) fn finish(self) -> MoveReport {
        let mut report = MoveReport {
            removed: Vec::new(),
            kept: Vec::new(),
            removal_errors: Vec::new(),
            operands_with_removal_errors: 0,
        };
        for (index, operand) in self.plan.operands().iter().enumerate() {
            let problems = self.find_problems(index);
            if !problems.is_empty() {
                report.kept.push(KeptOperand {
                    source: operand.source.clone(),
                    problems,
                });
                continue;
            }
            let errors = self.remove_originals(index);
            if errors.is_empty() {
                report.removed.push(operand.source.clone());
            } else {
                report.operands_with_removal_errors += 1;
                report.removal_errors.extend(errors);
            }
        }
        report
    }

    /// Collect every problem of one operand. See [`Findings`] for how the list is made.
    fn find_problems(&self, operand: usize) -> BTreeMap<PathBuf, Problem> {
        let mut findings = Findings::default();
        for (index, entry) in self.plan.entries().iter().enumerate() {
            if entry.operand.index() != operand {
                continue;
            }
            match self.records[index] {
                None => findings.note(&entry.source, Problem::NotCopied),
                Some(Record::File {
                    source,
                    destination,
                }) => {
                    if let Some(problem) = stamp_problem(
                        &entry.source,
                        source,
                        Problem::MissingAtSource,
                        Problem::SourceChanged,
                    ) {
                        findings.note(&entry.source, problem);
                    }
                    let problem = stamp_problem(
                        &entry.destination,
                        destination,
                        Problem::MissingAtDestination,
                        Problem::DestinationChanged,
                    );
                    findings.note_destination(&entry.destination, problem, destination.identity);
                }
                Some(Record::Symlink { identity }) => {
                    if let EntryKind::Symlink { target } = &entry.kind {
                        if let Some(problem) = link_problem(
                            &entry.source,
                            target,
                            Problem::MissingAtSource,
                            Problem::SourceChanged,
                        ) {
                            findings.note(&entry.source, problem);
                        }
                        let problem = link_problem(
                            &entry.destination,
                            target,
                            Problem::MissingAtDestination,
                            Problem::DestinationChanged,
                        )
                        .or_else(|| identity_problem(&entry.destination, identity));
                        findings.note_destination(&entry.destination, problem, identity);
                    }
                }
                Some(Record::Directory { identity }) => {
                    let problem = directory_problem(&entry.destination, identity);
                    findings.note_destination(&entry.destination, problem, identity);
                }
            }
        }
        for skipped in self.plan.skipped() {
            if skipped.operand.index() == operand {
                findings.note(&skipped.path, Problem::Skipped(skipped.reason));
            }
        }
        for error in self.plan.walk_errors() {
            if error.operand.index() == operand {
                findings.note(&error.path, Problem::Unreadable(error.message.clone()));
            }
        }
        self.compare_tree(operand, &mut findings);
        self.compare_destination(operand, &mut findings);
        findings.finish()
    }

    /// Walk the destination root of a Directory operand again, and compare it with the snapshot.
    ///
    /// A path that prcp did not write, and that was not there before, is
    /// `NewAtDestination`. A path from before that changed is
    /// `DestinationChanged`, and one that left is `MissingAtDestination`. An
    /// operand that is not a Directory has no tree to walk, and the call does
    /// nothing.
    fn compare_destination(&self, operand: usize, findings: &mut Findings) {
        let Some(root) = self.plan.operands().get(operand) else {
            return;
        };
        let Some(before) = self.destination_before.get(&root.destination) else {
            return;
        };
        let now = DestinationTree::take(&root.destination);
        for change in now.changes_since(before, &self.written) {
            match change {
                TreeChange::New { path, identity } => {
                    findings.note(&path, Problem::NewAtDestination);
                    if let Some(identity) = identity {
                        findings.appeared.entry(identity).or_insert(path);
                    }
                }
                TreeChange::Changed { path } => findings.note(&path, Problem::DestinationChanged),
                TreeChange::Missing { path, identity } => {
                    findings.note_destination(&path, Some(Problem::MissingAtDestination), identity);
                }
                TreeChange::Unreadable { path, message } => {
                    findings.note(&path, Problem::Unreadable(message));
                }
            }
        }
    }

    /// Walk a Directory operand again and compare the walk with the snapshot of the plan.
    ///
    /// A new path is `NewSinceCopy`. A path that left is `MissingAtSource`. A
    /// path with another kind is `SourceChanged`. An operand that is not a
    /// Directory has no snapshot, and the call does nothing.
    fn compare_tree(&self, operand: usize, findings: &mut Findings) {
        let Some(before) = self
            .plan
            .entries()
            .iter()
            .find(|entry| entry.operand.index() == operand)
            .and_then(|entry| self.plan.tree(entry.operand))
        else {
            return;
        };
        let Some(root) = self.plan.operands().get(operand) else {
            return;
        };
        let now = TreeSnapshot::take(&root.source);
        for (path, message) in now.errors() {
            findings.note(path, Problem::Unreadable(message.clone()));
        }
        for (path, kind) in now.nodes() {
            match before.nodes().get(path) {
                None => findings.note(path, Problem::NewSinceCopy),
                Some(old) if old != kind => findings.note(path, Problem::SourceChanged),
                Some(_) => {}
            }
        }
        for path in before.nodes().keys() {
            if !now.nodes().contains_key(path) {
                findings.note(path, Problem::MissingAtSource);
            }
        }
    }

    /// Remove the originals of one operand. Return each removal that failed.
    ///
    /// The call removes files and symlinks first, in plan order. It then
    /// removes directories in reverse plan order with `remove_dir`, which
    /// refuses a directory that is not empty.
    fn remove_originals(&self, operand: usize) -> Vec<(PathBuf, String)> {
        let entries = || {
            self.plan
                .entries()
                .iter()
                .filter(move |entry| entry.operand.index() == operand)
        };
        let mut errors = Vec::new();
        for entry in entries().filter(|entry| entry.kind != EntryKind::Directory) {
            if let Err(error) = fs::remove_file(&entry.source) {
                errors.push((entry.source.clone(), error.to_string()));
            }
        }
        let directories: Vec<_> = entries()
            .filter(|entry| entry.kind == EntryKind::Directory)
            .collect();
        for entry in directories.into_iter().rev() {
            if let Err(error) = fs::remove_dir(&entry.source) {
                errors.push((entry.source.clone(), error.to_string()));
            }
        }
        errors
    }
}

/// The problems of one operand while the gate collects them.
///
/// A path keeps the first problem noted for it. Two rules then shape the list
/// for a person:
///
/// 1. A destination node that left one path and appeared at another, with
///    the same identity, is one `MovedAtDestination` at the old path. The new
///    path then has no line of its own.
/// 2. A problem at a path stands for everything below that path. The list
///    keeps the topmost path of each tree of problems, so a tree that was
///    removed, moved, or added is one line, not one line for each node.
#[derive(Debug, Default)]
struct Findings {
    problems: BTreeMap<PathBuf, Problem>,
    /// Each destination path that left, with the identity that its node had.
    gone: Vec<(PathBuf, NodeIdentity)>,
    /// Each new destination path, by the identity of its node. The first path wins.
    appeared: BTreeMap<NodeIdentity, PathBuf>,
}

impl Findings {
    /// Note a problem for a path. A path that already has a problem keeps the first one.
    fn note(&mut self, path: &Path, problem: Problem) {
        self.problems.entry(path.to_path_buf()).or_insert(problem);
    }

    /// Note the result of a check of a destination path, and the identity that its node had.
    ///
    /// A path that left is kept with its identity, so [`Self::finish`] can find
    /// where the node went.
    fn note_destination(
        &mut self,
        path: &Path,
        problem: Option<Problem>,
        identity: Option<NodeIdentity>,
    ) {
        let Some(problem) = problem else {
            return;
        };
        if let (Problem::MissingAtDestination, Some(identity)) = (&problem, identity) {
            self.gone.push((path.to_path_buf(), identity));
        }
        self.note(path, problem);
    }

    /// Apply the two rules of [`Findings`] and return the list.
    fn finish(mut self) -> BTreeMap<PathBuf, Problem> {
        for (old, identity) in std::mem::take(&mut self.gone) {
            let Some(new) = self.appeared.remove(&identity) else {
                continue;
            };
            if self.problems.get(&new) == Some(&Problem::NewAtDestination) {
                self.problems.remove(&new);
            }
            self.problems
                .insert(old, Problem::MovedAtDestination { to: new });
        }
        // The map is in path order, and every path below a path comes right after it.
        let mut list = BTreeMap::new();
        let mut top: Option<PathBuf> = None;
        for (path, problem) in self.problems {
            if top.as_ref().is_some_and(|top| path.starts_with(top)) {
                continue;
            }
            top = Some(path.clone());
            list.insert(path, problem);
        }
        list
    }
}

/// Compare the file at `path` with the stamp taken earlier.
///
/// Return `missing` when the file is gone and `changed` when its size or
/// modification time differs. Another read error is `Unreadable`. Return
/// `None` when the stamp still matches.
fn stamp_problem(
    path: &Path,
    expected: FileStamp,
    missing: Problem,
    changed: Problem,
) -> Option<Problem> {
    match FileStamp::of(path) {
        Ok(now) if now == expected => None,
        Ok(_) => Some(changed),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(missing),
        Err(error) => Some(Problem::Unreadable(error.to_string())),
    }
}

/// Compare the symlink at `path` with the target text that the plan holds.
///
/// Return `missing` when nothing is at the path and `changed` when the link
/// has another target, or when the path is not a link. Another read error is
/// `Unreadable`. Return `None` when the link still reads back with `target`.
fn link_problem(path: &Path, target: &Path, missing: Problem, changed: Problem) -> Option<Problem> {
    match fs::read_link(path) {
        Ok(now) if now == target => None,
        Ok(_) => Some(changed),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(missing),
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => Some(changed),
        Err(error) => Some(Problem::Unreadable(error.to_string())),
    }
}

/// Compare the directory at `path` with the identity that its record holds.
///
/// Return `MissingAtDestination` when nothing is at the path, and
/// `DestinationChanged` when the path is no longer a directory or is another
/// directory. Another read error is `Unreadable`. Return `None` when the
/// directory is still the one that the loop made.
fn directory_problem(path: &Path, identity: Option<NodeIdentity>) -> Option<Problem> {
    match fs::metadata(path) {
        Ok(found) if found.is_dir() => identity_problem(path, identity),
        Ok(_) => Some(Problem::DestinationChanged),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Some(Problem::MissingAtDestination)
        }
        Err(error) => Some(Problem::Unreadable(error.to_string())),
    }
}

/// Compare the identity of the node at `path` with the identity that a record holds.
///
/// Return `DestinationChanged` when another node is at the path, and
/// `Unreadable` when the read fails. Return `None` when the node is the same.
fn identity_problem(path: &Path, identity: Option<NodeIdentity>) -> Option<Problem> {
    match NodeIdentity::of_node(path) {
        Ok(now) if now == identity => None,
        Ok(_) => Some(Problem::DestinationChanged),
        Err(error) => Some(Problem::Unreadable(error.to_string())),
    }
}

/// The result of the gate and of the removal.
#[derive(Debug)]
pub(crate) struct MoveReport {
    /// The operand sources whose originals are all removed.
    pub(crate) removed: Vec<PathBuf>,
    /// The operands that kept their originals, with the reasons.
    pub(crate) kept: Vec<KeptOperand>,
    /// Each removal that failed, with the text of the error.
    pub(crate) removal_errors: Vec<(PathBuf, String)>,
    operands_with_removal_errors: usize,
}

impl MoveReport {
    /// Return true when no operand kept its originals and no removal failed.
    pub(crate) fn is_complete(&self) -> bool {
        self.kept.is_empty() && self.removal_errors.is_empty()
    }

    /// Write the report for a person: one block for each kept operand, then one line
    /// for each failed removal.
    ///
    /// When a problem comes from outside prcp, the report ends with
    /// [`OUTSIDE_CHANGE_NOTE`], so a person does not think that prcp is broken.
    pub(crate) fn error_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for operand in &self.kept {
            lines.push(format!(
                "Kept the originals of '{}'. prcp removed nothing from it:",
                operand.source.display()
            ));
            for (path, problem) in &operand.problems {
                lines.push(format!("  '{}' {problem}", path.display()));
            }
        }
        for (path, error) in &self.removal_errors {
            lines.push(format!("Cannot remove '{}': {error}", path.display()));
        }
        let outside = self
            .kept
            .iter()
            .flat_map(|operand| operand.problems.values())
            .any(Problem::is_outside_change);
        if outside {
            lines.extend(OUTSIDE_CHANGE_NOTE.iter().map(|line| (*line).to_string()));
        }
        lines
    }

    /// Count the operands whose originals stay: kept by the gate, or left by a failed removal.
    pub(crate) fn unfinished_count(&self) -> usize {
        self.kept.len() + self.operands_with_removal_errors
    }
}

/// One operand that kept its originals.
#[derive(Debug)]
pub(crate) struct KeptOperand {
    /// The source path of the operand.
    pub(crate) source: PathBuf,
    /// The first problem of each path, in path order.
    pub(crate) problems: BTreeMap<PathBuf, Problem>,
}

/// The note at the end of a report that holds a change from outside prcp.
const OUTSIDE_CHANGE_NOTE: [&str; 2] = [
    "Something outside prcp changed the source or the destination while prcp ran.",
    "Another program or a person made those changes, not prcp. Make sure that \
     nothing else uses these paths, then run prcp again.",
];

/// One reason why the gate keeps the originals of an operand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Problem {
    /// The entry has no positive record.
    NotCopied,
    /// The plan skipped a special file of this kind.
    Skipped(&'static str),
    /// A part of the tree could not be read. The text names the error.
    Unreadable(String),
    /// The path appeared in the source after the plan was made.
    NewSinceCopy,
    /// The path is no longer in the source.
    MissingAtSource,
    /// The path is missing at the destination.
    MissingAtDestination,
    /// The source changed after its copy.
    SourceChanged,
    /// The destination changed after its hash check.
    DestinationChanged,
    /// The path appeared at the destination, and prcp did not make it.
    NewAtDestination,
    /// The node left this path at the destination, and is now at another path.
    MovedAtDestination {
        /// The path where the node is now.
        to: PathBuf,
    },
}

impl Problem {
    /// Return true when something outside prcp caused the problem.
    ///
    /// A path that appeared, changed, or left after prcp read or wrote it is
    /// such a change. A failed copy, a skipped special file, and a part that
    /// cannot be read are not, because prcp reports those as they occur.
    fn is_outside_change(&self) -> bool {
        match self {
            Self::NotCopied | Self::Skipped(_) | Self::Unreadable(_) => false,
            Self::NewSinceCopy
            | Self::MissingAtSource
            | Self::MissingAtDestination
            | Self::SourceChanged
            | Self::DestinationChanged
            | Self::NewAtDestination
            | Self::MovedAtDestination { .. } => true,
        }
    }
}

impl fmt::Display for Problem {
    /// Write the problem as a phrase that follows a quoted path.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotCopied => f.write_str("was not copied and verified"),
            Self::Skipped(kind) => write!(f, "is a {kind}, and prcp does not copy it"),
            Self::Unreadable(message) => write!(f, "cannot be read: {message}"),
            Self::NewSinceCopy => {
                f.write_str("appeared in the source after the copy started, and was not copied")
            }
            Self::MissingAtSource => f.write_str("is no longer in the source"),
            Self::MissingAtDestination => {
                f.write_str("was removed or moved away from the destination during the run")
            }
            Self::SourceChanged => f.write_str("changed in the source after its copy"),
            Self::DestinationChanged => f.write_str("changed at the destination during the run"),
            Self::NewAtDestination => {
                f.write_str("appeared at the destination during the run, and prcp did not make it")
            }
            Self::MovedAtDestination { to } => {
                write!(f, "was renamed or moved to '{}' during the run", to.display())
            }
        }
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
    use tempfile::TempDir;

    /// A source tree, a destination path, and the plan between them.
    struct Fixture {
        _temp: TempDir,
        src: PathBuf,
        dest: PathBuf,
        plan: CopyPlan,
    }

    /// Write a file and make its parent directories.
    fn write_file(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// Make the sample tree: files at three depths, an empty directory, and on Unix a symlink.
    fn make_sample_tree(root: &Path) {
        write_file(&root.join("top.txt"), "top");
        write_file(&root.join("sub").join("inner.txt"), "inner");
        write_file(&root.join("sub").join("deeper").join("deep.txt"), "deep");
        fs::create_dir_all(root.join("empty")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("top.txt", root.join("link")).unwrap();
    }

    /// Make the fixture: the sample tree and a recursive plan over it.
    fn sample() -> Fixture {
        sample_with(|_| {})
    }

    /// Make the fixture like `sample`, but let `prepare` change the source tree before the plan.
    fn sample_with(prepare: impl FnOnce(&Path)) -> Fixture {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        let dest = temp.path().join("dest");
        make_sample_tree(&src);
        prepare(&src);
        let plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();
        Fixture {
            _temp: temp,
            src,
            dest,
            plan,
        }
    }

    /// Make the fixture like `sample`, but into a destination directory that exists.
    ///
    /// The copy then lands in `dest/src`. `prepare` gets that root and fills it
    /// before the plan. The fixture names that root as its `dest`.
    fn sample_into_existing(prepare: impl FnOnce(&Path)) -> Fixture {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        let dest = temp.path().join("dest");
        make_sample_tree(&src);
        let root = dest.join("src");
        fs::create_dir_all(&root).unwrap();
        prepare(&root);
        let plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();
        Fixture {
            _temp: temp,
            src,
            dest: root,
            plan,
        }
    }

    /// Copy every entry of the plan as the copy loop does, except the sources in `skip`.
    /// A skipped entry gets no record, as after a failed Blake3 check.
    fn copy_all<'a>(plan: &'a CopyPlan, skip: &[&Path]) -> MoveLedger<'a> {
        let mut ledger = MoveLedger::new(plan);
        for (index, entry) in plan.entries().iter().enumerate() {
            match &entry.kind {
                EntryKind::Directory => {
                    fs::create_dir_all(&entry.destination).unwrap();
                    ledger.record_directory(index);
                }
                EntryKind::File => {
                    let stamp = FileStamp::of(&entry.source).unwrap();
                    fs::copy(&entry.source, &entry.destination).unwrap();
                    if !skip.contains(&entry.source.as_path()) {
                        ledger.record_file(index, stamp);
                    }
                }
                EntryKind::Symlink { target } => {
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(target, &entry.destination).unwrap();
                    #[cfg(not(unix))]
                    let _ = target;
                    ledger.record_symlink(index);
                }
            }
        }
        ledger
    }

    /// Return every source path of the plan that is a file or a symlink or a directory.
    fn all_sources(plan: &CopyPlan) -> Vec<PathBuf> {
        plan.entries()
            .iter()
            .map(|entry| entry.source.clone())
            .collect()
    }

    /// Assert that every original of the plan still exists.
    fn assert_originals_exist(plan: &CopyPlan) {
        for source in all_sources(plan) {
            assert!(
                fs::symlink_metadata(&source).is_ok(),
                "the original '{}' must stay",
                source.display()
            );
        }
    }

    /// Return the problems that the report holds for the one kept operand.
    fn only_problems(report: &MoveReport) -> &BTreeMap<PathBuf, Problem> {
        assert_eq!(report.kept.len(), 1, "report: {report:?}");
        &report.kept[0].problems
    }

    #[test]
    fn a_clean_move_removes_the_whole_source_tree() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);

        let report = ledger.finish();

        assert!(report.is_complete(), "report: {report:?}");
        assert_eq!(report.removed, vec![fixture.src.clone()]);
        assert!(!fixture.src.exists(), "the source root must be gone");
        assert!(fixture.dest.join("top.txt").exists());
        assert!(fixture.dest.join("sub").join("inner.txt").exists());
        assert!(fixture
            .dest
            .join("sub")
            .join("deeper")
            .join("deep.txt")
            .exists());
        assert!(fixture.dest.join("empty").is_dir());
    }

    #[test]
    fn an_entry_without_a_record_keeps_all_originals() {
        let fixture = sample();
        let unverified = fixture.src.join("sub").join("inner.txt");
        let ledger = copy_all(&fixture.plan, &[unverified.as_path()]);

        let report = ledger.finish();

        assert!(!report.is_complete());
        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&unverified),
            Some(&Problem::NotCopied)
        );
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_file_that_appears_after_the_plan_keeps_all_originals() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let late = fixture.src.join("sub").join("late.txt");
        write_file(&late, "late");

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&late),
            Some(&Problem::NewSinceCopy)
        );
        assert_originals_exist(&fixture.plan);
        assert!(late.exists());
        assert!(!fixture.dest.join("sub").join("late.txt").exists());
        assert!(fixture.dest.join("top.txt").exists());
    }

    #[test]
    fn a_source_file_that_is_deleted_keeps_all_originals() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let gone = fixture.src.join("sub").join("inner.txt");
        fs::remove_file(&gone).unwrap();

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&gone),
            Some(&Problem::MissingAtSource)
        );
        assert!(fixture.src.join("top.txt").exists());
        assert!(fixture
            .src
            .join("sub")
            .join("deeper")
            .join("deep.txt")
            .exists());
        assert!(fixture.src.join("empty").is_dir());
    }

    #[test]
    fn a_destination_file_that_is_deleted_keeps_all_originals() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let copy = fixture.dest.join("sub").join("inner.txt");
        fs::remove_file(&copy).unwrap();

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&copy),
            Some(&Problem::MissingAtDestination)
        );
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_source_file_with_a_new_length_keeps_all_originals() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let changed = fixture.src.join("top.txt");
        fs::write(&changed, "a much longer text than before").unwrap();

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&changed),
            Some(&Problem::SourceChanged)
        );
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_source_file_with_a_new_mtime_keeps_all_originals() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let changed = fixture.src.join("top.txt");
        let before = fs::metadata(&changed).unwrap().modified().unwrap();
        let file = fs::File::options().write(true).open(&changed).unwrap();
        file.set_modified(before + std::time::Duration::from_secs(10))
            .unwrap();
        drop(file);

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&changed),
            Some(&Problem::SourceChanged)
        );
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_destination_file_that_changes_after_its_record_keeps_all_originals() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let copy = fixture.dest.join("sub").join("deeper").join("deep.txt");
        fs::write(&copy, "changed at the destination").unwrap();

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&copy),
            Some(&Problem::DestinationChanged)
        );
        assert_originals_exist(&fixture.plan);
    }

    /// Put a new node at `path` with `make`, and keep the old node until the new one exists.
    ///
    /// The new node is made beside the old one, then renamed over it. The two
    /// nodes thus exist at the same time, so the file system cannot give the new
    /// node the inode number of the old one.
    fn replace_node(path: &Path, make: impl FnOnce(&Path)) {
        let mut name = path.file_name().unwrap().to_os_string();
        name.push(".replacement");
        let replacement = path.with_file_name(name);
        make(&replacement);
        if fs::symlink_metadata(path).unwrap().is_dir() {
            fs::remove_dir(path).unwrap();
        }
        fs::rename(&replacement, path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_destination_file_replaced_with_the_same_size_and_mtime_keeps_all_originals() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let copy = fixture.dest.join("top.txt");
        let modified = fs::metadata(&copy).unwrap().modified().unwrap();
        replace_node(&copy, |replacement| {
            fs::write(replacement, "TOP").unwrap();
            let file = fs::File::options().write(true).open(replacement).unwrap();
            file.set_modified(modified).unwrap();
        });
        assert_eq!(fs::read_to_string(&copy).unwrap().len(), "top".len());
        assert_eq!(fs::metadata(&copy).unwrap().modified().unwrap(), modified);

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&copy),
            Some(&Problem::DestinationChanged)
        );
        assert_originals_exist(&fixture.plan);
    }

    #[cfg(unix)]
    #[test]
    fn a_destination_directory_replaced_by_a_new_one_keeps_all_originals() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let copy = fixture.dest.join("empty");
        replace_node(&copy, |replacement| fs::create_dir(replacement).unwrap());

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&copy),
            Some(&Problem::DestinationChanged)
        );
        assert_originals_exist(&fixture.plan);
    }

    #[cfg(unix)]
    #[test]
    fn a_destination_symlink_replaced_by_an_equal_one_keeps_all_originals() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let copy = fixture.dest.join("link");
        replace_node(&copy, |replacement| {
            std::os::unix::fs::symlink("top.txt", replacement).unwrap();
        });

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&copy),
            Some(&Problem::DestinationChanged)
        );
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_file_that_appears_at_the_destination_keeps_all_originals() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let intruder = fixture.dest.join("sub").join("new.txt");
        write_file(&intruder, "new");

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&intruder),
            Some(&Problem::NewAtDestination)
        );
        assert_originals_exist(&fixture.plan);
        assert!(intruder.exists());
    }

    #[test]
    fn a_destination_that_held_files_before_the_copy_moves_cleanly() {
        let fixture = sample_into_existing(|root| {
            write_file(&root.join("extra.txt"), "extra");
            write_file(&root.join("old").join("kept.txt"), "kept");
        });
        let ledger = copy_all(&fixture.plan, &[]);

        let report = ledger.finish();

        assert!(report.is_complete(), "report: {report:?}");
        assert!(!fixture.src.exists(), "the source tree must be gone");
        assert_eq!(
            fs::read_to_string(fixture.dest.join("extra.txt")).unwrap(),
            "extra"
        );
        assert_eq!(
            fs::read_to_string(fixture.dest.join("old").join("kept.txt")).unwrap(),
            "kept"
        );
        assert!(fixture.dest.join("top.txt").exists());
    }

    #[test]
    fn a_destination_file_from_before_the_copy_that_changes_keeps_all_originals() {
        let fixture = sample_into_existing(|root| write_file(&root.join("extra.txt"), "extra"));
        let ledger = copy_all(&fixture.plan, &[]);
        let extra = fixture.dest.join("extra.txt");
        fs::write(&extra, "a longer text than before").unwrap();

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&extra),
            Some(&Problem::DestinationChanged)
        );
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_destination_file_from_before_the_copy_that_is_removed_keeps_all_originals() {
        let fixture = sample_into_existing(|root| write_file(&root.join("extra.txt"), "extra"));
        let ledger = copy_all(&fixture.plan, &[]);
        let extra = fixture.dest.join("extra.txt");
        fs::remove_file(&extra).unwrap();

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&extra),
            Some(&Problem::MissingAtDestination)
        );
        assert_originals_exist(&fixture.plan);
    }

    #[cfg(unix)]
    #[test]
    fn a_destination_part_that_was_unreadable_before_the_copy_does_not_stop_the_move() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = sample_into_existing(|root| {
            write_file(&root.join("locked").join("x.txt"), "x");
            fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
        });
        let locked = fixture.dest.join("locked");
        if fs::read_dir(&locked).is_ok() {
            // The lock does not work for root.
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        let ledger = copy_all(&fixture.plan, &[]);

        let report = ledger.finish();

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(report.is_complete(), "report: {report:?}");
        assert!(!fixture.src.exists(), "the source tree must be gone");
    }

    /// Return the problems of the one kept operand as a list in path order.
    fn problem_list(report: &MoveReport) -> Vec<(PathBuf, Problem)> {
        only_problems(report)
            .iter()
            .map(|(path, problem)| (path.clone(), problem.clone()))
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn a_destination_file_that_is_renamed_is_reported_once_with_its_new_name() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let copy = fixture.dest.join("top.txt");
        let renamed = fixture.dest.join("top-renamed.txt");
        fs::rename(&copy, &renamed).unwrap();

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            problem_list(&report),
            vec![(copy, Problem::MovedAtDestination { to: renamed })]
        );
        assert_originals_exist(&fixture.plan);
    }

    #[cfg(unix)]
    #[test]
    fn a_destination_directory_that_is_renamed_is_reported_once_with_its_new_name() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let copy = fixture.dest.join("sub");
        let renamed = fixture.dest.join("sub-renamed");
        fs::rename(&copy, &renamed).unwrap();

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            problem_list(&report),
            vec![(copy, Problem::MovedAtDestination { to: renamed })]
        );
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_new_destination_directory_is_reported_once() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let intruder = fixture.dest.join("new-dir");
        write_file(&intruder.join("a.txt"), "a");
        write_file(&intruder.join("deeper").join("b.txt"), "b");

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            problem_list(&report),
            vec![(intruder, Problem::NewAtDestination)]
        );
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_destination_directory_that_is_removed_is_reported_once() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let copy = fixture.dest.join("sub");
        fs::remove_dir_all(&copy).unwrap();

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            problem_list(&report),
            vec![(copy, Problem::MissingAtDestination)]
        );
        assert_originals_exist(&fixture.plan);
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_in_the_tree_keeps_all_originals() {
        let fixture = sample_with(|src| {
            let status = std::process::Command::new("mkfifo")
                .arg(src.join("pipe"))
                .status()
                .unwrap();
            assert!(status.success(), "mkfifo must work");
        });
        let ledger = copy_all(&fixture.plan, &[]);

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&fixture.src.join("pipe")),
            Some(&Problem::Skipped("fifo"))
        );
        assert_originals_exist(&fixture.plan);
        assert!(fixture.src.join("pipe").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_part_of_the_tree_that_the_plan_could_not_read_keeps_all_originals() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = sample_with(|src| {
            write_file(&src.join("locked").join("x.txt"), "x");
            fs::set_permissions(src.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
        });
        let locked = fixture.src.join("locked");
        if fs::read_dir(&locked).is_ok() {
            // The lock does not work for root.
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        let ledger = copy_all(&fixture.plan, &[]);
        // Open the directory again. The plan still holds the walk error.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert!(
            matches!(
                only_problems(&report).get(&locked),
                Some(Problem::Unreadable(_))
            ),
            "report: {report:?}"
        );
        assert_originals_exist(&fixture.plan);
    }

    #[cfg(unix)]
    #[test]
    fn a_destination_symlink_with_a_new_target_keeps_all_originals() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let copy = fixture.dest.join("link");
        fs::remove_file(&copy).unwrap();
        std::os::unix::fs::symlink("other.txt", &copy).unwrap();

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&copy),
            Some(&Problem::DestinationChanged)
        );
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_destination_directory_that_is_gone_keeps_all_originals() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let copy = fixture.dest.join("empty");
        fs::remove_dir(&copy).unwrap();

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&copy),
            Some(&Problem::MissingAtDestination)
        );
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_good_operand_moves_and_a_bad_operand_keeps_all_originals() {
        let temp = TempDir::new().unwrap();
        let good = temp.path().join("good");
        let bad = temp.path().join("bad");
        let dest = temp.path().join("dest");
        make_sample_tree(&good);
        make_sample_tree(&bad);
        let plan = CopyPlan::build(&[good.clone(), bad.clone()], &dest, true).unwrap();
        let ledger = copy_all(&plan, &[]);
        let late = bad.join("late.txt");
        write_file(&late, "late");

        let report = ledger.finish();

        assert_eq!(report.removed, vec![good.clone()], "report: {report:?}");
        assert_eq!(report.kept.len(), 1);
        assert_eq!(report.kept[0].source, bad);
        assert_eq!(
            report.kept[0].problems.get(&late),
            Some(&Problem::NewSinceCopy)
        );
        assert!(!good.exists(), "the good operand must be gone");
        assert!(bad.join("top.txt").exists());
        assert!(bad.join("sub").join("deeper").join("deep.txt").exists());
        assert!(bad.join("empty").is_dir());
        assert!(dest.join("good").join("top.txt").exists());
        assert!(dest.join("bad").join("top.txt").exists());
        assert_eq!(report.unfinished_count(), 1);
    }

    #[test]
    fn file_operands_that_all_verified_are_removed() {
        let temp = TempDir::new().unwrap();
        let first = temp.path().join("first.txt");
        let second = temp.path().join("second.txt");
        let dest = temp.path().join("dest");
        write_file(&first, "first");
        write_file(&second, "second");
        fs::create_dir(&dest).unwrap();
        let plan = CopyPlan::build(&[first.clone(), second.clone()], &dest, false).unwrap();
        let ledger = copy_all(&plan, &[]);

        let report = ledger.finish();

        assert!(report.is_complete(), "report: {report:?}");
        assert_eq!(report.removed, vec![first.clone(), second.clone()]);
        assert!(!first.exists());
        assert!(!second.exists());
        assert_eq!(fs::read_to_string(dest.join("first.txt")).unwrap(), "first");
        assert_eq!(
            fs::read_to_string(dest.join("second.txt")).unwrap(),
            "second"
        );
    }

    #[test]
    fn a_file_operand_without_a_record_stays_and_the_other_moves() {
        let temp = TempDir::new().unwrap();
        let first = temp.path().join("first.txt");
        let second = temp.path().join("second.txt");
        let dest = temp.path().join("dest");
        write_file(&first, "first");
        write_file(&second, "second");
        fs::create_dir(&dest).unwrap();
        let plan = CopyPlan::build(&[first.clone(), second.clone()], &dest, false).unwrap();
        let ledger = copy_all(&plan, &[second.as_path()]);

        let report = ledger.finish();

        assert!(!report.is_complete());
        assert_eq!(report.removed, vec![first.clone()]);
        assert_eq!(
            report.kept[0].problems.get(&second),
            Some(&Problem::NotCopied)
        );
        assert!(!first.exists());
        assert!(second.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_removal_that_fails_is_reported_and_the_operand_is_unfinished() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let locked_parent = temp.path().join("parent");
        let file = locked_parent.join("file.txt");
        let dest = temp.path().join("dest");
        write_file(&file, "content");
        let other = temp.path().join("other.txt");
        write_file(&other, "other");
        fs::create_dir(&dest).unwrap();
        let plan = CopyPlan::build(&[file.clone(), other.clone()], &dest, false).unwrap();
        let ledger = copy_all(&plan, &[]);
        fs::set_permissions(&locked_parent, fs::Permissions::from_mode(0o555)).unwrap();
        let writable_anyway = fs::File::create(locked_parent.join("probe")).is_ok();

        let report = ledger.finish();

        fs::set_permissions(&locked_parent, fs::Permissions::from_mode(0o755)).unwrap();
        if writable_anyway {
            // The lock does not work for root.
            return;
        }
        assert!(!report.is_complete(), "report: {report:?}");
        assert_eq!(report.removed, vec![other]);
        assert_eq!(report.removal_errors.len(), 1);
        assert_eq!(report.removal_errors[0].0, file);
        assert_eq!(report.unfinished_count(), 1);
        assert!(file.exists());
    }

    #[test]
    fn the_report_lists_each_kept_operand_with_its_problems_and_each_failed_removal() {
        let mut problems = BTreeMap::new();
        problems.insert(PathBuf::from("/s/late.txt"), Problem::NewSinceCopy);
        problems.insert(PathBuf::from("/s/old.txt"), Problem::NotCopied);
        let report = MoveReport {
            removed: Vec::new(),
            kept: vec![KeptOperand {
                source: PathBuf::from("/s"),
                problems,
            }],
            removal_errors: vec![(PathBuf::from("/t/x"), "denied".to_string())],
            operands_with_removal_errors: 1,
        };

        assert_eq!(
            report.error_lines(),
            vec![
                "Kept the originals of '/s'. prcp removed nothing from it:",
                "  '/s/late.txt' appeared in the source after the copy started, and was not copied",
                "  '/s/old.txt' was not copied and verified",
                "Cannot remove '/t/x': denied",
                "Something outside prcp changed the source or the destination while prcp ran.",
                "Another program or a person made those changes, not prcp. Make sure that \
                 nothing else uses these paths, then run prcp again.",
            ]
        );
        assert_eq!(report.unfinished_count(), 2);
    }

    #[test]
    fn the_report_has_no_outside_note_when_every_problem_is_from_prcp_itself() {
        let mut problems = BTreeMap::new();
        problems.insert(PathBuf::from("/s/old.txt"), Problem::NotCopied);
        problems.insert(PathBuf::from("/s/pipe"), Problem::Skipped("fifo"));
        let report = MoveReport {
            removed: Vec::new(),
            kept: vec![KeptOperand {
                source: PathBuf::from("/s"),
                problems,
            }],
            removal_errors: Vec::new(),
            operands_with_removal_errors: 0,
        };

        assert_eq!(
            report.error_lines(),
            vec![
                "Kept the originals of '/s'. prcp removed nothing from it:",
                "  '/s/old.txt' was not copied and verified",
                "  '/s/pipe' is a fifo, and prcp does not copy it",
            ]
        );
    }

    #[test]
    fn problem_display_texts_state_each_problem() {
        assert_eq!(
            Problem::NotCopied.to_string(),
            "was not copied and verified"
        );
        assert_eq!(
            Problem::Skipped("fifo").to_string(),
            "is a fifo, and prcp does not copy it"
        );
        assert_eq!(
            Problem::Unreadable("denied".to_string()).to_string(),
            "cannot be read: denied"
        );
        assert_eq!(
            Problem::NewSinceCopy.to_string(),
            "appeared in the source after the copy started, and was not copied"
        );
        assert_eq!(
            Problem::MissingAtSource.to_string(),
            "is no longer in the source"
        );
        assert_eq!(
            Problem::MissingAtDestination.to_string(),
            "was removed or moved away from the destination during the run"
        );
        assert_eq!(
            Problem::SourceChanged.to_string(),
            "changed in the source after its copy"
        );
        assert_eq!(
            Problem::DestinationChanged.to_string(),
            "changed at the destination during the run"
        );
        assert_eq!(
            Problem::NewAtDestination.to_string(),
            "appeared at the destination during the run, and prcp did not make it"
        );
        assert_eq!(
            Problem::MovedAtDestination {
                to: PathBuf::from("/d/b.txt")
            }
            .to_string(),
            "was renamed or moved to '/d/b.txt' during the run"
        );
    }
}

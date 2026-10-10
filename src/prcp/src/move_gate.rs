//! The final check of every run, and the move gate that decides when `--rm` may remove a source.
//!
//! # The final check
//!
//! The Blake3 check in the copy loop proves each copy at the moment that prcp
//! makes it. It cannot see a person or a program that works in the source or
//! the destination later in the run. [`RunLedger::new`] thus takes a snapshot
//! of the destination right before the first copy, and [`RunLedger::finish`]
//! checks the whole run again after the last copy. A copy reports what the
//! check finds and fails. A move also keeps the originals of each operand with
//! a problem. A run that stops early checks nothing and removes nothing.
//!
//! The check works for each operand. One operand is one source path from the
//! command line, after glob expansion. The check finds these problems:
//!
//! 1. An entry of the operand has no positive record in the ledger. A failed
//!    copy, a failed Blake3 check, a skipped file, and an entry that the loop
//!    did not reach all leave no record. Only a move reports this, because a
//!    copy reported each such failure as it occurred. A move also counts a
//!    file that skipped its Blake3 check, and a destination that
//!    `--skip-existing` kept, as not copied.
//! 2. The plan skipped a special file (FIFO, socket, device) under the
//!    operand. Only a move reports this, because a copy warned before the
//!    first copy.
//! 3. A part of the source tree cannot be read. A part that the plan could not
//!    read counts only for a move. A part that only the check cannot read
//!    counts for both.
//! 4. A Directory operand changed in the source. The check walks it again and
//!    compares the walk with the snapshot of the plan. A new path appeared
//!    after the copy started. A path that left, or a node that changed kind,
//!    is also a problem.
//! 5. A file changed on either side after its Blake3 check. The check does not
//!    read the data a third time. It compares the size, the modification time,
//!    and the [`NodeIdentity`] of both sides with the stamps taken before the
//!    copy and right after the check.
//! 6. A symlink no longer reads back with its target, on either side, or a
//!    destination link or directory is another node now.
//! 7. The destination tree of a Directory operand changed. The check walks the
//!    tree again and compares it with the snapshot (see the `destination`
//!    submodule). A path that prcp did not write and that was not there
//!    before appeared during the run. A path from before that changed or left
//!    is also a problem.
//! 8. A destination file that `--skip-existing` kept changed after the
//!    snapshot. A move already counts that file as not copied.
//! 9. A later entry of the same run wrote the destination of an entry. The
//!    destination then no longer holds that source, so a move keeps it. A copy
//!    reports nothing, because the person let the later copy overwrite it.
//!
//! The ledger holds positive records only. In a move, an entry without a
//! record is a problem. That default is the safety rule: a new failure branch
//! that forgets to record still keeps the originals.
//!
//! # The report
//!
//! A destination problem names the destination path, and a source problem
//! names the source path. A destination node that left one path and appeared
//! at another, with the same identity, is one line that names both paths. A
//! problem at a path stands for everything below that path, so a tree that
//! appeared, left, or moved is one line. A `.DS_Store` that appeared or
//! changed has its own text, because Finder writes that file when a person
//! opens a folder. A report with a change from outside prcp ends with a note
//! that says so, so a person does not think that prcp is broken.
//!
//! # Before each entry
//!
//! [`RunLedger::change_before_copy`] compares the destination path of an entry
//! with the snapshot right before the loop makes the entry. A path that
//! appeared, changed, or left since the snapshot belongs to somebody else
//! now, and the loop does not write over it. A path that an earlier entry of
//! the same run writes is the run's own, and keeps the overwrite question.
//!
//! # Overlapping operands
//!
//! A move refuses overlapping operands before the first copy, see
//! [`refuse_overlapping_operands`]. A source given twice, and a source inside
//! another source directory, are such operands. The removal of one operand
//! would otherwise touch what the check of another operand reads. A copy has
//! no such rule.
//!
//! # Removal
//!
//! Only a move removes. For an operand with no problem, the gate removes each
//! file and symlink source in plan order. It then removes each directory
//! source in reverse plan order, children before parents, with `remove_dir`.
//! That call refuses a directory that is not empty, so a file that appears
//! after the gate also keeps its directory. The gate collects each removal
//! error and goes on.

mod destination;

use crate::landing::Action;
use crate::node_identity::NodeIdentity;
use crate::plan::{CopyPlan, EntryKind, OperandKind, PlanEntry, TreeSnapshot};
use anyhow::Context;
use destination::{DestinationNode, DestinationTree, TreeChange};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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

/// How the copy loop proved the data of one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verification {
    /// The destination passed its Blake3 check against the source.
    Passed,
    /// The run skipped the Blake3 check (`--no-verify`). A move never skips it.
    Skipped,
}

/// What the copy loop proved for one plan entry.
#[derive(Debug, Clone, Copy)]
enum Record {
    /// The destination directory exists.
    Directory {
        /// The identity of the destination directory right after the loop made it.
        identity: Option<NodeIdentity>,
    },
    /// The loop left the destination as it was (`--skip-existing`). Nothing was copied.
    KeptExisting,
    /// The destination link exists and reads back with the target.
    Symlink {
        /// The identity of the destination link right after the loop made it.
        identity: Option<NodeIdentity>,
    },
    /// The destination of a file was written, and checked as `verification` says.
    File {
        /// The stamp of the source before the copy.
        source: FileStamp,
        /// The stamp of the destination right after the Blake3 check.
        destination: FileStamp,
        /// How the loop proved the data.
        verification: Verification,
    },
}

/// The positive results of one run, one slot for each plan entry, and the destination before the run.
#[derive(Debug)]
pub(crate) struct RunLedger<'a> {
    plan: &'a CopyPlan,
    action: Action,
    records: Vec<Option<Record>>,
    /// The indexes of the plan entries of each operand, in plan order. The
    /// check and the removal work one operand at a time, and this list keeps
    /// each of them from a scan of the whole plan.
    entries_by_operand: Vec<Vec<usize>>,
    /// The snapshot of the destination of each operand, taken before the first copy. A
    /// Directory operand has the walk of its root, and another operand has its one path.
    destination_before: BTreeMap<PathBuf, DestinationTree>,
    /// Every destination path of the plan, with the first entry that writes it. prcp writes
    /// these paths, and the records judge them.
    first_entry: BTreeMap<PathBuf, usize>,
}

impl<'a> RunLedger<'a> {
    /// Make an empty ledger for `plan`. Every entry starts without a record.
    ///
    /// The call also takes the snapshot of each destination tree. Make the
    /// ledger right before the first copy, so that the snapshot shows the
    /// destination as the copy found it.
    pub(crate) fn new(plan: &'a CopyPlan, action: Action) -> Self {
        let destination_before = plan
            .operands()
            .iter()
            .map(|operand| {
                let root = operand.destination.clone();
                let tree = if operand.kind == OperandKind::Directory {
                    DestinationTree::take(&root)
                } else {
                    DestinationTree::take_node(&root)
                };
                (root, tree)
            })
            .collect();
        let mut first_entry = BTreeMap::new();
        let mut entries_by_operand = vec![Vec::new(); plan.operands().len()];
        for (index, entry) in plan.entries().iter().enumerate() {
            first_entry
                .entry(entry.destination.clone())
                .or_insert(index);
            if let Some(entries) = entries_by_operand.get_mut(entry.operand.index()) {
                entries.push(index);
            }
        }
        Self {
            plan,
            action,
            records: vec![None; plan.entries().len()],
            entries_by_operand,
            destination_before,
            first_entry,
        }
    }

    /// Find out if the destination of entry `entry` changed after the snapshot.
    ///
    /// Call this right before the loop makes the entry. A path that appeared,
    /// changed, or left since the snapshot is a change. Somebody else works
    /// there now, so the loop must not write over it.
    ///
    /// Return `None` when the path is as the snapshot found it. Also return
    /// `None` when an earlier entry of this run writes the same path, when the
    /// snapshot could not read the path, and when the path cannot be read now.
    /// In the last case the copy fails with its own error.
    pub(crate) fn change_before_copy(&self, entry: usize) -> Option<ChangeBeforeCopy> {
        let plan_entry = self.plan.entries().get(entry)?;
        let destination = &plan_entry.destination;
        if self.first_entry.get(destination) != Some(&entry) {
            return None;
        }
        let problem = self.change_since_snapshot(destination)?;
        Some(ChangeBeforeCopy {
            source: plan_entry.source.clone(),
            destination: destination.clone(),
            problem: name_finder_file(destination, problem),
        })
    }

    /// Compare the destination path `path` with the snapshot. Return how it changed.
    ///
    /// A path that appeared is `NewAtDestination`, one that left is
    /// `MissingAtDestination`, and one that changed is `DestinationChanged`.
    /// Return `None` when the path is as the snapshot found it, when the
    /// snapshot could not read it, and when it cannot be read now.
    fn change_since_snapshot(&self, path: &Path) -> Option<Problem> {
        let before = self.snapshot_of(path)?;
        if !before.knows(path) {
            return None;
        }
        let now = DestinationNode::of(path).ok()?;
        match (before.node(path), now) {
            (None, None) => None,
            (None, Some(_)) => Some(Problem::NewAtDestination),
            (Some(_), None) => Some(Problem::MissingAtDestination),
            (Some(old), Some(now)) if !old.agrees_with(&now) => Some(Problem::DestinationChanged),
            (Some(_), Some(_)) => None,
        }
    }

    /// Return the identity that the snapshot holds for the destination path `path`.
    fn identity_in_snapshot(&self, path: &Path) -> Option<NodeIdentity> {
        self.snapshot_of(path)?.node(path)?.identity()
    }

    /// Return the snapshot that holds `path`: the one with the deepest root above or at it.
    fn snapshot_of(&self, path: &Path) -> Option<&DestinationTree> {
        path.ancestors()
            .find_map(|ancestor| self.destination_before.get(ancestor))
    }

    /// Return each plan entry of one operand with its index, in plan order.
    fn entries_of(&self, operand: usize) -> impl Iterator<Item = (usize, &'a PlanEntry)> + '_ {
        let plan = self.plan;
        self.entries_by_operand
            .get(operand)
            .into_iter()
            .flatten()
            .filter_map(move |&index| plan.entries().get(index).map(|entry| (index, entry)))
    }

    /// Record that the loop kept the destination of entry `entry` as it was (`--skip-existing`).
    ///
    /// A copy then compares that path with the snapshot in the final check. A
    /// move keeps the original, because nothing was copied.
    pub(crate) fn record_existing_kept(&mut self, entry: usize) {
        self.set(entry, Record::KeptExisting);
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

    /// Record that the loop wrote the file of entry `entry`, and how it proved the data.
    ///
    /// Call this right after the Blake3 check, or right after the copy when the
    /// run skips the check. The call stamps the destination now. When that
    /// stamp fails, the entry stays without a record, so the gate keeps its
    /// originals.
    pub(crate) fn record_file(
        &mut self,
        entry: usize,
        source_before_copy: FileStamp,
        verification: Verification,
    ) {
        let Some(plan_entry) = self.plan.entries().get(entry) else {
            return;
        };
        if let Ok(destination) = FileStamp::of(&plan_entry.destination) {
            self.set(
                entry,
                Record::File {
                    source: source_before_copy,
                    destination,
                    verification,
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

    /// Run the final check for every operand. For a move, then remove the originals of each
    /// operand that passed. A copy removes nothing.
    pub(crate) fn finish(self) -> RunReport {
        let mut report = RunReport {
            action: self.action,
            removed: Vec::new(),
            with_problems: Vec::new(),
            removal_errors: Vec::new(),
            operands_with_removal_errors: 0,
        };
        let last_writers = self.last_writers();
        for (index, operand) in self.plan.operands().iter().enumerate() {
            let problems = self.find_problems(index, &last_writers);
            if !problems.is_empty() {
                report.with_problems.push(OperandProblems {
                    source: operand.source.clone(),
                    problems,
                });
                continue;
            }
            if self.action == Action::Copy {
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

    /// Map each destination path that a file or link entry wrote to the last entry that wrote it.
    ///
    /// Two sources of one run can have the same destination. The later copy
    /// then replaces the earlier one, and only the last writer still matches
    /// what is at the path.
    fn last_writers(&self) -> BTreeMap<&Path, usize> {
        let mut last = BTreeMap::new();
        for (index, entry) in self.plan.entries().iter().enumerate() {
            if matches!(
                self.records[index],
                Some(Record::File { .. } | Record::Symlink { .. })
            ) {
                last.insert(entry.destination.as_path(), index);
            }
        }
        last
    }

    /// Collect every problem of one operand. See [`Findings`] for how the list is made.
    ///
    /// `last_writers` comes from [`Self::last_writers`]. A file or link entry
    /// whose destination a later entry wrote is not compared with the
    /// destination. A move keeps its source, because the destination no
    /// longer holds that source. A copy reports nothing, because the person
    /// let the later copy overwrite it.
    fn find_problems(
        &self,
        operand: usize,
        last_writers: &BTreeMap<&Path, usize>,
    ) -> BTreeMap<PathBuf, Problem> {
        let mut findings = Findings::default();
        for (index, entry) in self.entries_of(operand) {
            let replaced_by = last_writers
                .get(entry.destination.as_path())
                .filter(|last| **last != index && self.records[index].is_some())
                .and_then(|last| self.plan.entries().get(*last));
            if let Some(later) = replaced_by {
                if self.action == Action::Move {
                    let by = later.source.clone();
                    findings.note(&entry.source, Problem::ReplacedInRun { by });
                }
                continue;
            }
            match self.records[index] {
                // A copy reported the failure when it occurred. A move keeps the originals.
                None if self.action == Action::Copy => {}
                None => findings.note(&entry.source, Problem::NotCopied),
                Some(Record::File {
                    verification: Verification::Skipped,
                    ..
                })
                | Some(Record::KeptExisting)
                    if self.action == Action::Move =>
                {
                    findings.note(&entry.source, Problem::NotCopied);
                }
                Some(Record::KeptExisting) => {
                    let destination = &entry.destination;
                    let problem = self.change_since_snapshot(destination);
                    let identity = self.identity_in_snapshot(destination);
                    findings.note_destination(destination, problem, identity);
                }
                Some(Record::File {
                    source,
                    destination,
                    ..
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
        // A copy warned about these before the first copy. A move keeps the originals.
        if self.action == Action::Move {
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
        if root.kind != OperandKind::Directory {
            return;
        }
        let Some(before) = self.destination_before.get(&root.destination) else {
            return;
        };
        let now = DestinationTree::take(&root.destination);
        let written = |path: &Path| self.first_entry.contains_key(path);
        for change in now.changes_since(before, written) {
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
            .entries_of(operand)
            .next()
            .and_then(|(_, entry)| self.plan.tree(entry.operand))
        else {
            return;
        };
        let Some(root) = self.plan.operands().get(operand) else {
            return;
        };
        let now = TreeSnapshot::take(&root.source);
        for (path, message) in now.errors() {
            let known = before.errors().iter().any(|(old, _)| old == path);
            if !known {
                findings.note(path, Problem::Unreadable(message.clone()));
            }
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
        let entries = || self.entries_of(operand).map(|(_, entry)| entry);
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
    ///
    /// A Finder file that appeared or changed becomes `FinderMetadata`, so its
    /// line says why it is there.
    fn note(&mut self, path: &Path, problem: Problem) {
        let problem = name_finder_file(path, problem);
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
            if matches!(
                self.problems.get(&new),
                Some(Problem::NewAtDestination | Problem::FinderMetadata)
            ) {
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

/// Return `FinderMetadata` for a Finder file at `path` that appeared or changed, else `problem`.
fn name_finder_file(path: &Path, problem: Problem) -> Problem {
    let finder = path.file_name() == Some(OsStr::new(FINDER_FILE_NAME));
    let appeared_or_changed = matches!(
        problem,
        Problem::NewSinceCopy
            | Problem::SourceChanged
            | Problem::NewAtDestination
            | Problem::DestinationChanged
    );
    if finder && appeared_or_changed {
        Problem::FinderMetadata
    } else {
        problem
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

/// A destination path that changed after the snapshot, before the loop made its entry.
///
/// The loop does not write over such a path. Somebody else works there now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChangeBeforeCopy {
    /// The source of the entry.
    pub(crate) source: PathBuf,
    /// The destination of the entry, which changed.
    pub(crate) destination: PathBuf,
    /// How the destination changed.
    pub(crate) problem: Problem,
}

impl fmt::Display for ChangeBeforeCopy {
    /// Write the error for a person. A change from outside prcp ends with
    /// [`OUTSIDE_CHANGE_NOTE`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "prcp did not copy '{}': '{}' {}.",
            self.source.display(),
            self.destination.display(),
            self.problem
        )?;
        if self.problem.is_outside_change() {
            for line in OUTSIDE_CHANGE_NOTE {
                write!(f, "\n{line}")?;
            }
        }
        Ok(())
    }
}

/// The result of the gate and of the removal.
#[derive(Debug)]
pub(crate) struct RunReport {
    /// What the run did to its sources.
    action: Action,
    /// The operand sources whose originals are all removed.
    pub(crate) removed: Vec<PathBuf>,
    /// The operands whose final check found a problem, with the problems. A move keeps their
    /// originals.
    pub(crate) with_problems: Vec<OperandProblems>,
    /// Each removal that failed, with the text of the error.
    pub(crate) removal_errors: Vec<(PathBuf, String)>,
    operands_with_removal_errors: usize,
}

impl RunReport {
    /// Return true when no operand has a problem and no removal failed.
    pub(crate) fn is_complete(&self) -> bool {
        self.with_problems.is_empty() && self.removal_errors.is_empty()
    }

    /// Write the report for a person: one block for each operand with a problem, then one line
    /// for each failed removal.
    ///
    /// When a problem comes from outside prcp, the report ends with
    /// [`OUTSIDE_CHANGE_NOTE`], so a person does not think that prcp is broken.
    pub(crate) fn error_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for operand in &self.with_problems {
            let source = operand.source.display();
            lines.push(match self.action {
                Action::Move => {
                    format!("Kept the originals of '{source}'. prcp removed nothing from it:")
                }
                Action::Copy => format!("prcp found problems with the copy of '{source}':"),
            });
            for (path, problem) in &operand.problems {
                lines.push(format!("  '{}' {problem}", path.display()));
            }
        }
        for (path, error) in &self.removal_errors {
            lines.push(format!("Cannot remove '{}': {error}", path.display()));
        }
        let outside = self
            .with_problems
            .iter()
            .flat_map(|operand| operand.problems.values())
            .any(Problem::is_outside_change);
        if outside {
            lines.extend(OUTSIDE_CHANGE_NOTE.iter().map(|line| (*line).to_string()));
        }
        lines
    }

    /// Count the operands that did not finish: with a problem, or with a failed removal.
    pub(crate) fn unfinished_count(&self) -> usize {
        self.with_problems.len() + self.operands_with_removal_errors
    }

    /// Return the one-line error for a run that is not complete.
    pub(crate) fn summary(&self) -> String {
        match self.action {
            Action::Move => format!(
                "The move did not finish: the originals of {} source(s) stay.",
                self.unfinished_count()
            ),
            Action::Copy => format!(
                "The copy did not pass its final check: {} source(s) have problems.",
                self.unfinished_count()
            ),
        }
    }
}

/// One operand whose final check found a problem.
#[derive(Debug)]
pub(crate) struct OperandProblems {
    /// The source path of the operand.
    pub(crate) source: PathBuf,
    /// The first problem of each path, in path order.
    pub(crate) problems: BTreeMap<PathBuf, Problem>,
}

/// Refuse a move whose operands overlap. Call this before the first copy.
///
/// This is the precondition of the move gate: in a move, the removal of one
/// operand must not touch what another operand checks or removes. Without it,
/// the removal of `src` would make the check of `src/a` fail, and the report
/// would blame a person for a change that prcp made.
///
/// Two operands overlap in two cases:
///
/// 1. They have the same location. The same source given twice, or two
///    spellings of one path (`./src` and `src`), are this case.
/// 2. One location is inside the location of a Directory operand. Only a
///    Directory operand contains anything.
///
/// The location of an operand is its parent, resolved with
/// [`fs::canonicalize`], joined with its file name. The call does not follow
/// the last component, so a top-level symlink stays the link itself, and a
/// link to `src` does not overlap `src`. A source with no file name (`.`,
/// `..`, `dir/..`) has no last component to keep, so the call resolves the
/// whole path.
///
/// The call looks up each ancestor of a location in a map, not each pair of
/// operands, because a glob can give thousands of operands. It compares path
/// components, not text, so `src2` is not inside `src`.
///
/// # Errors
///
/// Return an error that names the first overlap in operand order, with the
/// paths as the user gave them. Also return an error when a location cannot
/// be resolved. A copy needs no call, because a copy of overlapping operands
/// works.
pub(crate) fn refuse_overlapping_operands(plan: &CopyPlan) -> anyhow::Result<()> {
    let operands = plan.operands();
    let locations = operands
        .iter()
        .map(|operand| operand_location(&operand.source))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut by_location: BTreeMap<&Path, usize> = BTreeMap::new();
    for (index, location) in locations.iter().enumerate() {
        by_location.entry(location.as_path()).or_insert(index);
    }
    for (index, location) in locations.iter().enumerate() {
        let operand = &operands[index];
        if let Some(&first) = by_location.get(location.as_path()) {
            if first != index {
                anyhow::bail!(
                    "{}",
                    same_source_message(&operands[first].source, &operand.source)
                );
            }
        }
        for ancestor in location.ancestors().skip(1) {
            let Some(&outer) = by_location.get(ancestor) else {
                continue;
            };
            if operands[outer].kind == OperandKind::Directory {
                anyhow::bail!(
                    "{}",
                    inside_message(&operand.source, &operands[outer].source)
                );
            }
        }
    }
    Ok(())
}

/// Return the location of one operand, as [`refuse_overlapping_operands`] defines it.
fn operand_location(source: &Path) -> anyhow::Result<PathBuf> {
    let resolve = |path: &Path| {
        fs::canonicalize(path)
            .with_context(|| format!("Cannot resolve the source '{}'", source.display()))
    };
    let Some(name) = source.file_name() else {
        return resolve(source);
    };
    let parent = match source.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    Ok(resolve(parent)?.join(name))
}

/// The refusal for two operands with one location.
fn same_source_message(first: &Path, second: &Path) -> String {
    format!(
        "Cannot move '{}' and '{}' in one run: they are the same source. Give it once.",
        first.display(),
        second.display()
    )
}

/// The refusal for an operand inside a Directory operand.
fn inside_message(inner: &Path, outer: &Path) -> String {
    format!(
        "Cannot move '{inner}' and '{outer}' in one run: '{inner}' is inside '{outer}'. Give only '{outer}'.",
        inner = inner.display(),
        outer = outer.display()
    )
}

/// The name of the file that Finder writes into a folder that a person opens.
const FINDER_FILE_NAME: &str = ".DS_Store";

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
    /// A Finder file (`.DS_Store`) appeared or changed, in the source or at the destination.
    FinderMetadata,
    /// A later source of the same run wrote the same destination path.
    ReplacedInRun {
        /// The later source.
        by: PathBuf,
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
            Self::NotCopied
            | Self::Skipped(_)
            | Self::Unreadable(_)
            | Self::ReplacedInRun { .. } => false,
            Self::NewSinceCopy
            | Self::MissingAtSource
            | Self::MissingAtDestination
            | Self::SourceChanged
            | Self::DestinationChanged
            | Self::NewAtDestination
            | Self::MovedAtDestination { .. }
            | Self::FinderMetadata => true,
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
                write!(
                    f,
                    "was renamed or moved to '{}' during the run",
                    to.display()
                )
            }
            Self::ReplacedInRun { by } => write!(
                f,
                "was replaced at the destination by '{}', another source of this run",
                by.display()
            ),
            Self::FinderMetadata => f.write_str(
                "was made or changed during the run. Finder writes this file when a person \
                 opens the folder in Finder",
            ),
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

    /// Copy every entry of the plan for a move, as the copy loop does, except the sources in `skip`.
    /// A skipped entry gets no record, as after a failed Blake3 check.
    fn copy_all<'a>(plan: &'a CopyPlan, skip: &[&Path]) -> RunLedger<'a> {
        run_all(plan, skip, Action::Move, Verification::Passed)
    }

    /// Copy every entry of the plan as the copy loop does for `action`, except the sources in
    /// `skip`. Each copied file is recorded with `verification`.
    fn run_all<'a>(
        plan: &'a CopyPlan,
        skip: &[&Path],
        action: Action,
        verification: Verification,
    ) -> RunLedger<'a> {
        let mut ledger = RunLedger::new(plan, action);
        for (index, entry) in plan.entries().iter().enumerate() {
            let record = !skip.contains(&entry.source.as_path());
            make_entry(&mut ledger, index, entry, record, verification);
        }
        ledger
    }

    /// Make one entry as the copy loop does, and record it. A file is recorded only when
    /// `record` is true, with `verification`.
    fn make_entry(
        ledger: &mut RunLedger<'_>,
        index: usize,
        entry: &PlanEntry,
        record: bool,
        verification: Verification,
    ) {
        match &entry.kind {
            EntryKind::Directory => {
                fs::create_dir_all(&entry.destination).unwrap();
                ledger.record_directory(index);
            }
            EntryKind::File => {
                let stamp = FileStamp::of(&entry.source).unwrap();
                fs::copy(&entry.source, &entry.destination).unwrap();
                if record {
                    ledger.record_file(index, stamp, verification);
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

    /// Return the problems that the report holds for the one operand with problems.
    fn only_problems(report: &RunReport) -> &BTreeMap<PathBuf, Problem> {
        assert_eq!(report.with_problems.len(), 1, "report: {report:?}");
        &report.with_problems[0].problems
    }

    /// Build a move plan over `sources` (paths as the test spells them) into `dest`.
    fn move_plan(sources: &[PathBuf], dest: &Path) -> CopyPlan {
        CopyPlan::build(sources, dest, true).unwrap()
    }

    /// Return the message of the refusal for `sources`, or fail the test when there is none.
    fn refusal_for(sources: &[PathBuf], dest: &Path) -> String {
        let plan = move_plan(sources, dest);
        refuse_overlapping_operands(&plan)
            .expect_err("overlapping operands must be refused")
            .to_string()
    }

    /// Make `src` with a sub directory `a` that holds a file, and a file `top.txt`.
    fn nested_tree(temp: &TempDir) -> PathBuf {
        let src = temp.path().join("src");
        write_file(&src.join("a").join("x.txt"), "x");
        write_file(&src.join("top.txt"), "top");
        src
    }

    #[test]
    fn a_move_refuses_a_directory_inside_an_earlier_directory() {
        let temp = TempDir::new().unwrap();
        let src = nested_tree(&temp);
        let inner = src.join("a");
        let message = refusal_for(&[src.clone(), inner.clone()], &temp.path().join("dest"));
        assert_eq!(
            message,
            format!(
                "Cannot move '{inner}' and '{src}' in one run: '{inner}' is inside '{src}'. Give only '{src}'.",
                inner = inner.display(),
                src = src.display()
            )
        );
    }

    #[test]
    fn a_move_refuses_a_directory_inside_a_later_directory() {
        let temp = TempDir::new().unwrap();
        let src = nested_tree(&temp);
        let inner = src.join("a");
        let message = refusal_for(&[inner, src.clone()], &temp.path().join("dest"));
        assert!(message.contains("is inside"), "message: {message}");
        assert!(message.contains(&format!("Give only '{}'", src.display())));
    }

    #[test]
    fn a_move_refuses_a_file_inside_a_directory_operand() {
        let temp = TempDir::new().unwrap();
        let src = nested_tree(&temp);
        let file = src.join("top.txt");
        let message = refusal_for(&[src.clone(), file.clone()], &temp.path().join("dest"));
        assert!(
            message.contains(&format!(
                "'{}' is inside '{}'",
                file.display(),
                src.display()
            )),
            "message: {message}"
        );
    }

    #[test]
    fn a_move_refuses_a_source_that_is_given_twice() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("a.txt");
        write_file(&file, "a");
        let message = refusal_for(&[file.clone(), file.clone()], &temp.path().join("dest"));
        assert_eq!(
            message,
            format!(
                "Cannot move '{f}' and '{f}' in one run: they are the same source. Give it once.",
                f = file.display()
            )
        );
    }

    #[test]
    fn a_move_refuses_two_spellings_of_one_source() {
        let temp = TempDir::new().unwrap();
        let src = nested_tree(&temp);
        let dotted = temp.path().join(".").join("src");
        let message = refusal_for(&[dotted, src], &temp.path().join("dest"));
        assert!(message.contains("same source"), "message: {message}");
    }

    #[cfg(unix)]
    #[test]
    fn a_move_refuses_a_symlink_that_sits_inside_a_directory_operand() {
        let temp = TempDir::new().unwrap();
        let src = nested_tree(&temp);
        let outside = temp.path().join("outside");
        write_file(&outside.join("o.txt"), "o");
        let link = src.join("link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let message = refusal_for(&[src.clone(), link.clone()], &temp.path().join("dest"));
        assert!(
            message.contains(&format!(
                "'{}' is inside '{}'",
                link.display(),
                src.display()
            )),
            "message: {message}"
        );
    }

    #[test]
    fn a_move_accepts_two_directories_whose_names_share_a_prefix() {
        let temp = TempDir::new().unwrap();
        let src = nested_tree(&temp);
        let other = temp.path().join("src2");
        write_file(&other.join("y.txt"), "y");
        let plan = move_plan(&[src, other], &temp.path().join("dest"));
        refuse_overlapping_operands(&plan).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_move_accepts_a_symlink_to_a_directory_beside_that_directory() {
        let temp = TempDir::new().unwrap();
        let src = nested_tree(&temp);
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&src, &link).unwrap();
        let plan = move_plan(&[link, src], &temp.path().join("dest"));
        refuse_overlapping_operands(&plan).unwrap();
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

    /// Return the problems of the one operand with problems as a list in path order.
    fn problem_list(report: &RunReport) -> Vec<(PathBuf, Problem)> {
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

    #[test]
    fn a_finder_file_that_appears_at_the_destination_names_finder() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let finder = fixture.dest.join("sub").join(".DS_Store");
        write_file(&finder, "finder");

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            problem_list(&report),
            vec![(finder, Problem::FinderMetadata)]
        );
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_finder_file_that_appears_in_the_source_names_finder() {
        let fixture = sample();
        let ledger = copy_all(&fixture.plan, &[]);
        let finder = fixture.src.join("sub").join(".DS_Store");
        write_file(&finder, "finder");

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            problem_list(&report),
            vec![(finder, Problem::FinderMetadata)]
        );
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_copied_finder_file_that_changes_at_the_destination_names_finder() {
        let fixture = sample_with(|src| write_file(&src.join(".DS_Store"), "old"));
        let ledger = copy_all(&fixture.plan, &[]);
        let finder = fixture.dest.join(".DS_Store");
        fs::write(&finder, "rewritten by Finder").unwrap();

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            problem_list(&report),
            vec![(finder, Problem::FinderMetadata)]
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
        assert_eq!(report.with_problems.len(), 1);
        assert_eq!(report.with_problems[0].source, bad);
        assert_eq!(
            report.with_problems[0].problems.get(&late),
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

    #[cfg(unix)]
    #[test]
    fn a_move_checks_a_symlink_source_before_the_file_it_points_to_is_removed() {
        let temp = TempDir::new().unwrap();
        let real = temp.path().join("real.txt");
        let link = temp.path().join("link.txt");
        let dest = temp.path().join("dest");
        write_file(&real, "real");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        fs::create_dir(&dest).unwrap();
        let plan = CopyPlan::build(&[real.clone(), link.clone()], &dest, false).unwrap();
        let ledger = copy_all(&plan, &[]);

        let report = ledger.finish();

        assert!(report.is_complete(), "report: {report:?}");
        assert_eq!(report.removed, vec![real.clone(), link.clone()]);
        assert!(report.with_problems.is_empty(), "report: {report:?}");
        assert!(fs::symlink_metadata(&real).is_err());
        assert!(fs::symlink_metadata(&link).is_err());
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
            report.with_problems[0].problems.get(&second),
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
    fn a_clean_copy_removes_nothing() {
        let fixture = sample();
        let ledger = run_all(&fixture.plan, &[], Action::Copy, Verification::Passed);

        let report = ledger.finish();

        assert!(report.is_complete(), "report: {report:?}");
        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_copy_reports_a_file_that_appears_at_the_destination() {
        let fixture = sample();
        let ledger = run_all(&fixture.plan, &[], Action::Copy, Verification::Passed);
        let intruder = fixture.dest.join("sub").join("new.txt");
        write_file(&intruder, "new");

        let report = ledger.finish();

        assert!(!report.is_complete(), "report: {report:?}");
        assert_eq!(
            problem_list(&report),
            vec![(intruder, Problem::NewAtDestination)]
        );
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_copy_leaves_out_an_entry_that_the_loop_did_not_copy() {
        let fixture = sample();
        let failed = fixture.src.join("sub").join("inner.txt");
        let ledger = run_all(
            &fixture.plan,
            &[failed.as_path()],
            Action::Copy,
            Verification::Passed,
        );

        let report = ledger.finish();

        assert!(report.is_complete(), "report: {report:?}");
        assert_originals_exist(&fixture.plan);
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_leaves_out_a_skipped_special_file() {
        let fixture = sample_with(|src| {
            let status = std::process::Command::new("mkfifo")
                .arg(src.join("pipe"))
                .status()
                .unwrap();
            assert!(status.success(), "mkfifo must work");
        });
        let ledger = run_all(&fixture.plan, &[], Action::Copy, Verification::Passed);

        let report = ledger.finish();

        assert!(report.is_complete(), "report: {report:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_leaves_out_a_part_that_the_plan_could_not_read() {
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
        let ledger = run_all(&fixture.plan, &[], Action::Copy, Verification::Passed);

        let report = ledger.finish();

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(report.is_complete(), "report: {report:?}");
    }

    #[test]
    fn a_move_without_a_hash_check_keeps_all_originals() {
        let fixture = sample();
        let ledger = run_all(&fixture.plan, &[], Action::Move, Verification::Skipped);

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&fixture.src.join("top.txt")),
            Some(&Problem::NotCopied)
        );
        assert_originals_exist(&fixture.plan);
    }

    #[test]
    fn a_copy_without_a_hash_check_still_finds_a_changed_destination_file() {
        let fixture = sample();
        let ledger = run_all(&fixture.plan, &[], Action::Copy, Verification::Skipped);
        let copy = fixture.dest.join("top.txt");
        fs::write(&copy, "a longer text than before").unwrap();

        let report = ledger.finish();

        assert_eq!(
            problem_list(&report),
            vec![(copy, Problem::DestinationChanged)]
        );
    }

    #[test]
    fn the_report_of_a_copy_names_the_copy_and_keeps_no_originals() {
        let mut problems = BTreeMap::new();
        problems.insert(PathBuf::from("/d/s/new.txt"), Problem::NewAtDestination);
        let report = RunReport {
            action: Action::Copy,
            removed: Vec::new(),
            with_problems: vec![OperandProblems {
                source: PathBuf::from("/s"),
                problems,
            }],
            removal_errors: Vec::new(),
            operands_with_removal_errors: 0,
        };

        assert_eq!(
            report.error_lines(),
            vec![
                "prcp found problems with the copy of '/s':",
                "  '/d/s/new.txt' appeared at the destination during the run, and prcp did not \
                 make it",
                "Something outside prcp changed the source or the destination while prcp ran.",
                "Another program or a person made those changes, not prcp. Make sure that \
                 nothing else uses these paths, then run prcp again.",
            ]
        );
        assert_eq!(
            report.summary(),
            "The copy did not pass its final check: 1 source(s) have problems."
        );
    }

    #[test]
    fn the_summary_of_a_move_names_the_originals_that_stay() {
        let mut problems = BTreeMap::new();
        problems.insert(PathBuf::from("/s/old.txt"), Problem::NotCopied);
        let report = RunReport {
            action: Action::Move,
            removed: Vec::new(),
            with_problems: vec![OperandProblems {
                source: PathBuf::from("/s"),
                problems,
            }],
            removal_errors: Vec::new(),
            operands_with_removal_errors: 0,
        };

        assert_eq!(
            report.summary(),
            "The move did not finish: the originals of 1 source(s) stay."
        );
    }

    /// Return the index of the plan entry whose source is `source`.
    fn entry_index(plan: &CopyPlan, source: &Path) -> usize {
        plan.entries()
            .iter()
            .position(|entry| entry.source == source)
            .unwrap()
    }

    #[test]
    fn a_planned_path_that_appears_before_its_copy_is_a_change_before_copy() {
        let fixture = sample();
        let ledger = RunLedger::new(&fixture.plan, Action::Move);
        let source = fixture.src.join("top.txt");
        let intruder = fixture.dest.join("top.txt");
        write_file(&intruder, "not from prcp");

        let change = ledger.change_before_copy(entry_index(&fixture.plan, &source));

        assert_eq!(
            change,
            Some(ChangeBeforeCopy {
                source,
                destination: intruder,
                problem: Problem::NewAtDestination,
            })
        );
    }

    #[test]
    fn a_planned_file_from_before_the_copy_that_changes_is_a_change_before_copy() {
        let fixture = sample_into_existing(|root| write_file(&root.join("top.txt"), "old"));
        let ledger = RunLedger::new(&fixture.plan, Action::Copy);
        fs::write(fixture.dest.join("top.txt"), "a longer text than before").unwrap();

        let source = fixture.src.join("top.txt");
        let change = ledger.change_before_copy(entry_index(&fixture.plan, &source));

        assert_eq!(
            change.map(|change| change.problem),
            Some(Problem::DestinationChanged)
        );
    }

    #[test]
    fn a_planned_file_from_before_the_copy_that_is_removed_is_a_change_before_copy() {
        let fixture = sample_into_existing(|root| write_file(&root.join("top.txt"), "old"));
        let ledger = RunLedger::new(&fixture.plan, Action::Copy);
        fs::remove_file(fixture.dest.join("top.txt")).unwrap();

        let source = fixture.src.join("top.txt");
        let change = ledger.change_before_copy(entry_index(&fixture.plan, &source));

        assert_eq!(
            change.map(|change| change.problem),
            Some(Problem::MissingAtDestination)
        );
    }

    #[test]
    fn a_planned_finder_file_that_appears_before_its_copy_names_finder() {
        let fixture = sample_with(|src| write_file(&src.join(".DS_Store"), "source"));
        let ledger = RunLedger::new(&fixture.plan, Action::Copy);
        write_file(&fixture.dest.join(".DS_Store"), "made by Finder");

        let source = fixture.src.join(".DS_Store");
        let change = ledger.change_before_copy(entry_index(&fixture.plan, &source));

        assert_eq!(
            change.map(|change| change.problem),
            Some(Problem::FinderMetadata)
        );
    }

    #[test]
    fn a_file_operand_whose_destination_appears_before_its_copy_is_a_change_before_copy() {
        let temp = TempDir::new().unwrap();
        let first = temp.path().join("first.txt");
        let second = temp.path().join("second.txt");
        let dest = temp.path().join("dest");
        write_file(&first, "first");
        write_file(&second, "second");
        fs::create_dir(&dest).unwrap();
        let plan = CopyPlan::build(&[first.clone(), second], &dest, false).unwrap();
        let ledger = RunLedger::new(&plan, Action::Copy);
        write_file(&dest.join("first.txt"), "not from prcp");

        let change = ledger.change_before_copy(entry_index(&plan, &first));

        assert_eq!(
            change.map(|change| change.problem),
            Some(Problem::NewAtDestination)
        );
    }

    #[test]
    fn a_planned_path_that_nobody_touches_is_no_change() {
        let fixture = sample_into_existing(|root| write_file(&root.join("top.txt"), "old"));
        let ledger = RunLedger::new(&fixture.plan, Action::Copy);

        for index in 0..fixture.plan.entries().len() {
            assert_eq!(ledger.change_before_copy(index), None, "entry {index}");
        }
    }

    #[test]
    fn a_planned_path_that_an_earlier_entry_of_the_run_claims_is_no_change() {
        let temp = TempDir::new().unwrap();
        let a = temp.path().join("a").join("same.txt");
        let b = temp.path().join("b").join("same.txt");
        let dest = temp.path().join("dest");
        write_file(&a, "from a");
        write_file(&b, "from b");
        fs::create_dir(&dest).unwrap();
        let plan = CopyPlan::build(&[a, b.clone()], &dest, false).unwrap();
        let ledger = RunLedger::new(&plan, Action::Copy);
        write_file(&dest.join("same.txt"), "from a");

        assert_eq!(ledger.change_before_copy(entry_index(&plan, &b)), None);
    }

    #[test]
    fn the_error_for_a_change_before_copy_names_both_paths_and_the_outside_cause() {
        let change = ChangeBeforeCopy {
            source: PathBuf::from("/s/c.txt"),
            destination: PathBuf::from("/d/s/c.txt"),
            problem: Problem::NewAtDestination,
        };

        assert_eq!(
            change.to_string(),
            "prcp did not copy '/s/c.txt': '/d/s/c.txt' appeared at the destination during the \
             run, and prcp did not make it.\n\
             Something outside prcp changed the source or the destination while prcp ran.\n\
             Another program or a person made those changes, not prcp. Make sure that nothing \
             else uses these paths, then run prcp again."
        );
    }

    /// Make two file sources with the same name in two directories, and a plan that copies
    /// both into one destination directory. Return the sources and the plan.
    fn two_sources_for_one_destination(temp: &TempDir) -> (PathBuf, PathBuf, CopyPlan) {
        let a = temp.path().join("a").join("same.txt");
        let b = temp.path().join("b").join("same.txt");
        let dest = temp.path().join("dest");
        write_file(&a, "from a");
        write_file(&b, "from b, which is longer");
        fs::create_dir(&dest).unwrap();
        let plan = CopyPlan::build(&[a.clone(), b.clone()], &dest, false).unwrap();
        (a, b, plan)
    }

    #[test]
    fn a_move_of_two_sources_for_one_destination_keeps_the_first_and_blames_no_outsider() {
        let temp = TempDir::new().unwrap();
        let (a, b, plan) = two_sources_for_one_destination(&temp);
        let ledger = copy_all(&plan, &[]);

        let report = ledger.finish();

        assert_eq!(report.removed, vec![b.clone()], "report: {report:?}");
        assert_eq!(
            problem_list(&report),
            vec![(a.clone(), Problem::ReplacedInRun { by: b })]
        );
        assert!(a.exists(), "the replaced source must stay");
        assert!(
            !report
                .error_lines()
                .iter()
                .any(|line| line.starts_with("Something outside prcp")),
            "report: {report:?}"
        );
    }

    #[test]
    fn a_copy_of_two_sources_for_one_destination_reports_nothing() {
        let temp = TempDir::new().unwrap();
        let (_a, _b, plan) = two_sources_for_one_destination(&temp);
        let ledger = run_all(&plan, &[], Action::Copy, Verification::Passed);

        let report = ledger.finish();

        assert!(report.is_complete(), "report: {report:?}");
    }

    /// Run the plan as `run_all` does, but leave the destination of the source `kept` as it
    /// was, as `--skip-existing` does.
    fn run_all_keeping<'a>(plan: &'a CopyPlan, kept: &Path, action: Action) -> RunLedger<'a> {
        let mut ledger = RunLedger::new(plan, action);
        for (index, entry) in plan.entries().iter().enumerate() {
            if entry.source == kept {
                ledger.record_existing_kept(index);
            } else {
                make_entry(&mut ledger, index, entry, true, Verification::Passed);
            }
        }
        ledger
    }

    #[test]
    fn a_copy_finds_a_change_to_a_destination_file_that_it_kept() {
        let fixture = sample_into_existing(|root| write_file(&root.join("top.txt"), "old"));
        let kept = fixture.src.join("top.txt");
        let copy = fixture.dest.join("top.txt");
        let ledger = run_all_keeping(&fixture.plan, &kept, Action::Copy);
        fs::write(&copy, "a longer text than before").unwrap();

        let report = ledger.finish();

        assert_eq!(
            problem_list(&report),
            vec![(copy, Problem::DestinationChanged)]
        );
    }

    #[test]
    fn a_copy_that_kept_a_destination_file_that_nobody_touches_passes() {
        let fixture = sample_into_existing(|root| write_file(&root.join("top.txt"), "old"));
        let kept = fixture.src.join("top.txt");
        let ledger = run_all_keeping(&fixture.plan, &kept, Action::Copy);

        let report = ledger.finish();

        assert!(report.is_complete(), "report: {report:?}");
        assert_eq!(
            fs::read_to_string(fixture.dest.join("top.txt")).unwrap(),
            "old"
        );
    }

    #[test]
    fn a_move_that_kept_a_destination_file_keeps_that_original() {
        let fixture = sample_into_existing(|root| write_file(&root.join("top.txt"), "old"));
        let kept = fixture.src.join("top.txt");
        let ledger = run_all_keeping(&fixture.plan, &kept, Action::Move);

        let report = ledger.finish();

        assert_eq!(problem_list(&report), vec![(kept, Problem::NotCopied)]);
    }

    #[test]
    fn a_move_source_that_was_not_copied_is_not_called_replaced() {
        let temp = TempDir::new().unwrap();
        let (a, b, plan) = two_sources_for_one_destination(&temp);
        let ledger = copy_all(&plan, &[a.as_path()]);

        let report = ledger.finish();

        assert_eq!(report.removed, vec![b], "report: {report:?}");
        assert_eq!(problem_list(&report), vec![(a, Problem::NotCopied)]);
    }

    #[test]
    fn the_report_lists_each_kept_operand_with_its_problems_and_each_failed_removal() {
        let mut problems = BTreeMap::new();
        problems.insert(PathBuf::from("/s/late.txt"), Problem::NewSinceCopy);
        problems.insert(PathBuf::from("/s/old.txt"), Problem::NotCopied);
        let report = RunReport {
            action: Action::Move,
            removed: Vec::new(),
            with_problems: vec![OperandProblems {
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
        let report = RunReport {
            action: Action::Move,
            removed: Vec::new(),
            with_problems: vec![OperandProblems {
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
        assert_eq!(
            Problem::ReplacedInRun {
                by: PathBuf::from("/b/same.txt")
            }
            .to_string(),
            "was replaced at the destination by '/b/same.txt', another source of this run"
        );
        assert_eq!(
            Problem::FinderMetadata.to_string(),
            "was made or changed during the run. Finder writes this file when a person opens \
             the folder in Finder"
        );
    }
}

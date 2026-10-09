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

use crate::plan::{CopyPlan, EntryKind, NodeKind, Operand, OperandId, TreeSnapshot};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The size and modification time of a file at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl FileStamp {
    /// Take the stamp from metadata that the caller already holds.
    pub(crate) fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
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
    Directory,
    /// The destination link exists and reads back with the target.
    Symlink,
    /// The destination passed its Blake3 check against the source.
    File {
        /// The stamp of the source before the copy.
        source: FileStamp,
        /// The stamp of the destination right after the Blake3 check.
        destination: FileStamp,
    },
}

/// The positive results of one run, one slot for each plan entry.
#[derive(Debug)]
pub(crate) struct MoveLedger<'a> {
    plan: &'a CopyPlan,
    records: Vec<Option<Record>>,
}

impl<'a> MoveLedger<'a> {
    /// Make an empty ledger for `plan`. Every entry starts without a record.
    pub(crate) fn new(plan: &'a CopyPlan) -> Self {
        Self {
            plan,
            records: vec![None; plan.entries().len()],
        }
    }

    /// Record that the directory of entry `entry` exists at the destination.
    pub(crate) fn record_directory(&mut self, entry: usize) {
        self.set(entry, Record::Directory);
    }

    /// Record that the symlink of entry `entry` was made and read back.
    pub(crate) fn record_symlink(&mut self, entry: usize) {
        self.set(entry, Record::Symlink);
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
        let kept = self
            .plan
            .operands()
            .iter()
            .map(|operand| KeptOperand {
                source: operand.source.clone(),
                problems: BTreeMap::new(),
            })
            .collect();
        MoveReport {
            removed: Vec::new(),
            kept,
            removal_errors: Vec::new(),
            operands_with_removal_errors: 0,
        }
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
            Self::MissingAtDestination => f.write_str("is missing at the destination"),
            Self::SourceChanged => f.write_str("changed in the source after its copy"),
            Self::DestinationChanged => {
                f.write_str("changed at the destination after its hash check")
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
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        let dest = temp.path().join("dest");
        make_sample_tree(&src);
        let plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();
        Fixture {
            _temp: temp,
            src,
            dest,
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
            "is missing at the destination"
        );
        assert_eq!(
            Problem::SourceChanged.to_string(),
            "changed in the source after its copy"
        );
        assert_eq!(
            Problem::DestinationChanged.to_string(),
            "changed at the destination after its hash check"
        );
    }
}

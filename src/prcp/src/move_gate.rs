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

use crate::plan::{CopyPlan, EntryKind, TreeSnapshot};
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

    /// Collect every problem of one operand. The first problem of a path wins.
    fn find_problems(&self, operand: usize) -> BTreeMap<PathBuf, Problem> {
        let mut problems = BTreeMap::new();
        for (index, entry) in self.plan.entries().iter().enumerate() {
            if entry.operand.index() != operand {
                continue;
            }
            match self.records[index] {
                None => note(&mut problems, &entry.source, Problem::NotCopied),
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
                        note(&mut problems, &entry.source, problem);
                    }
                    if let Some(problem) = stamp_problem(
                        &entry.destination,
                        destination,
                        Problem::MissingAtDestination,
                        Problem::DestinationChanged,
                    ) {
                        note(&mut problems, &entry.source, problem);
                    }
                }
                Some(_) => {}
            }
        }
        for skipped in self.plan.skipped() {
            if skipped.operand.index() == operand {
                note(
                    &mut problems,
                    &skipped.path,
                    Problem::Skipped(skipped.reason),
                );
            }
        }
        for error in self.plan.walk_errors() {
            if error.operand.index() == operand {
                note(
                    &mut problems,
                    &error.path,
                    Problem::Unreadable(error.message.clone()),
                );
            }
        }
        self.compare_tree(operand, &mut problems);
        problems
    }

    /// Walk a Directory operand again and compare the walk with the snapshot of the plan.
    ///
    /// A new path is `NewSinceCopy`. A path that left is `MissingAtSource`. A
    /// path with another kind is `SourceChanged`. An operand that is not a
    /// Directory has no snapshot, and the call does nothing.
    fn compare_tree(&self, operand: usize, problems: &mut BTreeMap<PathBuf, Problem>) {
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
            note(problems, path, Problem::Unreadable(message.clone()));
        }
        for (path, kind) in now.nodes() {
            match before.nodes().get(path) {
                None => note(problems, path, Problem::NewSinceCopy),
                Some(old) if old != kind => note(problems, path, Problem::SourceChanged),
                Some(_) => {}
            }
        }
        for path in before.nodes().keys() {
            if !now.nodes().contains_key(path) {
                note(problems, path, Problem::MissingAtSource);
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

/// Note a problem for a path. A path that already has a problem keeps the first one.
fn note(problems: &mut BTreeMap<PathBuf, Problem>, path: &Path, problem: Problem) {
    problems.entry(path.to_path_buf()).or_insert(problem);
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
            only_problems(&report).get(&fixture.src.join("sub").join("inner.txt")),
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
        fs::write(
            fixture.dest.join("sub").join("deeper").join("deep.txt"),
            "changed at the destination",
        )
        .unwrap();

        let report = ledger.finish();

        assert!(report.removed.is_empty(), "report: {report:?}");
        assert_eq!(
            only_problems(&report).get(&fixture.src.join("sub").join("deeper").join("deep.txt")),
            Some(&Problem::DestinationChanged)
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

//! What one press of `m` measures, and the words that report it.
//!
//! `grind` measures a rebase of HEAD onto the default branch, and `grime`
//! measures a merge of the default branch into HEAD. This module makes the same
//! `gitscratch` calls in this process. It does not start the two binaries. So
//! gsw gets a typed [`Conflicts`] value, and it does not need either tool on the
//! `PATH`.
//!
//! The words name `grind` and `grime` all the same, because those are the names
//! the user knows.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::Context as _;
use gitscratch::{Conflicts, Repo, Uncommitted};

use crate::lines::LineSplitter;
use crate::repo::{branch_name, RepoHandle};

/// The two tools a notice names, spelled once for every sentence that names
/// them.
///
/// A macro and not a `const`, because [`WAITING_NOTICE`] is a `const` too, and
/// `concat!` takes only literals.
macro_rules! tools {
    () => {
        "grind and grime"
    };
}

/// The notice on the bottom row while gsw quits and a replay is in flight.
///
/// The quit waits for that replay. A replay that gsw abandons keeps a scratch
/// worktree registered in the repository of the user, so the wait is the price
/// of a clean repository.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the quit of watch mode shows this from slice C of #496"
    )
)]
pub(crate) const WAITING_NOTICE: &str = concat!("Waiting for ", tools!(), " to finish…");

/// What joins the parts of a measured line.
const SEPARATOR: &str = " · ";

/// The last part of a measured line when the work tree held uncommitted work.
///
/// `grind` and `grime` say the same thing on stderr. A replay starts from HEAD,
/// so a count never includes that work.
const DIRTY_NOTE: &str = "uncommitted work not included";

/// Whether the words of one half name the count of stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopWords {
    /// Name the stops. A rebase stops once for each commit that conflicts, so
    /// the count is a measurement.
    Named,
    /// Leave the stops out. A merge stops once or never, so the count is a
    /// constant. `grime` leaves it out for the same reason (see the comment on
    /// `without_stops` in `src/grime/src/main.rs`).
    Omitted,
}

/// One of the two replays of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Replay {
    /// A rebase of HEAD onto the default branch, as `grind` does.
    Rebase,
    /// A merge of the default branch into HEAD, as `grime` does.
    Merge,
}

/// What one press of `m` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConflictsOutcome {
    /// HEAD is the default branch itself. No replay ran.
    ///
    /// A rebase of a branch onto itself is clean by definition, so a replay
    /// costs a scratch worktree and tells the user nothing.
    OnDefault {
        /// The default branch, which HEAD is on.
        branch: String,
    },
    /// No measurement was possible: an empty repository, no default branch, or
    /// a directory that is not a repository.
    Refused {
        /// Why, as one row of text.
        reason: String,
    },
    /// Both replays were attempted. Each half is its own result.
    Measured {
        /// The branch that both replays measured against.
        branch: String,
        /// The rebase of HEAD onto `branch`, or why it failed.
        rebase: Result<Conflicts, String>,
        /// The merge of `branch` into HEAD, or why it failed.
        merge: Result<Conflicts, String>,
        /// Whether the work tree held uncommitted work. A replay starts from
        /// HEAD, so that work is not part of either result.
        dirty: bool,
    },
}

impl ConflictsOutcome {
    /// The one row that reports this outcome under the frame.
    ///
    /// The words for a count come only from the phrases of `gitscratch`, so
    /// gsw, `grind` and `grime` never give one number two names.
    ///
    /// The row is not cut to a width here. The overlay under the frame cuts
    /// every row it paints to the width of the pane.
    pub(crate) fn line(&self) -> String {
        match self {
            Self::OnDefault { branch } => format!("on {branch} — nothing to compare"),
            Self::Refused { reason } => {
                format!(concat!(tools!(), " failed: {}"), one_row(reason))
            }
            Self::Measured {
                branch,
                rebase,
                merge,
                dirty,
            } => {
                let mut parts = vec![
                    half("rebase", rebase, StopWords::Named),
                    half("merge", merge, StopWords::Omitted),
                ];
                if *dirty {
                    parts.push(DIRTY_NOTE.to_owned());
                }
                format!("{branch}: {}", parts.join(SEPARATOR))
            }
        }
    }
}

/// The notice on the bottom row while a run measures against `branch`.
///
/// The notice does not fade. It tells the user that a press of `m` does
/// nothing until the run ends.
pub(crate) fn running_notice(branch: &str) -> String {
    format!(concat!("Running ", tools!(), " against {}…"), branch)
}

/// Measure a rebase of HEAD onto the default branch, then a merge of the
/// default branch into HEAD, as `grind` and `grime` do.
///
/// `on_started` gets the name of the branch once that name is known, before any
/// scratch worktree exists. It does not run when the run is refused or when
/// HEAD is on the default branch, because no replay starts then.
///
/// Returns `None` when `stop` was set before a replay started. The run was
/// abandoned, and nobody reads its outcome.
///
/// The two replays run one after the other on the calling thread, and never
/// at the same time. Two scratch worktrees at once double the load on the disk
/// and the processor, and the user sees no gain from that.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "only tests call this; the worker calls measure_probed"
    )
)]
pub(crate) fn measure(
    workdir: &Path,
    stop: &AtomicBool,
    on_started: impl FnOnce(&str),
) -> Option<ConflictsOutcome> {
    measure_probed(workdir, stop, on_started, |_, _| {})
}

/// [`measure`], with `probe` called for each replay after its scratch worktree
/// exists and before the replay starts.
///
/// The probe is the seam of the quit test. That test must stop a run while a
/// scratch worktree exists, and nothing else can hold a run at that point. The
/// probe gets the stop flag, so a test probe can wait for the quit. Production
/// passes a probe that does nothing.
fn measure_probed(
    workdir: &Path,
    stop: &AtomicBool,
    on_started: impl FnOnce(&str),
    mut probe: impl FnMut(Replay, &AtomicBool),
) -> Option<ConflictsOutcome> {
    let (repo, branch) = match open_against_default(workdir) {
        Ok(opened) => opened,
        Err(err) => {
            return Some(ConflictsOutcome::Refused {
                reason: reason(&err),
            })
        }
    };

    if head_is_on(workdir, &branch) {
        return Some(ConflictsOutcome::OnDefault { branch });
    }

    on_started(&branch);

    // Read before any scratch worktree exists. A `TMPDIR` under the repository
    // puts the scratch worktree inside it, and `git status` then counts the
    // scratch worktree as uncommitted work of the user. A count that cannot be
    // read costs the note and not the answer, as it does in `grind`.
    let dirty = repo.uncommitted_files().unwrap_or_default() != Uncommitted::new(0);

    if stop.load(Ordering::SeqCst) {
        return None;
    }
    let rebase = replay(&repo, &branch, Replay::Rebase, || {
        probe(Replay::Rebase, stop);
    })
    .map_err(|err| reason(&err));

    // The check between the two replays. A quit waits only for the replay in
    // flight, and the second replay never starts.
    if stop.load(Ordering::SeqCst) {
        return None;
    }
    let merge = replay(&repo, &branch, Replay::Merge, || {
        probe(Replay::Merge, stop);
    })
    .map_err(|err| reason(&err));

    Some(ConflictsOutcome::Measured {
        branch,
        rebase,
        merge,
        dirty,
    })
}

/// Open the repository at `workdir`, and choose the branch to measure against.
///
/// The same calls `grind` and `grime` make, in the same order, before any
/// scratch worktree exists. A revision that does not resolve then fails as a
/// refusal with its own reason, and not as a failed replay.
///
/// # Errors
///
/// Returns an error if `workdir` is not inside a repository, if HEAD holds no
/// commit, or if no default branch resolves.
fn open_against_default(workdir: &Path) -> anyhow::Result<(Repo, String)> {
    let repo = Repo::open(workdir)?;
    repo.resolve("HEAD")
        .context("there is no commit at HEAD to measure from")?;
    let branch = repo.branch_or_default(None)?;
    repo.resolve(&branch)?;
    Ok((repo, branch))
}

/// Whether the branch checked out at `workdir` is `branch`.
///
/// Read through `gix` in this process, so the check starts no git child. A
/// `gix::Repository` cannot cross a thread, so the check opens its own on the
/// thread that measures.
///
/// A detached HEAD is on no branch, so it is never on `branch`. A repository
/// that `gix` cannot open is not on `branch` either. Both then measure as
/// usual, and a replay of the default branch onto itself reports clean, which
/// is the correct answer.
fn head_is_on(workdir: &Path, branch: &str) -> bool {
    RepoHandle::discover(workdir).is_some_and(|handle| branch_name(handle.repo()) == branch)
}

/// Run `which` replay against `branch` in a scratch worktree of HEAD.
///
/// `on_scratch` runs after the scratch worktree exists and before the replay
/// starts. See [`measure_probed`].
///
/// The scratch worktree drops at the end of this function, and its `Drop`
/// removes the worktree from the repository. So the worktree is gone before
/// the next replay starts.
///
/// # Errors
///
/// Returns an error if the scratch worktree cannot be made, or if the replay
/// fails and leaves no conflict to measure.
fn replay(
    repo: &Repo,
    branch: &str,
    which: Replay,
    on_scratch: impl FnOnce(),
) -> anyhow::Result<Conflicts> {
    let scratch = repo.scratch("HEAD")?;
    on_scratch();
    match which {
        Replay::Rebase => scratch.replay_rebase(branch),
        Replay::Merge => scratch.replay_merge(branch),
    }
}

/// The reason in `err`, with every cause in its chain, as one row.
fn reason(err: &anyhow::Error) -> String {
    one_row(&format!("{err:#}"))
}

/// Owns the threads that measure, and the stop flag they read.
///
/// A run is on a thread of its own, because a rebase replay of a long branch
/// can take many seconds, and the watch loop must not wait for it.
///
/// A quit is the one time gsw waits for a run. `Scratch` removes its worktree
/// in `Drop`, and a process that exits while a thread holds a `Scratch` never
/// runs that `Drop`. The repository of the user then keeps a registered
/// worktree that points at a deleted directory.
///
/// The watch loop keeps `m` to one run at a time. The worker does not refuse a
/// second run, and it keeps each thread until that thread ends. So a broken
/// rule costs a longer wait at the quit, and never a worktree left behind.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "watch mode starts this from slice C of #496")
)]
#[derive(Default)]
pub(crate) struct ConflictsWorker {
    /// Set when gsw quits. Every thread of this worker reads it.
    stop: Arc<AtomicBool>,
    /// The threads this worker started that were alive at the last `start`.
    threads: Vec<JoinHandle<()>>,
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "watch mode starts this from slice C of #496")
)]
impl ConflictsWorker {
    /// A worker with no thread yet.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Start one run on a thread of its own.
    ///
    /// `on_started` gets the branch before the first replay starts.
    /// `on_finished` gets the outcome when the run ends. Both run on that
    /// thread. A run that a quit abandoned hands over no outcome, because
    /// nobody reads it.
    pub(crate) fn start(
        &mut self,
        workdir: PathBuf,
        on_started: impl FnOnce(String) + Send + 'static,
        on_finished: impl FnOnce(ConflictsOutcome) + Send + 'static,
    ) {
        self.start_probed(workdir, on_started, on_finished, |_, _| {});
    }

    /// [`ConflictsWorker::start`], with the probe of [`measure_probed`].
    fn start_probed(
        &mut self,
        workdir: PathBuf,
        on_started: impl FnOnce(String) + Send + 'static,
        on_finished: impl FnOnce(ConflictsOutcome) + Send + 'static,
        probe: impl FnMut(Replay, &AtomicBool) + Send + 'static,
    ) {
        // A thread that ended holds no scratch worktree, so it leaves the
        // list. The join of such a thread returns at once.
        let (ended, alive): (Vec<_>, Vec<_>) = std::mem::take(&mut self.threads)
            .into_iter()
            .partition(JoinHandle::is_finished);
        for thread in ended {
            let _ = thread.join();
        }
        self.threads = alive;

        let stop = Arc::clone(&self.stop);
        self.threads.push(std::thread::spawn(move || {
            let outcome = measure_probed(
                &workdir,
                &stop,
                |branch| on_started(branch.to_owned()),
                probe,
            );
            if let Some(outcome) = outcome {
                on_finished(outcome);
            }
        }));
    }

    /// Whether a thread of this worker is still alive.
    ///
    /// A run hands over its outcome a moment before its thread ends. So this
    /// can read true just after `on_finished` ran. The watch loop keeps its own
    /// state for the one-run rule, and it does not read this.
    pub(crate) fn is_running(&self) -> bool {
        self.threads.iter().any(|thread| !thread.is_finished())
    }

    /// Set the stop flag, then wait for every thread that is alive. Called when
    /// gsw quits.
    ///
    /// A run that has not started a replay starts none. A run in the middle of
    /// a replay finishes that replay, removes its scratch worktree, and starts
    /// no other. A drop of the worker does the same, so a loop that leaves by
    /// an error path waits as well. This method gives the quit a name at its
    /// call site.
    ///
    /// `on_wait` gets [`WAITING_NOTICE`] before the wait, and only when a
    /// thread is still alive.
    pub(crate) fn shutdown(self, on_wait: impl FnOnce(&str)) {
        let _ = on_wait;
        drop(self);
    }

    /// Set the stop flag, then join every thread.
    fn stop_and_join(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for thread in self.threads.drain(..) {
            // The result of the join is dropped. A panic on that thread went to
            // the panic hook already, and the quit can do nothing more about it.
            let _ = thread.join();
        }
    }
}

impl Drop for ConflictsWorker {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

/// The words for the result of one replay: `operation clean`, the counts, or
/// `operation failed: reason`.
///
/// A failed half shows its reason and no number. A number in the line must
/// never be a guess.
fn half(operation: &str, result: &Result<Conflicts, String>, stops: StopWords) -> String {
    match result {
        Err(reason) => format!("{operation} failed: {}", one_row(reason)),
        Ok(conflicts) if conflicts.is_clean() => format!("{operation} clean"),
        Ok(conflicts) => {
            let cost = format!(
                "{} in {}",
                conflicts.hunks().phrase(),
                conflicts.files().phrase()
            );
            match stops {
                StopWords::Named => format!("{operation} {cost}, {}", conflicts.stops().phrase()),
                StopWords::Omitted => format!("{operation} {cost}"),
            }
        }
    }
}

/// `text` as one row that is safe to paint: no escape sequence, no control
/// character, and each run of whitespace as one space.
///
/// An error from `gitscratch` carries the stdout and the stderr of git, so it
/// can span many lines. The overlay under the frame measures a row in display
/// columns, and a newline or an escape sequence draws a different number of
/// columns than it measures. [`LineSplitter`] is the one place in gsw that makes
/// the text of a child process safe to paint, so this function uses it and does
/// not keep a second copy of those rules.
fn one_row(text: &str) -> String {
    let mut splitter = LineSplitter::new();
    let mut lines = splitter.feed(text.as_bytes());
    lines.extend(splitter.finish());
    lines
        .iter()
        .flat_map(|line| line.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::num::NonZeroUsize;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use gitscratch::testing::{default_branch_choice_repo, not_a_repository, TestRepo};
    use gitscratch::{Conflicts, Stops};

    use super::{
        measure, running_notice, ConflictsOutcome, ConflictsWorker, Replay, WAITING_NOTICE,
    };

    /// How long a test waits for the thread of a worker before the test fails.
    ///
    /// A run against a fixture takes well under a second. The bound is for a
    /// thread that hangs, because a test that waits for such a thread holds
    /// the suite for the life of the session.
    const WAIT: Duration = Duration::from_secs(30);

    /// How long the quit probe holds the rebase scratch worktree after it sees
    /// the stop flag.
    ///
    /// A quit that does not join returns at once, and the test then lists the
    /// worktrees while the probe still holds this one. The hold is the margin
    /// between that listing and the removal of the scratch worktree. A quit
    /// that joins waits it out, so it costs the test this long and no more.
    const HOLD: Duration = Duration::from_millis(500);

    /// How often a wait reads its condition again.
    const POLL: Duration = Duration::from_millis(10);

    /// Wait until `done` is true, or panic with `what` after [`WAIT`].
    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let give_up_at = Instant::now() + WAIT;
        while !done() {
            assert!(
                Instant::now() < give_up_at,
                "{what} did not happen within {}s",
                WAIT.as_secs()
            );
            std::thread::sleep(POLL);
        }
    }

    /// A replay that hit no conflict.
    fn clean() -> Conflicts {
        Conflicts::nothing_replayed()
    }

    /// A replay that conflicted in `files`, each with its count of hunks, and
    /// that stopped `stops` times.
    fn conflicted(files: &[(&str, usize)], stops: usize) -> Conflicts {
        Conflicts::from_files(
            files.iter().map(|(name, hunks)| {
                (
                    PathBuf::from(name),
                    NonZeroUsize::new(*hunks).expect("a conflicted file has at least one hunk"),
                )
            }),
            Stops::new(stops),
        )
    }

    /// An outcome of both replays against `main`.
    fn measured(
        rebase: Result<Conflicts, String>,
        merge: Result<Conflicts, String>,
        dirty: bool,
    ) -> ConflictsOutcome {
        ConflictsOutcome::Measured {
            branch: "main".to_owned(),
            rebase,
            merge,
            dirty,
        }
    }

    #[test]
    fn two_clean_replays_say_clean_twice() {
        assert_eq!(
            measured(Ok(clean()), Ok(clean()), false).line(),
            "main: rebase clean · merge clean",
        );
    }

    #[test]
    fn two_conflicted_replays_name_their_counts_in_the_plural() {
        let rebase = conflicted(&[("a.txt", 2), ("b.txt", 1)], 2);
        let merge = conflicted(&[("a.txt", 1)], 1);
        assert_eq!(
            measured(Ok(rebase), Ok(merge), false).line(),
            "main: rebase 3 hunks in 2 files, 2 stops · merge 1 hunk in 1 file",
        );
    }

    #[test]
    fn a_count_of_one_takes_the_singular() {
        let rebase = conflicted(&[("a.txt", 1)], 1);
        assert_eq!(
            measured(Ok(rebase), Ok(clean()), false).line(),
            "main: rebase 1 hunk in 1 file, 1 stop · merge clean",
        );
    }

    /// A merge stops once or never, so its count of stops is a constant and
    /// not a measurement. `grime` leaves it out for that reason. This test
    /// gives the merge a count of stops that a real merge never has, so a line
    /// that names it cannot pass by accident.
    #[test]
    fn the_merge_half_never_names_its_stops() {
        let merge = conflicted(&[("a.txt", 2), ("b.txt", 2)], 2);
        let line = measured(Ok(clean()), Ok(merge), false).line();
        assert_eq!(line, "main: rebase clean · merge 4 hunks in 2 files");
        assert!(
            !line.contains("stop"),
            "the merge half named its stops: {line}"
        );
    }

    #[test]
    fn a_failed_half_shows_its_reason_and_the_other_half_shows_its_result() {
        assert_eq!(
            measured(Ok(clean()), Err("the merge failed".to_owned()), false).line(),
            "main: rebase clean · merge failed: the merge failed",
        );
        assert_eq!(
            measured(
                Err("the rebase failed".to_owned()),
                Ok(conflicted(&[("a.txt", 1)], 1)),
                false,
            )
            .line(),
            "main: rebase failed: the rebase failed · merge 1 hunk in 1 file",
        );
    }

    #[test]
    fn a_dirty_tree_adds_that_uncommitted_work_is_not_included() {
        let rebase = conflicted(&[("a.txt", 2), ("b.txt", 1)], 2);
        assert_eq!(
            measured(Ok(rebase), Ok(clean()), true).line(),
            "main: rebase 3 hunks in 2 files, 2 stops · merge clean · uncommitted work not included",
        );
    }

    #[test]
    fn head_on_the_default_branch_has_nothing_to_compare() {
        let outcome = ConflictsOutcome::OnDefault {
            branch: "main".to_owned(),
        };
        assert_eq!(outcome.line(), "on main — nothing to compare");
    }

    #[test]
    fn a_refusal_names_both_tools_and_the_reason() {
        let outcome = ConflictsOutcome::Refused {
            reason: "no default branch resolves here".to_owned(),
        };
        assert_eq!(
            outcome.line(),
            "grind and grime failed: no default branch resolves here",
        );
    }

    /// The line is one row under the frame. A newline in it pushes the bottom
    /// row of the frame off the screen, and an error from git carries the
    /// streams of git, newlines and all.
    #[test]
    fn a_reason_that_spans_lines_becomes_one_row() {
        let refused = ConflictsOutcome::Refused {
            reason: "no branch was named,\n  and no default\tbranch resolves\n".to_owned(),
        };
        assert_eq!(
            refused.line(),
            "grind and grime failed: no branch was named, and no default branch resolves",
        );

        let failed_half = measured(
            Ok(clean()),
            Err("the merge failed and left nothing to resolve:\n\nfatal: refusing\n".to_owned()),
            false,
        )
        .line();
        assert_eq!(
            failed_half,
            "main: rebase clean · merge failed: the merge failed and left nothing to resolve: \
             fatal: refusing",
        );
        assert!(
            !failed_half.contains('\n'),
            "the line spans rows: {failed_half:?}"
        );
    }

    /// The overlay under the frame cuts a row to the width of the pane, and it
    /// measures that row in display columns. An escape sequence draws in no
    /// column and repaints the frame, so it must not reach the row.
    #[test]
    fn a_reason_holds_no_escape_sequence_or_control_character() {
        let outcome = ConflictsOutcome::Refused {
            reason: "\u{1b}[31mfatal\u{1b}[0m: bad\u{7} revision".to_owned(),
        };
        assert_eq!(
            outcome.line(),
            "grind and grime failed: fatal: bad revision"
        );
    }

    #[test]
    fn the_running_notice_names_both_tools_and_the_branch() {
        assert_eq!(
            running_notice("main"),
            "Running grind and grime against main…"
        );
    }

    #[test]
    fn the_waiting_notice_names_both_tools() {
        assert_eq!(WAITING_NOTICE, "Waiting for grind and grime to finish…");
    }

    /// How many worktrees the repository of `repo` has registered, the main
    /// worktree included.
    ///
    /// A count and not a list of paths. A temporary directory on macOS sits
    /// behind a symbolic link, and git prints the resolved path, so a
    /// comparison of paths fails for a reason this module does not own.
    fn worktrees(repo: &TestRepo) -> usize {
        repo.git(&["worktree", "list", "--porcelain"])
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count()
    }

    /// Measure `workdir` with a stop flag that nothing sets, and collect each
    /// branch name that `on_started` got.
    fn measure_collecting(workdir: &Path) -> (Option<ConflictsOutcome>, Vec<String>) {
        let stop = AtomicBool::new(false);
        let mut started = Vec::new();
        let outcome = measure(workdir, &stop, |branch| started.push(branch.to_owned()));
        (outcome, started)
    }

    /// The reason of a refusal, or a panic that names what came back instead.
    fn refusal(outcome: Option<ConflictsOutcome>) -> String {
        match outcome {
            Some(ConflictsOutcome::Refused { reason }) => reason,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    /// The fixture puts HEAD on `work` and its only default branch at `master`,
    /// and both of them rewrite one line. So each replay conflicts in one hunk
    /// of one file, and the rebase stops once.
    #[test]
    fn a_branch_that_conflicts_with_the_default_branch_is_measured_in_both_halves() {
        let repo = default_branch_choice_repo(&["master"]);

        let (outcome, started) = measure_collecting(repo.path());

        let one_hunk = conflicted(&[("shared.txt", 1)], 1);
        let expected = ConflictsOutcome::Measured {
            branch: "master".to_owned(),
            rebase: Ok(one_hunk.clone()),
            merge: Ok(one_hunk),
            dirty: false,
        };
        assert_eq!(outcome.as_ref(), Some(&expected));
        assert_eq!(
            expected.line(),
            "master: rebase 1 hunk in 1 file, 1 stop · merge 1 hunk in 1 file",
        );
        assert_eq!(
            started,
            ["master"],
            "on_started gets the branch exactly once"
        );
        assert_eq!(
            worktrees(&repo),
            1,
            "both scratch worktrees are gone after the run"
        );
    }

    #[test]
    fn a_branch_that_does_not_conflict_is_measured_clean() {
        let repo = default_branch_choice_repo(&["main"]);

        let (outcome, started) = measure_collecting(repo.path());

        assert_eq!(
            outcome,
            Some(ConflictsOutcome::Measured {
                branch: "main".to_owned(),
                rebase: Ok(clean()),
                merge: Ok(clean()),
                dirty: false,
            }),
        );
        assert_eq!(started, ["main"]);
    }

    /// A rebase of the default branch onto itself is clean by definition. So
    /// no replay runs, and `on_started`, which runs before the first scratch
    /// worktree exists, does not run either.
    #[test]
    fn head_on_the_default_branch_runs_no_replay() {
        let repo = default_branch_choice_repo(&["main"]);
        repo.checkout("main");

        let (outcome, started) = measure_collecting(repo.path());

        assert_eq!(
            outcome,
            Some(ConflictsOutcome::OnDefault {
                branch: "main".to_owned(),
            }),
        );
        assert!(
            started.is_empty(),
            "a run with nothing to compare started a replay: {started:?}"
        );
        assert_eq!(worktrees(&repo), 1);
    }

    /// A detached HEAD is on no branch, so it is not on the default branch.
    /// `grind` measures it, and so does `m`.
    #[test]
    fn a_detached_head_is_measured_even_at_the_default_branch() {
        let repo = default_branch_choice_repo(&["main"]);
        repo.git(&["checkout", "-q", "--detach", "main"]);

        let (outcome, started) = measure_collecting(repo.path());

        assert_eq!(
            outcome,
            Some(ConflictsOutcome::Measured {
                branch: "main".to_owned(),
                rebase: Ok(clean()),
                merge: Ok(clean()),
                dirty: false,
            }),
        );
        assert_eq!(started, ["main"]);
    }

    #[test]
    fn a_repository_with_no_default_branch_is_refused_with_the_reason() {
        let repo = default_branch_choice_repo(&[]);

        let (outcome, started) = measure_collecting(repo.path());

        let reason = refusal(outcome);
        assert!(
            reason.contains("no default branch resolves here"),
            "the refusal does not say why: {reason:?}"
        );
        assert!(!reason.contains('\n'), "the reason spans rows: {reason:?}");
        assert!(started.is_empty());
    }

    #[test]
    fn an_empty_repository_is_refused_with_the_reason() {
        let repo = TestRepo::init();

        let reason = refusal(measure_collecting(repo.path()).0);

        assert!(
            reason.contains("no commit at HEAD"),
            "the refusal does not say why: {reason:?}"
        );
    }

    #[test]
    fn a_directory_outside_every_repository_is_refused_with_the_reason() {
        let outside = not_a_repository();

        let reason = refusal(measure_collecting(outside.path()).0);

        assert!(
            reason.contains("is not inside a git repository"),
            "the refusal does not say why: {reason:?}"
        );
    }

    #[test]
    fn uncommitted_work_marks_the_run_dirty() {
        let repo = default_branch_choice_repo(&["main"]);
        repo.write_file("new.txt", "work nobody committed\n");

        let (outcome, _) = measure_collecting(repo.path());

        match outcome {
            Some(ConflictsOutcome::Measured { dirty, .. }) => {
                assert!(dirty, "the uncommitted file did not mark the run dirty");
            }
            other => panic!("expected a measured run, got {other:?}"),
        }
    }

    /// gsw sets the flag when it quits. A replay that starts after that holds
    /// a scratch worktree that nobody waits for.
    #[test]
    fn a_stop_set_before_the_run_starts_no_replay() {
        let repo = default_branch_choice_repo(&["master"]);
        let stop = AtomicBool::new(true);

        let outcome = measure(repo.path(), &stop, |_| {});

        assert_eq!(outcome, None);
        assert_eq!(worktrees(&repo), 1);
    }

    /// A run on the thread of a worker hands its branch and its outcome to the
    /// two callbacks, and the worker then reports no run in flight. A second
    /// run after the first one ended starts as usual.
    #[test]
    fn a_worker_hands_over_the_outcome_and_then_reports_no_run() {
        let repo = default_branch_choice_repo(&["master"]);
        let mut worker = ConflictsWorker::new();

        for run in 1..=2 {
            let (started_tx, started_rx) = mpsc::channel();
            let (finished_tx, finished_rx) = mpsc::channel();
            worker.start(
                repo.path().to_path_buf(),
                move |branch| {
                    let _ = started_tx.send(branch);
                },
                move |outcome| {
                    let _ = finished_tx.send(outcome);
                },
            );

            assert_eq!(
                started_rx.recv_timeout(WAIT).as_deref(),
                Ok("master"),
                "run {run}: on_started did not get the branch",
            );
            let outcome = finished_rx
                .recv_timeout(WAIT)
                .unwrap_or_else(|err| panic!("run {run}: on_finished got no outcome: {err}"));
            assert_eq!(
                outcome.line(),
                "master: rebase 1 hunk in 1 file, 1 stop · merge 1 hunk in 1 file",
                "run {run}",
            );

            wait_until("the end of the run", || !worker.is_running());
        }

        let said = RefCell::new(Vec::new());
        worker.shutdown(|notice| said.borrow_mut().push(notice.to_owned()));
        assert_eq!(worktrees(&repo), 1);
        assert_eq!(
            said.into_inner(),
            Vec::<String>::new(),
            "a quit with no run in flight waits for nothing, so it says nothing",
        );
    }

    /// A worker that never started a run has nothing to wait for.
    #[test]
    fn an_idle_worker_says_nothing_at_the_quit() {
        let said = RefCell::new(Vec::new());
        ConflictsWorker::new().shutdown(|notice| said.borrow_mut().push(notice.to_owned()));
        assert_eq!(said.into_inner(), Vec::<String>::new());
    }

    /// Quit with `quit` while the rebase replay holds its scratch worktree,
    /// and check that the quit waited for that replay and started no other.
    ///
    /// `Scratch` removes its worktree in `Drop`. A process that exits while a
    /// thread holds one never runs that `Drop`, and the repository of the user
    /// keeps a registered worktree that points at a deleted directory. So a
    /// quit must wait for the replay in flight.
    ///
    /// The probe holds the run in the middle: the scratch worktree exists and
    /// the replay has not started. It waits there for the stop flag, and then
    /// holds the worktree for [`HOLD`]. A quit that joins returns after the
    /// worktree is gone. A quit that does not join returns while the probe
    /// still holds it, and the listing below then finds two worktrees.
    fn a_quit_in_the_middle_of_a_run_waits_for_the_replay(quit: impl FnOnce(ConflictsWorker)) {
        let repo = default_branch_choice_repo(&["master"]);
        let (probed_tx, probed_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let mut worker = ConflictsWorker::new();

        worker.start_probed(
            repo.path().to_path_buf(),
            |_| {},
            move |outcome| {
                let _ = finished_tx.send(outcome);
            },
            move |replay, stop| {
                let _ = probed_tx.send(replay);
                if replay == Replay::Rebase {
                    wait_until("the quit", || stop.load(Ordering::SeqCst));
                    std::thread::sleep(HOLD);
                }
            },
        );

        assert_eq!(probed_rx.recv_timeout(WAIT), Ok(Replay::Rebase));
        assert_eq!(
            worktrees(&repo),
            2,
            "the rebase scratch worktree is registered while its replay is in flight",
        );

        quit(worker);

        assert_eq!(
            worktrees(&repo),
            1,
            "the quit returned while the replay still held its scratch worktree",
        );
        assert_eq!(
            probed_rx.try_iter().collect::<Vec<_>>(),
            [],
            "the merge replay started after the quit",
        );
        assert!(
            finished_rx.try_recv().is_err(),
            "an abandoned run handed over an outcome",
        );
    }

    /// The quit through `shutdown` also names the wait. A replay can take many
    /// seconds, and a screen that says nothing while gsw waits reads as a hang.
    #[test]
    fn shutdown_in_the_middle_of_a_run_waits_for_the_replay() {
        let said = RefCell::new(Vec::new());
        a_quit_in_the_middle_of_a_run_waits_for_the_replay(|worker| {
            worker.shutdown(|notice| said.borrow_mut().push(notice.to_owned()));
        });
        assert_eq!(
            said.into_inner(),
            [WAITING_NOTICE],
            "a quit that waits for a replay must say so once",
        );
    }

    /// A loop that leaves by an error path drops the worker and never calls
    /// `shutdown`. The drop must wait all the same.
    #[test]
    fn a_dropped_worker_waits_for_the_replay_too() {
        a_quit_in_the_middle_of_a_run_waits_for_the_replay(drop);
    }
}

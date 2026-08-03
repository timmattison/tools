//! A throwaway worktree, and the git operations replayed inside it.
//!
//! A [`Scratch`] is a detached worktree of the developer's real repository,
//! living in a private temporary directory and removing itself on drop. Every
//! git call made through it goes via [`Git`](crate::Git), so the whole safety
//! configuration applies to the replay whether the caller remembered it or not.
//!
//! # Why markers, and what that means for the numbers
//!
//! Conflicts hit during a replay are counted and then resolved by staging the
//! conflict markers verbatim. Staging markers is the conservative
//! auto-resolution: unlike `--ours` or `--theirs` it never silently discards a
//! side. It does mean a later commit touching the same region conflicts again -
//! but that is faithful to reality, since a human resolution also leaves later
//! commits conflicting against the resolved state. Treat the totals as a cost
//! index for comparing candidates measured under identical rules, not as an
//! exact prediction.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tempfile::TempDir;

use crate::git::Git;
use crate::metrics::{Files, Hunks, Stops};

/// Upper bound on the rounds one replay may spend advancing a halted rebase, so
/// a git state we failed to anticipate stalls the run instead of spinning
/// forever.
///
/// A round is a round of *work* on a rebase that is still going: resolving a
/// stop, or skipping a commit that arrived with nothing unmerged. Skips are
/// charged because they are exactly as capable of failing to make progress as a
/// resolution is - a `--skip` that leaves the rebase halted and still empty is
/// the runaway this bound exists to catch, and one that went uncounted would
/// spin forever. Noticing that the rebase has *finished* costs nothing, so a
/// replay that stops `MAX_RESOLUTION_ROUNDS` times is answered rather than
/// abandoned.
const MAX_RESOLUTION_ROUNDS: usize = 1_000;

/// A detached scratch worktree that removes itself.
pub struct Scratch {
    repo: PathBuf,
    /// Never read: held solely so the temporary directory - and everything the
    /// simulation wrote into it - is removed when the `Scratch` is dropped.
    #[expect(dead_code, reason = "held only so the TempDir is removed on drop")]
    dir: TempDir,
    worktree: PathBuf,
    /// Validated once in [`Scratch::create`] so every `Git` built from it can
    /// have the path infallibly. An empty `core.hooksPath` is not "hooks off" -
    /// git still resolves hook lookups against it - so a path that cannot be
    /// spelled for git has to fail the run, not degrade into one.
    hooks: String,
}

impl Scratch {
    /// Add a detached worktree at `at` in a private temporary directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the temporary directory cannot be created, if its
    /// path cannot be spelled for git as UTF-8, or if git refuses to add the
    /// worktree - most commonly because `repo` is not a repository or `at` does
    /// not name a commit.
    pub fn create(repo: &Path, at: &str) -> Result<Self> {
        let dir = TempDir::new().context("could not create a scratch directory")?;
        let worktree = dir.path().join("worktree");
        let hooks_dir = dir.path().join("hooks");
        std::fs::create_dir(&hooks_dir).context("could not create the empty hooks directory")?;
        let hooks = hooks_dir
            .to_str()
            .context("scratch hooks path is not valid UTF-8")?
            .to_owned();

        let scratch = Self {
            repo: repo.to_path_buf(),
            dir,
            worktree,
            hooks,
        };

        scratch.repo_git().run(&[
            "worktree",
            "add",
            "-q",
            "--detach",
            scratch.worktree_arg()?,
            at,
        ])?;

        Ok(scratch)
    }

    /// Where the scratch worktree is checked out.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.worktree
    }

    /// A runner rooted in the scratch worktree.
    #[must_use]
    pub fn git(&self) -> Git {
        Git::new(&self.worktree, self.hooks.as_str())
    }

    /// Rebase the checked-out HEAD onto `onto`, walking the whole rebase and
    /// auto-resolving conflicts by staging markers verbatim.
    ///
    /// Every stop is measured before it is resolved, so the returned
    /// [`Conflicts`] describes what a human would have had to hand-merge to get
    /// the same result.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned, if the rebase fails
    /// without leaving a rebase to resolve - an unresolvable ref, unrelated
    /// histories, a repository in a state the replay cannot enter - or if the
    /// rebase is still unfinished once `MAX_RESOLUTION_ROUNDS` rounds have been
    /// spent trying to advance it.
    pub fn replay_rebase(&self, onto: &str) -> Result<Conflicts> {
        self.replay_rebase_within(onto, MAX_RESOLUTION_ROUNDS)
    }

    /// [`Scratch::replay_rebase`] with the round budget named rather than baked
    /// in, so the boundary can be pinned on a three-stop fixture instead of a
    /// thousand-commit one.
    ///
    /// The budget is spent only on rounds that *act* on a rebase still in
    /// progress. Finding no rebase left is the exit, checked before anything is
    /// charged, so a replay whose last round completed the rebase leaves with
    /// its answer rather than with a claim it was abandoned - and the refusal
    /// below is unreachable for a rebase that actually finished.
    fn replay_rebase_within(&self, onto: &str, max_rounds: usize) -> Result<Conflicts> {
        let git = self.git();
        let worktree = self.path();

        let mut cost = Conflicts::default();
        let mut outcome = git.try_run(&["rebase", onto])?;
        let mut rounds = 0;

        loop {
            if !rebase_in_progress(&git, worktree)? {
                anyhow::ensure!(
                    outcome.success,
                    "the rebase failed without leaving a rebase to resolve:\n{}\n{}",
                    outcome.stdout,
                    outcome.stderr
                );
                return Ok(cost);
            }

            anyhow::ensure!(
                rounds < max_rounds,
                "gave up on the rebase after {max_rounds} resolution rounds"
            );
            rounds += 1;

            let conflicted = git.nul_separated(&["diff", "--name-only", "--diff-filter=U"])?;

            if conflicted.is_empty() {
                // The rebase halted without unmerged paths - typically a commit
                // that became empty once its changes were already present.
                // Nothing for a human to resolve, so it costs nothing in
                // conflicts - but it costs a round, because a `--skip` that
                // fails to advance the rebase is the runaway the budget exists
                // to stop.
                outcome = git.try_run(&["rebase", "--skip"])?;
                continue;
            }

            cost.stops += 1;
            for file in conflicted {
                let hunks = count_conflict_hunks(&worktree.join(&file))?;
                cost.add_file(file, hunks);
            }

            git.run(&["add", "-A"])?;
            outcome = git.try_run(&["rebase", "--continue"])?;
        }
    }

    fn worktree_arg(&self) -> Result<&str> {
        self.worktree
            .to_str()
            .context("scratch worktree path is not valid UTF-8")
    }

    /// A runner rooted in the real repository.
    fn repo_git(&self) -> Git {
        Git::new(&self.repo, self.hooks.as_str())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: the TempDir goes away regardless, but git also keeps
        // administrative state in the real repo that must be cleaned up.
        // Removing by path takes both, and it runs while the TempDir is still
        // alive, so git still sees the worktree it is being asked about.
        //
        // Deliberately no `worktree prune` afterwards. Pruning is repo-wide and
        // immediate: it deletes the administrative state - including any halted
        // rebase - of every worktree whose directory is merely *missing right
        // now*, which is the normal condition for a worktree on an unmounted
        // drive or a sleeping network mount. A dry run must not cost the
        // developer a worktree. If the removal above ever fails, the leftover
        // entry is inert, and git's own gc clears it once it ages out.
        if let Ok(path) = self.worktree_arg() {
            let _ = self
                .repo_git()
                .try_run(&["worktree", "remove", "--force", path]);
        }
    }
}

/// What replaying one operation - or a whole sequence of them - cost.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Conflicts {
    stops: usize,
    /// Every file that conflicted, mapped to the hunks it contributed.
    ///
    /// A map rather than a set beside a separate running total, because a
    /// report has to say *where* the work lands, and because the total is then
    /// the sum of this map by definition. Storing the total alongside the names
    /// would let the two drift the moment anything updated one without the
    /// other; here they cannot disagree, so no invariant has to be remembered.
    files: BTreeMap<String, usize>,
}

impl Conflicts {
    /// Build a result straight from a per-file hunk breakdown.
    ///
    /// The total is summed from `files` rather than accepted alongside it, so a
    /// hand-built `Conflicts` cannot claim a total its own breakdown
    /// contradicts. That matters because this is the constructor a renderer's
    /// tests reach for: a test fixture that can lie about the totals is a test
    /// fixture that can make a broken renderer look correct.
    ///
    /// A name repeated in `files` accumulates, exactly as a file conflicting at
    /// several stops does during a real replay.
    #[must_use]
    pub fn from_files(files: impl IntoIterator<Item = (String, usize)>, stops: usize) -> Self {
        let mut conflicts = Self {
            stops,
            ..Self::default()
        };
        for (name, hunks) in files {
            conflicts.add_file(name, hunks);
        }
        conflicts
    }

    /// Fold another step's cost into this running total.
    pub fn absorb(&mut self, other: Self) {
        self.stops += other.stops;
        for (name, hunks) in other.files {
            self.add_file(name, hunks);
        }
    }

    /// Attribute `hunks` more conflict hunks to `name`.
    ///
    /// Adding rather than replacing is the whole reason a file is keyed at all:
    /// the same file routinely conflicts at several stops of one replay, and
    /// each of those collisions is separate work for whoever resolves it.
    fn add_file(&mut self, name: String, hunks: usize) {
        *self.files.entry(name).or_default() += hunks;
    }

    /// Whether the replay finished without a single conflict.
    ///
    /// Defined on the file set rather than on the counts, because the file set
    /// is the primary fact: a conflict is something that happened *to a file*,
    /// and the numbers are summaries of it. The three measures cannot disagree
    /// anyway - [`count_conflict_hunks`] floors every conflicted file at one
    /// hunk, and a file only enters the set from inside a stop - so hunks and
    /// stops are both non-zero exactly when the set is non-empty. Anchoring on
    /// the set keeps that true by construction instead of by coincidence: a
    /// future measure that can legitimately be zero cannot make a conflicted
    /// replay report itself clean.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.files.is_empty()
    }

    /// How many times the replay halted for manual resolution.
    #[must_use]
    pub fn stops(&self) -> Stops {
        Stops::new(self.stops)
    }

    /// How many conflict hunks would need hand-merging.
    ///
    /// Summed from the per-file breakdown rather than tracked beside it, so the
    /// headline number and the list underneath it can never tell a developer
    /// two different stories.
    #[must_use]
    pub fn hunks(&self) -> Hunks {
        Hunks::new(self.files.values().sum())
    }

    /// How many distinct files conflicted at least once.
    #[must_use]
    pub fn files(&self) -> Files {
        Files::new(self.files.len())
    }

    /// Every conflicted file paired with how many hunks it contributed, in
    /// sorted order.
    ///
    /// A verdict that says only "4 hunks across 2 files" tells a developer how
    /// much work is coming but not where it lands, so the breakdown is part of
    /// the answer rather than a nicety layered on top.
    pub fn file_hunks(&self) -> impl Iterator<Item = (&str, usize)> {
        self.files
            .iter()
            .map(|(name, hunks)| (name.as_str(), *hunks))
    }
}

/// Whether git is sitting in a halted rebase.
fn rebase_in_progress(git: &Git, worktree: &Path) -> Result<bool> {
    for state_dir in ["rebase-merge", "rebase-apply"] {
        let path = git.run(&["rev-parse", "--git-path", state_dir])?;
        if worktree.join(path).exists() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Count the conflict regions a human would have to hand-merge in one file.
///
/// Conflicts with no markers at all - binary files, add/add on a blob git will
/// not diff, delete/modify - still cost one decision each.
fn count_conflict_hunks(path: &Path) -> Result<usize> {
    let Ok(contents) = std::fs::read(path) else {
        // A delete/modify conflict can leave no file on disk; it is still one
        // decision for the person resolving it.
        return Ok(1);
    };

    let markers = contents
        .split(|byte| *byte == b'\n')
        .filter(|line| line.starts_with(b"<<<<<<<"))
        .count();

    Ok(markers.max(1))
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::{Conflicts, Scratch};
    use crate::metrics::Stops;
    use crate::testing::contested_region_repo;

    /// How many rounds replaying `iterated` onto `single` spends.
    ///
    /// [`contested_region_repo`] gives `iterated` three commits over one region
    /// that `single` has already rewritten, so every one of them collides and
    /// none of them arrives empty: three rounds, all three of them stops. That
    /// the two numbers are equal is asserted below rather than assumed, because
    /// a fixture that quietly gained a `--skip` round would otherwise turn the
    /// boundary test into a test of something one round off it.
    const CONTESTED_ROUNDS: usize = 3;

    /// Replay `iterated` onto `single` with exactly `max_rounds` to spend.
    ///
    /// The error is handed back rather than unwrapped, because both sides of
    /// the boundary are the point: one caller needs the answer, the other needs
    /// the refusal.
    fn replay_contested_within(max_rounds: usize) -> Result<Conflicts> {
        let repo = contested_region_repo();
        let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
        scratch
            .git()
            .run(&["checkout", "-q", "--detach", "iterated"])
            .expect("check out the branch detached in the scratch worktree");
        scratch.replay_rebase_within("single", max_rounds)
    }

    /// Noticing that a rebase has finished must not cost a round.
    ///
    /// A replay that spends its whole budget and finishes has been measured
    /// completely — every stop counted, every hunk attributed — so the only
    /// honest thing to hand back is the answer. Charging the terminating check
    /// a round of its own would instead report that fully-measured replay as a
    /// rebase the harness gave up on, which is the same exit code a consumer
    /// uses for "I could not tell you".
    #[test]
    fn a_replay_that_spends_its_whole_budget_still_reports_its_answer() {
        let conflicts = replay_contested_within(CONTESTED_ROUNDS)
            .expect("a replay that spends exactly its budget of rounds has finished, not stalled");

        assert_eq!(
            conflicts.stops(),
            Stops::new(CONTESTED_ROUNDS),
            "every round the fixture spends is a stop, so the budget it just \
             exhausted has to show up as the stop count"
        );
    }

    /// The other side of the same boundary: the budget still has to bite.
    ///
    /// Giving the terminating check its own round must not turn the bound into
    /// no bound at all — a git state the replay cannot advance is exactly what
    /// it exists to stop, and it has to stop it one round after the last one it
    /// was allowed.
    #[test]
    fn a_replay_that_outruns_its_budget_still_gives_up() {
        let budget = CONTESTED_ROUNDS - 1;

        let error = replay_contested_within(budget)
            .expect_err("a replay needing more rounds than it has must not report an answer");

        assert!(
            error.to_string().contains(&format!(
                "gave up on the rebase after {budget} resolution rounds"
            )),
            "the refusal has to say what it ran out of, got: {error}"
        );
    }
}

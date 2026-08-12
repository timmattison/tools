//! Replays a candidate merge ordering against throwaway git state.
//!
//! # What is being modelled
//!
//! Landing a branch by squash merge collapses it into a single commit with no
//! ancestry link to the original. A sibling branch rebased afterwards therefore
//! re-applies work that is already present, and git has no patch identity left
//! to recognise it with. Whichever branch lands *second* pays that price, so the
//! question "which order is cheaper?" has a real answer.
//!
//! For each branch, in order, the simulation rebases it onto the current
//! simulated main and then squashes the result in. Conflicts are counted, then
//! resolved by staging the conflict markers verbatim.
//!
//! # Where the replay itself lives
//!
//! The scratch worktree, the pinned safety configuration, and the
//! marker-staging resolution loop belong to [`gitscratch::Scratch`]. See
//! [`gitscratch::scratch`] for why markers are staged and what that means for
//! the numbers: the totals are a cost index for comparing orderings measured
//! under identical rules, not an exact prediction. `grist` adds the squash and
//! the ranking on top.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::{Context, Result};
use gitscratch::{BranchName, Conflicts, Git, Repo, Scratch};

use crate::metrics::OrderingScore;
use crate::plan::{ordering_count, permutations};
use crate::rank::rank;

/// Orderings grow factorially. Seven branches is 5,040 replays - far past the
/// point where waiting for an answer beats just picking one and rebasing.
pub const MAX_BRANCHES: usize = 6;

/// Notified as each branch is replayed, so a long run is not silent.
type ProgressListener = Box<dyn Fn(&str)>;

/// Check that `branches` is a list grist will simulate, and report how many
/// orderings doing so means replaying.
///
/// [`Simulator::evaluate`] applies this itself, so nothing has to validate on
/// its way in. It is public because announcing the size of a run is the other
/// thing a caller wants that count for, and deriving it independently is how the
/// count and the limit drift apart - or overflow. The limit is tested before any
/// count is derived, so a branch list too long to have a countable number of
/// orderings is refused by its length alone.
///
/// # Errors
///
/// Returns an error if `branches` is empty, repeats a branch, or is longer than
/// [`MAX_BRANCHES`].
pub fn orderings_to_simulate(branches: &[BranchName]) -> Result<usize> {
    anyhow::ensure!(!branches.is_empty(), "no branches to order");
    anyhow::ensure!(
        branches.len() <= MAX_BRANCHES,
        "{} branches is more than grist's limit of {MAX_BRANCHES}",
        branches.len(),
    );

    let distinct: BTreeSet<_> = branches.iter().collect();
    anyhow::ensure!(
        distinct.len() == branches.len(),
        "each branch may only be listed once"
    );

    ordering_count(branches.len()).with_context(|| {
        format!(
            "{} branches has more orderings than grist can count",
            branches.len()
        )
    })
}

/// Measures what a candidate ordering would cost to carry out for real.
pub struct Simulator {
    repo: Repo,
    base: String,
    progress: Option<ProgressListener>,
}

impl Simulator {
    /// Simulate against the repository containing `repo`, landing branches on
    /// top of `base`.
    ///
    /// Opening the repository is `gitscratch`'s pre-flight, and it happens here,
    /// in the constructor, rather than at the first replay. That is what makes
    /// "you are not in a repository" answerable *before* a caller has announced
    /// a run: left to the first replay, the answer arrives as git's own
    /// complaint from inside `worktree add`, which names `.git` rather than the
    /// directory the user pointed at and reads as a simulation that fell over
    /// rather than a bad argument. A `Simulator` that exists is a `Simulator`
    /// with somewhere to run.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned, or if `repo` is not inside
    /// a git repository; the message names the directory.
    pub fn new(repo: impl AsRef<Path>, base: impl Into<String>) -> Result<Self> {
        Ok(Self {
            repo: Repo::open(repo.as_ref())?,
            base: base.into(),
            progress: None,
        })
    }

    /// Report each replay step to `listener` as it happens.
    #[must_use]
    pub fn with_progress(mut self, listener: impl Fn(&str) + 'static) -> Self {
        self.progress = Some(Box::new(listener));
        self
    }

    /// Tell the listener, if there is one, what is happening.
    fn report(&self, message: &str) {
        if let Some(listener) = &self.progress {
            listener(message);
        }
    }

    /// The base ref that branches land on top of.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Replay `order` and report what resolving it would cost.
    ///
    /// # Errors
    ///
    /// Returns an error if git is unavailable, the scratch worktree cannot be
    /// created, a branch in `order` does not resolve, or a rebase reaches a
    /// state the resolution loop cannot drive forward.
    pub fn score(&self, order: &[BranchName]) -> Result<OrderingScore> {
        let scratch = self.repo.scratch(&self.base)?;
        let mut simulated_main = scratch.git().rev_parse(&self.base)?;
        let mut total = Conflicts::default();

        for branch in order {
            let (next_main, step) = self.land(&scratch, &simulated_main, branch)?;
            simulated_main = next_main;
            total.absorb(step);
        }

        Ok(into_score(&total, order.to_vec()))
    }

    /// Score every order `branches` could land in, cheapest first.
    ///
    /// Orderings that share a leading run of branches share the work of
    /// simulating it: results are memoised on the ordered prefix. The prefix has
    /// to be ordered rather than a set, because the auto-resolved tree a prefix
    /// leaves behind depends on the sequence that produced it.
    ///
    /// # Errors
    ///
    /// Returns an error if the branch list is empty, repeats a branch, is
    /// longer than [`MAX_BRANCHES`], or if any simulation fails.
    pub fn evaluate(&self, branches: &[BranchName]) -> Result<Vec<OrderingScore>> {
        orderings_to_simulate(branches)?;

        let scratch = self.repo.scratch(&self.base)?;
        let base_commit = scratch.git().rev_parse(&self.base)?;

        let mut memo: HashMap<Vec<BranchName>, (String, Conflicts)> = HashMap::new();
        memo.insert(Vec::new(), (base_commit, Conflicts::default()));

        let mut scores = Vec::new();

        for ordering in permutations(branches) {
            let mut prefix: Vec<BranchName> = Vec::new();

            for branch in &ordering {
                let mut extended = prefix.clone();
                extended.push(branch.clone());

                if !memo.contains_key(&extended) {
                    let (onto, mut cumulative) = memo
                        .get(&prefix)
                        .cloned()
                        .context("internal error: a shorter prefix was not simulated first")?;

                    let (next_main, step) = self.land(&scratch, &onto, branch)?;
                    cumulative.absorb(step);
                    memo.insert(extended.clone(), (next_main, cumulative));
                }

                prefix = extended;
            }

            let (_, total) = memo
                .get(&prefix)
                .context("internal error: the full ordering was not simulated")?;
            scores.push(into_score(total, ordering));
        }

        Ok(rank(scores))
    }

    /// Rebase `branch` onto `onto` and squash it in, reporting the new
    /// simulated main and what the step cost.
    fn land(
        &self,
        scratch: &Scratch,
        onto: &str,
        branch: &BranchName,
    ) -> Result<(String, Conflicts)> {
        let git = scratch.git();

        self.report(&format!("replaying {branch}"));

        // Detached, so the real branch ref is never moved.
        git.run(&["checkout", "-q", "--detach", branch.as_str()])
            .with_context(|| format!("could not check out '{branch}'"))?;

        let cost = scratch
            .replay_rebase(onto)
            .with_context(|| format!("could not replay '{branch}' onto the simulated main"))?;
        let next_main = squash_into(&git, onto, branch)?;

        Ok((next_main, cost))
    }
}

/// Attribute a replay's cost to the ordering that produced it.
fn into_score(conflicts: &Conflicts, order: Vec<BranchName>) -> OrderingScore {
    OrderingScore::new(
        order,
        conflicts.stops(),
        conflicts.files(),
        conflicts.hunks(),
    )
}

/// Collapse the checked-out (already rebased) branch into a single commit on
/// top of `parent`, discarding its ancestry exactly as a squash merge does.
fn squash_into(git: &Git, parent: &str, branch: &BranchName) -> Result<String> {
    let tree = git.run(&["rev-parse", "HEAD^{tree}"])?;
    git.run(&[
        "commit-tree",
        &tree,
        "-p",
        parent,
        "-m",
        &format!("squash {branch}"),
    ])
}

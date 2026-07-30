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
//! # Why markers, and what that means for the numbers
//!
//! Staging markers is the conservative auto-resolution: unlike `--ours` or
//! `--theirs` it never silently discards a side. It does mean a later commit
//! touching the same region conflicts again - but that is faithful to reality,
//! since a human resolution also leaves later commits conflicting against the
//! resolved state. Treat the totals as a cost index for comparing orderings
//! measured under identical rules, not as an exact prediction.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tempfile::TempDir;

use crate::git::Git;
use crate::metrics::{BranchName, Files, Hunks, OrderingScore, Stops};
use crate::plan::permutations;
use crate::rank::rank;

/// Upper bound on rebase resolution rounds per branch, so a git state we failed
/// to anticipate stalls the run instead of spinning forever.
const MAX_RESOLUTION_ROUNDS: usize = 1_000;

/// Orderings grow factorially. Seven branches is 5,040 replays - far past the
/// point where waiting for an answer beats just picking one and rebasing.
pub const MAX_BRANCHES: usize = 6;

/// Measures what a candidate ordering would cost to carry out for real.
pub struct Simulator {
    repo: PathBuf,
    base: String,
}

impl Simulator {
    /// Simulate against `repo`, landing branches on top of `base`.
    #[must_use]
    pub fn new(repo: impl Into<PathBuf>, base: impl Into<String>) -> Self {
        Self {
            repo: repo.into(),
            base: base.into(),
        }
    }

    /// The repository being simulated against.
    #[must_use]
    pub fn repo(&self) -> &Path {
        &self.repo
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
        let scratch = Scratch::create(&self.repo, &self.base)?;
        let mut simulated_main = scratch.git().rev_parse(&self.base)?;
        let mut total = Cost::default();

        for branch in order {
            let (next_main, step) = self.land(&scratch, &simulated_main, branch)?;
            simulated_main = next_main;
            total.absorb(step);
        }

        Ok(total.into_score(order.to_vec()))
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
        anyhow::ensure!(!branches.is_empty(), "no branches to order");
        anyhow::ensure!(
            branches.len() <= MAX_BRANCHES,
            "{} branches means {} orderings to simulate; grist stops at {MAX_BRANCHES}",
            branches.len(),
            (1..=branches.len()).product::<usize>(),
        );

        let distinct: BTreeSet<_> = branches.iter().collect();
        anyhow::ensure!(
            distinct.len() == branches.len(),
            "each branch may only be listed once"
        );

        let scratch = Scratch::create(&self.repo, &self.base)?;
        let base_commit = scratch.git().rev_parse(&self.base)?;

        let mut memo: HashMap<Vec<BranchName>, (String, Cost)> = HashMap::new();
        memo.insert(Vec::new(), (base_commit, Cost::default()));

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
                        .expect("the shorter prefix is always simulated first");

                    let (next_main, step) = self.land(&scratch, &onto, branch)?;
                    cumulative.absorb(step);
                    memo.insert(extended.clone(), (next_main, cumulative));
                }

                prefix = extended;
            }

            let (_, total) = memo
                .get(&prefix)
                .expect("the full ordering was just simulated");
            scores.push(total.clone().into_score(ordering));
        }

        Ok(rank(scores))
    }

    /// Rebase `branch` onto `onto` and squash it in, reporting the new
    /// simulated main and what the step cost.
    fn land(&self, scratch: &Scratch, onto: &str, branch: &BranchName) -> Result<(String, Cost)> {
        let git = scratch.git();

        // Detached, so the real branch ref is never moved.
        git.run(&["checkout", "-q", branch.as_str()])
            .with_context(|| format!("could not check out '{branch}'"))?;

        let cost = self.replay_onto(&git, scratch.path(), onto, branch)?;
        let next_main = squash_into(&git, onto, branch)?;

        Ok((next_main, cost))
    }

    /// Rebase the checked-out branch onto `onto`, resolving as it goes.
    fn replay_onto(
        &self,
        git: &Git,
        worktree: &Path,
        onto: &str,
        branch: &BranchName,
    ) -> Result<Cost> {
        let mut cost = Cost::default();
        let mut outcome = git.try_run(&["rebase", onto])?;

        for _ in 0..MAX_RESOLUTION_ROUNDS {
            if !rebase_in_progress(git, worktree)? {
                anyhow::ensure!(
                    outcome.success,
                    "rebasing '{branch}' onto the simulated main failed without leaving a \
                     rebase to resolve:\n{}\n{}",
                    outcome.stdout,
                    outcome.stderr
                );
                return Ok(cost);
            }

            let conflicted = git.lines(&["diff", "--name-only", "--diff-filter=U"])?;

            if conflicted.is_empty() {
                // The rebase halted without unmerged paths - typically a commit
                // that became empty once its changes were already present.
                // Nothing for a human to resolve, so it costs nothing.
                outcome = git.try_run(&["rebase", "--skip"])?;
                continue;
            }

            cost.stops += 1;
            for file in conflicted {
                cost.hunks += count_conflict_hunks(&worktree.join(&file))?;
                cost.files.insert(file);
            }

            git.run(&["add", "-A"])?;
            outcome = git.try_run(&["rebase", "--continue"])?;
        }

        anyhow::bail!(
            "gave up rebasing '{branch}' after {MAX_RESOLUTION_ROUNDS} resolution rounds"
        )
    }
}

/// What replaying one branch - or a whole prefix of them - cost.
#[derive(Default, Clone)]
struct Cost {
    stops: usize,
    hunks: usize,
    files: BTreeSet<String>,
}

impl Cost {
    /// Fold another step's cost into this running total.
    fn absorb(&mut self, other: Self) {
        self.stops += other.stops;
        self.hunks += other.hunks;
        self.files.extend(other.files);
    }

    /// Attribute this cost to the ordering that produced it.
    fn into_score(self, order: Vec<BranchName>) -> OrderingScore {
        OrderingScore::new(
            order,
            Stops::new(self.stops),
            Files::new(self.files.len()),
            Hunks::new(self.hunks),
        )
    }
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

/// A detached scratch worktree that removes itself.
struct Scratch {
    repo: PathBuf,
    dir: TempDir,
    worktree: PathBuf,
    hooks: PathBuf,
}

impl Scratch {
    /// Add a detached worktree at `base` in a private temporary directory.
    fn create(repo: &Path, base: &str) -> Result<Self> {
        let dir = TempDir::new().context("could not create a scratch directory")?;
        let worktree = dir.path().join("worktree");
        let hooks = dir.path().join("hooks");
        std::fs::create_dir(&hooks).context("could not create the empty hooks directory")?;

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
            base,
        ])?;

        Ok(scratch)
    }

    fn path(&self) -> &Path {
        &self.worktree
    }

    fn worktree_arg(&self) -> Result<&str> {
        self.worktree
            .to_str()
            .context("scratch worktree path is not valid UTF-8")
    }

    fn hooks_arg(&self) -> &str {
        self.hooks.to_str().unwrap_or("")
    }

    /// A runner rooted in the real repository.
    fn repo_git(&self) -> Git {
        Git::new(&self.repo, self.hooks_arg())
    }

    /// A runner rooted in the scratch worktree.
    fn git(&self) -> Git {
        Git::new(&self.worktree, self.hooks_arg())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: the TempDir goes away regardless, but git also keeps
        // administrative state in the real repo that must be cleaned up.
        if let Ok(path) = self.worktree_arg() {
            let _ = self.repo_git().try_run(&["worktree", "remove", "--force", path]);
        }
        let _ = self.repo_git().try_run(&["worktree", "prune"]);
        let _ = &self.dir;
    }
}

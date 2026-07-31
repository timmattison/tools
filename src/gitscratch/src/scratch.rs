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

/// Upper bound on rebase resolution rounds per branch, so a git state we failed
/// to anticipate stalls the run instead of spinning forever.
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
    /// resolution loop still has not finished after `MAX_RESOLUTION_ROUNDS`
    /// rounds.
    pub fn replay_rebase(&self, onto: &str) -> Result<Conflicts> {
        let git = self.git();
        let worktree = self.path();

        let mut cost = Conflicts::default();
        let mut outcome = git.try_run(&["rebase", onto])?;

        for _ in 0..MAX_RESOLUTION_ROUNDS {
            if !rebase_in_progress(&git, worktree)? {
                anyhow::ensure!(
                    outcome.success,
                    "the rebase failed without leaving a rebase to resolve:\n{}\n{}",
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
                let hunks = count_conflict_hunks(&worktree.join(&file))?;
                cost.add_file(file, hunks);
            }

            git.run(&["add", "-A"])?;
            outcome = git.try_run(&["rebase", "--continue"])?;
        }

        anyhow::bail!("gave up on the rebase after {MAX_RESOLUTION_ROUNDS} resolution rounds")
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

    /// Which files conflicted, in sorted order.
    ///
    /// A caller rendering the result needs the names, not just how many there
    /// were; [`Conflicts::files`] is the count of exactly this sequence.
    pub fn file_names(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
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

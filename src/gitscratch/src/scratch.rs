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
//!
//! # Why a halt with nothing unmerged is a question, not an answer
//!
//! A rebase can also stop with no unmerged paths at all, and that state has
//! more than one cause. Git stops there for a commit that adds nothing to the
//! new base, which is free to drop - and it stops there for a commit it could
//! not *write*, where dropping it throws the work away and reports a cost for a
//! branch that was never replayed. Signing, hooks, a full or read-only object
//! database, an unusable editor: they all land in the same place, and git's
//! exit status is non-zero for the harmless case too, so nothing about the
//! invocation separates them.
//!
//! So the replay classifies that halt from repository state - see the `Halt`
//! enum below - rather than assuming the harmless cause. A dry run may legitimately answer
//! "this is expensive" or "I cannot answer"; it must never answer "this is
//! cheap" because it quietly discarded the work it was asked to measure.

use std::collections::BTreeSet;
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
    /// histories, a repository in a state the replay cannot enter - if git
    /// could not *write* a commit it was replaying, since carrying on would
    /// mean discarding that commit and reporting a cost for a branch that was
    /// never replayed, if git refused to skip a commit that had become empty,
    /// which leaves the rebase unfinished however many times it is asked again,
    /// or if the resolution loop still has not finished after
    /// `MAX_RESOLUTION_ROUNDS` rounds.
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

            match classify_halt(&git)? {
                Halt::Conflict(conflicted) => {
                    cost.stops += 1;
                    for file in conflicted {
                        cost.hunks += count_conflict_hunks(&worktree.join(&file))?;
                        cost.files.insert(file);
                    }

                    git.run(&["add", "-A"])?;
                    outcome = git.try_run(&["rebase", "--continue"])?;
                }
                Halt::EmptyCommit { stopped } => {
                    // Nothing for a human to resolve and nothing lost by
                    // dropping it, so it costs nothing.
                    outcome = git
                        .try_run(&["rebase", "--skip"])
                        .with_context(|| format!("could not skip the empty commit {stopped}"))?;

                    // Read here, before the loop can come round again, because
                    // coming round again is how this used to be lost. A skip git
                    // refused cannot start working - re-issuing it only spins to
                    // `MAX_RESOLUTION_ROUNDS` - and each new invocation
                    // overwrites the one message that said what went wrong,
                    // while the next round classifies wherever the failed skip
                    // left the rebase sitting rather than the commit the skip was
                    // dropping.
                    anyhow::ensure!(
                        outcome.success,
                        "the rebase halted on a commit that adds nothing to the new base, but \
                         git would not `rebase --skip` it: {stopped}\ngit said:\n{}\n{}",
                        outcome.stdout,
                        outcome.stderr
                    );
                }
                Halt::UnwritableCommit { stopped, evidence } => {
                    // `outcome` still holds the invocation that failed, which is
                    // where git explained itself. Skipping used to overwrite it
                    // before anyone could read it, so the one message that said
                    // what had gone wrong was discarded along with the commit.
                    anyhow::bail!(
                        "the rebase halted with nothing to merge, but git did not write the \
                         commit it was replaying: {stopped}\n{evidence}\n\
                         Skipping it would silently throw that work away and report a cost for \
                         a branch that was never replayed. git said:\n{}\n{}",
                        outcome.stdout,
                        outcome.stderr
                    );
                }
            }
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
    hunks: usize,
    files: BTreeSet<String>,
}

impl Conflicts {
    /// Fold another step's cost into this running total.
    pub fn absorb(&mut self, other: Self) {
        self.stops += other.stops;
        self.hunks += other.hunks;
        self.files.extend(other.files);
    }

    /// How many times the replay halted for manual resolution.
    #[must_use]
    pub fn stops(&self) -> Stops {
        Stops::new(self.stops)
    }

    /// How many conflict hunks would need hand-merging.
    #[must_use]
    pub fn hunks(&self) -> Hunks {
        Hunks::new(self.hunks)
    }

    /// How many distinct files conflicted at least once.
    #[must_use]
    pub fn files(&self) -> Files {
        Files::new(self.files.len())
    }

    /// Which files conflicted, in sorted order.
    ///
    /// A caller rendering the result needs the names, not just how many there
    /// were; [`Conflicts::files`] is the count of exactly this set.
    #[must_use]
    pub fn file_names(&self) -> &BTreeSet<String> {
        &self.files
    }
}

/// Why a replay is sitting in a halted rebase.
enum Halt {
    /// Paths git could not merge; a human would hand-merge these.
    Conflict(Vec<String>),
    /// Git stopped at a commit that adds nothing to the new base, so dropping
    /// it loses no work. `stopped` describes it for any message about it.
    EmptyCommit { stopped: String },
    /// Git could not write the commit it was replaying. Skipping would throw
    /// that work away; `evidence` says which state proved it.
    UnwritableCommit { stopped: String, evidence: String },
}

/// Work out, from repository state alone, why the rebase is halted.
///
/// A halt with nothing unmerged is a *classification point*, not a single known
/// case. Git stops there for a commit that has become empty, which is free to
/// drop, and it stops there for a commit it could not write, where dropping it
/// loses the work and reports a cost for a branch that was never replayed.
/// Nothing in git's exit status separates the two, so the answer has to come
/// from what the repository looks like. Every probe below errs toward the loud
/// answer, which is the safe direction: a dry run may say "expensive" or "I
/// cannot answer", never "cheap" because it quietly discarded something.
fn classify_halt(git: &Git) -> Result<Halt> {
    let conflicted = git.lines(&["diff", "--name-only", "--diff-filter=U"])?;
    if !conflicted.is_empty() {
        return Ok(Halt::Conflict(conflicted));
    }

    // Without REBASE_HEAD the loop cannot even name the commit it is about to
    // drop, so it has no business dropping it.
    let Ok(stopped) = git.run(&["log", "-1", "--format=%h %s", "REBASE_HEAD"]) else {
        return Ok(Halt::UnwritableCommit {
            stopped: "a commit git would not name".to_owned(),
            evidence: "REBASE_HEAD does not resolve, so the replay cannot say which commit the \
                       rebase halted on"
                .to_owned(),
        });
    };

    // Content left behind is content that failed to be committed: a commit that
    // truly became empty leaves the index matching HEAD and the worktree
    // matching the index. Asked as `lines`, not as a `--quiet` exit code, so
    // git failing to answer is an error rather than a vote for "empty".
    let mut uncommitted = git.lines(&["diff", "--cached", "--name-only", "HEAD"])?;
    uncommitted.extend(git.lines(&["diff", "--name-only"])?);
    uncommitted.sort();
    uncommitted.dedup();

    if !uncommitted.is_empty() {
        return Ok(Halt::UnwritableCommit {
            stopped,
            evidence: format!(
                "this content was left uncommitted: {}",
                uncommitted.join(", ")
            ),
        });
    }

    stopped_commit_is_already_in_head(git, stopped)
}

/// Decide whether the halted commit adds anything the new base does not already
/// have - the second probe, and the only one left once the repository is
/// pristine.
///
/// A commit write that fails on a *clean* pick leaves nothing behind at all: git
/// rolls the index back and reschedules the pick, so index, worktree and HEAD
/// all agree and the probe above has nothing to see. What still separates that
/// from a commit that really did become empty is the commit itself.
///
/// The test is: for every path the stopped commit touches, does HEAD already
/// hold exactly that commit's content? If so the commit is empty, and that
/// answer is airtight rather than a heuristic. Applying commit `C` onto HEAD is
/// a three-way merge with base `C^`, ours HEAD and theirs `C`. On a path where
/// HEAD's blob already equals `C`'s blob both sides agree, so the merge changes
/// nothing there; a path `C` never touched cannot change either, since neither
/// side moved it. So the merge result is HEAD exactly, and the commit adds
/// nothing.
///
/// Like the first probe this errs toward the loud answer: a path the commit
/// touched whose content is *not* in HEAD is work about to be dropped, and the
/// replay says so rather than reporting a cheap number for a branch it never
/// replayed.
fn stopped_commit_is_already_in_head(git: &Git, stopped: String) -> Result<Halt> {
    let touched = git.lines(&[
        "diff-tree",
        "--no-commit-id",
        "--name-only",
        "-r",
        "--root",
        "REBASE_HEAD",
    ])?;

    // Guarded before the diff below is built, because `git diff ... --` with an
    // empty pathspec is not "diff nothing", it is "diff everything" - which
    // would invert this answer for the one commit that cannot possibly lose
    // anything, since it changes no path at all.
    if touched.is_empty() {
        return Ok(Halt::EmptyCommit { stopped });
    }

    let mut diff = vec!["diff", "--name-only", "REBASE_HEAD", "HEAD", "--"];
    diff.extend(touched.iter().map(String::as_str));
    let missing = git.lines(&diff)?;

    if missing.is_empty() {
        Ok(Halt::EmptyCommit { stopped })
    } else {
        Ok(Halt::UnwritableCommit {
            stopped,
            evidence: format!(
                "the new base does not have this commit's changes to: {}",
                missing.join(", ")
            ),
        })
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

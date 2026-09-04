//! A throwaway worktree, and the git operations replayed inside it.
//!
//! A [`Scratch`] is a detached worktree of the developer's real repository,
//! living in a private temporary directory and removing itself on drop. Every
//! git call made through it goes via the crate-private `Git` runner, so the
//! whole safety configuration applies to the replay whether the caller
//! remembered it or not. The runner never leaves the crate, which is the other
//! half of the same rule - see [`Scratch`].
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

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
#[cfg(any(test, feature = "testing"))]
use std::num::NonZeroUsize;
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
/// stop, or skipping a commit that arrived with nothing unmerged. The charge
/// happens before the loop learns which of the two it has, so every round costs
/// the same. Noticing that the rebase has *finished* costs nothing, so a replay
/// that stops `MAX_RESOLUTION_ROUNDS` times is answered rather than abandoned.
///
/// **What the bound catches today is a resolution that makes no progress, and
/// only that.** Of the three halt arms below, one comes round again. A stop
/// resolves and returns to the top of the loop. A skip reads git's outcome at
/// once and stops the replay unless git exited zero, and `git rebase --skip`
/// exits zero only when it has finished the rebase. Git 2.55 was watched to exit
/// 1 for a skip that worked and then met a conflict, and again for a skip that
/// worked and then met a second empty commit. A commit git could not write stops
/// the replay outright. So no round can follow a skip round, and charging a skip
/// decides nothing a test can watch fail.
///
/// The charge stays at the top of the loop all the same, because the rule is
/// that a round of work costs a round. An arm that starts coming round again
/// after a skip needs the charge already there, and a charge written into one
/// arm alone leaves the next arm uncounted. `MUTATIONS.md` records this as an
/// unfalsifiable guard rather than claiming one nobody has watched fail.
const MAX_RESOLUTION_ROUNDS: usize = 1_000;

/// A detached scratch worktree that removes itself.
///
/// # The runner stays inside this crate
///
/// A scratch worktree is a *linked* worktree of the developer's real
/// repository, so it shares that repository's refs, configuration and object
/// store. The hardening this crate applies is configuration: it pins the
/// settings that make a *replay* non-destructive. It says nothing at all about
/// `branch -D`, `update-ref`, `config --local` or `push`, because those are
/// different commands and no setting refuses them. A consumer holding a runner
/// can therefore send any of them straight into the real repository through the
/// scratch worktree, and the crate's central promise - that a consumer cannot
/// reach an unhardened git - is then false.
///
/// So the runner does not leave this crate. A consumer asks for the operation
/// it wants by name, and this type builds the git call.
///
/// The named operations compile:
///
/// ```no_run
/// let scratch = gitscratch::Repo::open(std::path::Path::new("."))
///     .expect("a repository")
///     .scratch("HEAD")
///     .expect("a scratch worktree");
/// scratch.check_out_detached("feature").expect("a checkout");
/// let conflicts = scratch.replay_rebase("main").expect("a replay");
/// let tree = scratch.head_tree().expect("a tree");
/// let commit = scratch
///     .commit_tree(&tree, "main", "squash feature")
///     .expect("a commit");
/// ```
///
/// A reach for the runner does not. The block below is the same setup with one
/// line added, so what it proves is that the added line is what stops it:
///
/// ```compile_fail
/// let scratch = gitscratch::Repo::open(std::path::Path::new("."))
///     .expect("a repository")
///     .scratch("HEAD")
///     .expect("a scratch worktree");
/// let runner = scratch.git();
/// ```
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
    /// Crate-private on purpose, and for the same reason [`Git::new`] is:
    /// `repo` here is an unvalidated path, so a public entrance would let a
    /// caller build a worktree of a directory nobody had established was a
    /// repository - and then meet that fact halfway through a simulation, as
    /// git's own complaint from inside `worktree add`.
    /// [`Repo::scratch`](crate::Repo::scratch) is the only way in, so the path
    /// that arrives here is one [`Repo::open`](crate::Repo::open) has already
    /// checked.
    ///
    /// # Errors
    ///
    /// Returns an error if the temporary directory cannot be created, if its
    /// path cannot be spelled for git as UTF-8, or if git refuses to add the
    /// worktree - most commonly because `at` does not name a commit.
    pub(crate) fn create(repo: &Path, at: &str) -> Result<Self> {
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

        // `--end-of-options` ahead of both positionals, because `at` arrives
        // from a caller and a caller can spell a revision that starts with a
        // dash. Without it `git worktree add -q --detach <path> --force` is a
        // complete and valid command: git reads `--force` as its own flag,
        // finds no commit-ish left, and builds the worktree at HEAD - exit 0,
        // no complaint. The caller then measures a branch nobody asked about,
        // which is the cheap answer this crate exists never to give. With it,
        // both positionals are read in order and git refuses the revision by
        // name. Pinned by `tests/repo.rs`.
        scratch.repo_git().run(
            "worktree",
            &[
                "add",
                "-q",
                "--detach",
                "--end-of-options",
                scratch.worktree_arg()?,
                at,
            ],
        )?;

        Ok(scratch)
    }

    /// Where the scratch worktree is checked out.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.worktree
    }

    /// A runner rooted in the scratch worktree.
    ///
    /// Crate-private, and that is the second half of the promise the crate
    /// makes. [`Git::new`] being crate-private stops a consumer *building* a
    /// runner; this stops a consumer being *handed* one. The type documentation
    /// above says what a consumer could do with it, and the operations below
    /// are what stands in its place.
    #[must_use]
    pub(crate) fn git(&self) -> Git {
        Git::new(&self.worktree, self.hooks.as_str())
    }

    /// The same runner, for a test suite outside this crate.
    ///
    /// This crate's own integration tests are out-of-crate consumers - the test
    /// targets in `tests/` compile against the public API - and they have to
    /// arm the controls the safety suite rests on and read the scratch worktree
    /// back afterwards. Both jobs need a git call this type does not name, and
    /// deliberately never will: `tests/safety.rs` spells its detached checkout
    /// out rather than calling [`Scratch::check_out_detached`], because that
    /// checkout is one of the guards under test and a guard read through the
    /// code it guards proves nothing.
    ///
    /// A raw runner is safe to hand a test and not a consumer, and the
    /// difference is what each of them is pointed at. A test builds a throwaway
    /// repository, measures it, and deletes it, so the worst a stray command
    /// costs is that fixture. A consumer is pointed at the developer's own
    /// repository, where the same command costs a branch.
    ///
    /// The `testing` feature is how this crate marks everything that exists for
    /// a test target and for nothing else, so this is gated the way
    /// [`Conflicts::from_files`] is. Turning the feature on grants no new
    /// power over a *real* repository: [`Repo::open`](crate::Repo::open) is
    /// still the only door, and it is the door the pre-flight guards.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn testing_git(&self) -> Git {
        self.git()
    }

    /// Check `revision` out with a detached HEAD.
    ///
    /// Detached, so no branch ref moves. That is what lets a branch already
    /// checked out in another worktree be replayed at all, and it is what keeps
    /// the replay off the developer's own refs.
    ///
    /// `--end-of-options` stands ahead of the name, because `revision` arrives
    /// from a caller and a revision can start with a dash. Without it
    /// `git checkout -q --detach --progress` is a complete and valid command:
    /// git reads `--progress` as its own option, finds no revision left to
    /// check out, and detaches HEAD where it already stands - exit 0, no
    /// complaint. The scratch worktree then stays on the base, a replay finds
    /// nothing to replay, and the caller reports a cost of zero for work nobody
    /// did. Zero is what a genuinely free replay reports too, so nothing
    /// downstream tells the two apart. A plain `--` is the wrong separator
    /// here: `checkout` reads everything after it as a pathspec, so
    /// `--detach -- <revision>` refuses the revisions that do exist. Pinned by
    /// `refuses_a_branch_whose_name_starts_with_a_dash_rather_than_scoring_a_replay_it_never_did`
    /// in `grist`'s `tests/simulation.rs`.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned, or if git refused the
    /// checkout - most commonly because `revision` does not name a commit. The
    /// message names the revision, because that name is what the caller typed
    /// and has to correct.
    pub fn check_out_detached(&self, revision: &str) -> Result<()> {
        self.git()
            .run(
                "checkout",
                &["-q", "--detach", "--end-of-options", revision],
            )
            .with_context(|| format!("could not check out '{revision}'"))?;

        Ok(())
    }

    /// The id of the tree the scratch worktree's HEAD points at.
    ///
    /// The content of a commit without its ancestry, which is what a caller
    /// building a squash needs: [`Scratch::commit_tree`] takes this and gives
    /// it a parent of the caller's choosing.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned, or if HEAD does not
    /// resolve - a worktree with no commit at HEAD being the ordinary way to
    /// reach that.
    pub fn head_tree(&self) -> Result<String> {
        self.git()
            .run("rev-parse", &["HEAD^{tree}"])
            .context("could not read the tree the scratch worktree's HEAD points at")
    }

    /// Write a commit holding `tree`, with `parent` as its one parent, and
    /// report the id of the commit written.
    ///
    /// This is the squash: the new commit carries the whole content of `tree`
    /// and none of the ancestry that produced it, exactly as a squash merge
    /// does. It writes an object and moves no ref, so the commit it reports is
    /// reachable from nothing until a caller names it as the parent of the
    /// next one.
    ///
    /// `tree` and `parent` are object ids rather than revisions a person typed.
    /// [`Scratch::head_tree`] produces the first, and the second is either an
    /// id this method returned before or one
    /// [`Repo::resolve`](crate::Repo::resolve) settled.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned, or if git refused to write
    /// the commit - `tree` naming no tree, or `parent` naming no commit.
    pub fn commit_tree(&self, tree: &str, parent: &str, message: &str) -> Result<String> {
        self.git()
            .run("commit-tree", &[tree, "-p", parent, "-m", message])
            .with_context(|| format!("could not write a commit holding the tree {tree}"))
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
    /// or if the rebase is still unfinished once `MAX_RESOLUTION_ROUNDS` rounds
    /// have been spent trying to advance it.
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
        // `--end-of-options` ahead of `onto`, because `onto` arrives from a
        // caller and git knows `--root` as an option of `rebase`. Without it a
        // replay onto `--root` rebases the whole history onto nothing, finishes
        // without a single conflict, and reports a cost of zero for a revision
        // that names no commit. Zero is what a genuinely free replay reports
        // too, so nothing downstream tells the two apart. With it git refuses
        // the upstream by name. Pinned by
        // `refuses_an_upstream_that_starts_with_a_dash_rather_than_replaying_onto_the_root`.
        let mut outcome = git.try_run("rebase", &["--end-of-options", onto])?;
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
            // Charged here rather than inside an arm, so every kind of round
            // costs the same and a new arm cannot arrive uncounted. See
            // `MAX_RESOLUTION_ROUNDS` for which arms this can decide anything
            // about today.
            rounds += 1;

            match classify_halt(&git)? {
                Halt::Conflict(conflicted) => {
                    cost.stops += 1;
                    for file in conflicted {
                        let hunks = count_conflict_hunks(&worktree.join(&file))?;
                        cost.add_file(file, hunks);
                    }

                    git.run("add", &["-A"])?;
                    outcome = git.try_run("rebase", &["--continue"])?;
                }
                Halt::EmptyCommit { stopped } => {
                    // Nothing for a human to resolve and nothing lost by
                    // dropping it, so it costs nothing.
                    outcome = git
                        .try_run("rebase", &["--skip"])
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
                .try_run("worktree", &["remove", "--force", path]);
        }
    }
}

/// What replaying one operation - or a whole sequence of them - cost.
///
/// # A verdict is measured, never minted
///
/// The clean verdict is the one that renders "hit no conflicts" and exits 0,
/// and it is exactly the value a `Default` derive hands out: no files, no
/// stops. A derive puts that value behind the spelling every caller reaches for
/// first, and behind every generic route to it besides -
/// `..Default::default()`, `unwrap_or_default`, a collection filled with
/// `Default::default`. [`Conflicts::from_files`] is compiled only for tests and
/// for the `testing` feature on the grounds that a released binary has no
/// business stating a cost that nothing measured, and a derive beside it makes
/// that gate a form of words.
///
/// So there is no `Default`. A running total that a fold has taken nothing into
/// yet is a real thing a caller needs, and it comes from a constructor that
/// says so by name - `Conflicts::nothing_replayed`, which reads as the seed it
/// is at every call site that uses it.
///
/// A replay measures a cost:
///
/// ```no_run
/// let scratch = gitscratch::Repo::open(std::path::Path::new("."))
///     .expect("a repository")
///     .scratch("HEAD")
///     .expect("a scratch worktree");
/// let cost = scratch.replay_rebase("main").expect("a replay");
/// ```
///
/// A derive does not. The block below is the same setup with the last line
/// changed, so what it proves is that the changed line is what stops it:
///
/// ```compile_fail
/// let scratch = gitscratch::Repo::open(std::path::Path::new("."))
///     .expect("a repository")
///     .scratch("HEAD")
///     .expect("a scratch worktree");
/// let cost = gitscratch::Conflicts::default();
/// ```
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
    ///
    /// Keyed on an [`OsString`] rather than on the [`PathBuf`] every public
    /// method speaks in, because the key decides the order the breakdown prints
    /// in and the two types disagree about it. `OsString` orders by bytes on
    /// unix, which is git's own order and today's output. `Path` orders by
    /// *component*, so it puts `src/lib.rs` before `src.txt` where a byte
    /// comparison puts `src.txt` first (`.` is `0x2e`, `/` is `0x2f`) - a
    /// reordering no test here would have caught and every reader of a
    /// breakdown would have seen. Handing the names back out as `&Path` costs
    /// nothing, since `Path::new` on an `&OsStr` is a cast.
    files: BTreeMap<OsString, usize>,
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
    /// The parameter types carry the rest of that honesty. A hunk count is a
    /// [`NonZeroUsize`] because a file only reaches a breakdown by having
    /// conflicted, and a file that conflicted is at least one decision - so "a
    /// conflicted file that cost nothing" is not a fixture this can be asked
    /// for. The stop count is a [`Stops`] rather than a second bare number, so
    /// it cannot be transposed with the file count it is read beside.
    ///
    /// A name repeated in `files` accumulates, exactly as a file conflicting at
    /// several stops does during a real replay.
    ///
    /// Compiled only for tests and for the `testing` feature. Every call site
    /// is a fixture, and production code has no business minting a verdict that
    /// nothing measured.
    ///
    /// # Panics
    ///
    /// Panics if the breakdown and the stop count disagree about whether
    /// anything conflicted at all - files with no stops, or stops with no
    /// files. A file only ever enters the breakdown from inside a stop, so the
    /// two are non-empty together or not at all. A fixture that broke that
    /// would render either a clean verdict that swallowed its stops or a
    /// conflict verdict for a replay that never halted, and both read as
    /// perfectly plausible output.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub fn from_files(
        files: impl IntoIterator<Item = (PathBuf, NonZeroUsize)>,
        stops: Stops,
    ) -> Self {
        let mut conflicts = Self {
            stops: stops.count(),
            ..Self::default()
        };
        for (name, hunks) in files {
            conflicts.add_file(name, hunks.get());
        }

        assert_eq!(
            conflicts.is_clean(),
            conflicts.stops == 0,
            "a hand-built result has to agree with itself about whether \
             anything conflicted, got {} and {}",
            conflicts.files().phrase(),
            stops.phrase()
        );

        conflicts
    }

    /// Fold another step's cost into this running total.
    pub fn absorb(&mut self, other: Self) {
        self.stops += other.stops;
        for (name, hunks) in other.files {
            self.add_file(PathBuf::from(name), hunks);
        }
    }

    /// Attribute `hunks` more conflict hunks to `name`, never fewer than one.
    ///
    /// Adding rather than replacing is the whole reason a file is keyed at all:
    /// the same file routinely conflicts at several stops of one replay, and
    /// each of those collisions is separate work for whoever resolves it.
    ///
    /// The floor lives here, at the single door into the breakdown, rather than
    /// in whichever caller remembered it. Being in this map at all means the
    /// file conflicted, and a file that conflicted is at least one decision, so
    /// the rule holds for every route in - the replay loop and the fixture
    /// constructor alike - and the invariant [`Conflicts::is_clean`] rests on
    /// is structural rather than incidental.
    ///
    /// Takes the name as a [`PathBuf`] - what git reported, unaltered - and
    /// stores it as the [`OsString`] the map is keyed on, which is the same
    /// bytes under a type whose ordering is git's own. Neither step interprets
    /// the name, so a path that is not valid UTF-8 keeps every byte of itself
    /// from the reader through to the breakdown.
    fn add_file(&mut self, name: PathBuf, hunks: usize) {
        *self.files.entry(name.into_os_string()).or_default() += hunks.max(1);
    }

    /// Whether the replay finished without a single conflict.
    ///
    /// Defined on the file set rather than on the counts, because the file set
    /// is the primary fact: a conflict is something that happened *to a file*,
    /// and the numbers are summaries of it. The three measures cannot disagree,
    /// and that holds by construction on every route in rather than on the
    /// replay path alone. [`Conflicts::add_file`] is the only door into the
    /// set and it floors each entry at one hunk, so hunks are non-zero exactly
    /// when the set is non-empty. Stops track the set for two different
    /// reasons: the replay only ever adds a file from inside a stop, and
    /// `from_files` refuses a stop count its own breakdown contradicts.
    ///
    /// Anchoring on the set keeps that true by construction instead of by
    /// coincidence: a future measure that can legitimately be zero cannot make
    /// a conflicted replay report itself clean.
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
    ///
    /// Each count comes out as the same [`Hunks`] the headline
    /// [`Conflicts::hunks`] returns, so a renderer never throws the type away
    /// and immediately rebuilds it to say the word "hunk" - and cannot pair a
    /// bare number with the wrong noun if it forgets which of the three counts
    /// it is holding.
    ///
    /// Each name comes out as a [`Path`] rather than as a `&str`, because that
    /// is what it is: git reported it as bytes and it was never decoded, so on
    /// unix it may be a name no `str` can hold. A caller printing one converts
    /// it lossily at that point and no earlier - a U+FFFD on the screen is a
    /// legible answer, while a U+FFFD in the map is a name that opens no file.
    pub fn file_hunks(&self) -> impl Iterator<Item = (&Path, Hunks)> {
        self.files
            .iter()
            .map(|(name, hunks)| (Path::new(name), Hunks::new(*hunks)))
    }
}

/// Why a replay is sitting in a halted rebase.
enum Halt {
    /// Paths git could not merge; a human would hand-merge these.
    Conflict(Vec<PathBuf>),
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
    let conflicted = git.nul_separated_paths("diff", &["--name-only", "--diff-filter=U"])?;
    if !conflicted.is_empty() {
        return Ok(Halt::Conflict(conflicted));
    }

    // Without REBASE_HEAD the loop cannot even name the commit it is about to
    // drop, so it has no business dropping it.
    let Ok(stopped) = git.run("log", &["-1", "--format=%h %s", "REBASE_HEAD"]) else {
        return Ok(Halt::UnwritableCommit {
            stopped: "a commit git would not name".to_owned(),
            evidence: "REBASE_HEAD does not resolve, so the replay cannot say which commit the \
                       rebase halted on"
                .to_owned(),
        });
    };

    // Content left behind is content that failed to be committed: a commit that
    // truly became empty leaves the index matching HEAD and the worktree
    // matching the index. Asked for the paths themselves, not as a `--quiet`
    // exit code, so git failing to answer is an error rather than a vote for
    // "empty".
    let mut uncommitted = git.paths("diff", &["--cached", "--name-only", "HEAD"])?;
    uncommitted.extend(git.paths("diff", &["--name-only"])?);
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
///
/// **The two answers meet here, not inside git.** `missing` is the intersection
/// of the paths the commit touched with the paths that differ between
/// `REBASE_HEAD` and `HEAD`, and each side of it is the bytes git printed. The
/// alternative is a round trip - every touched path handed back to the second
/// invocation as a pathspec - and it fails in two ways this one cannot. It puts
/// every path of the commit on one argv, and an argv has a length limit: past
/// the system's `ARG_MAX` the spawn fails with `E2BIG`, so one commit that
/// touches enough paths takes the whole simulation down with it. A vendored
/// dependency drop, a formatting sweep and a generated-code refresh all reach
/// that size. The failure is loud, so no work is lost by it, but a repository
/// holding one such commit and one halt with nothing unmerged cannot be
/// measured at all. The second way is quieter: a path on the way out is a path,
/// and a path on the way back in is a pathspec, where a leading `:` is magic and
/// `*` is a wildcard. An intersection is bounded by memory rather than by argv,
/// and it reads a name as a name and nothing else.
///
/// **Both invocations ask for `--ignore-submodules=none`, and that flag is the
/// question rather than decoration.** `git diff` is porcelain, so it reads
/// `diff.ignoreSubmodules` out of the developer's own `~/.gitconfig`;
/// `diff-tree` is plumbing, so it reads only the flag - git documents the
/// setting as reaching the porcelain alone, and git 2.55 was watched to agree.
/// Left to the config, one tree therefore gets read under two sets of rules. A
/// commit that moves a submodule pointer and touches nothing else is a path to
/// `diff-tree` and nothing at all to `git diff` under
/// `diff.ignoreSubmodules=all`: the touched set holds the submodule, so the
/// guard below stays quiet, and the differing set has nothing to intersect with
/// it. The commit reads as empty, `rebase --skip` throws the pointer away, and
/// a branch nobody replayed is reported cheap - the one answer this crate exists
/// never to give. The flag goes on both invocations rather than on the porcelain
/// alone, because the rule is that the two probes read one tree under one set of
/// rules. Which of them consults a config key is a fact about this version of
/// git, and this crate takes the rule. Pinned by
/// `refuses_to_report_a_cost_when_a_clean_pick_of_a_submodule_pointer_could_not_be_committed`
/// in `tests/halts.rs`.
///
/// **`--root` on the `diff-tree` invocation is the same kind of flag, for the
/// commit that has no parent.** `diff-tree` compares a commit against its parent,
/// so it prints no path at all for a root commit until it is asked to compare
/// that commit against nothing. Without the flag the touched set of a root commit
/// comes back empty, the guard below states the answer an empty set states, and
/// `rebase --skip` drops the first commit of a whole history. A root commit
/// arrives here in ordinary use: replaying a branch onto one that shares no
/// history with it replays every commit of that branch, its root commit included.
/// The refusal above is not the guard for this one - a root commit has no parent,
/// so the count is zero and the refusal passes it through, correctly. Pinned by
/// `refuses_to_report_a_cost_when_a_clean_pick_of_a_root_commit_could_not_be_committed`
/// in `tests/halts.rs`.
fn stopped_commit_is_already_in_head(git: &Git, stopped: String) -> Result<Halt> {
    // Read before anything else, because a merge commit makes every answer
    // below meaningless. `diff-tree` prints no path at all for a merge unless it
    // is asked for `-c`, `--cc` or `-m`, and neither invocation here asks, so
    // the touched set comes back empty and the guard under it reads the halt as
    // a commit that changes nothing. `rebase --skip` then drops a whole side of
    // history and the replay reports a cost for a branch it never replayed.
    //
    // `rebase.rebaseMerges=false` in `Git::safety_config` closes the route a
    // developer's own configuration opens. This refusal is the structural half:
    // it reads the shape of the commit rather than a setting, so the
    // classification stays correct whatever a later setting does. A refusal is
    // the loud direction, which is the only direction this crate allows.
    let parents = stopped_commit_parent_count(git)?;
    anyhow::ensure!(
        parents < 2,
        "the rebase halted on {stopped}, a merge commit with {parents} parents. A merge commit at \
         a halt is not something the replay can measure: git reports no changed path for one \
         unless it is asked for a merge diff, so the probe that decides whether a halted commit \
         adds anything to the new base cannot answer about it."
    );

    let touched = git.paths(
        "diff-tree",
        &[
            "--no-commit-id",
            "--name-only",
            "-r",
            "--root",
            "--ignore-submodules=none",
            "REBASE_HEAD",
        ],
    )?;

    // A commit that changes no path at all cannot lose anything, and saying so
    // here spares the second invocation. The intersection below reaches the same
    // answer on its own - nothing intersected with anything is nothing - so this
    // is the answer stated rather than an answer arrived at by set algebra.
    //
    // The statement is about a commit with one parent, which is the only kind
    // that reaches this line: the refusal above has already taken the merge, and
    // for a merge an empty list means git was not asked for a merge diff rather
    // than a commit that changes nothing.
    if touched.is_empty() {
        return Ok(Halt::EmptyCommit { stopped });
    }

    // Every path that differs between the stopped commit and the new base, with
    // no pathspec narrowing it. Most of them belong to other commits; the
    // intersection below takes only the ones this commit touched.
    let differing: HashSet<String> = git
        .paths(
            "diff",
            &[
                "--name-only",
                "--ignore-submodules=none",
                "REBASE_HEAD",
                "HEAD",
            ],
        )?
        .into_iter()
        .collect();

    let missing: Vec<&str> = touched
        .iter()
        .filter(|path| differing.contains(path.as_str()))
        .map(String::as_str)
        .collect();

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

/// How many parents the commit the rebase halted on has.
///
/// **`rev-list --parents` rather than `rev-parse REBASE_HEAD^@`, and the reason
/// is the empty answer.** Both list the parents, and both list none for a root
/// commit, so both answer the question. They part company on what an empty
/// answer means: `rev-parse ^@` prints nothing for a root commit, which is the
/// same nothing a reader that lost its answer would hand back, so a count taken
/// from it reads every such loss as a root commit and lets it through.
/// `rev-list --parents` prints the commit's own id first and its parents after
/// it, so a resolvable commit can never answer with nothing at all, and an empty
/// answer is refused below instead of being counted as no parents. That refusal
/// is the direction this crate takes everywhere: a dry run may say "I cannot
/// answer", never "cheap" because a probe came back blank.
///
/// `git rev-list --no-walk --count --parents` is not a third option. `--count`
/// prints how many commits were listed, which `--no-walk` has already fixed at
/// one, so it answers `1` for a root commit and `1` for a merge alike - watched
/// on git 2.55.
///
/// `--no-walk` keeps the answer to the one commit asked about, rather than to
/// the history behind it.
///
/// # Errors
///
/// Returns an error if git could not be spawned, if `REBASE_HEAD` does not
/// resolve, or if git listed no id at all for it.
fn stopped_commit_parent_count(git: &Git) -> Result<usize> {
    let listed = git.run("rev-list", &["--no-walk", "--parents", "REBASE_HEAD"])?;
    let mut fields = listed.split_whitespace();

    anyhow::ensure!(
        fields.next().is_some(),
        "git listed no id at all for REBASE_HEAD, so the replay cannot count the parents of the \
         commit the rebase halted on"
    );

    Ok(fields.count())
}

/// Whether git is sitting in a halted rebase.
///
/// The state directory is read through [`Git::path`], not through `Git::run`,
/// because the answer is a path and this asks the filesystem about it. In a
/// linked worktree git builds that answer out of the *developer's* own
/// repository path, so bytes nobody here chose sit in the middle of it. A byte
/// outside UTF-8 among them comes back from `run` as U+FFFD, since that reader
/// decodes lossily, and the result names a directory nothing holds. `exists()`
/// is then false, the loop reports no rebase, and the caller says "the rebase
/// failed without leaving a rebase to resolve" - which names the wrong cause
/// for a real halt.
///
/// `run` trims as well, and that half cannot reach *this* answer: `--git-path`
/// glues the state directory name onto the end, so the repository's own last
/// character never lands at either end of what git prints. It reaches any
/// answer that does end there, `rev-parse --show-toplevel` among them. Both
/// losses live in the one reader, so the call site takes the right reader
/// rather than reasoning about which loss its own question is open to.
fn rebase_in_progress(git: &Git, worktree: &Path) -> Result<bool> {
    for state_dir in ["rebase-merge", "rebase-apply"] {
        let path = git.path("rev-parse", &["--git-path", state_dir])?;
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
///
/// [`Conflicts::add_file`] floors its entries at one too, which makes this
/// floor redundant for the total but not for the measurement, so it stays. The
/// two encode different facts: `add_file` says that a file in a breakdown
/// conflicted, while this says what a marker-less conflict actually costs the
/// person resolving it. Dropping it here would leave this function returning a
/// zero that is simply wrong about the file, rescued downstream by a rule that
/// knows nothing about binary blobs.
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

    use super::{stopped_commit_is_already_in_head, Conflicts, NonZeroUsize, Path, PathBuf};
    use crate::git::Git;
    use crate::metrics::{Hunks, Stops};
    use crate::testing::{contested_region_repo, TestRepo};

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
        let scratch = repo.scratch("main");
        scratch
            .git()
            .run("checkout", &["-q", "--detach", "iterated"])
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

    /// A file in the breakdown is a file that conflicted, so it costs at least
    /// one hunk - and that floor has to sit where every path into the breakdown
    /// crosses it.
    ///
    /// [`Conflicts::add_file`] is that place, which is why the test goes
    /// through it rather than through a constructor. The public constructor
    /// takes a [`NonZeroUsize`] per file, so a zero-hunk file cannot even be
    /// spelled there; the replay path, by contrast, still hands in a count
    /// measured at runtime, and this is the rule that catches one that came
    /// back zero. Without the floor the accessors contradict each other:
    /// [`Conflicts::is_clean`] says something conflicted while
    /// [`Conflicts::hunks`] says nothing did, and a report built from it reads
    /// "0 hunks across 1 file" with a "0 hunks" row underneath.
    #[test]
    fn a_conflicted_file_that_measured_no_hunks_still_costs_one() {
        let mut conflicts = Conflicts::default();
        conflicts.add_file(PathBuf::from("src/lib.rs"), 0);

        assert!(
            !conflicts.is_clean(),
            "a name in the breakdown is a file that conflicted"
        );
        assert_eq!(
            conflicts.hunks(),
            Hunks::new(1),
            "a conflicted file is at least one decision for whoever resolves \
             it, so it can never contribute zero to the total its breakdown is \
             supposed to explain"
        );
    }

    /// A hand-built result has to agree with itself about whether anything
    /// conflicted.
    ///
    /// Stops with no files is the quiet half of that: `is_clean` reads the file
    /// set, so an empty set carrying a stop count renders the clean line and
    /// the stops vanish without a word. A fixture that can do that is a fixture
    /// that can make a broken renderer look correct, which is the one thing
    /// this constructor exists not to do.
    #[test]
    #[should_panic(expected = "has to agree with itself")]
    fn a_hand_built_result_cannot_claim_stops_it_has_no_conflicted_files_for() {
        let _ = Conflicts::from_files(std::iter::empty::<(PathBuf, NonZeroUsize)>(), Stops::new(7));
    }

    /// The other direction of the same disagreement. A file only ever enters
    /// the breakdown from inside a stop, so a breakdown with no stops describes
    /// a replay that never halted and still found work - rendered as "1 hunk
    /// across 1 file, 0 stops".
    #[test]
    #[should_panic(expected = "has to agree with itself")]
    fn a_hand_built_result_cannot_claim_conflicted_files_it_has_no_stops_for() {
        let one = NonZeroUsize::new(1).expect("1 is not zero");

        let _ = Conflicts::from_files([(PathBuf::from("src/lib.rs"), one)], Stops::new(0));
    }

    /// The breakdown prints in git's order, which is the order of the bytes.
    ///
    /// The keys are names, and there are two orderings for a name. `OsString`
    /// compares bytes, which is what git sorts its own output by and what this
    /// crate has always printed. `Path` compares *components*, and the two
    /// disagree the moment a separator meets a byte below it: `.` is `0x2e` and
    /// `/` is `0x2f`, so a byte comparison puts `src.txt` before `src/lib.rs`
    /// while a component comparison puts the directory first.
    ///
    /// Keying the map on a `PathBuf` - the type every public method here speaks
    /// in - would therefore have quietly reordered every breakdown containing a
    /// pair like this one, in output nobody was asserting on. This is the guard
    /// that makes that change loud instead.
    #[test]
    fn the_breakdown_is_ordered_by_bytes_the_way_git_orders_its_own_output() {
        let one = NonZeroUsize::new(1).expect("1 is not zero");
        let conflicts = Conflicts::from_files(
            [
                (PathBuf::from("src/lib.rs"), one),
                (PathBuf::from("src.txt"), one),
            ],
            Stops::new(1),
        );

        assert_eq!(
            conflicts
                .file_hunks()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            vec![Path::new("src.txt"), Path::new("src/lib.rs")],
            "'.' sorts before '/', so a byte ordering names src.txt first; a \
             component ordering would name the directory first"
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

    /// The paths a stopped commit touched, asked for the way
    /// [`stopped_commit_is_already_in_head`] asks for them. Spelled here so the
    /// control below demonstrates the hazard with the invocation the probe
    /// really makes, rather than with a plausible-looking neighbour of it.
    const TOUCHED_PATHS: [&str; 6] = [
        "--no-commit-id",
        "--name-only",
        "-r",
        "--root",
        "--ignore-submodules=none",
        "REBASE_HEAD",
    ];

    /// A merge commit at a halt has to stop the replay, not read as a commit
    /// that changes nothing.
    ///
    /// `diff-tree` prints no path at all for a merge commit unless it is asked
    /// for `-c`, `--cc` or `-m`, and this probe asks for none of them. So an
    /// unguarded probe finds an empty touched set, calls the halt
    /// `Halt::EmptyCommit`, and the replay drops the commit with
    /// `rebase --skip`. That throws away a whole side of history and reports a
    /// cost for a branch that was never replayed, which is the one answer this
    /// crate exists never to give.
    ///
    /// The refusal is structural rather than configured, and both halves are
    /// wanted. `rebase.rebaseMerges=false` in `Git::safety_config` keeps the
    /// replay away from a merge on the todo list at all, which is the route a
    /// developer's own configuration opens today. This refusal counts
    /// `REBASE_HEAD`'s parents before it asks anything else, so the
    /// classification is correct whatever a later setting does.
    ///
    /// Two controls stand ahead of the assertion, because an assertion that
    /// something did not happen passes just as readily when it was never
    /// possible. The first proves the fixture really halts on a merge - two
    /// parents, read back through plain git. The second proves `diff-tree`
    /// really is silent about that merge, which is the hazard itself. A third
    /// control follows the assertion: the same probe, pointed at a
    /// single-parent commit, has to answer rather than refuse, or a probe that
    /// refused every stopped commit would pass the assertion above and stop
    /// every replay.
    #[test]
    fn refuses_a_merge_commit_at_a_halt_rather_than_reading_it_as_a_commit_that_changes_nothing() {
        let repo = TestRepo::init();
        repo.commit_file("base.txt", "base\n", "base");
        repo.branch("side");
        repo.commit_file("side.txt", "the other side's work\n", "side work");
        repo.checkout("main");
        repo.commit_file("main.txt", "main's work\n", "main work");
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
        // A halted rebase names the commit it stopped on with this ref, so the
        // probe reads the fixture exactly as it reads a real halt.
        repo.git(&["update-ref", "REBASE_HEAD", "HEAD"]);

        let git = Git::new(repo.path(), "");

        let parents = repo.git(&["rev-list", "--no-walk", "--parents", "REBASE_HEAD"]);
        assert_eq!(
            parents.split_whitespace().count(),
            3,
            "the fixture has to stop on a merge commit - its own id and two parents - or there is \
             nothing here to refuse: {parents}"
        );

        assert!(
            git.paths("diff-tree", &TOUCHED_PATHS)
                .expect("ask which paths the stopped commit touched")
                .is_empty(),
            "`diff-tree` no longer stays silent about a merge commit, so this test could only \
             pass vacuously; that silence is what makes an unguarded probe read a merge as a \
             commit that changes nothing"
        );

        let stopped = repo.git(&["log", "-1", "--format=%h %s", "REBASE_HEAD"]);
        let error = stopped_commit_is_already_in_head(&git, stopped.clone())
            .map(|_| ())
            .expect_err(
                "a merge commit at a halt has to stop the replay. `diff-tree` reports no changed \
                 path for one, so classifying it hands back `EmptyCommit`, and the replay skips a \
                 whole side of history and reports a cost for a branch it never replayed",
            );

        assert!(
            format!("{error:#}").contains(&stopped),
            "the refusal has to name the commit the rebase stopped on, because that name is what \
             the developer looks at next: {error:#}"
        );
        assert!(
            format!("{error:#}").contains("merge commit"),
            "the refusal has to say a merge commit at a halt is not something the replay can \
             measure, or a reader repairs the wrong half: {error:#}"
        );

        repo.git(&["update-ref", "REBASE_HEAD", "side"]);
        assert!(
            stopped_commit_is_already_in_head(&git, "a single-parent commit".to_owned()).is_ok(),
            "the refusal has to be about a merge and nothing else; a probe that refuses every \
             stopped commit passes the assertion above and stops every replay"
        );
    }

    /// An upstream that starts with a dash is an upstream, and the rebase has
    /// to read it as one.
    ///
    /// `git rebase --root` is a complete and valid command: it replays the
    /// whole history onto nothing. So a replay handed `--root` as its upstream
    /// finished without a single conflict and reported a clean result for a
    /// revision that names no commit. Zero is also what a genuinely free replay
    /// reports, so nothing downstream can tell the two apart - and the tool
    /// that read this answer printed `clean` for a branch nobody has.
    ///
    /// [`Repo::resolve`](crate::Repo::resolve) refuses the same name earlier
    /// and every tool in this repository asks it first. This method is public
    /// and takes a revision of its own, so the refusal has to hold here too:
    /// `grist` reaches it directly, once per branch, with no second pre-flight
    /// between the two.
    ///
    /// The control replays a revision that does name a commit, on the same
    /// scratch worktree, because a replay that refused every upstream would
    /// pass the assertion above and answer nothing at all.
    #[test]
    fn refuses_an_upstream_that_starts_with_a_dash_rather_than_replaying_onto_the_root() {
        let repo = contested_region_repo();
        let scratch = repo.scratch("main");
        scratch
            .git()
            .run("checkout", &["-q", "--detach", "iterated"])
            .expect("check out the branch detached in the scratch worktree");

        let error = scratch
            .replay_rebase("--root")
            .map(|cost| format!("{cost:?}"))
            .expect_err(
                "an upstream that names no commit has to stop the replay. Git knows `--root` as \
                 an option of `rebase`, so reading it as one replays the whole history onto \
                 nothing, hits no conflict, and reports a cost of zero for a revision nobody has",
            );

        assert!(
            format!("{error:#}").contains("--root"),
            "the refusal has to name the upstream git would not use: {error:#}"
        );

        let control = scratch
            .replay_rebase("single")
            .expect("replay onto a revision the fixture really has");

        assert_eq!(
            control.stops(),
            Stops::new(CONTESTED_ROUNDS),
            "the fixture has to cost something, or the refusal above proves only that this \
             replay answers nothing at all"
        );
    }
}

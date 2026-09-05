//! The questions worth asking *before* a scratch worktree is ever built.
//!
//! Creating a [`Scratch`] is not free: a temporary directory, a real
//! `git worktree add`, and administrative state in the developer's repository
//! that has to be cleaned up afterwards. Paying all of that only to discover the
//! branch name was a typo is both slow and, worse, misleading — the failure
//! arrives looking like a failed simulation rather than a bad argument.
//!
//! [`Repo`] is the pre-flight: open the repository, resolve the revisions, see
//! whether the tree is dirty, and only then decide whether a replay is worth
//! starting. It exists here rather than in each consuming tool because the git
//! runner is deliberately crate-private — nothing outside this crate may build
//! one rooted at a real repository, and nothing outside is handed one either —
//! so the queries a caller legitimately needs have to be part of this crate's
//! public door.
//!
//! It is also the *only* door. [`Repo::scratch`] is how a [`Scratch`] is built,
//! because a pre-flight a caller can walk around is not a pre-flight — it is a
//! suggestion. Handing back the opened path for the caller to pass on itself
//! would be exactly that: the checked path and an unchecked one would be the
//! same `&Path`, indistinguishable at the call site, and every consumer would be
//! free to skip straight to the worktree. So the validated path never leaves
//! this type, and the worktree comes out of the thing that validated it.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::git::Git;
use crate::metrics::Uncommitted;
use crate::scratch::Scratch;

/// Where hook lookups are pointed while answering a pre-flight question.
///
/// [`Scratch`] creates a real, empty directory for this because
/// a replay runs commands - `rebase`, `commit`, `checkout` - that genuinely
/// fire hooks, and an empty `core.hooksPath` is not "hooks off": git joins the
/// configured directory onto the hook name, so an empty one resolves
/// `pre-commit` to `/pre-commit`, at the root of the file system.
/// [`Git::new`](crate::git::Git::new) refuses that value for exactly this
/// reason. Nothing here is such a command. Every query on a [`Repo`] is a read
/// (`rev-parse`, `status`), and reads fire no hook at all, so the redirect only
/// has to name somewhere that will never hold an executable.
///
/// That distinction is worth the const rather than a `TempDir`, because it is
/// what makes the pre-flight unconditionally cheap: telling a developer they
/// typo'd a branch name must not be able to fail for want of a writable
/// temporary directory, and must not leave a scratch worktree behind on the way
/// to saying so. A relative path this crate never creates keeps the redirect
/// pointed somewhere harmless without touching the filesystem at all.
///
/// Crate-visible because the unit tests of [`Git`] and
/// [`Scratch`] ask the same question of it: each one reads
/// through git rather than writing, so each takes the path that reads fire no
/// hook from. It is the one hooks path a test can name without building a
/// directory for it.
pub(crate) const PREFLIGHT_HOOKS_PATH: &str = ".git/gitscratch-preflight-no-hooks";

/// The branches a replay measures against when the caller named none, in the
/// order they are tried.
///
/// Local names only, and both of them. `git rev-parse main` reads local refs,
/// so a repository whose default branch exists only as `origin/main` matches
/// neither candidate and gets the refusal below. That is the intended answer: a
/// search of the remote refs makes the rule harder to state, and it hides which
/// branch a run measured behind a name the developer never typed.
///
/// Public because the refusal names every candidate it tried, and a test that
/// asserts on those names has to read them from here. Two copies of the list
/// could agree today and disagree the day a third candidate is added.
pub const DEFAULT_BRANCHES: [&str; 2] = ["main", "master"];

/// A git repository, opened for the read-only questions that precede a replay.
#[derive(Debug)]
pub struct Repo {
    path: PathBuf,
}

impl Repo {
    /// Open the git repository containing `path`.
    ///
    /// `path` may be any directory inside the repository, not only its root,
    /// because the directory a tool is run in - which is the directory it hands
    /// here - is hardly ever the root. Everything the resulting [`Repo`] answers
    /// is about the repository rather than about that directory: a revision
    /// resolves the same, the uncommitted count covers work sitting anywhere in
    /// the tree, and a conflicted path is named from the repository root.
    /// `tests/repo.rs` and `grind`'s `tests/cli.rs` pin all three, since the
    /// validated path never leaves this type and nothing else about a
    /// subdirectory run is visible from outside.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned, or if `path` is not inside
    /// a git repository.
    pub fn open(path: &Path) -> Result<Self> {
        let repo = Self {
            path: path.to_path_buf(),
        };

        // Asking git where the repository is proves one exists, and proves it
        // from `path` itself - so a subdirectory of a repository opens fine,
        // while somewhere outside every repository fails now rather than
        // halfway through a simulation.
        //
        // Only the exit status is read here, and the answer is dropped. It goes
        // through `Git::path` all the same, because what git prints is a path,
        // and a path read back through `Git::run` is a bug this crate keeps no
        // examples of. A reader that copies this line gets the right one.
        repo.git()
            .path("rev-parse", &["--git-dir"])
            .with_context(|| format!("{} is not inside a git repository", path.display()))?;

        Ok(repo)
    }

    /// Build a scratch worktree of this repository, checked out at `at`.
    ///
    /// The only way to obtain a [`Scratch`] from outside this crate, and
    /// deliberately so: the path this type validated never leaves it, so the
    /// pre-flight cannot be walked around. See the module documentation.
    ///
    /// # Errors
    ///
    /// Returns an error if the temporary directory cannot be created, if its
    /// path cannot be spelled for git as UTF-8, or if git refuses to add the
    /// worktree — most commonly because `at` does not name a commit. "This is
    /// not a repository" is not among them: [`Repo::open`] has already settled
    /// that, which is the whole point of arriving here through it.
    pub fn scratch(&self, at: &str) -> Result<Scratch> {
        Scratch::create(&self.path, at)
    }

    /// Resolve a revision to a full commit id, without creating a scratch
    /// worktree.
    ///
    /// # Errors
    ///
    /// Returns an error if the revision does not name a commit; the message
    /// names the revision that could not be resolved.
    pub fn resolve(&self, revision: &str) -> Result<String> {
        self.git().rev_parse(revision)
    }

    /// The branch a replay measures against: `named` when the caller was given
    /// one, and otherwise the first of [`DEFAULT_BRANCHES`] this repository
    /// holds.
    ///
    /// The whole choice lives here rather than in each tool, because two
    /// implementations of "which branch did they mean" is two implementations
    /// that drift - and the one that drifts is the one measuring a different
    /// branch than the one it printed.
    ///
    /// A named branch is handed straight back without being resolved. Resolving
    /// it is the caller's, and the caller's error message is the one that names
    /// the typo the developer actually made. Answering a typo with this
    /// function's words would describe a default that was never reached.
    ///
    /// # Errors
    ///
    /// Returns an error if `named` is `None` and no candidate resolves; the
    /// message names every candidate that was tried.
    pub fn branch_or_default(&self, named: Option<&str>) -> Result<String> {
        if let Some(branch) = named {
            return Ok(branch.to_string());
        }

        for candidate in DEFAULT_BRANCHES {
            if self.resolve(candidate).is_ok() {
                return Ok(candidate.to_string());
            }
        }

        // Never a fall back to HEAD. A replay of HEAD onto HEAD answers "clean"
        // for every repository there is, so the fallback would turn "I could not
        // tell you which branch you meant" into a confident wrong answer.
        Err(anyhow!(
            "no branch was named, and no default branch resolves here (tried: {}) \
             - name the branch to measure against",
            DEFAULT_BRANCHES.join(", ")
        ))
    }

    /// How many files are uncommitted — staged, unstaged, or untracked.
    ///
    /// A replay only ever sees committed work, so this is how a caller can warn
    /// that the answer describes the tree as committed rather than as it sits
    /// on disk. `--untracked-files=all` is what makes the number honest: git
    /// otherwise collapses an untracked directory into a single entry, so a
    /// hundred new files would be reported as one.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned or reported a failure — a
    /// bare repository being the ordinary way to reach the latter, since there
    /// is no working tree to take a status of. A caller wanting the count as a
    /// *caveat* should treat that as no caveat rather than as fatal
    /// (`unwrap_or_default`, which [`Uncommitted`] derives for the purpose):
    /// a replay needs no working tree, so a repository that cannot answer this
    /// question can still answer the expensive one.
    pub fn uncommitted_files(&self) -> Result<Uncommitted> {
        let records = self
            .git()
            .nul_separated("status", &["--porcelain", "--untracked-files=all"])?;

        // Not `records.len()`: a rename and a copy each spend two fields on one
        // file. See `moved_from_elsewhere`.
        let mut fields = records.iter();
        let mut count = 0;
        while let Some(record) = fields.next() {
            count += 1;
            if moved_from_elsewhere(record) {
                fields.next();
            }
        }

        Ok(Uncommitted::new(count))
    }

    /// A runner rooted in the real repository, carrying the crate's safety
    /// configuration like every other git call made from here.
    fn git(&self) -> Git {
        Git::new(&self.path, PREFLIGHT_HOOKS_PATH)
    }
}

/// Whether a porcelain record is a rename or a copy, and so spends a *second*
/// NUL-separated field naming where the content came from.
///
/// The one place the two porcelain formats disagree about shape. Without `-z`
/// git writes a rename as a single `R  old -> new`, and counting records is
/// counting lines; with `-z` it writes `R  new`, NUL, `old`, because a path
/// containing ` -> ` would otherwise be unparseable. Counting fields would call
/// that two uncommitted files. It is one file, moved.
///
/// The status is the first two bytes of the record - one for the index, one for
/// the working tree - and both of them carry the letter. All four spellings are
/// reachable, and `tests/repo.rs` holds one test for each pair, because an arm
/// nothing can fail is an arm nobody can trust:
///
/// - `R` in the index column, which is what `git mv` stages.
/// - `C` in the index column, which git writes for a copy it detects beside the
///   modification of the source. Copy detection is off unless the developer
///   turns it on with `status.renames`, and this crate pins nothing about that
///   key, so the setting arrives out of the developer's own configuration.
/// - `R` and `C` in the working-tree column, which git writes where the
///   destination is in the index with no content behind it. `git add -N` records
///   exactly that, and so does `git add -p` for a new file, so the spelling
///   reaches a developer who never types `-N`.
///
/// Both bytes are ASCII by definition: git writes one of a fixed set of letters
/// and spaces there, and a space always separates them from the path, so the
/// path can never reach these two positions. Reading them as bytes is therefore
/// exact, and it is what lets the record stay the bytes git wrote - a path is a
/// byte string on unix, and a record decoded into a `str` on the way here would
/// have replaced every byte of one that is not valid UTF-8.
fn moved_from_elsewhere(record: &[u8]) -> bool {
    [record.first(), record.get(1)]
        .into_iter()
        .flatten()
        .any(|status| *status == b'R' || *status == b'C')
}

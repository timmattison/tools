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

use anyhow::{Context, Result};

use crate::git::Git;
use crate::metrics::Uncommitted;
use crate::scratch::Scratch;

/// Where hook lookups are pointed while answering a pre-flight question.
///
/// [`Scratch`](crate::Scratch) creates a real, empty directory for this because
/// a replay runs commands - `rebase`, `commit`, `checkout` - that genuinely
/// fire hooks, and an empty `core.hooksPath` is not "hooks off": git still
/// resolves hook lookups against it. Nothing here is such a command. Every
/// query on a [`Repo`] is a read (`rev-parse`, `status`), and reads fire no
/// hook at all, so the redirect only has to name somewhere that will never hold
/// an executable.
///
/// That distinction is worth the const rather than a `TempDir`, because it is
/// what makes the pre-flight unconditionally cheap: telling a developer they
/// typo'd a branch name must not be able to fail for want of a writable
/// temporary directory, and must not leave a scratch worktree behind on the way
/// to saying so. A relative path this crate never creates keeps the redirect
/// pointed somewhere harmless without touching the filesystem at all.
const PREFLIGHT_HOOKS_PATH: &str = ".git/gitscratch-preflight-no-hooks";

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

        // Not `records.len()`: a rename spends two fields on one file. See
        // `moved_from_elsewhere`.
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
/// the working tree - and either may carry the letter. Both are ASCII by
/// definition: git writes one of a fixed set of letters and spaces there, and a
/// space always separates them from the path, so the path can never reach these
/// two positions. Reading them as bytes is therefore exact, and it is what lets
/// the record stay the bytes git wrote - a path is a byte string on unix, and a
/// record decoded into a `str` on the way here would have replaced every byte of
/// one that is not valid UTF-8.
fn moved_from_elsewhere(record: &[u8]) -> bool {
    // MUTATION, deliberate, and the next commit takes it back out: the copy
    // letter is gone from the set, and only the index column is read. Both are
    // what the two new tests in `tests/repo.rs` are here to catch.
    [record.first()]
        .into_iter()
        .flatten()
        .any(|status| *status == b'R')
}

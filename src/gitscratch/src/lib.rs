//! The hardened harness for dry-running a git operation without touching
//! anything real.
//!
//! Answering "would this rebase conflict, and how badly?" means actually
//! performing it, and performing it means running git against the developer's
//! own repository. That is only safe because of a specific set of pinned
//! settings — `rebase.updateRefs=false` so the replay does not rewrite the very
//! branches being simulated, `rerere.enabled=false` so a simulated resolution
//! never poisons the shared rr-cache, `core.hooksPath` redirected at an empty
//! directory so no hook fires, `GIT_EDITOR=true` so a halted rebase cannot hang
//! on an editor, and more besides.
//!
//! One of those guards is about the environment rather than the configuration,
//! and it is the one that decides *which* repository all the others protect:
//! git obeys `GIT_DIR` and its relatives before it obeys the directory it was
//! pointed at, and it exports them into every hook it runs. Run from inside one
//! — a pre-push gate, `git bisect run`, `rebase --exec`, `cargo test` from
//! `.husky/pre-commit` — an unscrubbed tool would aim the whole simulation at
//! the hook's repository. [`NoInheritedRepository`] takes them back off, at the
//! single place a git process is created and at every fixture spawn besides.
//! A hook exports who is committing as well as where, and those variables
//! outrank every config source, so [`NoInheritedIdentity`] takes that second set
//! off at the same two places — otherwise the identity pinned below loses to
//! whichever tool is driving the run.
//!
//! Every one of those guarantees is a guard that a second implementation would
//! silently be missing. So this crate owns them, and it owns them behind a
//! narrow door: a scratch worktree can only be built through [`Scratch`], a
//! [`Scratch`] can only be built through [`Repo::scratch`], and a [`Scratch`]
//! only hands out a [`Git`] that already carries the whole safety
//! configuration. There is no way to get a worktree from here without also
//! getting the hardening, which is the point — the tools built on top
//! (`grist`, and the `grime`/`grind` dry-run reporters) cannot drift apart on
//! safety, because there is only one implementation of it.
//!
//! The result of a replay is a [`Conflicts`]: how many times the operation
//! halted, how many hunks a human would hand-merge, and which files were
//! involved. Turning that into the words a developer reads is [`Report`]'s job,
//! and it lives here for the same reason the hardening does — `grime` and
//! `grind` answer different questions and must print the same shape, which two
//! renderers could not stay agreed on.
//!
//! Not every question needs a worktree, though. [`Repo`] answers the cheap ones
//! a caller should ask first — does this directory contain a repository, does
//! that revision resolve, is the tree dirty — so a typo'd branch name fails in
//! milliseconds with a clear message instead of masquerading as a failed
//! simulation. Those queries need the same crate-private [`Git`], which is why
//! they live here too.
//!
//! And "should ask first" is not left to a caller's discretion, because that is
//! how a pre-flight becomes decorative. [`Repo`] is where the door is: opening
//! the repository is the *only* way to reach a [`Scratch`], and [`Repo`] never
//! hands its path back, so there is no unchecked route to a worktree to be
//! tempted by. Skipping the cheap question is not a shortcut a consumer can take
//! — `grist` did take it, before the path stopped being reachable, and paid for
//! it by reporting "not a git repository (or any of the parent directories):
//! .git" from inside `worktree add` after announcing a run it could not start.
//!
//! Fixtures for building throwaway repositories with known conflict shapes live
//! in `testing`, behind the `testing` feature, so every consumer's test suite
//! shares one copy instead of each compiling its own.

pub mod git;
pub mod metrics;
pub mod repo;
pub mod report;
pub mod scratch;
#[cfg(feature = "testing")]
pub mod testing;

pub use git::{
    shed_inherited_git_environment, Git, GitOutput, NoInheritedIdentity, NoInheritedRepository,
    INHERITED_IDENTITY_VARS, REPOSITORY_LOCATION_VARS,
};
pub use metrics::{BranchName, Files, Hunks, Stops, Uncommitted};
pub use repo::Repo;
pub use report::Report;
pub use scratch::{Conflicts, Scratch};

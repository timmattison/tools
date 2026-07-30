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
//! Every one of those guarantees is a guard that a second implementation would
//! silently be missing. So this crate owns them, and it owns them behind a
//! narrow door: a scratch worktree can only be built through [`Scratch`], and a
//! [`Scratch`] only hands out a [`Git`] that already carries the whole safety
//! configuration. There is no way to get a worktree from here without also
//! getting the hardening, which is the point — the tools built on top
//! (`grist`, and the `grime`/`grind` dry-run reporters) cannot drift apart on
//! safety, because there is only one implementation of it.
//!
//! The result of a replay is a [`Conflicts`]: how many times the operation
//! halted, how many hunks a human would hand-merge, and which files were
//! involved.
//!
//! Not every question needs a worktree, though. [`Repo`] answers the cheap ones
//! a caller should ask first — does this directory contain a repository, does
//! that revision resolve, is the tree dirty — so a typo'd branch name fails in
//! milliseconds with a clear message instead of masquerading as a failed
//! simulation. Those queries need the same crate-private [`Git`], which is why
//! they live here too.
//!
//! Fixtures for building throwaway repositories with known conflict shapes live
//! in `testing`, behind the `testing` feature, so every consumer's test suite
//! shares one copy instead of each compiling its own.

pub mod git;
pub mod metrics;
pub mod repo;
pub mod scratch;
#[cfg(feature = "testing")]
pub mod testing;

pub use git::{Git, GitOutput};
pub use metrics::{BranchName, Files, Hunks, Stops};
pub use repo::Repo;
pub use scratch::{Conflicts, Scratch};

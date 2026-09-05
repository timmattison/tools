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
//! the hook's repository. [`NoInheritedGitEnvironment`] takes them back off, at
//! the single place a git process is created and at every fixture spawn
//! besides. A hook exports who is committing as well as where, and those
//! variables outrank every config source, so the same sweep takes that second
//! set off at the same two places — otherwise the identity pinned below loses
//! to whichever tool is driving the run. The rule is the `GIT_` prefix and
//! never a list of names, because a list strips nothing new the day git adds a
//! variable and reports the same clean answer either way.
//!
//! Every one of those guarantees is a guard that a second implementation would
//! silently be missing. So this crate owns them, and it owns them behind a
//! narrow door: a scratch worktree can only be built through [`Scratch`], a
//! [`Scratch`] can only be built through [`Repo::scratch`], and a [`Scratch`]
//! answers only the operations it names. There is no way to get a worktree from
//! here without also getting the hardening, which is the point — the tools built
//! on top (`grist`, and the `grime`/`grind` dry-run reporters) cannot drift
//! apart on safety, because there is only one implementation of it.
//!
//! Two operations are replayed behind that door, and they answer the two
//! questions a developer asks before doing the work. [`Scratch::replay_rebase`]
//! walks a whole rebase, halt by halt, and folds a cost per stop.
//! [`Scratch::replay_merge`] makes one three-way merge and reads what git could
//! not merge, so it counts one stop or none. Both answer with the same
//! [`Conflicts`], and both refuse rather than answer when git would not perform
//! the operation at all - "refusing to merge unrelated histories" is neither a
//! clean verdict nor a conflicted one.
//!
//! The git runner is no part of that door. It is crate-private, and both halves
//! of that are needed: nothing outside this crate can *build* a runner, because
//! `Git::new` is crate-private, and nothing outside is *handed* one,
//! because [`Scratch`] answers with the operation rather than with the thing
//! that performs it. Only the second half is about a consumer's reach, and the
//! crate went a long time with the first half alone — a scratch worktree is a
//! linked worktree of the developer's real repository, so a runner in a
//! consumer's hands answers `branch -D`, `update-ref`, `config --local` and
//! `push` against that repository. The configuration pinned above says nothing
//! about any of them. See [`Scratch`] for the operations that took the runner's
//! place.
//!
//! The result of a replay is a [`Conflicts`]: how many times the operation
//! halted, how many hunks a human would hand-merge, and which files were
//! involved. Turning that into the words a developer reads is [`Report`]'s job,
//! and it lives here for the same reason the hardening does — `grime` and
//! `grind` answer different questions and must print the same shape, which two
//! renderers could not stay agreed on.
//!
//! The program around that print lives here too, on the third reading of the
//! same argument. [`Console`] is everything such a tool says, and
//! [`Console::answer`] is the whole of its `main`: it maps a [`Conflicts`] onto
//! the three exit codes those tools publish, sends each kind of sentence to the
//! stream it belongs on, silences all of them on `-q`, and writes every one of
//! them the one way that cannot panic when the reader closes the pipe first.
//! Each of those is a guarantee a README states, so two copies of them is one
//! fix that reaches one binary — and the two copies had already begun to drift
//! apart. What a tool keeps is the question: its arguments, the sentence naming
//! what it did, which replay answers it, and the width of its own terminal,
//! which stays with the tool because measuring one is a decision about a single
//! program's output.
//!
//! Not every question needs a worktree, though. [`Repo`] answers the cheap ones
//! a caller should ask first — does this directory contain a repository, does
//! that revision resolve, is the tree dirty — so a typo'd branch name fails in
//! milliseconds with a clear message instead of masquerading as a failed
//! simulation. Those queries need the same crate-private runner, which is why
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

pub mod console;

/// The git runner, and the two environment guards that are safe to share.
///
/// The module is private because almost all of it is the runner, and the runner
/// is not a consumer's to hold - see [`Scratch`]. The two items a consumer does
/// need are re-exported at the crate root below, so the only public paths into
/// this module are the two that can do no harm.
mod git;
pub mod metrics;
pub mod repo;
pub mod report;
pub mod scratch;
#[cfg(feature = "testing")]
pub mod testing;

/// The environment guards, which every consumer that spawns git of its own
/// needs. Both of them only *remove* variables, so neither one can point a
/// command at a repository or give it a command to run.
pub use git::{shed_inherited_git_environment, NoInheritedGitEnvironment};

/// The runner itself, and only for the test scaffolding.
///
/// [`Scratch::testing_git`] hands a runner to a test suite, so the type it
/// hands back needs a name there. The `testing` feature is how this crate marks
/// everything that exists for a test target and for nothing else, and a
/// consumer that turns it on gains no way to *build* a runner - `Git::new` is
/// crate-private in every build.
#[cfg(feature = "testing")]
pub use git::Git;

pub use console::Console;
pub use metrics::{BranchName, Files, Hunks, Stops, Uncommitted};
pub use repo::{Repo, DEFAULT_BRANCHES};
pub use report::{Report, UnwordedReport};
pub use scratch::{Conflicts, Scratch};

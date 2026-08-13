//! Pushing the current branch from watch mode: what a push will do, how it is
//! described to the user before it runs, and how it is run.
//!
//! `gix` cannot push, so the push itself is a `git` child process. Everything
//! that *decides* — which remote, which arguments, what the confirmation says,
//! and what an outcome means — lives here as pure, terminal-free code so it can
//! be tested without a network or a pty. Only [`spawn`] touches a process.

use crate::render::UpstreamStatus;
use crate::repo::DETACHED_HEAD;

/// What pressing `p` will actually do, resolved from the snapshot *before* the
/// confirmation appears — so the prompt describes the command that will run
/// rather than a guess the user has to check afterwards.
///
/// The three variants are the three answers the user needs: nothing will
/// happen, an existing remote branch moves, or a branch appears on the remote
/// that was not there before. That last one is the reason this is an enum
/// rather than a struct with a `create: bool`: creating a remote branch is a
/// different act from updating one, and it gets different wording and a
/// different command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PushPlan {
    /// The upstream already carries every local commit, so a push would send
    /// nothing. Kept as a plan rather than folded into `None` so the caller can
    /// say *why* it is not prompting.
    UpToDate {
        /// Short upstream name, e.g. `origin/gsw-push`.
        target: String,
    },
    /// The branch tracks a remote branch that exists and is behind HEAD. Runs a
    /// bare `git push`, which follows the configured upstream — so gsw never
    /// has to re-derive a remote and a refspec git already knows.
    Update {
        /// Short upstream name, e.g. `origin/gsw-push`.
        target: String,
        /// Commits HEAD has that the upstream does not.
        commits: u32,
    },
    /// The branch has no upstream, so the push creates the branch on the remote
    /// and records it as the upstream (`-u`). This is the case the confirmation
    /// must call out: it puts a branch on a shared remote that nobody has seen.
    Create {
        /// Remote to create the branch on.
        remote: String,
        /// Local branch name, which is also the remote branch name.
        branch: String,
    },
}

impl PushPlan {
    /// Resolve what a push would do, or `None` when there is nothing gsw can
    /// push *from* or *to*.
    ///
    /// `None` covers the two cases where a prompt would be a lie: a detached
    /// HEAD (no branch to push) and a repository with no usable remote. Both
    /// are reported to the user as a status line rather than a prompt.
    ///
    /// `upstream` is the snapshot's tracking status, which is `Some` only when
    /// the upstream is configured *and* its remote-tracking ref resolves. That
    /// is exactly the signal the wording needs: a branch whose remote ref is
    /// missing gets the create wording, because a push really will create it.
    ///
    /// `remote` is only consulted for [`PushPlan::Create`]. An [`PushPlan::Update`]
    /// runs a bare `git push` and lets git read the remote out of the branch
    /// config, so a branch tracking a remote other than the repository's default
    /// still pushes to the right place.
    pub(crate) fn resolve(
        branch: &str,
        remote: Option<&str>,
        upstream: Option<&UpstreamStatus>,
    ) -> Option<Self> {
        // Checked before the upstream, so a tracking status left over from
        // before the checkout cannot make a detached HEAD look pushable.
        if branch == DETACHED_HEAD {
            return None;
        }

        match upstream {
            // Level with the upstream, or behind it only: `git push` would
            // report "Everything up-to-date". Say so without the round trip.
            Some(up) if up.ahead == 0 => Some(Self::UpToDate {
                target: up.name.clone(),
            }),
            // Ahead — including ahead *and* behind. A diverged branch is very
            // likely rejected as a non-fast-forward, and that rejection is what
            // the user needs to read. gsw does not pre-empt git's decision.
            Some(up) => Some(Self::Update {
                target: up.name.clone(),
                commits: up.ahead,
            }),
            None => remote.map(|remote| Self::Create {
                remote: remote.to_string(),
                branch: branch.to_string(),
            }),
        }
    }

    /// The arguments to pass to `git`, not including the program name.
    ///
    /// # Panics
    ///
    /// Panics on [`PushPlan::UpToDate`], which describes a push that must never
    /// run. Callers prompt only for the other two variants.
    pub(crate) fn command_args(&self) -> Vec<String> {
        match self {
            // Bare `push`: git reads the remote and the refspec out of the
            // branch config, so a branch tracking something other than the
            // repository's default remote still goes to the right place.
            Self::Update { .. } => vec!["push".to_string()],
            Self::Create { remote, branch } => vec![
                "push".to_string(),
                "-u".to_string(),
                remote.clone(),
                branch.clone(),
            ],
            Self::UpToDate { target } => {
                unreachable!("gsw never runs a push for an up-to-date branch ({target})")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upstream(name: &str, ahead: u32, behind: u32) -> UpstreamStatus {
        UpstreamStatus {
            name: name.to_string(),
            ahead,
            behind,
        }
    }

    #[test]
    fn a_branch_with_no_upstream_creates_it_on_the_remote() {
        // The common case in this workflow: a fresh worktree branch that has
        // never been pushed. The push must create the remote branch and record
        // it as the upstream, so the header's tracking segment appears and the
        // next push is a plain update.
        let plan = PushPlan::resolve("gsw-push", Some("origin"), None)
            .expect("a branch plus a remote is enough to push");
        assert_eq!(
            plan,
            PushPlan::Create {
                remote: "origin".to_string(),
                branch: "gsw-push".to_string(),
            },
        );
        assert_eq!(plan.command_args(), ["push", "-u", "origin", "gsw-push"]);
    }

    #[test]
    fn a_tracked_branch_that_is_ahead_updates_the_remote_branch() {
        // The upstream exists, so git already knows the remote and the refspec.
        // A bare `git push` uses them, which keeps gsw from re-deriving a
        // refspec git would only override.
        let up = upstream("origin/gsw-push", 3, 0);
        let plan = PushPlan::resolve("gsw-push", Some("origin"), Some(&up))
            .expect("a tracked branch that is ahead has something to push");
        assert_eq!(
            plan,
            PushPlan::Update {
                target: "origin/gsw-push".to_string(),
                commits: 3,
            },
        );
        assert_eq!(plan.command_args(), ["push"]);
    }

    #[test]
    fn a_tracked_branch_that_is_level_has_nothing_to_push() {
        // Level with the upstream: a push would send nothing, so the plan says
        // so and the caller shows a status line instead of a prompt.
        let up = upstream("origin/gsw-push", 0, 0);
        let plan = PushPlan::resolve("gsw-push", Some("origin"), Some(&up))
            .expect("a level branch still resolves, so the caller can say why");
        assert_eq!(
            plan,
            PushPlan::UpToDate {
                target: "origin/gsw-push".to_string(),
            },
        );
    }

    #[test]
    fn a_tracked_branch_that_is_only_behind_has_nothing_to_push() {
        // Behind but not ahead: `git push` would report "Everything up-to-date".
        // Prompting for that wastes a network round trip and reads as a failure
        // when nothing failed.
        let up = upstream("origin/gsw-push", 0, 7);
        let plan =
            PushPlan::resolve("gsw-push", Some("origin"), Some(&up)).expect("still resolves");
        assert_eq!(
            plan,
            PushPlan::UpToDate {
                target: "origin/gsw-push".to_string(),
            },
        );
    }

    #[test]
    fn a_tracked_branch_that_is_ahead_and_behind_still_pushes() {
        // Diverged. The push will very likely be rejected as a non-fast-forward,
        // and that rejection is exactly what the user needs to see — gsw must
        // not pre-empt git's decision by refusing to try.
        let up = upstream("origin/gsw-push", 2, 5);
        let plan =
            PushPlan::resolve("gsw-push", Some("origin"), Some(&up)).expect("still resolves");
        assert_eq!(
            plan,
            PushPlan::Update {
                target: "origin/gsw-push".to_string(),
                commits: 2,
            },
        );
    }

    #[test]
    fn a_branch_with_no_upstream_and_no_remote_cannot_be_pushed() {
        // A repository with no remote at all (or none gsw can pick). There is
        // nowhere to push, so there is nothing to confirm.
        assert_eq!(PushPlan::resolve("gsw-push", None, None), None);
    }

    #[test]
    fn a_detached_head_cannot_be_pushed() {
        // `repo::branch_name` reports `HEAD` when HEAD is detached, and git
        // refuses `HEAD` as a branch name, so this sentinel can never collide
        // with a real branch. Pushing it would create a remote branch literally
        // named `HEAD`.
        assert_eq!(PushPlan::resolve(DETACHED_HEAD, Some("origin"), None), None);
    }

    #[test]
    fn a_detached_head_cannot_be_pushed_even_with_a_stale_upstream() {
        // Belt and braces: a detached HEAD must be refused before the upstream
        // is consulted, so a leftover tracking status cannot make it pushable.
        let up = upstream("origin/main", 4, 0);
        assert_eq!(
            PushPlan::resolve(DETACHED_HEAD, Some("origin"), Some(&up)),
            None,
        );
    }

    #[test]
    fn the_create_plan_uses_the_remote_it_was_given() {
        // A repository whose only remote is not named `origin`. The plan must
        // carry that name through to the command rather than assuming `origin`.
        let plan = PushPlan::resolve("gsw-push", Some("fork"), None).expect("one remote is enough");
        assert_eq!(plan.command_args(), ["push", "-u", "fork", "gsw-push"]);
    }
}

//! Which pull request the indicator asks about.
//!
//! The answer is a pure read of the repository configuration. It uses no network.

use ghrest::GithubRepo;

/// The pull request that the indicator asks about: a repository and a head branch.
///
/// The head owner is the owner of the repository, so this type does not store it twice.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PullTarget {
    repo: GithubRepo,
    head: String,
}

impl PullTarget {
    /// Builds a target from its two parts. Tests use it to state a target directly.
    #[cfg(test)]
    pub(crate) fn new(repo: GithubRepo, head: impl Into<String>) -> Self {
        Self {
            repo,
            head: head.into(),
        }
    }

    /// The repository that the pull request belongs to.
    pub(crate) fn repo(&self) -> &GithubRepo {
        &self.repo
    }

    /// The name of the head branch on that repository, without `refs/heads/`.
    pub(crate) fn head(&self) -> &str {
        &self.head
    }
}

/// Decides which pull request the current branch of `repo` maps to.
pub(crate) fn target_of(repo: &gix::Repository, extra_host: Option<&str>) -> Option<PullTarget> {
    let _ = (repo, extra_host);
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testrepo::{git, init_repo, init_repo_with_worktree};

    fn target(dir: &std::path::Path, extra: Option<&str>) -> Option<PullTarget> {
        let repo = gix::open(dir).expect("open fixture");
        target_of(&repo, extra)
    }

    fn expect(target: Option<PullTarget>, repo: &str, head: &str) {
        let target = target.expect("a target");
        assert_eq!(target.repo().to_string(), repo);
        assert_eq!(target.head(), head);
    }

    fn track(dir: &std::path::Path, branch: &str, remote: &str, merge: &str) {
        git(dir, &["config", &format!("branch.{branch}.remote"), remote]);
        git(dir, &["config", &format!("branch.{branch}.merge"), merge]);
    }

    #[test]
    fn tracking_branch_gives_remote_and_branch() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "feature"]);
        git(
            p,
            &["remote", "add", "origin", "git@github.com:owner/name.git"],
        );
        track(p, "feature", "origin", "refs/heads/feature");
        expect(target(p, None), "github.com/owner/name", "feature");
    }

    #[test]
    fn head_is_the_merge_branch_not_the_local_name() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "local-name"]);
        git(
            p,
            &["remote", "add", "origin", "https://github.com/owner/name"],
        );
        track(p, "local-name", "origin", "refs/heads/remote-name");
        expect(target(p, None), "github.com/owner/name", "remote-name");
    }

    #[test]
    fn remote_name_with_a_slash_is_not_split() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "feature"]);
        git(
            p,
            &[
                "remote",
                "add",
                "team/origin",
                "git@github.com:team/name.git",
            ],
        );
        track(p, "feature", "team/origin", "refs/heads/feature/sub");
        expect(target(p, None), "github.com/team/name", "feature/sub");
    }

    #[test]
    fn without_tracking_the_push_remote_and_local_name_are_used() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "topic"]);
        git(p, &["remote", "add", "fork", "git@github.com:me/name.git"]);
        expect(target(p, None), "github.com/me/name", "topic");
    }

    #[test]
    fn without_tracking_several_remotes_give_no_target() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["remote", "add", "a", "git@github.com:o/a.git"]);
        git(p, &["remote", "add", "b", "git@github.com:o/b.git"]);
        assert_eq!(target(p, None), None);
    }

    #[test]
    fn detached_head_gives_no_target() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["remote", "add", "origin", "git@github.com:o/n.git"]);
        git(p, &["checkout", "-q", "--detach"]);
        assert_eq!(target(p, None), None);
    }

    #[test]
    fn a_remote_that_is_not_github_gives_no_target_unless_the_host_is_extra() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["remote", "add", "origin", "git@gitlab.com:o/n.git"]);
        track(p, "main", "origin", "refs/heads/main");
        assert_eq!(target(p, None), None);
        let found = target(p, Some("gitlab.com")).expect("a target");
        assert_eq!(found.repo().host(), "gitlab.com");
        assert_eq!(found.head(), "main");
    }

    #[test]
    fn a_local_tracking_branch_gives_no_target() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["remote", "add", "origin", "git@github.com:o/n.git"]);
        track(p, "main", ".", "refs/heads/other");
        assert_eq!(target(p, None), None);
    }

    #[test]
    fn a_merge_ref_outside_heads_gives_no_target() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["remote", "add", "origin", "git@github.com:o/n.git"]);
        track(p, "main", "origin", "refs/tags/v1");
        assert_eq!(target(p, None), None);
    }

    #[test]
    fn a_remote_name_that_names_no_remote_gives_no_target() {
        let dir = init_repo();
        let p = dir.path();
        track(p, "main", "ghost", "refs/heads/main");
        assert_eq!(target(p, None), None);
    }

    #[test]
    fn a_url_in_place_of_a_remote_name_is_used() {
        let dir = init_repo();
        let p = dir.path();
        track(
            p,
            "main",
            "https://github.com/owner/name.git",
            "refs/heads/main",
        );
        expect(target(p, None), "github.com/owner/name", "main");
    }

    #[test]
    fn insteadof_rewrites_apply() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["config", "url.git@github.com:.insteadOf", "gh:"]);
        git(p, &["remote", "add", "origin", "gh:owner/name"]);
        track(p, "main", "origin", "refs/heads/main");
        expect(target(p, None), "github.com/owner/name", "main");
    }

    #[test]
    fn insteadof_rewrites_apply_to_the_push_remote() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["config", "url.git@github.com:.insteadOf", "gh:"]);
        git(p, &["remote", "add", "origin", "gh:owner/name"]);
        expect(target(p, None), "github.com/owner/name", "main");
    }

    #[test]
    fn a_multibyte_branch_name_round_trips() {
        let dir = init_repo();
        let p = dir.path();
        let branch = "feat/日本語+🎉#café";
        git(p, &["checkout", "-q", "-b", branch]);
        git(p, &["remote", "add", "origin", "git@github.com:o/n.git"]);
        track(p, branch, "origin", &format!("refs/heads/{branch}"));
        expect(target(p, None), "github.com/o/n", branch);
        git(
            p,
            &["config", "--unset", &format!("branch.{branch}.remote")],
        );
        git(p, &["config", "--unset", &format!("branch.{branch}.merge")]);
        expect(target(p, None), "github.com/o/n", branch);
    }

    #[test]
    fn a_linked_worktree_reads_the_shared_config() {
        let (dir, linked) = init_repo_with_worktree();
        git(
            dir.path(),
            &["remote", "add", "origin", "git@github.com:o/n.git"],
        );
        track(dir.path(), "linked", "origin", "refs/heads/linked-remote");
        expect(target(&linked, None), "github.com/o/n", "linked-remote");
    }
}

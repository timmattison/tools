//! gix-backed git operations for gsw.
//!
//! gsw is a read-only monitor. Every function here reads the repository
//! in-process via `gix` and never writes the index, so it can never take
//! `.git/index.lock` and can never race a concurrent rebase — the reason the
//! old `git` CLI path needed a private index snapshot.

/// Open the repository containing `cwd`, or `None` when there isn't one with a
/// working tree (outside any repo, or a bare repo — gsw has nothing per-file to
/// render in either case).
pub fn open() -> Option<gix::Repository> {
    let repo = gix::discover(".").ok()?;
    // Bare repos have no work tree; gsw renders a per-file working-tree view,
    // so there's nothing to show. Treat them like "not a repo".
    if repo.workdir().is_some() { Some(repo) } else { None }
}

/// The short current-branch name (e.g. `main`), or `"HEAD"` when detached —
/// matching what `git rev-parse --abbrev-ref HEAD` prints.
pub fn branch_name(repo: &gix::Repository) -> String {
    match repo.head_name() {
        Ok(Some(full)) => full.shorten().to_string(),
        _ => "HEAD".to_string(),
    }
}

/// Pick the first base ref that resolves: `main`, then `master`, then
/// `origin/HEAD`'s target, else `"HEAD"` (so commits-ahead degrades to 0).
pub fn resolve_base(repo: &gix::Repository) -> String {
    for candidate in ["main", "master"] {
        if repo.rev_parse_single(candidate).is_ok() {
            return candidate.to_string();
        }
    }
    if let Ok(reference) = repo.find_reference("refs/remotes/origin/HEAD") {
        if let Some(target) = reference.target().try_name() {
            // target is e.g. refs/remotes/origin/main → shorten to origin/main
            return target.shorten().to_string();
        }
    }
    "HEAD".to_string()
}

/// Count commits reachable from HEAD but not from `base`
/// (`git rev-list --count base..HEAD`). Returns 0 on any failure.
pub fn commits_ahead(repo: &gix::Repository, base: &str) -> u32 {
    let resolve = || -> anyhow::Result<u32> {
        let head = repo.head_id()?.detach();
        let base_id = repo.rev_parse_single(base)?.detach();
        if head == base_id {
            return Ok(0);
        }
        let count = repo
            .rev_walk(std::iter::once(head))
            .with_hidden(std::iter::once(base_id))
            .all()?
            .count();
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    };
    resolve().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    /// Run a git command in `dir`, isolated from the host's global/system
    /// config, asserting success. Test-only fixture construction.
    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("invoke git");
        assert!(status.success(), "git {args:?} failed");
    }

    /// A fresh repo on branch `main` with one commit. Parallel-safe: unique tempdir.
    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@example.com"]);
        git(p, &["config", "user.name", "Test"]);
        git(p, &["config", "commit.gpgsign", "false"]);
        std::fs::write(p.join("a.txt"), "initial\n").unwrap();
        git(p, &["add", "a.txt"]);
        git(p, &["commit", "-q", "-m", "initial"]);
        dir
    }

    /// Open a repo at an explicit path (tests can't rely on cwd under a
    /// parallel test runner). Mirrors `open()`'s logic but takes a path.
    fn open_at(path: &Path) -> Option<gix::Repository> {
        let repo = gix::discover(path).ok()?;
        if repo.workdir().is_some() { Some(repo) } else { None }
    }

    #[test]
    fn branch_name_reports_current_branch() {
        let dir = init_repo();
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(super::branch_name(&repo), "main");
    }

    #[test]
    fn branch_name_reports_head_when_detached() {
        let dir = init_repo();
        git(dir.path(), &["checkout", "-q", "--detach"]);
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(super::branch_name(&repo), "HEAD");
    }

    #[test]
    fn open_at_finds_worktree_repo() {
        let dir = init_repo();
        assert!(open_at(dir.path()).is_some(), "should open a worktree repo");
    }

    #[test]
    fn open_at_rejects_bare_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "--bare", "-q"]);
        assert!(
            open_at(dir.path()).is_none(),
            "a bare repo has no work tree; gsw must treat it like no repo",
        );
    }

    #[test]
    fn resolve_base_prefers_main() {
        let dir = init_repo(); // already on main
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(super::resolve_base(&repo), "main");
    }

    #[test]
    fn resolve_base_falls_back_to_master() {
        let dir = init_repo();
        git(dir.path(), &["branch", "-m", "main", "master"]);
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(super::resolve_base(&repo), "master");
    }

    #[test]
    fn commits_ahead_counts_commits_past_base() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(p.join("b.txt"), "two\n").unwrap();
        git(p, &["add", "b.txt"]);
        git(p, &["commit", "-q", "-m", "second"]);
        std::fs::write(p.join("c.txt"), "three\n").unwrap();
        git(p, &["add", "c.txt"]);
        git(p, &["commit", "-q", "-m", "third"]);
        let repo = open_at(p).unwrap();
        assert_eq!(super::commits_ahead(&repo, "main"), 2);
    }

    #[test]
    fn commits_ahead_is_zero_when_base_equals_head() {
        let dir = init_repo();
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(super::commits_ahead(&repo, "main"), 0);
    }
}

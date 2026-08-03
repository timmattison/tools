//! The one way a tool in this repository is allowed to invoke git.
//!
//! Every simulation runs against the developer's *real* repository, so a stray
//! git invocation could rewrite branches they care about. All calls funnel
//! through [`Git`], which pins the configuration that makes simulation
//! non-destructive — most importantly `rebase.updateRefs=false`, which would
//! otherwise move the very branch refs being simulated.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};

/// The outcome of one git invocation.
pub struct GitOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// A git command runner pinned to one working directory.
pub struct Git {
    cwd: PathBuf,
    hooks_path: String,
}

impl Git {
    /// Run git in `cwd`, with hooks redirected to the empty `hooks_path`.
    ///
    /// Crate-private on purpose. A caller outside this crate could otherwise
    /// build a runner rooted in the developer's real repository, with a
    /// `hooks_path` that redirects nothing — an empty one still resolves hook
    /// lookups, relative to `cwd`. Both guards are established by
    /// [`Scratch::create`](crate::Scratch::create), so it stays the only way in.
    #[must_use]
    pub(crate) fn new(cwd: impl Into<PathBuf>, hooks_path: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            hooks_path: hooks_path.into(),
        }
    }

    /// Run git, returning the outcome whether or not it succeeded.
    ///
    /// # Errors
    ///
    /// Returns an error only if git could not be spawned at all.
    pub fn try_run(&self, args: &[&str]) -> Result<GitOutput> {
        let output = Command::new("git")
            .args(self.safety_config())
            .args(args)
            .current_dir(&self.cwd)
            // A rebase that stops would otherwise try to open an editor and
            // hang forever on a commit message or a todo list.
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .with_context(|| format!("failed to run git {}", args.join(" ")))?;

        Ok(GitOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }

    /// Run git and fail if it does, returning trimmed stdout.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned or exited non-zero.
    pub fn run(&self, args: &[&str]) -> Result<String> {
        let output = self.try_run(args)?;

        anyhow::ensure!(
            output.success,
            "git {} failed:\n{}\n{}",
            args.join(" "),
            output.stdout,
            output.stderr
        );

        Ok(output.stdout)
    }

    /// Run git and return stdout split into non-empty lines.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned or exited non-zero.
    pub fn lines(&self, args: &[&str]) -> Result<Vec<String>> {
        Ok(self
            .run(args)?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect())
    }

    /// Resolve a revision to a full commit id.
    ///
    /// # Errors
    ///
    /// Returns an error if the revision does not name a commit.
    pub fn rev_parse(&self, revision: &str) -> Result<String> {
        self.run(&["rev-parse", &format!("{revision}^{{commit}}")])
            .with_context(|| format!("could not resolve '{revision}' to a commit"))
    }

    /// Configuration that keeps a simulation from touching anything real.
    fn safety_config(&self) -> Vec<String> {
        [
            // Recording resolutions from a simulated conflict would poison the
            // shared rr-cache and silently pre-resolve the developer's real
            // merges later.
            "rerere.enabled=false",
            "rerere.autoupdate=false",
            // Without this, rebasing a detached HEAD still rewrites every branch
            // ref pointing into the replayed range - including the branch being
            // simulated. Proven by tests/safety.rs: with the setting enabled and
            // this override removed, a dry run destroys the developer's branch.
            "rebase.updateRefs=false",
            "rebase.autoStash=false",
            "rebase.autosquash=false",
            // Simulated mains are loose commits nothing references yet; an
            // opportunistic gc mid-run could collect one out from under us.
            "gc.auto=0",
            "commit.gpgsign=false",
            "gpg.format=openpgp",
            // The identity belongs to this crate, not to whichever tool is
            // driving it, so every consumer's scratch commits are attributable
            // to the harness that actually made them.
            "user.name=gitscratch",
            "user.email=gitscratch@localhost",
        ]
        .iter()
        .flat_map(|setting| ["-c".to_string(), (*setting).to_string()])
        .chain([
            "-c".to_string(),
            format!("core.hooksPath={}", self.hooks_path),
        ])
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::Git;

    /// `git var GIT_AUTHOR_IDENT` reports exactly the identity git would stamp
    /// on a commit, so it proves what [`Git::safety_config`] actually pins
    /// without having to build a repository and commit into it. It resolves
    /// outside a repository too, which is why `temp_dir` is only ever a cwd
    /// here — nothing is created in it, so concurrent test runs cannot collide.
    #[test]
    fn commits_under_the_crate_s_own_identity_not_a_consuming_tool_s() {
        let git = Git::new(std::env::temp_dir(), "");

        let ident = git
            .run(&["var", "GIT_AUTHOR_IDENT"])
            .expect("git var GIT_AUTHOR_IDENT");

        assert!(
            ident.starts_with("gitscratch <gitscratch@localhost>"),
            "scratch commits should be authored by the crate, not a consumer: {ident}"
        );
    }

    /// Every pin in [`Git::safety_config`] is only as good as the environment it
    /// runs in, because git's environment beats `-c`. That is not a hypothetical:
    /// a consumer invoked from inside a git hook is handed `GIT_AUTHOR_NAME`,
    /// `GIT_AUTHOR_EMAIL` and `GIT_AUTHOR_DATE` naming the developer, plus
    /// `GIT_INDEX_FILE` — *relative*, so it re-anchors on whatever directory the
    /// runner happens to be in — and other hooks add `GIT_DIR` and
    /// `GIT_WORK_TREE`. So an inherited environment can both sign the harness's
    /// commits with the developer's name and aim the whole replay at the
    /// repository they were committing to, which is the one thing this crate
    /// exists to keep it away from.
    ///
    /// Both halves are asserted together, in one test, so this binary's
    /// environment is only ever mutated in one place; the mutation is what the
    /// guard is being asked to survive, and after the guard exists it cannot
    /// reach the sibling test above either.
    #[test]
    fn ignores_an_inherited_git_environment_naming_another_identity_or_repository() {
        // Stands in for the developer's real repository - the place a leaked
        // environment would redirect the replay to.
        let elsewhere = TempDir::new().expect("create the stand-in for a real repository");
        let elsewhere_git_dir = elsewhere.path().join("their-repo.git");
        let elsewhere_index = elsewhere.path().join("their-index");

        std::env::set_var("GIT_AUTHOR_NAME", "A Developer");
        std::env::set_var("GIT_AUTHOR_EMAIL", "developer@example.com");
        std::env::set_var("GIT_COMMITTER_NAME", "A Developer");
        std::env::set_var("GIT_COMMITTER_EMAIL", "developer@example.com");
        std::env::set_var("GIT_DIR", &elsewhere_git_dir);
        std::env::set_var("GIT_WORK_TREE", elsewhere.path());
        std::env::set_var("GIT_INDEX_FILE", &elsewhere_index);

        let here = TempDir::new().expect("create the scratch stand-in");
        let git = Git::new(here.path(), "");
        git.run(&["init", "-q", "-b", "main"])
            .expect("initialise the repository the runner is rooted in");

        for variable in ["GIT_AUTHOR_IDENT", "GIT_COMMITTER_IDENT"] {
            let ident = git
                .run(&["var", variable])
                .expect("read the identity git would stamp");
            assert!(
                ident.starts_with("gitscratch <gitscratch@localhost>"),
                "an inherited environment must not put a developer's name on a scratch \
                 commit, but {variable} is {ident}"
            );
        }

        // Canonicalised on both sides: macOS resolves the temporary directory's
        // /var to /private/var, so the raw paths would never compare equal.
        let expected = std::fs::canonicalize(here.path()).expect("canonicalise the scratch path");
        let git_dir = git
            .run(&["rev-parse", "--absolute-git-dir"])
            .expect("ask git which repository it is operating on");
        assert!(
            std::fs::canonicalize(&git_dir)
                .expect("canonicalise git's answer")
                .starts_with(&expected),
            "the runner must operate on the repository it is rooted in ({}), not the one an \
             inherited GIT_DIR names ({git_dir})",
            expected.display()
        );

        let index = git
            .run(&["rev-parse", "--git-path", "index"])
            .expect("ask git which index it would write");
        assert!(
            !index.contains("their-index"),
            "an inherited GIT_INDEX_FILE must not become the index a replay stages into: {index}"
        );
    }
}
